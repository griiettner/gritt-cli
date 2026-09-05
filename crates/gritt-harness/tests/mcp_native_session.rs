//! A native session calling an approved MCP tool, and the same call denied.
//!
//! This is the end of the dispatch path: the provider adapter sees an
//! ordinary function tool, the permission engine gates it like any other,
//! and a denied call leaves no trace on the server.

use std::sync::Arc;

use gritt_core::config::Config;
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event, EventKind};
use gritt_core::mcp::McpRuntimeSettings;
use gritt_core::policy::{PolicyConfig, PolicyOutcome, PolicyRule};
use gritt_core::provider::{Protocol, ProviderProfile};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, Phase};
use gritt_core::tool::native;
use gritt_harness::agent::{AgentBuilder, ApprovalMode, SessionSelector, TurnStatus, Ui};
use gritt_harness::control::ControlPlane;
use gritt_harness::mcp::trust::MemoryTrustStore;
use gritt_harness::mcp::McpRuntime;
use gritt_harness::policy::Decision;
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_harness::CancellationToken;
use gritt_provider::models::ModelCatalog;
use gritt_provider::{FixtureResponse, FixtureTransport, StaticKey};

const FIXTURE: &str = env!("CARGO_BIN_EXE_gritt-mcp-fixture");
const KEY: &str = "fixture-key-never-printed";

/// A streamed response that asks for one tool call.
fn tool_call_sse(tool: &str, arguments: serde_json::Value) -> Vec<u8> {
    let first = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": null,
            "tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": tool, "arguments": arguments.to_string()}}]},
            "finish_reason": null}]
    });
    let second = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {first}\n\ndata: {second}\n\ndata: [DONE]\n\n").into_bytes()
}

fn text_sse(text: &str) -> Vec<u8> {
    let chunk = serde_json::json!({
        "id": "chatcmpl-f", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": "stop"}]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n").into_bytes()
}

fn config(policy: PolicyConfig) -> Config {
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
    config.policy = policy;
    config
}

/// Records events and answers approvals from a script.
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

struct Fixture {
    dir: tempfile::TempDir,
    plane: ControlPlane,
    marker: std::path::PathBuf,
}

/// A workspace with one MCP server that records every tool call it receives.
async fn fixture(policy: PolicyConfig, approval: ApprovalMode) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("tool-was-called");
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {"probe": {
            "command": FIXTURE,
            "args": ["marker"],
            "env": {"FIXTURE_MARKER": marker.to_string_lossy()},
        }}})
        .to_string(),
    )
    .unwrap();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
    let config = config(policy);
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    let responses = vec![
        FixtureResponse::sse(tool_call_sse(
            "mcp__probe__echo",
            serde_json::json!({"text": "hi"}),
        )),
        FixtureResponse::sse(text_sse("done")),
    ];
    let transport = Arc::new(FixtureTransport::new(responses, 17));
    let mcp = Arc::new(
        McpRuntime::new(
            Workspace::open(dir.path()).unwrap().root(),
            McpRuntimeSettings::default(),
        )
        .with_trust(MemoryTrustStore::trust_all()),
    );
    mcp.open(&CancellationToken::new()).await.unwrap();
    let builder = AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(StaticKey(Secret::new(KEY))),
        transport,
        catalog: ModelCatalog::new(),
        cache: None,
        workspace: Workspace::open(dir.path()).unwrap(),
        approval,
        mcp: Some(mcp),
    };
    Fixture {
        dir,
        plane: ControlPlane::native(Arc::new(builder)),
        marker,
    }
}

fn rule(tool: &str, outcome: PolicyOutcome) -> PolicyRule {
    PolicyRule {
        tool: tool.into(),
        resource: "*".into(),
        outcome,
        reason: "test rule".into(),
    }
}

