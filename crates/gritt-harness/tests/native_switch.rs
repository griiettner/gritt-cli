//! TKT-0027: moving a native session's next turn to a different provider
//! profile or model without leaving the conversation. Same session id and
//! transcript, a replacement adapter seeded with the outgoing one's
//! normalized history, and a rejection that leaves the driver untouched.

use std::sync::Arc;

use gritt_core::config::Config;
use gritt_core::provider::{Protocol, ProviderProfile, ReasoningEffort};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{Phase, SessionKind, SessionStore};
use gritt_harness::agent::{AgentBuilder, ApprovalMode, SessionSelector};
use gritt_harness::control::ControlPlane;
use gritt_harness::draft::SwitchOutcome;
use gritt_harness::modes::print::{PrintUi, PrintUiOptions, SharedBuffer};
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_provider::models::ModelCatalog;
use gritt_provider::{FixtureResponse, FixtureTransport, StaticKey};

const KEY: &str = "fixture-key-never-printed";

fn text_sse(text: &str) -> Vec<u8> {
    let chunk = serde_json::json!({
        "id": "chatcmpl-f", "object": "chat.completion.chunk", "model": "irrelevant",
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": "stop"}]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n").into_bytes()
}

fn profile(name: &str, base_url: &str) -> ProviderProfile {
    ProviderProfile {
        name: name.into(),
        protocol: Protocol::ChatCompletions,
        base_url: base_url.into(),
        key: SecretRef::for_profile(name, format!("{}_API_KEY", name.to_uppercase())),
        aliases: Default::default(),
        fallback_model: None,
    }
}

fn config() -> Config {
    let mut config = Config::default();
    config.profiles.insert(
        "openrouter".into(),
        profile("openrouter", "https://one.example/v1"),
    );
    config.profiles.insert(
        "anthropic".into(),
        profile("anthropic", "https://two.example/v1"),
    );
    config.default_profile = Some("openrouter".into());
    config.default_model = Some("model-a".into());
    config
}

struct Fixture {
    _dir: tempfile::TempDir,
    plane: ControlPlane,
    transport: Arc<FixtureTransport>,
}

async fn fixture(responses: Vec<FixtureResponse>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
    let config = config();
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    let transport = Arc::new(FixtureTransport::new(responses, 17));
    let builder = AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(StaticKey(Secret::new(KEY))),
        transport: transport.clone(),
        catalog: ModelCatalog::new(),
        cache: None,
        workspace: Workspace::open(dir.path()).unwrap(),
        approval: ApprovalMode::DenyAll,
        mcp: None,
    };
    Fixture {
        _dir: dir,
        plane: ControlPlane::native(Arc::new(builder)),
        transport,
    }
}

fn ui() -> PrintUi<SharedBuffer, SharedBuffer> {
    PrintUi::new(
        SharedBuffer::default(),
        SharedBuffer::default(),
        PrintUiOptions::deny_all(false),
    )
}

fn last_request_json(transport: &FixtureTransport) -> serde_json::Value {
    let requests = transport.requests();
    let body = requests.last().expect("a request was sent").body.clone();
    serde_json::from_slice(&body.expect("a JSON body")).expect("valid JSON")
}

#[tokio::test]
async fn switching_the_model_keeps_the_session_and_replays_prior_text_into_the_new_request() {
    let fx = fixture(vec![
        FixtureResponse::sse(text_sse("first answer")),
        FixtureResponse::sse(text_sse("second answer")),
    ])
    .await;
    let mut agent = fx
        .plane
        .builder
        .open(
            SessionSelector::New { name: None },
            Some("openrouter"),
            Some("model-a"),
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let session_id = agent.session().id.clone();
    let mut out = ui();
    agent
        .run_turn("first prompt", &mut out)
        .await
        .expect("first turn");

    let outcome = agent
        .switch_native("openrouter".into(), "model-b".into(), ReasoningEffort::Auto)
        .await
        .expect("switch resolves");
    assert!(
        matches!(outcome, SwitchOutcome::Applied { .. }),
        "{outcome:?}"
    );
    assert_eq!(agent.session().id, session_id, "the session id moved");
    let SessionKind::Native {
        provider_profile,
        model,
        ..
    } = &agent.session().kind
    else {
        panic!("session stopped being native")
    };
    assert_eq!(provider_profile, "openrouter");
    assert_eq!(model, "model-b");

    // The store agrees: a resume built fresh from the row sees the switch.
    let stored = fx
        .plane
        .builder
        .store
        .get(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.kind,
        SessionKind::Native {
            provider_profile: "openrouter".into(),
            model: "model-b".into(),
            effort: ReasoningEffort::Auto,
        }
    );
    // The transcript from before the switch is intact.
    let events = fx
        .plane
        .builder
        .store
        .read_events(&session_id)
        .await
        .unwrap();
    assert!(!events.is_empty(), "the prior turn's events were lost");

    agent
        .run_turn("second prompt", &mut out)
        .await
        .expect("second turn");
    let body = last_request_json(&fx.transport);
    assert_eq!(body["model"], "model-b");
    let messages = body["messages"].as_array().unwrap();
    // The replayed turn from the outgoing adapter, then the new prompt:
    // system, first user, first assistant reply, second user.
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "first prompt");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[2]["content"], "first answer");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"], "second prompt");
}

