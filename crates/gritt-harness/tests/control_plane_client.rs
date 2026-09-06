//! Non-terminal Rust client fixture (TKT-0023): proves that a caller which
//! never imports Ratatui, Crossterm, or any terminal type can drive the
//! same control-plane seam the CLI, REPL, and TUI use for profile, model,
//! mode, and effort selection; session resume; permission decisions; and
//! normalized events. `ClientUi` below stands in for a future non-terminal
//! frontend (ADR-011, T3Code): it forwards events and approvals to plain
//! Rust values instead of rendering anything, the same shape
//! `gritt_harness::tui::run::ChannelUi` uses for the full-screen mode.

use std::sync::Arc;

use chrono::Utc;
use gritt_core::config::Config;
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event, EventKind};
use gritt_core::provider::{
    ModelCapabilities, ModelInfo, ModelList, ModelListStatus, Protocol, ProviderProfile,
    ReasoningEffort,
};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, ExecutionMode, SessionStore};
use gritt_core::tool::native;
use gritt_harness::agent::{AgentBuilder, ApprovalMode, TurnStatus, Ui};
use gritt_harness::control::{ControlPlane, DraftOpen};
use gritt_harness::draft::SessionDraft;
use gritt_harness::policy::Decision;
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_provider::models::ModelCatalog;
use gritt_provider::{FixtureResponse, FixtureTransport, StaticKey};

const KEY: &str = "fixture-key-never-printed";

/// A `Ui` with no rendering and no terminal I/O: it records events and
/// answers approvals from a script, the same shape a non-terminal client
/// uses to forward events to its own channel or store instead of a
/// terminal prompt.
#[derive(Default)]
struct ClientUi {
    events: Vec<Event>,
    answers: Vec<ApprovalDecision>,
}

impl Ui for ClientUi {
    fn event(&mut self, event: &Event) {
        self.events.push(event.clone());
    }

    fn approve<'a>(
        &'a mut self,
        _request: &'a ApprovalRequest,
        _decision: &'a Decision,
        _preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        let answer = if self.answers.is_empty() {
            ApprovalDecision::Approved
        } else {
            self.answers.remove(0)
        };
        Box::pin(async move { answer })
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../gritt-provider/tests/fixtures/chat-completions/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

/// A one-call streamed response for `tool` with `arguments`.
fn tool_call_sse(tool: &str, arguments: serde_json::Value) -> Vec<u8> {
    let first = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": null,
            "tool_calls": [{"index": 0, "id": format!("call_{tool}"), "type": "function",
                "function": {"name": tool, "arguments": arguments.to_string()}}]},
            "finish_reason": null}]
    });
    let second = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {first}\n\ndata: {second}\n\ndata: [DONE]\n\n").into_bytes()
}

fn config() -> Config {
    let mut config = Config::default();
    config.profiles.insert(
        "openrouter".into(),
        ProviderProfile {
            name: "openrouter".into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://openrouter.ai/api/v1".into(),
            key: SecretRef::for_profile("openrouter", "OPENROUTER_API_KEY"),
            aliases: Default::default(),
            fallback_model: None,
        },
    );
    config.default_profile = Some("openrouter".into());
    config.default_model = Some("openai/gpt-5-nano".into());
    config
}

/// Puts a fresh model list in the catalog, the same warmed state print,
/// REPL, and the TUI resolve models against.
fn seed_catalog(plane: &ControlPlane, profile: &str, model: &str) {
    plane.builder.catalog.insert(ModelList {
        profile: profile.into(),
        status: ModelListStatus::Fresh {
            fetched_at: Utc::now(),
        },
        models: vec![ModelInfo {
            id: model.into(),
            display_name: None,
            capabilities: ModelCapabilities::default(),
            replaced_by: None,
            deprecated: false,
        }],
    });
}

struct Fixture {
    _dir: tempfile::TempDir,
    plane: ControlPlane,
    transport: Arc<FixtureTransport>,
}

async fn fixture_plane(responses: Vec<FixtureResponse>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let config = config();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
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
        approval: ApprovalMode::Ask,
        mcp: None,
    };
    let plane = ControlPlane::native(Arc::new(builder));
    seed_catalog(&plane, "openrouter", "openai/gpt-5-nano");
    Fixture {
        _dir: dir,
        plane,
        transport,
    }
}

#[tokio::test]
async fn a_non_terminal_client_selects_profile_model_mode_and_effort_then_runs_a_turn() {
    let fx = fixture_plane(vec![FixtureResponse::sse(fixture("stream-text.sse"))]).await;

    // The same draft validator print, REPL, and the TUI use for profile,
    // model, mode, and effort selection -- no CLI flags, no terminal picker.
    let draft = SessionDraft::default()
        .with_name("client-session")
        .with_profile("openrouter")
        .with_model("openai/gpt-5-nano")
        .with_effort(ReasoningEffort::Auto)
        .with_mode(ExecutionMode::Supervised);
    let DraftOpen::Opened {
        mut driver,
        warnings,
        ..
    } = fx.plane.open_draft(draft).await.unwrap()
    else {
        panic!("draft was rejected");
    };
    assert!(warnings.is_empty());
    assert_eq!(driver.mode(), Some(ExecutionMode::Supervised));
    assert_eq!(driver.effort(), Some(ReasoningEffort::Auto));

    let mut ui = ClientUi::default();
    let outcome = driver.run_turn("hello", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(outcome.text, "Hello, world");

    // Events are the normalized contract, not rendered text: nothing here
    // carries a terminal escape sequence.
    assert!(!ui.events.is_empty());
    for event in &ui.events {
        let dump = serde_json::to_string(event).unwrap();
        assert!(!dump.contains('\u{1b}'), "event carries an escape sequence");
    }
    assert!(ui
        .events
        .iter()
        .any(|event| matches!(event.kind, EventKind::Completed { .. })));

    // Resuming by name reuses the same startup and pinning services: the
    // profile and model do not need to be resupplied, and the stored
    // session backs the same driver contract.
    let DraftOpen::Opened {
        driver: resumed, ..
    } = fx
        .plane
        .open_draft(SessionDraft::default().with_name("client-session"))
        .await
        .unwrap()
    else {
        panic!("resume was rejected");
    };
    assert_eq!(resumed.session().id, driver.session().id);
    assert_eq!(fx.plane.builder.store.list().await.unwrap().len(), 1);
    assert_eq!(fx.transport.request_count(), 1);
}

#[tokio::test]
async fn a_non_terminal_client_answers_a_permission_decision_without_rendering_anything() {
    let fx = fixture_plane(vec![
        FixtureResponse::sse(tool_call_sse(
            native::FILE_WRITE,
            serde_json::json!({"path": "note.txt", "content": "hi\n"}),
        )),
        FixtureResponse::sse(fixture("stream-text.sse")),
    ])
    .await;
    let DraftOpen::Opened { mut driver, .. } = fx
        .plane
        .open_draft(SessionDraft::default().with_mode(ExecutionMode::Supervised))
        .await
        .unwrap()
    else {
        panic!("draft was rejected");
    };

    // The decision API (allow/ask/deny) is answered directly, the way a
    // non-terminal client routes it to its own approval channel instead of
    // a terminal prompt or a rendered diff.
    let mut ui = ClientUi {
        answers: vec![ApprovalDecision::Approved],
        ..Default::default()
    };
    let outcome = driver.run_turn("write it", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert!(ui
        .events
        .iter()
        .any(|event| matches!(event.kind, EventKind::ApprovalRequested { .. })));
    assert!(fx.plane.builder.workspace_root().join("note.txt").exists());
}