async fn open(fixture: &Fixture) -> Box<dyn gritt_harness::driver::Driver> {
    fixture
        .plane
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn an_approved_mcp_tool_runs_through_the_permission_engine() {
    let policy = PolicyConfig {
        rules: vec![rule("mcp__*", PolicyOutcome::Ask)],
        fallback: PolicyOutcome::Deny,
    };
    let fixture = fixture(policy, ApprovalMode::Ask).await;
    let mut driver = open(&fixture).await;
    let mut ui = RecordingUi {
        answers: vec![ApprovalDecision::Approved],
        ..RecordingUi::default()
    };
    let outcome = driver.run_turn("search the notes", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    // The prompt named the server and the tool, not the dispatch name only.
    assert_eq!(ui.asked.len(), 1);
    assert_eq!(ui.asked[0].tool, "mcp__probe__echo");
    assert_eq!(ui.asked[0].resource, "mcp:probe/echo");
    let results = tool_results(&ui.events);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "mcp__probe__echo");
    assert!(!results[0].1, "{results:?}");
    assert_eq!(results[0].2, "echo ran");
    // The server really ran it.
    assert!(fixture.marker.exists());
    fixture.plane.builder.mcp().unwrap().shutdown().await;
}

#[tokio::test]
async fn a_denied_mcp_call_never_reaches_the_server() {
    let policy = PolicyConfig {
        rules: vec![rule("mcp__*", PolicyOutcome::Deny)],
        fallback: PolicyOutcome::Deny,
    };
    let fixture = fixture(policy, ApprovalMode::Ask).await;
    let mut driver = open(&fixture).await;
    let mut ui = RecordingUi::default();
    let outcome = driver.run_turn("search the notes", &mut ui).await.unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    // A denied call is never asked about and never executed.
    assert!(ui.asked.is_empty());
    let results = tool_results(&ui.events);
    assert_eq!(results.len(), 1);
    assert!(results[0].1);
    assert!(results[0].2.contains("denied by policy"), "{results:?}");
    assert!(
        !fixture.marker.exists(),
        "the server was reached despite the denial"
    );
    fixture.plane.builder.mcp().unwrap().shutdown().await;
}

#[tokio::test]
async fn a_declined_approval_also_leaves_the_server_untouched() {
    let policy = PolicyConfig {
        rules: vec![rule("mcp__*", PolicyOutcome::Ask)],
        fallback: PolicyOutcome::Deny,
    };
    let fixture = fixture(policy, ApprovalMode::Ask).await;
    let mut driver = open(&fixture).await;
    let mut ui = RecordingUi {
        answers: vec![ApprovalDecision::Denied],
        ..RecordingUi::default()
    };
    driver.run_turn("search the notes", &mut ui).await.unwrap();
    let results = tool_results(&ui.events);
    assert!(results[0].2.contains("the user declined"), "{results:?}");
    assert!(!fixture.marker.exists());
    fixture.plane.builder.mcp().unwrap().shutdown().await;
}

#[tokio::test]
async fn mcp_tools_reach_the_provider_in_coding_and_never_in_planning() {
    let fixture = fixture(PolicyConfig::workspace_defaults(), ApprovalMode::DenyAll).await;
    let agent = fixture
        .plane
        .builder
        .open(
            SessionSelector::New { name: None },
            None,
            None,
            Some(Phase::Coding),
        )
        .await
        .unwrap();
    let request = agent.request_preview("do the thing");
    let names: Vec<&str> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    assert!(names.contains(&native::FILE_READ));
    assert!(names.contains(&"mcp__probe__echo"));
    assert!(names.contains(&"mcp__probe__search"));
    // Schemas travel as ordinary function parameters; no adapter knows MCP.
    let echo = request
        .tools
        .iter()
        .find(|tool| tool.name == "mcp__probe__echo")
        .unwrap();
    assert_eq!(echo.parameters["type"], "object");

    let planning = fixture
        .plane
        .builder
        .open(
            SessionSelector::New {
                name: Some("planner".into()),
            },
            None,
            None,
            Some(Phase::Planning),
        )
        .await
        .unwrap();
    assert!(planning.request_preview("do the thing").tools.is_empty());
    let _ = &fixture.dir;
    fixture.plane.builder.mcp().unwrap().shutdown().await;
}