#[tokio::test]
async fn switching_provider_carries_the_conversation_to_the_new_endpoint() {
    let fx = fixture(vec![
        FixtureResponse::sse(text_sse("from openrouter")),
        FixtureResponse::sse(text_sse("from anthropic")),
    ])
    .await;
    let mut agent = fx
        .plane
        .builder
        .open(
            SessionSelector::New { name: None },
            Some("openrouter"),
            Some("model-a"),
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let mut out = ui();
    agent.run_turn("hello", &mut out).await.expect("first turn");

    let outcome = agent
        .switch_native(
            "anthropic".into(),
            "claude-model".into(),
            ReasoningEffort::Auto,
        )
        .await
        .expect("switch resolves");
    assert!(
        matches!(outcome, SwitchOutcome::Applied { .. }),
        "{outcome:?}"
    );

    agent
        .run_turn("again", &mut out)
        .await
        .expect("second turn");
    let requests = fx.transport.requests();
    let second = requests.last().unwrap();
    assert!(
        second.url.starts_with("https://two.example"),
        "the second request did not reach the new profile's endpoint: {}",
        second.url
    );
    let body: serde_json::Value = serde_json::from_slice(second.body.as_ref().unwrap()).unwrap();
    assert_eq!(body["model"], "claude-model");
    let messages = body["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == "hello"),
        "the prior turn's prompt did not carry over: {messages:?}"
    );
}

#[tokio::test]
async fn a_rejected_switch_leaves_the_driver_and_the_stored_session_unchanged() {
    let fx = fixture(vec![
        FixtureResponse::sse(text_sse("hi")),
        FixtureResponse::sse(text_sse("still here, too")),
    ])
    .await;
    let mut agent = fx
        .plane
        .builder
        .open(
            SessionSelector::New { name: None },
            Some("openrouter"),
            Some("model-a"),
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let mut out = ui();
    agent.run_turn("hello", &mut out).await.expect("first turn");

    let outcome = agent
        .switch_native(
            "not-configured".into(),
            "some-model".into(),
            ReasoningEffort::Auto,
        )
        .await
        .expect("resolution itself does not error");
    let SwitchOutcome::Rejected { errors, .. } = outcome else {
        panic!("an unknown profile was accepted")
    };
    assert!(!errors.is_empty());
    let SessionKind::Native {
        provider_profile,
        model,
        ..
    } = &agent.session().kind
    else {
        panic!("session stopped being native")
    };
    assert_eq!(
        provider_profile, "openrouter",
        "the rejection moved the profile"
    );
    assert_eq!(model, "model-a", "the rejection moved the model");

    // The old driver still works: a turn still reaches the original model.
    agent
        .run_turn("still here", &mut out)
        .await
        .expect("turn after rejection");
    let body = last_request_json(&fx.transport);
    assert_eq!(body["model"], "model-a");
}

/// A switch changes which provider the session's continuation row belongs
/// to; resuming before another turn runs must not choke on a continuation
/// state a different adapter wrote.
#[tokio::test]
async fn resuming_right_after_a_switch_does_not_fail_on_the_old_continuation() {
    let fx = fixture(vec![FixtureResponse::sse(text_sse("hi"))]).await;
    let mut agent = fx
        .plane
        .builder
        .open(
            SessionSelector::New { name: None },
            Some("openrouter"),
            Some("model-a"),
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let session_id = agent.session().id.clone();
    let mut out = ui();
    agent.run_turn("hello", &mut out).await.expect("first turn");
    // Same protocol, so the continuation row after the switch is still
    // readable by `anthropic`'s adapter here; the row is nonetheless the
    // one `openrouter`'s driver wrote for the first turn, and no turn has
    // run on the new driver yet to overwrite it.
    let outcome = agent
        .switch_native(
            "anthropic".into(),
            "claude-model".into(),
            ReasoningEffort::Auto,
        )
        .await
        .expect("switch resolves");
    assert!(
        matches!(outcome, SwitchOutcome::Applied { .. }),
        "{outcome:?}"
    );
    drop(agent);

    let resumed = fx
        .plane
        .builder
        .open(SessionSelector::Id(session_id.clone()), None, None, None)
        .await
        .expect("resume must not fail on a stale continuation");
    assert_eq!(resumed.session().id, session_id);
    let SessionKind::Native {
        provider_profile,
        model,
        ..
    } = &resumed.session().kind
    else {
        panic!("session stopped being native")
    };
    assert_eq!(provider_profile, "anthropic");
    assert_eq!(model, "claude-model");
}
