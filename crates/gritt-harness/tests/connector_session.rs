//! Connector sessions through the control plane: an external connector
//! (the fake agent) and the native path behind the `Connector` contract,
//! stored, displayed, resumed, and recorded beside native sessions.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use gritt_connector::models::{CachedConnectorModels, ConnectorModelCache};
use gritt_connector::protocols::codex::Codex;
use gritt_connector::{ExternalConnector, Timeouts};
use gritt_core::config::{Config, ConnectorSettings, ModelListPolicy};
use gritt_core::connector::{
    AuthState, Connector, ConnectorId, ConnectorModelDiscovery, ConnectorModelFreshness,
    ConnectorUpdateOutcome, ConnectorVersionCheck, TaskRequest, TaskState, VersionCheckMode,
};
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event, EventKind, EventSource};
use gritt_core::provider::{Protocol, ProviderProfile};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, Phase, SessionId, SessionKind, SessionStore};
use gritt_core::tool::native;
use gritt_harness::agent::{AgentBuilder, ApprovalMode, SessionSelector, TurnStatus, Ui};
use gritt_harness::connector_session::ConnectorSession;
use gritt_harness::control::ControlPlane;
use gritt_harness::modes::print::{PrintUi, PrintUiOptions, SharedBuffer};
use gritt_harness::modes::repl::{run_repl, CancelSlot, LineInput};
use gritt_harness::native_connector::NativeConnector;
use gritt_harness::policy::Decision;
use gritt_harness::startup::StartupRequest;
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_provider::models::ModelCatalog;
use gritt_provider::{FixtureResponse, FixtureTransport, StaticKey};

const KEY: &str = "fixture-key-never-printed";

fn connector_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../gritt-connector/tests/fixtures/codex")
        .join(format!("{name}.jsonl"))
}

fn provider_fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../gritt-provider/tests/fixtures/chat-completions")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

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

/// A wrapper script around the fake agent with its variables baked in.
fn fake_agent(dir: &Path, vars: &[(&str, String)]) -> PathBuf {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../gritt-connector/tests/fake-agent/agent.sh");
    let wrapper = dir.join("fake-codex");
    let mut text = String::from("#!/bin/sh\n");
    for (name, value) in vars {
        text.push_str(&format!("{name}='{value}'\nexport {name}\n"));
    }
    text.push_str(&format!("exec '{}' \"$@\"\n", script.display()));
    std::fs::write(&wrapper, text).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    wrapper
}

struct Fixture {
    dir: tempfile::TempDir,
    plane: ControlPlane,
}

async fn fixture(
    responses: Vec<FixtureResponse>,
    approval: ApprovalMode,
    connectors: Vec<Arc<dyn Connector>>,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "# Readme\n").unwrap();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
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
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    let builder = AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(StaticKey(Secret::new(KEY))),
        transport: Arc::new(FixtureTransport::new(responses, 17)),
        catalog: ModelCatalog::new(),
        cache: None,
        workspace: Workspace::open(dir.path()).unwrap(),
        approval,
        mcp: None,
    };
    Fixture {
        dir,
        plane: ControlPlane::new(Arc::new(builder), connectors),
    }
}

fn codex_connector(wrapper: &Path) -> Arc<dyn Connector> {
    let settings = ConnectorSettings {
        executables: BTreeMap::from([("codex".to_owned(), wrapper.display().to_string())]),
        ..ConnectorSettings::default()
    };
    Arc::new(
        ExternalConnector::new(Codex, &settings).with_timeouts(Timeouts {
            health: Duration::from_secs(5),
            startup: Duration::from_secs(10),
            idle: Duration::from_secs(10),
        }),
    )
}

#[derive(Default)]
struct RecordingUi {
    events: Vec<Event>,
    answers: Vec<ApprovalDecision>,
    asked: Vec<ApprovalRequest>,
}

impl Ui for RecordingUi {
    fn event(&mut self, event: &Event) {
        self.events.push(event.clone());
    }

    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        _decision: &'a Decision,
        _preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        self.asked.push(request.clone());
        let answer = if self.answers.is_empty() {
            ApprovalDecision::Denied
        } else {
            self.answers.remove(0)
        };
        Box::pin(async move { answer })
    }
}

