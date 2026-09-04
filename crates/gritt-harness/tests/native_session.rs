//! Fixture-driven native sessions in print mode: planning, tool use with
//! every policy outcome, cancellation with a child process, resume with
//! continuation state, workspace boundaries, and content-safe telemetry.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gritt_core::config::Config;
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event, EventKind, StopReason};
use gritt_core::policy::{PolicyConfig, PolicyOutcome, PolicyRule};
use gritt_core::provider::{Protocol, ProviderProfile};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, Phase, SessionStore};
use gritt_core::tool::native;
use gritt_harness::agent::{AgentBuilder, ApprovalMode, SessionSelector, TurnStatus, Ui};
use gritt_harness::modes::print::{PrintUi, PrintUiOptions, SharedBuffer};
use gritt_harness::policy::Decision;
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_provider::models::ModelCatalog;
use gritt_provider::{FixtureResponse, FixtureTransport, StaticKey};

const KEY: &str = "fixture-key-never-printed";

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

fn config(policy: Option<PolicyConfig>) -> Config {
    let mut config = Config::default();
    config.profiles.insert(
        "openrouter".into(),
        ProviderProfile {
            name: "openrouter".into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://openrouter.ai/api/v1".into(),
            key: SecretRef::for_profile("openrouter", "OPENROUTER_API_KEY"),
            aliases: Default::default(),
        },
    );
    config.default_profile = Some("openrouter".into());
    config.default_model = Some("openai/gpt-5-nano".into());
    if let Some(policy) = policy {
        config.policy = policy;
    }
    config
}

struct Fixture {
    _dir: tempfile::TempDir,
    builder: AgentBuilder,
    transport: Arc<FixtureTransport>,
}

async fn fixture_builder(
    responses: Vec<FixtureResponse>,
    approval: ApprovalMode,
    policy: Option<PolicyConfig>,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "# Readme\nhi\n").unwrap();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
    let config = config(policy);
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
        approval,
    };
    Fixture {
        _dir: dir,
        builder,
        transport,
    }
}

/// Records events and answers approvals from a script.
#[derive(Default)]
struct RecordingUi {
    events: Vec<Event>,
    answers: Vec<ApprovalDecision>,
    asked: Vec<(ApprovalRequest, Decision, Option<String>)>,
}

impl Ui for RecordingUi {
    fn event(&mut self, event: &Event) {
        self.events.push(event.clone());
    }

    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        decision: &'a Decision,
        preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        self.asked.push((
            request.clone(),
            decision.clone(),
            preview.map(str::to_owned),
        ));
        let answer = if self.answers.is_empty() {
            ApprovalDecision::Denied
        } else {
            self.answers.remove(0)
        };
        Box::pin(async move { answer })
    }
}

fn kinds(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match &event.kind {
            EventKind::TextDelta { .. } => "text",
            EventKind::ReasoningSummary { .. } => "reasoning",
            EventKind::ToolCall { .. } => "tool_call",
            EventKind::ToolResult { .. } => "tool_result",
            EventKind::ApprovalRequested { .. } => "approval_requested",
            EventKind::ApprovalDecided { .. } => "approval_decided",
            EventKind::Usage { .. } => "usage",
            EventKind::StatusChanged { .. } => "status",
            EventKind::Error { .. } => "error",
            EventKind::Completed { .. } => "completed",
            EventKind::Cancelled => "cancelled",
        })
        .collect()
}

