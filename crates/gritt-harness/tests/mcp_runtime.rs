//! The MCP runtime against real child processes and a local HTTP endpoint.
//!
//! Every server here is a fake with an arbitrary name. Nothing in the
//! runtime may depend on a name, a vendor, or how many entries a file has,
//! so the fixtures use names no product code could know.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use gritt_core::mcp::{McpRuntimeSettings, McpServerSnapshot, McpServerState, TrustDecision};
use gritt_harness::mcp::trust::MemoryTrustStore;
use gritt_harness::mcp::McpRuntime;
use gritt_harness::CancellationToken;

const FIXTURE: &str = env!("CARGO_BIN_EXE_gritt-mcp-fixture");

/// Writes a `.mcp.json` with the given entries and returns the workspace.
fn workspace(entries: serde_json::Value) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({ "mcpServers": entries }).to_string(),
    )
    .unwrap();
    dir
}

/// One stdio entry running the fixture with `behavior`.
fn stdio(behavior: &str) -> serde_json::Value {
    serde_json::json!({"command": FIXTURE, "args": [behavior]})
}

fn settings() -> McpRuntimeSettings {
    McpRuntimeSettings {
        init_timeout: Duration::from_secs(10),
        call_timeout: Duration::from_secs(10),
        shutdown_grace: Duration::from_millis(300),
        ..McpRuntimeSettings::default()
    }
}

/// A runtime that trusts every entry, for the cases that are not about
/// trust itself.
fn runtime(root: &Path) -> McpRuntime {
    McpRuntime::new(root, settings()).with_trust(MemoryTrustStore::trust_all())
}

fn state_of<'a>(snapshots: &'a [McpServerSnapshot], name: &str) -> &'a McpServerState {
    &snapshots
        .iter()
        .find(|snapshot| snapshot.name == name)
        .unwrap_or_else(|| panic!("no entry named {name} in {snapshots:?}"))
        .state
}

fn snapshot<'a>(snapshots: &'a [McpServerSnapshot], name: &str) -> &'a McpServerSnapshot {
    snapshots
        .iter()
        .find(|snapshot| snapshot.name == name)
        .unwrap()
}