fn text_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_connector_session_runs_stores_and_resumes_like_a_native_one() {
    let scratch = tempfile::tempdir().unwrap();
    let args_path = scratch.path().join("args.txt");
    let wrapper = fake_agent(
        scratch.path(),
        &[
            (
                "FAKE_AGENT_FIXTURE",
                connector_fixture("tool").display().to_string(),
            ),
            ("FAKE_AGENT_ARGS_FILE", args_path.display().to_string()),
        ],
    );
    let fx = fixture(
        Vec::new(),
        ApprovalMode::Ask,
        vec![codex_connector(&wrapper)],
    )
    .await;
    let mut driver = fx
        .plane
        .open(
            SessionSelector::Named("ext".into()),
            Some(ConnectorId::Codex),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        driver.session().kind,
        SessionKind::Connector {
            id: ConnectorId::Codex,
            model: None,
        }
    );
    assert_eq!(driver.phase(), Phase::Coding);
    assert_eq!(driver.info().backend, "codex");
    assert_eq!(driver.info().detail, "1.0.0");

    // Print mode renders it exactly like a native session.
    let out = SharedBuffer::default();
    let err = SharedBuffer::default();
    let mut ui = PrintUi::new(out.clone(), err.clone(), PrintUiOptions::deny_all(true));
    let outcome = driver.run_turn("run it", &mut ui).await.unwrap();
    ui.finish();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(outcome.usage.input_tokens, Some(32727));
    assert_eq!(out.contents(), "I will run that exact command now.\nDONE\n");
    assert!(err
        .contents()
        .contains("-> shell /bin/zsh -lc 'echo hello-from-tool'"));
    assert!(err.contents().contains("<- shell ok"));

    // Stored beside native sessions, with the connector as source and a
    // monotonic harness sequence.
    let store = &fx.plane.builder.store;
    let session = store.find_by_name("ext").await.unwrap().unwrap();
    let events = store.read_events(&session.id).await.unwrap();
    assert!(events.len() > 5);
    assert!(events.iter().all(|e| e.source
        == EventSource::Connector {
            id: ConnectorId::Codex
        }));
    let sequences: Vec<u64> = events.iter().map(|e| e.sequence).collect();
    assert_eq!(sequences, (0..events.len() as u64).collect::<Vec<_>>());
    assert!(events.iter().any(|e| e
        .diagnostic
        .as_ref()
        .is_some_and(|d| d.get("connector_sequence").is_some())));
    let listed = store.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].kind,
        SessionKind::Connector {
            id: ConnectorId::Codex,
            model: None,
        }
    );

    // The external thread id is the continuation.
    let continuation = store.load_continuation(&session.id).await.unwrap().unwrap();
    assert_eq!(continuation.owner, "connector:codex");
    assert_eq!(continuation.state["external_id"], "thread-codex-0002");

    // Telemetry is content-free and labelled by connector.
    let telemetry = fx.plane.builder.telemetry.dump_text().await.unwrap();
    assert!(telemetry.contains("\"connector\":\"codex\""), "{telemetry}");
    assert!(!telemetry.contains("hello-from-tool"));
    assert!(!telemetry.contains("run it"));
    assert_eq!(fx.plane.builder.telemetry.content_rows().await.unwrap(), 0);

    // A second turn on a fresh driver resumes the external thread.
    drop(driver);
    let mut resumed = fx
        .plane
        .open(SessionSelector::Named("ext".into()), None, None, None, None)
        .await
        .unwrap();
    let mut ui = RecordingUi::default();
    let outcome = resumed.run_turn("again", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    let args = std::fs::read_to_string(&args_path).unwrap();
    assert!(args.contains("resume\n--json\n"), "{args}");
    assert!(args.contains("thread-codex-0002\nagain\n"), "{args}");
    let events = store.read_events(&session.id).await.unwrap();
    let sequences: Vec<u64> = events.iter().map(|e| e.sequence).collect();
    assert_eq!(sequences, (0..events.len() as u64).collect::<Vec<_>>());

    // Planning turns carry the planning request in the prompt.
    resumed.set_phase(Phase::Planning).await.unwrap();
    let mut ui = RecordingUi::default();
    resumed.run_turn("plan it", &mut ui).await.unwrap();
    let args = std::fs::read_to_string(&args_path).unwrap();
    assert!(args.contains("[Planning phase"), "{args}");

    // The wrong connector for an existing session is refused.
    let error = fx
        .plane
        .open(
            SessionSelector::Named("ext".into()),
            Some(ConnectorId::ClaudeCode),
            None,
            None,
            None,
        )
        .await
        .err()
        .unwrap();
    assert!(error.message.contains("runs on codex"));
}