fn tool_results(events: &[Event]) -> Vec<(String, bool, String)> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolResult { result } => {
                Some((result.name.clone(), result.is_error, result.output.clone()))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn planning_turn_streams_text_without_tools_and_keeps_telemetry_content_free() {
    let fx = fixture_builder(
        vec![FixtureResponse::sse(fixture("stream-text.sse"))],
        ApprovalMode::Ask,
        None,
    )
    .await;
    let mut agent = fx
        .builder
        .open(SessionSelector::Named("plan".into()), None, None, None)
        .await
        .unwrap();
    assert_eq!(agent.phase(), Phase::Planning);
    let prompt = "zebra-quilt-9931 tell me about the readme";
    let mut ui = RecordingUi::default();
    let outcome = agent.run_turn(prompt, &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(outcome.text, "Hello, world");
    assert_eq!(outcome.usage.input_tokens, Some(10));
    let request = fx.transport.requests().remove(0);
    let body = request.body_json().unwrap();
    assert!(body.get("tools").is_none(), "planning sends no tools");
    assert_eq!(body["messages"][0]["role"], "system");
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("planning"));
    assert_eq!(body["messages"][1]["content"], prompt);

    let stored = fx
        .builder
        .store
        .read_events(&agent.session().id)
        .await
        .unwrap();
    assert_eq!(stored.len(), ui.events.len());
    for pair in stored.windows(2) {
        assert!(pair[1].sequence > pair[0].sequence);
    }
    assert!(kinds(&stored).contains(&"completed"));
    let telemetry_text = fx.builder.telemetry.dump_text().await.unwrap();
    assert!(telemetry_text.contains("turn"));
    assert!(telemetry_text.contains("completed"));
    assert!(!telemetry_text.contains("zebra-quilt-9931"));
    assert!(!telemetry_text.contains("Hello, world"));
    assert!(!telemetry_text.contains(KEY));
    assert_eq!(fx.builder.telemetry.content_rows().await.unwrap(), 0);
}

#[tokio::test]
async fn coding_turn_reads_a_file_under_the_allow_rule_and_continues() {
    let fx = fixture_builder(
        vec![
            FixtureResponse::sse(fixture("stream-tool-call.sse")),
            FixtureResponse::sse(fixture("stream-tool-result.sse")),
        ],
        ApprovalMode::Ask,
        None,
    )
    .await;
    let mut agent = fx
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let mut ui = RecordingUi::default();
    let outcome = agent.run_turn("read the readme", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(outcome.text, "The README says hi.");
    assert_eq!(outcome.tool_calls, 1);
    assert!(ui.asked.is_empty(), "reads inside the workspace do not ask");
    let results = tool_results(&ui.events);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, native::FILE_READ);
    assert!(!results[0].1);
    assert_eq!(results[0].2, "# Readme\nhi\n");
    let requests = fx.transport.requests();
    assert_eq!(requests.len(), 2);
    let first = requests[0].body_json().unwrap();
    assert_eq!(first["tools"].as_array().unwrap().len(), 3);
    let second = requests[1].body_json().unwrap();
    let messages = second["messages"].as_array().unwrap();
    let tool_message = messages.last().unwrap();
    assert_eq!(tool_message["role"], "tool");
    assert_eq!(tool_message["content"], "# Readme\nhi\n");
    let k = kinds(&ui.events);
    assert!(k.contains(&"tool_call"));
    assert!(k.contains(&"tool_result"));
    assert_eq!(k.last(), Some(&"status"));
    assert!(ui.events.iter().any(|e| matches!(
        e.kind,
        EventKind::Completed {
            stop_reason: StopReason::EndTurn
        }
    )));
}

#[tokio::test]
async fn ask_approve_writes_the_file_after_showing_a_diff() {
    let fx = fixture_builder(
        vec![
            FixtureResponse::sse(tool_call_sse(
                native::FILE_WRITE,
                serde_json::json!({"path": "notes.txt", "content": "hello\n"}),
            )),
            FixtureResponse::sse(fixture("stream-text.sse")),
        ],
        ApprovalMode::Ask,
        None,
    )
    .await;
    let mut agent = fx
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let mut ui = RecordingUi {
        answers: vec![ApprovalDecision::Approved],
        ..Default::default()
    };
    let outcome = agent.run_turn("write notes", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(ui.asked.len(), 1);
    let (request, decision, preview) = &ui.asked[0];
    assert_eq!(request.tool, native::FILE_WRITE);
    assert_eq!(decision.outcome, PolicyOutcome::Ask);
    assert!(preview.as_deref().unwrap().contains("+hello"));
    let written = std::fs::read_to_string(fx.builder.workspace_root().join("notes.txt")).unwrap();
    assert_eq!(written, "hello\n");
    let k = kinds(&ui.events);
    assert!(k.contains(&"approval_requested"));
    assert!(k.contains(&"approval_decided"));
    let results = tool_results(&ui.events);
    assert!(!results[0].1);
}

#[tokio::test]
async fn ask_deny_reports_the_refusal_to_the_model() {
    let fx = fixture_builder(
        vec![
            FixtureResponse::sse(tool_call_sse(
                native::SHELL,
                serde_json::json!({"command": "rm -rf build"}),
            )),
            FixtureResponse::sse(fixture("stream-text.sse")),
        ],
        ApprovalMode::Ask,
        None,
    )
    .await;
    let mut agent = fx
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let mut ui = RecordingUi {
        answers: vec![ApprovalDecision::Denied],
        ..Default::default()
    };
    let outcome = agent.run_turn("clean up", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert!(ui.asked[0].1.destructive, "rm -rf is flagged destructive");
    let results = tool_results(&ui.events);
    assert!(results[0].1);
    assert!(results[0].2.contains("declined"));
    let second = fx.transport.requests()[1].body_json().unwrap();
    let tool_message = second["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert!(tool_message["content"]
        .as_str()
        .unwrap()
        .contains("declined"));
    assert!(!fx.builder.workspace_root().join("build").exists());
}

#[tokio::test]
async fn deny_all_mode_and_policy_deny_never_ask() {
    // A policy that denies reads outright, plus deny-all approvals.
    let mut policy = PolicyConfig::workspace_defaults();
    policy.rules.insert(
        0,
        PolicyRule {
            tool: native::FILE_READ.into(),
            resource: "*".into(),
            outcome: PolicyOutcome::Deny,
            reason: "reads are off".into(),
        },
    );
    let fx = fixture_builder(
        vec![
            FixtureResponse::sse(fixture("stream-tool-call.sse")),
            FixtureResponse::sse(tool_call_sse(
                native::SHELL,
                serde_json::json!({"command": "echo hi"}),
            )),
            FixtureResponse::sse(fixture("stream-text.sse")),
        ],
        ApprovalMode::DenyAll,
        Some(policy),
    )
    .await;
    let mut agent = fx
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let mut ui = RecordingUi::default();
    let outcome = agent.run_turn("do things", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert!(ui.asked.is_empty(), "deny-all never reaches the interface");
    let results = tool_results(&ui.events);
    assert_eq!(results.len(), 2);
    assert!(results[0].1 && results[0].2.contains("denied by policy"));
    assert!(results[1].1 && results[1].2.contains("declined"));
    let k = kinds(&ui.events);
    assert_eq!(k.iter().filter(|k| **k == "approval_requested").count(), 1);
}

#[tokio::test]
async fn approve_all_runs_shell_and_workspace_escapes_are_rejected() {
    let fx = fixture_builder(
        vec![
            FixtureResponse::sse(tool_call_sse(
                native::SHELL,
                serde_json::json!({"command": "printf ok"}),
            )),
            FixtureResponse::sse(tool_call_sse(
                native::FILE_READ,
                serde_json::json!({"path": "../../etc/passwd"}),
            )),
            FixtureResponse::sse(tool_call_sse(
                native::FILE_WRITE,
                serde_json::json!({"path": "/tmp/gritt-escape.txt", "content": "x"}),
            )),
            FixtureResponse::sse(fixture("stream-text.sse")),
        ],
        ApprovalMode::ApproveAll,
        None,
    )
    .await;
    let mut agent = fx
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let mut ui = RecordingUi::default();
    let outcome = agent.run_turn("go", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    let results = tool_results(&ui.events);
    assert_eq!(results.len(), 3);
    assert!(!results[0].1);
    assert_eq!(results[0].2, "ok");
    assert!(results[1].1 && results[1].2.contains("outside the workspace"));
    assert!(results[2].1 && results[2].2.contains("outside the workspace"));
    assert!(!std::path::Path::new("/tmp/gritt-escape.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_stops_the_stream_and_the_child_process() {
    let marker = format!("gritt-session-cancel-{}", std::process::id());
    let fx = fixture_builder(
        vec![FixtureResponse::sse(tool_call_sse(
            native::SHELL,
            serde_json::json!({"command": format!("sleep 30 # {marker}")}),
        ))],
        ApprovalMode::ApproveAll,
        None,
    )
    .await;
    let mut agent = fx
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let handle = agent.handle();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        handle.cancel();
    });
    let mut ui = RecordingUi::default();
    let outcome = agent.run_turn("wait", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Cancelled);
    assert!(kinds(&ui.events).contains(&"cancelled"));
    assert_eq!(
        fx.transport.request_count(),
        1,
        "no continuation after cancel"
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let ps = std::process::Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&ps.stdout);
    assert!(
        !listing
            .lines()
            .any(|line| line.contains(&marker) && !line.contains("ps ")),
        "child survived cancellation"
    );
    let telemetry_text = fx.builder.telemetry.dump_text().await.unwrap();
    assert!(telemetry_text.contains("cancelled"));
}

#[tokio::test]
async fn resume_restores_continuation_and_skips_the_system_prompt() {
    let fx = fixture_builder(
        vec![
            FixtureResponse::sse(fixture("stream-text.sse")),
            FixtureResponse::sse(fixture("stream-text.sse")),
        ],
        ApprovalMode::Ask,
        None,
    )
    .await;
    let mut first = fx
        .builder
        .open(SessionSelector::Named("work".into()), None, None, None)
        .await
        .unwrap();
    let mut ui = RecordingUi::default();
    first.run_turn("first", &mut ui).await.unwrap();
    let events_after_first = ui.events.len();
    let id = first.session().id.clone();
    drop(first);

    let mut resumed = fx
        .builder
        .open(
            SessionSelector::Named("work".into()),
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    assert_eq!(resumed.session().id, id);
    assert_eq!(resumed.phase(), Phase::Coding);
    let mut ui = RecordingUi::default();
    let outcome = resumed.run_turn("second", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    let second = fx.transport.requests()[1].body_json().unwrap();
    let messages = second["messages"].as_array().unwrap();
    // System prompt, first user turn, first assistant turn, second user turn.
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["content"], "first");
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[3]["content"], "second");
    let stored = fx.builder.store.read_events(&id).await.unwrap();
    assert!(stored.len() > events_after_first);
    assert_eq!(stored.first().unwrap().sequence, 0);
    assert_eq!(stored.last().unwrap().sequence as usize, stored.len() - 1);
    let sessions = fx.builder.store.list().await.unwrap();
    assert_eq!(sessions.len(), 1);
}

#[tokio::test]
async fn print_mode_writes_text_to_stdout_and_activity_to_stderr() {
    let fx = fixture_builder(
        vec![
            FixtureResponse::sse(fixture("stream-tool-call.sse")),
            FixtureResponse::sse(fixture("stream-tool-result.sse")),
        ],
        ApprovalMode::Ask,
        None,
    )
    .await;
    let mut agent = fx
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let out = SharedBuffer::default();
    let err = SharedBuffer::default();
    let asked = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&asked);
    let options = PrintUiOptions {
        verbose: false,
        prompter: Arc::new(move |_, _, _| {
            *counter.lock().unwrap() += 1;
            ApprovalDecision::Approved
        }),
    };
    let mut ui = PrintUi::new(out.clone(), err.clone(), options);
    let outcome = agent.run_turn("read it", &mut ui).await.unwrap();
    ui.finish();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(out.contents(), "The README says hi.\n");
    let activity = err.contents();
    assert!(activity.contains("-> file_read README.md"));
    assert!(activity.contains("<- file_read ok"));
    assert_eq!(*asked.lock().unwrap(), 0);
    let _labels: BTreeMap<String, String> = BTreeMap::new();
}