#[tokio::test]
async fn a_handshake_discovers_tools_and_a_call_returns_its_text() {
    let dir = workspace(serde_json::json!({"zeta-notes": stdio("basic")}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    assert_eq!(snapshots.len(), 1);
    let entry = snapshot(&snapshots, "zeta-notes");
    assert_eq!(entry.state, McpServerState::Ready);
    assert_eq!(entry.protocol_version.as_deref(), Some("2025-06-18"));
    assert_eq!(entry.server_version.as_deref(), Some("0.1.0"));
    assert_eq!(entry.tool_count, 2);
    assert_eq!(
        entry.tools,
        vec!["mcp__zeta-notes__search", "mcp__zeta-notes__echo"]
    );
    let tools = runtime.tool_set().await;
    assert_eq!(tools.len(), 2);
    let definition = tools
        .definitions()
        .iter()
        .find(|definition| definition.name == "mcp__zeta-notes__echo")
        .unwrap();
    assert_eq!(definition.description, "return the text given");
    assert_eq!(definition.parameters["type"], "object");
    let result = runtime
        .call(
            "mcp__zeta-notes__echo",
            &serde_json::json!({"text": "hello"}),
            &cancel,
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert_eq!(result.output, "echo: hello");
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_older_supported_revision_is_accepted_and_a_newer_one_is_refused() {
    let dir = workspace(serde_json::json!({
        "old-server": stdio("old"),
        "future-server": stdio("future"),
    }));
    let runtime = runtime(dir.path());
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    assert_eq!(
        snapshot(&snapshots, "old-server")
            .protocol_version
            .as_deref(),
        Some("2024-11-05")
    );
    let McpServerState::Failed { reason } = state_of(&snapshots, "future-server") else {
        panic!("{snapshots:?}");
    };
    assert!(reason.contains("2099-01-01"), "{reason}");
    assert!(reason.contains("2025-06-18"), "{reason}");
    runtime.shutdown().await;
}

#[tokio::test]
async fn discovery_follows_every_page_and_refuses_an_endless_cursor() {
    let dir = workspace(serde_json::json!({
        "paged-one": stdio("paged"),
        "endless": stdio("loop"),
    }));
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            max_list_pages: 5,
            ..settings()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    let paged = snapshot(&snapshots, "paged-one");
    assert_eq!(paged.state, McpServerState::Ready);
    assert_eq!(paged.tool_count, 3);
    assert!(paged.tools.contains(&"mcp__paged-one__third".to_string()));
    let McpServerState::Failed { reason } = state_of(&snapshots, "endless") else {
        panic!("{snapshots:?}");
    };
    assert!(reason.contains("cursor"), "{reason}");
    runtime.shutdown().await;
}

#[tokio::test]
async fn duplicate_tool_names_across_servers_stay_separately_callable() {
    let dir = workspace(serde_json::json!({
        "alpha": stdio("basic"),
        "omega": stdio("basic"),
    }));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let tools = runtime.tool_set().await;
    assert_eq!(tools.len(), 4);
    let alpha = tools.lookup("mcp__alpha__search").unwrap();
    let omega = tools.lookup("mcp__omega__search").unwrap();
    assert_eq!(alpha.tool, "search");
    assert_eq!(omega.tool, "search");
    assert_ne!(alpha.server, omega.server);
    let result = runtime
        .call(
            "mcp__omega__search",
            &serde_json::json!({"text": "x"}),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(result.output, "search: x");
    runtime.shutdown().await;
}

#[tokio::test]
async fn one_broken_server_never_disables_a_healthy_one() {
    let dir = workspace(serde_json::json!({
        "healthy": stdio("basic"),
        "exits": stdio("crash"),
        "noise": stdio("garbage"),
        "no-such-binary": {"command": "./definitely-not-here"},
        "legacy-sse": {"type": "sse", "url": "https://example.test/sse"},
        "needs-a-variable": {"command": FIXTURE, "args": ["basic"],
                             "env": {"REGION": "${GRITT_TEST_ABSENT_VARIABLE}"}},
        "literal-secret": {"command": FIXTURE, "env": {"API_KEY": "sk-must-not-appear"}},
    }));
    let runtime = runtime(dir.path());
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    // Every configured entry is accounted for, none silently omitted.
    assert_eq!(snapshots.len(), 7);
    assert_eq!(state_of(&snapshots, "healthy"), &McpServerState::Ready);
    assert_eq!(snapshot(&snapshots, "healthy").tool_count, 2);
    for name in ["exits", "noise", "no-such-binary"] {
        assert!(
            matches!(state_of(&snapshots, name), McpServerState::Failed { .. }),
            "{name}: {:?}",
            state_of(&snapshots, name)
        );
    }
    assert!(matches!(
        state_of(&snapshots, "legacy-sse"),
        McpServerState::UnsupportedTransport { .. }
    ));
    let McpServerState::Invalid { reason } = state_of(&snapshots, "needs-a-variable") else {
        panic!("{snapshots:?}");
    };
    assert!(reason.contains("GRITT_TEST_ABSENT_VARIABLE"), "{reason}");
    let McpServerState::Invalid { reason } = state_of(&snapshots, "literal-secret") else {
        panic!("{snapshots:?}");
    };
    assert!(!reason.contains("sk-must-not-appear"), "{reason}");
    // No snapshot may carry a secret anywhere.
    let json = serde_json::to_string(&snapshots).unwrap();
    assert!(!json.contains("sk-must-not-appear"), "{json}");
    // The healthy server is still usable.
    assert_eq!(runtime.tool_set().await.len(), 2);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_silent_server_stops_at_the_initialization_deadline() {
    let dir = workspace(serde_json::json!({"never-answers": stdio("silent")}));
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            init_timeout: Duration::from_millis(400),
            shutdown_grace: Duration::from_millis(200),
            ..settings()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());
    let started = std::time::Instant::now();
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    let McpServerState::Failed { reason } = state_of(&snapshots, "never-answers") else {
        panic!("{snapshots:?}");
    };
    assert!(reason.contains("not ready within"), "{reason}");
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_cancelled_call_stops_waiting_and_is_not_replayed() {
    let dir = workspace(serde_json::json!({"hangs": stdio("slowcall")}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        canceller.cancel();
    });
    let started = std::time::Instant::now();
    let error = runtime
        .call(
            "mcp__hangs__echo",
            &serde_json::json!({"text": "x"}),
            &cancel,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, gritt_core::ErrorKind::Cancelled);
    // The wait ended on cancellation, not on the ten-second deadline.
    assert!(started.elapsed() < Duration::from_secs(3));
    // The server is still connected: a cancelled call is not a failure, and
    // nothing retried it.
    let snapshots = runtime.snapshots().await;
    assert_eq!(state_of(&snapshots, "hangs"), &McpServerState::Ready);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_call_deadline_ends_the_wait_without_killing_the_server() {
    let dir = workspace(serde_json::json!({"hangs": stdio("slowcall")}));
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            call_timeout: Duration::from_millis(300),
            ..settings()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let error = runtime
        .call("mcp__hangs__echo", &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert!(error.message.contains("did not answer within"), "{error}");
    assert_eq!(
        state_of(&runtime.snapshots().await, "hangs"),
        &McpServerState::Ready
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn tool_errors_structured_output_and_unsupported_blocks_all_come_back() {
    let dir = workspace(serde_json::json!({
        "fails": stdio("toolerror"),
        "unknown": stdio("unknowntool"),
        "rich": stdio("structured"),
    }));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    // An execution error is a result, not a transport failure: the model
    // sees it and can correct itself.
    let failed = runtime
        .call("mcp__fails__echo", &serde_json::json!({}), &cancel)
        .await
        .unwrap();
    assert!(failed.is_error);
    assert_eq!(failed.output, "the upstream API rejected the query");
    // A protocol error is an error.
    let error = runtime
        .call("mcp__unknown__echo", &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert!(error.message.contains("Unknown tool"), "{error}");
    let rich = runtime
        .call("mcp__rich__echo", &serde_json::json!({}), &cancel)
        .await
        .unwrap();
    assert!(rich.output.contains("two rows"));
    assert!(rich
        .output
        .contains("[unsupported `image` content omitted]"));
    assert!(rich.output.contains("\"rows\": 2"));
    assert_eq!(rich.unsupported, vec!["image".to_string()]);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_server_with_no_tools_capability_is_ready_with_no_tools() {
    let dir = workspace(serde_json::json!({"quiet": stdio("notools")}));
    let runtime = runtime(dir.path());
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    assert_eq!(state_of(&snapshots, "quiet"), &McpServerState::Ready);
    assert_eq!(snapshot(&snapshots, "quiet").tool_count, 0);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_tool_list_change_is_applied_between_turns() {
    let dir = workspace(serde_json::json!({"changes": stdio("listchanged")}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    assert_eq!(runtime.tool_set().await.len(), 2);
    // The turn's own snapshot does not change under it.
    let during_turn = runtime.tool_set().await;
    runtime
        .call("mcp__changes__echo", &serde_json::json!({}), &cancel)
        .await
        .unwrap();
    assert_eq!(during_turn.len(), 2);
    assert!(during_turn.lookup("mcp__changes__extra").is_none());
    // Between turns the notification is applied.
    let snapshots = runtime.refresh(&cancel).await;
    assert_eq!(snapshot(&snapshots, "changes").tool_count, 2);
    let after = runtime.tool_set().await;
    assert!(after.lookup("mcp__changes__extra").is_some());
    runtime.shutdown().await;
}

#[tokio::test]
async fn the_child_gets_the_workspace_its_arguments_and_only_declared_variables() {
    std::env::set_var("GRITT_TEST_LEAK_API_KEY", "leaked-secret-7712");
    std::env::set_var("GRITT_TEST_FIXTURE_SOURCE", "from-the-environment");
    let dir = workspace(serde_json::json!({"env-probe": {
        "command": FIXTURE,
        "args": ["env", "--literal-$NOT_EXPANDED"],
        "env": {"FIXTURE_DECLARED": "${GRITT_TEST_FIXTURE_SOURCE}"},
    }}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let result = runtime
        .call("mcp__env-probe__echo", &serde_json::json!({}), &cancel)
        .await
        .unwrap();
    assert!(
        result.output.contains("declared=from-the-environment"),
        "{}",
        result.output
    );
    // An unrelated credential in Gritt's environment never reaches a server.
    assert!(result.output.contains("leaked=unset"), "{}", result.output);
    assert!(!result.output.contains("leaked-secret-7712"));
    // The argument array is passed verbatim; nothing expanded it.
    assert!(
        result.output.contains("--literal-$NOT_EXPANDED"),
        "{}",
        result.output
    );
    let canonical = dir.path().canonicalize().unwrap();
    assert!(
        result
            .output
            .contains(&canonical.to_string_lossy().to_string()),
        "{}",
        result.output
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn zero_entries_and_a_missing_file_both_mean_no_servers() {
    let empty = tempfile::tempdir().unwrap();
    let no_file = runtime(empty.path());
    assert!(no_file
        .open(&CancellationToken::new())
        .await
        .unwrap()
        .is_empty());

    let declared = workspace(serde_json::json!({}));
    let no_entries = runtime(declared.path());
    assert!(no_entries
        .open(&CancellationToken::new())
        .await
        .unwrap()
        .is_empty());

    let broken = tempfile::tempdir().unwrap();
    std::fs::write(broken.path().join(".mcp.json"), "{ not json").unwrap();
    let malformed = runtime(broken.path());
    let error = malformed.open(&CancellationToken::new()).await.unwrap_err();
    assert_eq!(error.kind, gritt_core::ErrorKind::Config);
    assert!(error.message.contains(".mcp.json"));
}

#[tokio::test]
async fn more_entries_than_the_concurrency_limit_all_start() {
    let mut entries = serde_json::Map::new();
    for index in 0..9 {
        entries.insert(format!("server-{index:02}-{}", index * 7), stdio("basic"));
    }
    let dir = workspace(serde_json::Value::Object(entries));
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            max_concurrent_init: 2,
            ..settings()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    assert_eq!(snapshots.len(), 9);
    assert!(snapshots
        .iter()
        .all(|snapshot| snapshot.state == McpServerState::Ready));
    assert_eq!(runtime.tool_set().await.len(), 18);
    runtime.shutdown().await;
}

#[tokio::test]
async fn reload_adds_removes_and_renames_while_keeping_healthy_connections() {
    let dir = workspace(serde_json::json!({
        "keep-me": stdio("basic"),
        "remove-me": stdio("basic"),
        "rename-me": stdio("basic"),
    }));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let before = runtime.child_pids().await;
    assert_eq!(before.len(), 3);
    let kept_pid = *runtime
        .snapshots()
        .await
        .iter()
        .find(|snapshot| snapshot.name == "keep-me")
        .map(|_| &before[0])
        .unwrap();
    let _ = kept_pid;

    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {
            "keep-me": {"command": FIXTURE, "args": ["basic"]},
            "renamed": {"command": FIXTURE, "args": ["basic"]},
            "added": {"command": FIXTURE, "args": ["paged"]},
        }})
        .to_string(),
    )
    .unwrap();
    runtime.reload().await.unwrap();
    let snapshots = runtime.snapshots().await;
    assert_eq!(snapshots.len(), 3);
    // The untouched entry never stopped.
    assert_eq!(state_of(&snapshots, "keep-me"), &McpServerState::Ready);
    assert_eq!(state_of(&snapshots, "renamed"), &McpServerState::Starting);
    assert_eq!(state_of(&snapshots, "added"), &McpServerState::Starting);
    // The removed entry is gone from the registry as well as the list.
    assert!(runtime
        .tool_set()
        .await
        .lookup("mcp__remove-me__search")
        .is_none());
    let snapshots = runtime.start(&cancel).await;
    assert!(snapshots
        .iter()
        .all(|snapshot| snapshot.state == McpServerState::Ready));
    assert_eq!(snapshot(&snapshots, "added").tool_count, 3);
    runtime.shutdown().await;
    assert!(all_gone(&before).await, "processes survived the reload run");
}

#[tokio::test]
async fn an_invalid_replacement_leaves_the_running_servers_alone() {
    let dir = workspace(serde_json::json!({"steady": stdio("basic")}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let pids = runtime.child_pids().await;
    std::fs::write(dir.path().join(".mcp.json"), "{ truncated").unwrap();
    let error = runtime.reload().await.unwrap_err();
    assert_eq!(error.kind, gritt_core::ErrorKind::Config);
    assert_eq!(
        state_of(&runtime.snapshots().await, "steady"),
        &McpServerState::Ready
    );
    assert_eq!(runtime.child_pids().await, pids);
    runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_leaves_no_child_process_behind_even_when_one_lingers() {
    let dir = workspace(serde_json::json!({
        "well-behaved": stdio("basic"),
        "hangs-on-exit": stdio("lingering"),
    }));
    let runtime = runtime(dir.path());
    runtime.open(&CancellationToken::new()).await.unwrap();
    let pids = runtime.child_pids().await;
    assert_eq!(pids.len(), 2);
    runtime.shutdown().await;
    assert!(runtime.child_pids().await.is_empty());
    assert!(all_gone(&pids).await, "an MCP child survived shutdown");
    let snapshots = runtime.snapshots().await;
    assert!(snapshots
        .iter()
        .all(|snapshot| snapshot.state == McpServerState::Stopped));
}

#[tokio::test]
async fn a_restart_replaces_the_process_and_its_tools() {
    let dir = workspace(serde_json::json!({"restartable": stdio("basic")}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let first = runtime.child_pids().await;
    runtime.restart("restartable", &cancel).await.unwrap();
    let second = runtime.child_pids().await;
    assert_eq!(second.len(), 1);
    assert_ne!(first, second);
    assert!(
        all_gone(&first).await,
        "the old process survived the restart"
    );
    assert_eq!(
        state_of(&runtime.snapshots().await, "restartable"),
        &McpServerState::Ready
    );
    assert_eq!(runtime.tool_set().await.len(), 2);
    runtime.shutdown().await;
}

#[tokio::test]
async fn reading_the_file_does_not_authorize_running_it() {
    let dir = workspace(serde_json::json!({"needs-consent": stdio("basic")}));
    let trust = MemoryTrustStore::new();
    let runtime = McpRuntime::new(dir.path(), settings()).with_trust(Arc::clone(&trust) as Arc<_>);
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    assert_eq!(
        state_of(&snapshots, "needs-consent"),
        &McpServerState::AwaitingApproval
    );
    assert!(runtime.child_pids().await.is_empty());
    assert!(runtime.tool_set().await.is_empty());

    // A refusal is remembered as a refusal.
    runtime
        .decide("needs-consent", TrustDecision::Denied)
        .await
        .unwrap();
    runtime.start(&cancel).await;
    assert!(runtime.child_pids().await.is_empty());
    assert_eq!(
        state_of(&runtime.snapshots().await, "needs-consent"),
        &McpServerState::Denied
    );

    runtime
        .decide("needs-consent", TrustDecision::Approved)
        .await
        .unwrap();
    let snapshots = runtime.start(&cancel).await;
    assert_eq!(
        state_of(&snapshots, "needs-consent"),
        &McpServerState::Ready
    );
    assert_eq!(runtime.child_pids().await.len(), 1);

    // Editing the entry invalidates the approval.
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {
            "needs-consent": {"command": FIXTURE, "args": ["paged"]}
        }})
        .to_string(),
    )
    .unwrap();
    runtime.reload().await.unwrap();
    assert_eq!(
        state_of(&runtime.snapshots().await, "needs-consent"),
        &McpServerState::AwaitingApproval
    );
    runtime.start(&cancel).await;
    assert!(runtime.child_pids().await.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_http_entry_without_a_transport_says_so_instead_of_pretending() {
    let dir = workspace(serde_json::json!({
        "remote": {"type": "http", "url": "https://example.invalid/mcp"}
    }));
    let runtime = runtime(dir.path());
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    let McpServerState::Failed { reason } = state_of(&snapshots, "remote") else {
        panic!("{snapshots:?}");
    };
    assert!(reason.contains("HTTP transport"), "{reason}");
}

/// True once none of `pids` names a live process.
async fn all_gone(pids: &[u32]) -> bool {
    for _ in 0..40 {
        if pids.iter().all(|pid| !is_alive(*pid)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    // `kill -0` reports whether the process exists without signalling it.
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_alive(_pid: u32) -> bool {
    false
}

/// A minimal Streamable HTTP MCP endpoint, so the HTTP path is exercised
/// over a real socket rather than a recorded fixture.
mod http_fixture {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// What the endpoint should do with the next request.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Mode {
        /// Answers `initialize` as JSON and everything else as SSE.
        Mixed,
        /// Forgets the session after the handshake.
        SessionLost,
    }

    /// One recorded request: `"<HTTP method> <JSON-RPC method>"` and the
    /// headers it carried.
    pub type SeenRequest = (String, BTreeMap<String, String>);

    pub struct Endpoint {
        pub url: String,
        pub seen: Arc<std::sync::Mutex<Vec<SeenRequest>>>,
    }

    pub async fn start(mode: Mode) -> Endpoint {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        tokio::spawn(async move {
            let mut calls = 0usize;
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let record = Arc::clone(&record);
                let mut buffer = vec![0u8; 16 * 1024];
                let read = match socket.read(&mut buffer).await {
                    Ok(0) | Err(_) => continue,
                    Ok(read) => read,
                };
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let (head, body) = request.split_once("\r\n\r\n").unwrap_or((&request, ""));
                let mut headers = BTreeMap::new();
                for line in head.lines().skip(1) {
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
                    }
                }
                let method = head.split_whitespace().next().unwrap_or("").to_owned();
                let message: serde_json::Value =
                    serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
                let rpc = message
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                record
                    .lock()
                    .unwrap()
                    .push((format!("{method} {rpc}"), headers.clone()));
                let id = message.get("id").cloned();
                let response = match (method.as_str(), rpc.as_str()) {
                    ("DELETE", _) => {
                        "HTTP/1.1 405 Method Not Allowed\r\ncontent-length: 0\r\n\r\n".to_owned()
                    }
                    (_, "initialize") => {
                        let payload = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {"tools": {"listChanged": true}},
                                "serverInfo": {"name": "http-fixture", "version": "9.9.9"},
                            }
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             mcp-session-id: session-abc\r\ncontent-length: {}\r\n\r\n{payload}",
                            payload.len()
                        )
                    }
                    (_, "tools/list") => {
                        let payload = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"tools": [{"name": "fetch",
                                "description": "fetch a page",
                                "inputSchema": {"type": "object"}}]}
                        });
                        // The response arrives on an SSE stream, after an
                        // unrelated notification the client must ignore.
                        sse(&[
                            serde_json::json!({"jsonrpc": "2.0",
                                "method": "notifications/message"}),
                            payload,
                        ])
                    }
                    (_, "tools/call") => {
                        calls += 1;
                        if mode == Mode::SessionLost {
                            "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_owned()
                        } else {
                            sse(&[serde_json::json!({"jsonrpc": "2.0", "id": id,
                                "result": {"content": [{"type": "text",
                                    "text": format!("call {calls}")}]}})])
                        }
                    }
                    // Notifications get 202 and no body.
                    _ => "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\n\r\n".to_owned(),
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        Endpoint {
            url: format!("http://127.0.0.1:{port}/mcp"),
            seen,
        }
    }

    fn sse(messages: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for (index, message) in messages.iter().enumerate() {
            body.push_str(&format!("id: {index}\ndata: {message}\n\n"));
        }
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
    }
}

#[tokio::test]
async fn a_streamable_http_server_handshakes_lists_and_calls() {
    let endpoint = http_fixture::start(http_fixture::Mode::Mixed).await;
    std::env::set_var("GRITT_TEST_MCP_HTTP_TOKEN", "Bearer http-secret-4471");
    let dir = workspace(serde_json::json!({"remote-docs": {
        "type": "http",
        "url": endpoint.url,
        "headers": {"Authorization": "${GRITT_TEST_MCP_HTTP_TOKEN}", "X-Region": "${MCP_REGION:-eu}"},
    }}));
    let transport: Arc<dyn gritt_provider::transport::HttpTransport> =
        Arc::new(gritt_provider::ReqwestTransport::new().unwrap());
    let runtime = McpRuntime::new(dir.path(), settings())
        .with_http_transport(transport)
        .with_trust(MemoryTrustStore::trust_all());
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    let entry = snapshot(&snapshots, "remote-docs");
    assert_eq!(entry.state, McpServerState::Ready);
    assert_eq!(entry.server_version.as_deref(), Some("9.9.9"));
    assert_eq!(entry.tool_count, 1);
    let result = runtime
        .call("mcp__remote-docs__fetch", &serde_json::json!({}), &cancel)
        .await
        .unwrap();
    assert_eq!(result.output, "call 1");

    let seen = endpoint.seen.lock().unwrap().clone();
    let initialize = seen
        .iter()
        .find(|(line, _)| line.ends_with("initialize"))
        .unwrap();
    assert_eq!(initialize.1["authorization"], "Bearer http-secret-4471");
    assert_eq!(initialize.1["x-region"], "eu");
    assert!(initialize.1["accept"].contains("text/event-stream"));
    assert!(initialize.1["accept"].contains("application/json"));
    assert!(!initialize.1.contains_key("mcp-session-id"));
    // Every later request carries the session and the negotiated revision.
    let later = seen
        .iter()
        .find(|(line, _)| line.ends_with("tools/list"))
        .unwrap();
    assert_eq!(later.1["mcp-session-id"], "session-abc");
    assert_eq!(later.1["mcp-protocol-version"], "2025-06-18");
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_lost_http_session_is_reported_and_never_retried() {
    let endpoint = http_fixture::start(http_fixture::Mode::SessionLost).await;
    let dir = workspace(serde_json::json!({"flaky": {"type": "http", "url": endpoint.url}}));
    let transport: Arc<dyn gritt_provider::transport::HttpTransport> =
        Arc::new(gritt_provider::ReqwestTransport::new().unwrap());
    let runtime = McpRuntime::new(dir.path(), settings())
        .with_http_transport(transport)
        .with_trust(MemoryTrustStore::trust_all());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let error = runtime
        .call("mcp__flaky__fetch", &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert!(error.message.contains("ended the MCP session"), "{error}");
    // Exactly one attempt was made; a call is never replayed after a
    // disconnect because the side effect may already have happened.
    let calls = endpoint
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|(line, _)| line.ends_with("tools/call"))
        .count();
    assert_eq!(calls, 1);
    assert!(matches!(
        state_of(&runtime.snapshots().await, "flaky"),
        McpServerState::Failed { .. }
    ));
    runtime.shutdown().await;
}