#[tokio::test]
async fn a_failing_or_missing_agent_never_breaks_the_native_path() {
    let scratch = tempfile::tempdir().unwrap();
    let wrapper = fake_agent(
        scratch.path(),
        &[
            ("FAKE_AGENT_EXIT", "2".into()),
            ("FAKE_AGENT_STDERR", "boom".into()),
        ],
    );
    let fx = fixture(
        vec![FixtureResponse::sse(provider_fixture("stream-text.sse"))],
        ApprovalMode::DenyAll,
        vec![codex_connector(&wrapper)],
    )
    .await;
    // Every other connector is absent.
    let infos = fx.plane.infos().await;
    assert_eq!(infos.len(), 5);
    assert_eq!(infos[0].0, ConnectorId::Native);
    assert_eq!(infos[0].1.as_ref().unwrap().auth, AuthState::Authenticated);
    assert_eq!(infos[2].1.as_ref().unwrap().auth, AuthState::NotInstalled);
    assert!(fx.plane.connector(ConnectorId::Cursor).is_none());

    // A connector session on the broken agent fails as an error event.
    let mut broken = fx
        .plane
        .open(
            SessionSelector::Named("broken".into()),
            Some(ConnectorId::Codex),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let mut ui = RecordingUi::default();
    let outcome = broken.run_turn("x", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Failed);
    assert!(outcome.error.unwrap().contains("status 2"));

    // A connector that is not installed fails when its turn starts.
    let error = fx
        .plane
        .open(
            SessionSelector::Named("nope".into()),
            Some(ConnectorId::Cursor),
            None,
            None,
            None,
        )
        .await
        .err()
        .unwrap();
    assert!(error.message.contains("not available"));

    // The native path still works in the same store.
    let mut native = fx
        .plane
        .open(SessionSelector::Named("nat".into()), None, None, None, None)
        .await
        .unwrap();
    assert!(matches!(native.session().kind, SessionKind::Native { .. }));
    let mut ui = RecordingUi::default();
    let outcome = native.run_turn("hello", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert!(!outcome.text.is_empty());
    let listed = fx.plane.builder.store.list().await.unwrap();
    assert_eq!(listed.len(), 2);
}

#[tokio::test]
async fn cancelling_a_connector_turn_stops_the_agent() {
    let scratch = tempfile::tempdir().unwrap();
    let wrapper = fake_agent(
        scratch.path(),
        &[
            (
                "FAKE_AGENT_FIXTURE",
                connector_fixture("text").display().to_string(),
            ),
            ("FAKE_AGENT_LINE_DELAY", "30".into()),
        ],
    );
    let fx = fixture(
        Vec::new(),
        ApprovalMode::DenyAll,
        vec![codex_connector(&wrapper)],
    )
    .await;
    let mut driver = fx
        .plane
        .open(
            SessionSelector::Named("slow".into()),
            Some(ConnectorId::Codex),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let handle = driver.handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        handle.cancel();
    });
    let mut ui = RecordingUi::default();
    let outcome = tokio::time::timeout(Duration::from_secs(15), driver.run_turn("x", &mut ui))
        .await
        .expect("turn ends after cancel")
        .unwrap();
    assert_eq!(outcome.status, TurnStatus::Cancelled);
    assert!(ui
        .events
        .iter()
        .any(|e| matches!(e.kind, EventKind::Cancelled)));
    let pid = ui
        .events
        .iter()
        .find_map(|e| {
            e.diagnostic
                .as_ref()
                .and_then(|d| d.get("pid"))
                .and_then(|p| p.as_u64())
        })
        .unwrap() as u32;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!gritt_connector::process::is_alive(pid).await);
}

#[tokio::test]
async fn the_native_path_runs_behind_the_connector_contract() {
    let fx = fixture(
        vec![
            FixtureResponse::sse(tool_call_sse(
                native::FILE_WRITE,
                serde_json::json!({"path": "out.txt", "content": "hi\n"}),
            )),
            FixtureResponse::sse(provider_fixture("stream-text.sse")),
            FixtureResponse::sse(provider_fixture("stream-text.sse")),
        ],
        ApprovalMode::Ask,
        Vec::new(),
    )
    .await;
    let native = fx.plane.connector(ConnectorId::Native).unwrap();
    let info = native.info().await.unwrap();
    assert_eq!(info.id, ConnectorId::Native);
    assert!(info.capabilities.approvals);
    let session_id = fx
        .plane
        .builder
        .open(
            gritt_harness::agent::SessionSelector::New {
                name: Some("native-contract".into()),
            },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap()
        .session()
        .id
        .clone();
    let mut stream = native
        .start(TaskRequest {
            session_id: session_id.clone(),
            prompt: "write it".into(),
            workspace: fx.dir.path().to_path_buf(),
            continuation: None,
            model: None,
        })
        .await
        .unwrap();
    let mut approved = false;
    let mut events = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("stream ends")
    {
        let event = item.unwrap();
        if let EventKind::ApprovalRequested { request } = &event.kind {
            assert_eq!(
                native.inspect(&session_id).await.unwrap().state,
                TaskState::AwaitingApproval
            );
            native
                .answer_approval(&session_id, request.id.clone(), ApprovalDecision::Approved)
                .await
                .unwrap();
            approved = true;
        }
        events.push(event);
    }
    assert!(approved);
    assert!(events
        .iter()
        .any(|e| matches!(&e.kind, EventKind::ToolResult { result } if !result.is_error)));
    assert_eq!(
        std::fs::read_to_string(fx.dir.path().join("out.txt")).unwrap(),
        "hi\n"
    );
    assert_eq!(
        native.inspect(&session_id).await.unwrap().state,
        TaskState::Completed
    );
    // Follow-up input runs another turn on the same native session.
    native
        .send_input(&session_id, "and now".into())
        .await
        .unwrap();
    let mut stream = native.resume(&session_id).await.unwrap();
    let mut text = String::new();
    while let Some(item) = stream.next().await {
        if let EventKind::TextDelta { text: t } = &item.unwrap().kind {
            text.push_str(t);
        }
    }
    assert!(!text.is_empty());
    assert_eq!(fx.plane.builder.store.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_connector_runner_relays_native_approvals() {
    // The native connector through the same runner every external agent
    // uses: approvals reach the interface and go back through the trait.
    let fx = fixture(
        vec![
            FixtureResponse::sse(tool_call_sse(
                native::FILE_WRITE,
                serde_json::json!({"path": "relay.txt", "content": "ok\n"}),
            )),
            FixtureResponse::sse(provider_fixture("stream-text.sse")),
        ],
        ApprovalMode::Ask,
        Vec::new(),
    )
    .await;
    let native = fx.plane.connector(ConnectorId::Native).unwrap();
    let now = chrono::Utc::now();
    let session = gritt_core::session::Session {
        id: SessionId("via-runner".into()),
        name: "via-runner".into(),
        kind: SessionKind::Connector {
            id: ConnectorId::Native,
            model: None,
        },
        phase: Phase::Coding,
        workspace: fx.dir.path().to_path_buf(),
        created_at: now,
        updated_at: now,
        parent_id: None,
    };
    fx.plane
        .builder
        .store
        .create(session.clone())
        .await
        .unwrap();
    let mut runner = ConnectorSession::open(
        session,
        native,
        Arc::clone(&fx.plane.builder.store),
        Arc::clone(&fx.plane.builder.telemetry),
        ApprovalMode::Ask,
        Vec::new(),
    )
    .await
    .unwrap();
    let mut ui = RecordingUi {
        answers: vec![ApprovalDecision::Approved],
        ..RecordingUi::default()
    };
    let outcome = tokio::time::timeout(Duration::from_secs(10), runner.run_turn("go", &mut ui))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(ui.asked.len(), 1);
    assert_eq!(ui.asked[0].tool, native::FILE_WRITE);
    assert_eq!(
        std::fs::read_to_string(fx.dir.path().join("relay.txt")).unwrap(),
        "ok\n"
    );
    assert!(
        ui.events
            .iter()
            .filter(|e| matches!(e.kind, EventKind::ApprovalDecided { .. }))
            .count()
            >= 1
    );
    assert!(!text_of(&ui.events).is_empty());
    // The same contract object the control plane hands out is buildable
    // on its own for an in-process client.
    let standalone = NativeConnector::new(Arc::clone(&fx.plane.builder));
    assert_eq!(standalone.id(), ConnectorId::Native);
}

#[tokio::test]
async fn inherited_credentials_echoed_by_an_agent_are_redacted() {
    // The agent keeps its environment (ADR-010); what it echoes back is
    // filtered on the way to the screen and the store.
    std::env::set_var("FAKE_SERVICE_TOKEN", "svc-token-7781");
    let scratch = tempfile::tempdir().unwrap();
    let leak = scratch.path().join("leak.jsonl");
    std::fs::write(
        &leak,
        "{\"type\":\"thread.started\",\"thread_id\":\"t-leak\"}\n{\"type\":\"item.completed\",\"item\":{\"id\":\"i\",\"type\":\"agent_message\",\"text\":\"token is svc-token-7781\"}}\n{\"type\":\"turn.completed\",\"usage\":{}}\n",
    )
    .unwrap();
    let wrapper = fake_agent(
        scratch.path(),
        &[("FAKE_AGENT_FIXTURE", leak.display().to_string())],
    );
    let fx = fixture(
        Vec::new(),
        ApprovalMode::DenyAll,
        vec![codex_connector(&wrapper)],
    )
    .await;
    let mut driver = fx
        .plane
        .open(
            SessionSelector::Named("leaky".into()),
            Some(ConnectorId::Codex),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let mut ui = RecordingUi::default();
    let outcome = driver.run_turn("x", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(text_of(&ui.events), "token is [redacted]");
    let session = fx
        .plane
        .builder
        .store
        .find_by_name("leaky")
        .await
        .unwrap()
        .unwrap();
    let stored = fx
        .plane
        .builder
        .store
        .read_events(&session.id)
        .await
        .unwrap();
    let dump = serde_json::to_string(&stored).unwrap();
    assert!(!dump.contains("svc-token-7781"), "{dump}");
    assert!(dump.contains("[redacted]"));
}

#[tokio::test]
async fn a_connector_that_is_not_installed_leaves_no_session_row() {
    let settings = ConnectorSettings {
        executables: BTreeMap::from([("codex".to_owned(), "/definitely/not/codex".to_owned())]),
        ..ConnectorSettings::default()
    };
    let missing: Arc<dyn Connector> = Arc::new(ExternalConnector::new(Codex, &settings));
    let fx = fixture(Vec::new(), ApprovalMode::DenyAll, vec![missing]).await;
    let error = fx
        .plane
        .open(
            SessionSelector::Named("ghost".into()),
            Some(ConnectorId::Codex),
            None,
            None,
            None,
        )
        .await
        .err()
        .unwrap();
    assert!(error.message.contains("not installed"), "{}", error.message);
    assert!(fx.plane.builder.store.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_slow_consumer_receives_every_native_event_in_order() {
    // Well over the old 256-event channel capacity, consumed only after
    // the producer has had time to emit everything.
    const DELTAS: usize = 400;
    let mut sse = String::new();
    for index in 0..DELTAS {
        let chunk = serde_json::json!({
            "id": "chatcmpl-many", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
            "choices": [{"index": 0, "delta": {"content": format!("t{index} ")}, "finish_reason": null}]
        });
        sse.push_str(&format!("data: {chunk}\n\n"));
    }
    let last = serde_json::json!({
        "id": "chatcmpl-many", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    sse.push_str(&format!("data: {last}\n\ndata: [DONE]\n\n"));
    let fx = fixture(
        vec![FixtureResponse::sse(sse.into_bytes())],
        ApprovalMode::DenyAll,
        Vec::new(),
    )
    .await;
    let native = fx.plane.connector(ConnectorId::Native).unwrap();
    let mut stream = native
        .start(TaskRequest {
            session_id: SessionId("many-events".into()),
            prompt: "count".into(),
            workspace: fx.dir.path().to_path_buf(),
            continuation: None,
            model: None,
        })
        .await
        .unwrap();
    // Let the whole turn run before reading a single event.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut deltas = Vec::new();
    let mut completed = false;
    while let Some(item) = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("stream ends")
    {
        let event = item.unwrap();
        match &event.kind {
            EventKind::TextDelta { text } => deltas.push(text.clone()),
            EventKind::Completed { .. } => completed = true,
            _ => {}
        }
    }
    assert!(completed, "completion event was dropped");
    assert_eq!(
        deltas.len(),
        DELTAS,
        "events were dropped: {}",
        deltas.len()
    );
    for (index, delta) in deltas.iter().enumerate() {
        assert_eq!(delta, &format!("t{index} "), "out of order at {index}");
    }
}

fn models_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../gritt-connector/tests/fixtures/models/codex")
        .join(name)
}

fn codex_connector_with_models(wrapper: &Path, cache_dir: &Path) -> Arc<dyn Connector> {
    let settings = ConnectorSettings {
        executables: BTreeMap::from([("codex".to_owned(), wrapper.display().to_string())]),
        ..ConnectorSettings::default()
    };
    Arc::new(
        ExternalConnector::new(Codex, &settings)
            .with_timeouts(Timeouts {
                health: Duration::from_secs(5),
                startup: Duration::from_secs(10),
                idle: Duration::from_secs(10),
            })
            .with_model_cache(
                ConnectorModelCache::new(cache_dir),
                ModelListPolicy {
                    refresh_interval_secs: 24 * 60 * 60,
                    stale_fallback: true,
                },
            ),
    )
}

#[tokio::test]
async fn explicit_connector_model_is_stored_and_default_is_omitted() {
    let scratch = tempfile::tempdir().unwrap();
    let args_path = scratch.path().join("args.txt");
    let wrapper = fake_agent(
        scratch.path(),
        &[
            (
                "FAKE_AGENT_FIXTURE",
                connector_fixture("text").display().to_string(),
            ),
            ("FAKE_AGENT_ARGS_FILE", args_path.display().to_string()),
            (
                "FAKE_AGENT_MODELS_FILE",
                models_fixture("current.json").display().to_string(),
            ),
        ],
    );
    let cache = scratch.path().join("cache");
    let fx = fixture(
        Vec::new(),
        ApprovalMode::DenyAll,
        vec![codex_connector_with_models(&wrapper, &cache)],
    )
    .await;
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("picked".into()),
            Some(ConnectorId::Codex),
            StartupRequest::from_flags(None, Some("gpt-5.4"), None),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        opened.driver.session().kind,
        SessionKind::Connector {
            id: ConnectorId::Codex,
            model: Some("gpt-5.4".into()),
        }
    );
    let discovery = opened.connector_models.as_ref().expect("discovery ran");
    assert!(discovery.catalog().is_some());
    drop(opened);

    let defaulted = fx
        .plane
        .open_with(
            SessionSelector::Named("defaulted".into()),
            Some(ConnectorId::Codex),
            StartupRequest::from_flags(None, None, None),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        defaulted.driver.session().kind,
        SessionKind::Connector {
            id: ConnectorId::Codex,
            model: None,
        }
    );
}

#[tokio::test]
async fn resumed_connector_sessions_keep_the_stored_model() {
    let scratch = tempfile::tempdir().unwrap();
    let wrapper = fake_agent(
        scratch.path(),
        &[
            (
                "FAKE_AGENT_FIXTURE",
                connector_fixture("text").display().to_string(),
            ),
            (
                "FAKE_AGENT_MODELS_FILE",
                models_fixture("current.json").display().to_string(),
            ),
        ],
    );
    let cache = scratch.path().join("cache");
    let fx = fixture(
        Vec::new(),
        ApprovalMode::DenyAll,
        vec![codex_connector_with_models(&wrapper, &cache)],
    )
    .await;
    fx.plane
        .open_with(
            SessionSelector::Named("keep".into()),
            Some(ConnectorId::Codex),
            StartupRequest::from_flags(None, Some("gpt-5.4"), None),
            None,
            false,
        )
        .await
        .unwrap();
    let resumed = fx
        .plane
        .open_with(
            SessionSelector::Named("keep".into()),
            Some(ConnectorId::Codex),
            StartupRequest::from_flags(None, Some("gpt-5.4-mini"), None),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        resumed.driver.session().kind,
        SessionKind::Connector {
            id: ConnectorId::Codex,
            model: Some("gpt-5.4".into()),
        }
    );
    assert!(
        resumed.connector_models.is_none(),
        "resume must not run a new selection"
    );
}

#[tokio::test]
async fn connector_refresh_failure_marks_the_cached_list_stale() {
    let scratch = tempfile::tempdir().unwrap();
    let cache_dir = scratch.path().join("cache");
    let cache = ConnectorModelCache::new(&cache_dir);
    cache
        .write(
            ConnectorId::Codex,
            &CachedConnectorModels {
                fetched_at: Some(chrono::Utc::now() - chrono::Duration::hours(48)),
                last_attempt_at: Some(chrono::Utc::now() - chrono::Duration::hours(48)),
                source: "codex debug models".into(),
                models: vec![gritt_core::connector::ConnectorModel {
                    id: "gpt-5.4".into(),
                    display_label: Some("GPT-5.4".into()),
                }],
            },
        )
        .unwrap();
    let wrapper = fake_agent(scratch.path(), &[("FAKE_AGENT_MODELS_EXIT", "1".into())]);
    let fx = fixture(
        Vec::new(),
        ApprovalMode::DenyAll,
        vec![codex_connector_with_models(&wrapper, &cache_dir)],
    )
    .await;
    let outcome = fx.plane.connector_models(ConnectorId::Codex, true).await;
    let ConnectorModelDiscovery::CachedStale { catalog, .. } = outcome else {
        panic!("expected stale fallback, got {outcome:?}");
    };
    assert_eq!(catalog.freshness, ConnectorModelFreshness::Stale);
    assert_eq!(catalog.models[0].id, "gpt-5.4");
}

#[tokio::test]
async fn print_and_repl_list_catalog_entries_from_the_shared_service() {
    let scratch = tempfile::tempdir().unwrap();
    let wrapper = fake_agent(
        scratch.path(),
        &[(
            "FAKE_AGENT_MODELS_FILE",
            models_fixture("current.json").display().to_string(),
        )],
    );
    let cache = scratch.path().join("cache");
    let fx = fixture(
        Vec::new(),
        ApprovalMode::DenyAll,
        vec![codex_connector_with_models(&wrapper, &cache)],
    )
    .await;
    let discovery = fx.plane.connector_models(ConnectorId::Codex, false).await;
    let lines = ControlPlane::connector_model_lines(&discovery);
    let text = lines.join("\n");
    assert!(
        text.contains("gpt-5.4"),
        "catalog listing omitted model ids: {text}"
    );
    assert!(
        text.contains("codex debug models"),
        "catalog listing omitted the source: {text}"
    );
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("listed".into()),
            Some(ConnectorId::Codex),
            StartupRequest::from_flags(None, Some("gpt-5.4"), None),
            None,
            false,
        )
        .await
        .unwrap();
    let opened_lines = ControlPlane::connector_model_lines(
        opened
            .connector_models
            .as_ref()
            .expect("new connector session runs discovery"),
    );
    assert!(opened_lines.iter().any(|line| line.contains("gpt-5.4")));

    let refreshed = fx
        .plane
        .open_with(
            SessionSelector::Named("refreshed".into()),
            Some(ConnectorId::Codex),
            StartupRequest::from_flags(None, None, None),
            None,
            true,
        )
        .await
        .unwrap();
    let refreshed_lines = ControlPlane::connector_model_lines(
        refreshed
            .connector_models
            .as_ref()
            .expect("refresh uses the same discovery service"),
    );
    assert!(refreshed_lines.iter().any(|line| line.contains("gpt-5.4")));
}

#[tokio::test]
async fn repl_models_lists_the_connector_catalog() {
    let scratch = tempfile::tempdir().unwrap();
    let wrapper = fake_agent(
        scratch.path(),
        &[
            (
                "FAKE_AGENT_FIXTURE",
                connector_fixture("text").display().to_string(),
            ),
            (
                "FAKE_AGENT_MODELS_FILE",
                models_fixture("current.json").display().to_string(),
            ),
        ],
    );
    let cache = scratch.path().join("cache");
    let fx = fixture(
        Vec::new(),
        ApprovalMode::DenyAll,
        vec![codex_connector_with_models(&wrapper, &cache)],
    )
    .await;
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("repl-models".into()),
            Some(ConnectorId::Codex),
            StartupRequest::from_flags(None, Some("gpt-5.4"), None),
            None,
            false,
        )
        .await
        .unwrap();
    let script = "/models\n/models refresh\n/quit\n";
    let input = LineInput::from_reader(std::io::Cursor::new(script.as_bytes().to_vec()));
    let out = SharedBuffer::default();
    let err = SharedBuffer::default();
    let slot: CancelSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
    run_repl(
        &fx.plane,
        opened.driver,
        &input,
        out.clone(),
        err.clone(),
        PrintUiOptions::deny_all(false),
        slot,
    )
    .await
    .unwrap();
    let body = out.contents();
    assert!(
        body.contains("gpt-5.4"),
        "REPL /models did not list the catalog: {body}"
    );
    assert!(
        body.contains("codex debug models"),
        "REPL /models omitted the source: {body}"
    );
    assert!(
        body.matches("gpt-5.4").count() >= 2,
        "refresh did not list again: {body}"
    );
}

// -- version checks and updates (TKT-0025) ---------------------------------

/// The fake agent installed the way OpenCode's install script does it:
/// under `<home>/.opencode/bin/`, with a version file it reads and a
/// self-update that rewrites it.
fn opencode_vendor_fixture(home: &Path, update_exit: &str) -> (Arc<dyn Connector>, PathBuf) {
    let version_file = home.join("version.txt");
    std::fs::write(&version_file, "1.0.0\n").unwrap();
    let exe = home.join(".opencode/bin/opencode");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../gritt-connector/tests/fake-agent/agent.sh");
    let mut body = String::from("#!/bin/sh\n");
    for (name, value) in [
        (
            "FAKE_AGENT_VERSION_FILE",
            version_file.display().to_string(),
        ),
        ("FAKE_AGENT_UPDATE_TO", "2.0.0".to_owned()),
        ("FAKE_AGENT_UPDATE_EXIT", update_exit.to_owned()),
        (
            "FAKE_AGENT_FIXTURE",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../gritt-connector/tests/fixtures/opencode/text.jsonl")
                .display()
                .to_string(),
        ),
    ] {
        body.push_str(&format!("{name}='{value}'\nexport {name}\n"));
    }
    body.push_str(&format!("exec '{}' \"$@\"\n", script.display()));
    std::fs::write(&exe, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let settings = ConnectorSettings {
        executables: BTreeMap::from([("opencode".to_owned(), exe.display().to_string())]),
        ..ConnectorSettings::default()
    };
    let connector =
        ExternalConnector::new(gritt_connector::protocols::opencode::OpenCode, &settings)
            .with_timeouts(Timeouts {
                health: Duration::from_secs(5),
                startup: Duration::from_secs(10),
                idle: Duration::from_secs(10),
            })
            .with_install_env(gritt_connector::InstallEnv {
                home: Some(home.to_path_buf()),
                cargo_home: Some(home.join(".cargo")),
            });
    (Arc::new(connector), exe)
}

#[tokio::test]
async fn opening_a_connector_session_reports_its_version_offline() {
    let scratch = tempfile::tempdir().unwrap();
    let (connector, exe) = opencode_vendor_fixture(scratch.path(), "0");
    let fx = fixture(Vec::new(), ApprovalMode::DenyAll, vec![connector]).await;
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("versioned".into()),
            Some(ConnectorId::OpenCode),
            StartupRequest::from_flags(None, None, None),
            None,
            false,
        )
        .await
        .unwrap();
    let check = opened
        .connector_version
        .as_ref()
        .expect("a new connector session carries its offline version state");
    let status = check.status().expect("the fake agent is installed");
    assert_eq!(status.installed.as_deref(), Some("1.0.0"));
    assert_eq!(
        status.source,
        gritt_core::connector::InstallSource::Vendor {
            installer: "OpenCode install script".into()
        }
    );
    let action = status
        .update
        .as_ref()
        .expect("a vendor install offers its self-update");
    assert_eq!(action.program, exe.display().to_string());
    assert_eq!(action.args, vec!["upgrade"]);
    assert!(
        !check.update_available(),
        "no newest version is known, so nothing is offered as outdated"
    );
    let lines = ControlPlane::connector_version_lines(check);
    assert!(lines[0].contains("opencode 1.0.0"), "{lines:?}");
    assert!(
        lines.iter().any(|line| line.contains("update: ")),
        "{lines:?}"
    );

    let resumed = fx
        .plane
        .open_with(
            SessionSelector::Named("versioned".into()),
            Some(ConnectorId::OpenCode),
            StartupRequest::from_flags(None, None, None),
            None,
            false,
        )
        .await
        .unwrap();
    assert!(
        resumed.connector_version.is_none(),
        "a resumed session does not check again"
    );
    assert!(matches!(
        fx.plane
            .connector_version(ConnectorId::Native, VersionCheckMode::Cached)
            .await,
        ConnectorVersionCheck::Unsupported { .. }
    ));
}

#[tokio::test]
async fn repl_version_and_update_run_through_the_shared_service_after_a_yes() {
    let scratch = tempfile::tempdir().unwrap();
    let (connector, _exe) = opencode_vendor_fixture(scratch.path(), "0");
    let fx = fixture(Vec::new(), ApprovalMode::DenyAll, vec![connector]).await;
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("repl-update".into()),
            Some(ConnectorId::OpenCode),
            StartupRequest::from_flags(None, None, None),
            None,
            false,
        )
        .await
        .unwrap();
    let script = "/version\n/update\ny\n/version\n/quit\n";
    let input = LineInput::from_reader(std::io::Cursor::new(script.as_bytes().to_vec()));
    let out = SharedBuffer::default();
    let err = SharedBuffer::default();
    let slot: CancelSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
    run_repl(
        &fx.plane,
        opened.driver,
        &input,
        out.clone(),
        err.clone(),
        PrintUiOptions::deny_all(false),
        slot,
    )
    .await
    .unwrap();
    let body = out.contents();
    assert!(body.contains("opencode 1.0.0"), "{body}");
    assert!(
        body.contains("run `"),
        "the command is shown before the question: {body}"
    );
    assert!(body.contains("upgrade`? [y/N]"), "{body}");
    assert!(body.contains("opencode updated to 2.0.0"), "{body}");
    assert!(
        body.contains("opencode 2.0.0"),
        "the check after the update sees the new version: {body}"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("version.txt"))
            .unwrap()
            .trim(),
        "2.0.0"
    );
}

#[tokio::test]
async fn repl_update_declined_or_failed_leaves_the_connector_usable() {
    let scratch = tempfile::tempdir().unwrap();
    let (connector, _exe) = opencode_vendor_fixture(scratch.path(), "0");
    let fx = fixture(Vec::new(), ApprovalMode::DenyAll, vec![connector]).await;
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("repl-decline".into()),
            Some(ConnectorId::OpenCode),
            StartupRequest::from_flags(None, None, None),
            None,
            false,
        )
        .await
        .unwrap();
    let script = "/update\nno\n/update\n\nhello\n/quit\n";
    let input = LineInput::from_reader(std::io::Cursor::new(script.as_bytes().to_vec()));
    let out = SharedBuffer::default();
    let err = SharedBuffer::default();
    let slot: CancelSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
    run_repl(
        &fx.plane,
        opened.driver,
        &input,
        out.clone(),
        err.clone(),
        PrintUiOptions::deny_all(false),
        slot,
    )
    .await
    .unwrap();
    let body = out.contents();
    assert_eq!(
        body.matches("update declined; nothing was run").count(),
        2,
        "{body}"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("version.txt"))
            .unwrap()
            .trim(),
        "1.0.0",
        "a declined update must not run"
    );
    assert!(
        body.contains("PONG"),
        "the session still runs a turn afterwards: {body}"
    );

    let failing = tempfile::tempdir().unwrap();
    let (connector, _exe) = opencode_vendor_fixture(failing.path(), "1");
    let fx = fixture(Vec::new(), ApprovalMode::DenyAll, vec![connector]).await;
    let outcome = {
        let check = fx
            .plane
            .connector_version(ConnectorId::OpenCode, VersionCheckMode::Cached)
            .await;
        let action = check.status().unwrap().update.clone().unwrap();
        fx.plane
            .connector_update(ConnectorId::OpenCode, action)
            .await
    };
    assert!(
        matches!(outcome, ConnectorUpdateOutcome::Failed { .. }),
        "{outcome:?}"
    );
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("after-failure".into()),
            Some(ConnectorId::OpenCode),
            StartupRequest::from_flags(None, None, None),
            None,
            false,
        )
        .await
        .unwrap();
    assert_eq!(
        opened
            .connector_version
            .unwrap()
            .status()
            .unwrap()
            .installed
            .as_deref(),
        Some("1.0.0"),
        "a failed update leaves the connector usable at its old version"
    );
}
