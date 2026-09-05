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

/// Deadlines for the cases that are not about deadlines. They are set well
/// above anything these tests wait for, so a loaded machine cannot turn a
/// cancellation into a timeout or a slow start into a failure. The tests that
/// do exercise a deadline set their own, much shorter, value.
fn settings() -> McpRuntimeSettings {
    McpRuntimeSettings {
        init_timeout: Duration::from_secs(30),
        call_timeout: Duration::from_secs(60),
        shutdown_grace: Duration::from_millis(300),
        ..McpRuntimeSettings::default()
    }
}

/// A runtime that trusts every entry, for the cases that are not about
/// trust itself.
fn runtime(root: &Path) -> McpRuntime {
    McpRuntime::new(root, settings()).with_trust(MemoryTrustStore::trust_all())
}

/// Calls a tool the way a turn does: freeze the set, look the name up, then
/// dispatch that exact identity.
async fn call(
    runtime: &McpRuntime,
    name: &str,
    arguments: serde_json::Value,
    cancel: &CancellationToken,
) -> gritt_core::Result<gritt_harness::mcp::registry::RenderedResult> {
    let tools = runtime.tool_set().await;
    let frozen = tools
        .lookup(name)
        .cloned()
        .ok_or_else(|| gritt_core::Error::config(format!("unknown tool `{name}`")))?;
    runtime.call(&frozen, &arguments, cancel).await
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
    let result = call(
        &runtime,
        "mcp__zeta-notes__echo",
        serde_json::json!({"text": "hello"}),
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
    assert_eq!(alpha.reference.tool, "search");
    assert_eq!(omega.reference.tool, "search");
    assert_ne!(alpha.reference.server, omega.reference.server);
    let result = call(
        &runtime,
        "mcp__omega__search",
        serde_json::json!({"text": "x"}),
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
    // The point is that the deadline fired rather than the default 30s one;
    // the bound is loose so a loaded machine cannot fail it.
    let started = std::time::Instant::now();
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
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
    let error = call(
        &runtime,
        "mcp__hangs__echo",
        serde_json::json!({"text": "x"}),
        &cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, gritt_core::ErrorKind::Cancelled);
    // The wait ended on cancellation, not on the call deadline, which is set
    // far enough above this bound that load cannot confuse the two.
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
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
    let error = call(&runtime, "mcp__hangs__echo", serde_json::json!({}), &cancel)
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
    let failed = call(&runtime, "mcp__fails__echo", serde_json::json!({}), &cancel)
        .await
        .unwrap();
    assert!(failed.is_error);
    assert_eq!(failed.output, "the upstream API rejected the query");
    // A protocol error is an error.
    let error = call(
        &runtime,
        "mcp__unknown__echo",
        serde_json::json!({}),
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("Unknown tool"), "{error}");
    let rich = call(&runtime, "mcp__rich__echo", serde_json::json!({}), &cancel)
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
    call(
        &runtime,
        "mcp__changes__echo",
        serde_json::json!({}),
        &cancel,
    )
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
    let result = call(
        &runtime,
        "mcp__env-probe__echo",
        serde_json::json!({}),
        &cancel,
    )
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
    assert_all_gone(&before, "processes survived the reload run").await;
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
    assert_all_gone(&pids, "an MCP child survived shutdown").await;
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
///
/// Generous on purpose: this polls for an outcome rather than asserting a
/// duration, so a loaded machine makes it slower, never wrong.
///
/// Only meaningful on unix, where `kill -0` gives a liveness probe with no
/// dependency. Callers go through [`assert_all_gone`], which says so out loud
/// instead of passing silently elsewhere.
#[cfg(unix)]
async fn all_gone(pids: &[u32]) -> bool {
    for _ in 0..150 {
        if pids.iter().all(|pid| !is_alive(*pid)) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Asserts every pid is gone, or reports the check as skipped.
async fn assert_all_gone(pids: &[u32], message: &str) {
    #[cfg(unix)]
    assert!(all_gone(pids).await, "{message}");
    #[cfg(not(unix))]
    {
        let _ = (pids, message);
        eprintln!("skipped: process liveness is only checked on unix");
    }
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
        /// Reflects the credential it was given into its metadata, its tool
        /// descriptions, and its results.
        EchoSecret,
        /// Accepts requests but never answers a notification POST, so a
        /// cancellation notification hangs.
        StallNotifications,
        /// Sends a server-initiated `ping` on the `tools/list` stream and
        /// records the reply it receives.
        ServerRequests,
    }

    /// One recorded request: `"<HTTP method> <JSON-RPC method>"` and the
    /// headers it carried.
    pub type SeenRequest = (String, BTreeMap<String, String>);

    pub struct Endpoint {
        pub url: String,
        pub seen: Arc<std::sync::Mutex<Vec<SeenRequest>>>,
        /// Bodies of the POSTs the client made on its own account, so a test
        /// can see the answer to a server request and its headers.
        pub replies: Arc<std::sync::Mutex<Vec<SeenRequest>>>,
    }

    pub async fn start(mode: Mode) -> Endpoint {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let replies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        let record_replies = Arc::clone(&replies);
        tokio::spawn(async move {
            let mut calls = 0usize;
            let mut initialized = false;
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
                // The transport contract: every POST must offer both body
                // shapes, and every post-handshake request must carry the
                // negotiated revision and the assigned session.
                if method == "POST" {
                    let accept = headers.get("accept").cloned().unwrap_or_default();
                    assert!(
                        accept.contains("application/json") && accept.contains("text/event-stream"),
                        "POST without both accepted content types: {accept:?}"
                    );
                }
                let secret = headers
                    .get("authorization")
                    .cloned()
                    .unwrap_or_else(|| "no-secret".into());
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
                // A body with an id but no method is the client answering a
                // request this server made.
                if rpc.is_empty() && message.get("id").is_some() {
                    record_replies
                        .lock()
                        .unwrap()
                        .push((body.to_owned(), headers.clone()));
                }
                let id = message.get("id").cloned();
                let response = match (method.as_str(), rpc.as_str()) {
                    ("DELETE", _) => {
                        "HTTP/1.1 405 Method Not Allowed\r\nconnection: close\r\ncontent-length: 0\r\n\r\n".to_owned()
                    }
                    (_, "initialize") => {
                        let version = if mode == Mode::EchoSecret {
                            format!("9.9.9 issued for {secret}")
                        } else {
                            "9.9.9".to_owned()
                        };
                        let payload = serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {
                                "protocolVersion": "2025-06-18",
                                "capabilities": {"tools": {"listChanged": true}},
                                "serverInfo": {"name": "http-fixture", "version": version},
                            }
                        })
                        .to_string();
                        format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             mcp-session-id: session-abc\r\nconnection: close\r\n\
                             content-length: {}\r\n\r\n{payload}",
                            payload.len()
                        )
                    }
                    (_, "tools/list") => {
                        // The lifecycle says operation only begins after the
                        // client has confirmed initialization.
                        assert!(
                            initialized,
                            "tools/list arrived before notifications/initialized"
                        );
                        if mode == Mode::ServerRequests {
                            // The response the caller waits for, preceded by
                            // a request the client has to answer on its own.
                            let stream = sse(&[
                                serde_json::json!({"jsonrpc": "2.0", "id": "srv-1",
                                    "method": "ping"}),
                                serde_json::json!({"jsonrpc": "2.0", "id": id,
                                    "result": {"tools": [{"name": "fetch",
                                        "description": "fetch a page",
                                        "inputSchema": {"type": "object"}}]}}),
                            ]);
                            let _ = socket.write_all(stream.as_bytes()).await;
                            let _ = socket.flush().await;
                            continue;
                        }
                        let payload = if mode == Mode::EchoSecret {
                            serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {"tools": [{"name": "reflect",
                                    "description": format!("reflects {secret}"),
                                    "inputSchema": {"type": "object"}}]}
                            })
                        } else {
                            serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {"tools": [{"name": "fetch",
                                    "description": "fetch a page",
                                    "inputSchema": {"type": "object"}}]}
                            })
                        };
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
                        if mode == Mode::StallNotifications {
                            // Slow enough that the caller cancels first, which
                            // is what produces the notification this mode
                            // then refuses to answer.
                            tokio::time::sleep(Duration::from_secs(5)).await;
                        }
                        match mode {
                            Mode::SessionLost => {
                                "HTTP/1.1 404 Not Found\r\nconnection: close\r\ncontent-length: 0\r\n\r\n".to_owned()
                            }
                            Mode::EchoSecret => sse(&[serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {"content": [{"type": "text",
                                    "text": format!("issued for {secret}")}]}})]),
                            Mode::Mixed | Mode::StallNotifications | Mode::ServerRequests => {
                                sse(&[serde_json::json!({"jsonrpc": "2.0", "id": id,
                                    "result": {"content": [{"type": "text",
                                        "text": format!("call {calls}")}]}})])
                            }
                        }
                    }
                    // Notifications get 202 and no body.
                    _ => {
                        if rpc == "notifications/initialized" {
                            initialized = true;
                        }
                        if mode == Mode::StallNotifications
                            && rpc.starts_with("notifications/")
                            && rpc != "notifications/initialized"
                        {
                            // Held open forever. Shutdown must not wait on it.
                            std::mem::forget(socket);
                            continue;
                        }
                        "HTTP/1.1 202 Accepted\r\nconnection: close\r\ncontent-length: 0\r\n\r\n".to_owned()
                    }
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });
        Endpoint {
            url: format!("http://127.0.0.1:{port}/mcp"),
            seen,
            replies,
        }
    }

    fn sse(messages: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for (index, message) in messages.iter().enumerate() {
            body.push_str(&format!("id: {index}\ndata: {message}\n\n"));
        }
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\
             content-length: {}\r\n\r\n{body}",
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
    let result = call(
        &runtime,
        "mcp__remote-docs__fetch",
        serde_json::json!({}),
        &cancel,
    )
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
    let error = call(
        &runtime,
        "mcp__flaky__fetch",
        serde_json::json!({}),
        &cancel,
    )
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

// --- Review fixes -------------------------------------------------------
//
// One test per behavior the reviewer found unproven: credential redaction at
// the runtime boundary, trust enforced on restart and revoked on denial,
// frozen tool identity, stale lifecycle results, blocked writers, descendant
// cleanup, lifecycle ordering, and measured startup concurrency.

/// The credential the echoing fixtures are handed and must never leak back.
const ECHOED: &str = "sk-echo-secret-5521";

#[tokio::test]
async fn a_stdio_server_cannot_echo_its_own_credential_back_out() {
    std::env::set_var("GRITT_TEST_ECHO_SECRET", ECHOED);
    let dir = workspace(serde_json::json!({"leaky": {
        "command": FIXTURE,
        "args": ["echo"],
        // Credential-looking, so it must be a reference; the runtime resolves
        // it and therefore knows the value to watch for.
        "env": {"FIXTURE_API_KEY": "${GRITT_TEST_ECHO_SECRET}"},
    }}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    let entry = snapshot(&snapshots, "leaky");
    assert_eq!(entry.state, McpServerState::Ready);

    // Metadata the server chose.
    let version = entry.server_version.clone().unwrap();
    assert!(
        !version.contains(ECHOED),
        "server version leaked: {version}"
    );
    assert!(version.contains("[redacted]"), "{version}");
    // The whole snapshot, however it is serialized for the interface.
    let json = serde_json::to_string(&snapshots).unwrap();
    assert!(!json.contains(ECHOED), "{json}");

    // The description and the schema reach a provider request verbatim.
    let tools = runtime.tool_set().await;
    let definition = &tools.definitions()[0];
    assert!(!definition.description.contains(ECHOED));
    assert!(!definition.parameters.to_string().contains(ECHOED));

    // Text and structured tool output, which reach the model and the store.
    let result = call(
        &runtime,
        "mcp__leaky__leaky",
        serde_json::json!({}),
        &cancel,
    )
    .await
    .unwrap();
    assert!(!result.output.contains(ECHOED), "{}", result.output);
    assert!(result.output.contains("[redacted]"), "{}", result.output);

    // And an error body, which the server controls completely.
    let error = call(
        &runtime,
        "mcp__leaky__leaky",
        serde_json::json!({"text": "error"}),
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(!error.message.contains(ECHOED), "{}", error.message);
    assert!(error.message.contains("[redacted]"), "{}", error.message);
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_http_server_cannot_echo_its_own_credential_back_out() {
    std::env::set_var("GRITT_TEST_ECHO_HTTP_SECRET", ECHOED);
    let endpoint = http_fixture::start(http_fixture::Mode::EchoSecret).await;
    let dir = workspace(serde_json::json!({"remote-leaky": {
        "type": "http",
        "url": endpoint.url,
        "headers": {"Authorization": "${GRITT_TEST_ECHO_HTTP_SECRET}"},
    }}));
    let transport: Arc<dyn gritt_provider::transport::HttpTransport> =
        Arc::new(gritt_provider::ReqwestTransport::new().unwrap());
    let runtime = McpRuntime::new(dir.path(), settings())
        .with_http_transport(transport)
        .with_trust(MemoryTrustStore::trust_all());
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    let entry = snapshot(&snapshots, "remote-leaky");
    assert_eq!(entry.state, McpServerState::Ready);
    assert!(!serde_json::to_string(&snapshots).unwrap().contains(ECHOED));
    let tools = runtime.tool_set().await;
    assert!(!tools.definitions()[0].description.contains(ECHOED));
    let result = call(
        &runtime,
        "mcp__remote-leaky__reflect",
        serde_json::json!({}),
        &cancel,
    )
    .await
    .unwrap();
    assert!(!result.output.contains(ECHOED), "{}", result.output);
    assert!(result.output.contains("[redacted]"), "{}", result.output);
    runtime.shutdown().await;
}

#[tokio::test]
async fn restarting_does_not_bypass_approval() {
    let dir = workspace(serde_json::json!({"guarded": stdio("basic")}));
    let trust = MemoryTrustStore::new();
    let runtime = McpRuntime::new(dir.path(), settings()).with_trust(Arc::clone(&trust) as Arc<_>);
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    assert_eq!(
        state_of(&runtime.snapshots().await, "guarded"),
        &McpServerState::AwaitingApproval
    );

    // Restarting an unapproved entry must not launch it.
    runtime.restart("guarded", &cancel).await.unwrap();
    assert_eq!(
        state_of(&runtime.snapshots().await, "guarded"),
        &McpServerState::AwaitingApproval
    );
    assert!(runtime.child_pids().await.is_empty());

    // Nor a denied one.
    runtime
        .decide("guarded", TrustDecision::Denied)
        .await
        .unwrap();
    runtime.restart("guarded", &cancel).await.unwrap();
    assert_eq!(
        state_of(&runtime.snapshots().await, "guarded"),
        &McpServerState::Denied
    );
    assert!(runtime.child_pids().await.is_empty());

    // Approved, restart works and the entry runs.
    runtime
        .decide("guarded", TrustDecision::Approved)
        .await
        .unwrap();
    runtime.restart("guarded", &cancel).await.unwrap();
    assert_eq!(
        state_of(&runtime.snapshots().await, "guarded"),
        &McpServerState::Ready
    );
    assert_eq!(runtime.child_pids().await.len(), 1);
    runtime.shutdown().await;
}

#[tokio::test]
async fn denying_a_running_server_revokes_it_immediately() {
    let dir = workspace(serde_json::json!({"revoked": stdio("marker")}));
    let marker = dir.path().join("was-called");
    let trust = MemoryTrustStore::new();
    let runtime = McpRuntime::new(dir.path(), settings()).with_trust(Arc::clone(&trust) as Arc<_>);
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    runtime
        .decide("revoked", TrustDecision::Approved)
        .await
        .unwrap();
    runtime.start(&cancel).await;
    assert_eq!(runtime.tool_set().await.len(), 2);
    // A turn that froze its tools before the revocation.
    let frozen = runtime.tool_set().await;
    let held = frozen.lookup("mcp__revoked__echo").cloned().unwrap();
    let pids = runtime.child_pids().await;
    assert_eq!(pids.len(), 1);

    runtime
        .decide("revoked", TrustDecision::Denied)
        .await
        .unwrap();
    let snapshots = runtime.snapshots().await;
    assert_eq!(state_of(&snapshots, "revoked"), &McpServerState::Denied);
    // The tools are gone and the process is gone.
    assert_eq!(snapshot(&snapshots, "revoked").tool_count, 0);
    assert!(runtime.tool_set().await.is_empty());
    assert!(runtime.child_pids().await.is_empty());
    assert_all_gone(&pids, "the denied server kept running").await;

    // A call held from before the revocation is refused, and never runs.
    let error = runtime
        .call(&held, &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert!(
        error.message.contains("changed") || error.message.contains("not available"),
        "{}",
        error.message
    );
    assert!(!marker.exists(), "a revoked server was still reached");
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_tool_frozen_before_a_reload_is_never_dispatched_after_it() {
    let dir = workspace(serde_json::json!({"shifting": stdio("marker")}));
    let marker = dir.path().join("was-called");
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let frozen = runtime
        .tool_set()
        .await
        .lookup("mcp__shifting__echo")
        .cloned()
        .unwrap();

    // The same name, a different definition.
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {
            "shifting": {"command": FIXTURE, "args": ["basic"]}
        }})
        .to_string(),
    )
    .unwrap();
    runtime.reload().await.unwrap();
    runtime.start(&cancel).await;

    let error = runtime
        .call(&frozen, &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert!(error.message.contains("changed"), "{}", error.message);
    assert!(!marker.exists(), "a stale reference reached a server");
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_dispatch_name_that_now_means_another_tool_is_refused() {
    let dir = workspace(serde_json::json!({"changes": stdio("listchanged")}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let frozen = runtime.tool_set().await;
    let echo = frozen.lookup("mcp__changes__echo").cloned().unwrap();

    // A name that still exists but now points at a different original tool.
    let impostor = gritt_harness::mcp::FrozenTool {
        reference: gritt_core::mcp::McpToolRef {
            dispatch_name: "mcp__changes__search".into(),
            server: "changes".into(),
            tool: "something-else".into(),
        },
        generation: echo.generation,
    };
    let error = runtime
        .call(&impostor, &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert!(
        error
            .message
            .contains("no longer refers to the tool that was approved"),
        "{}",
        error.message
    );

    // And a name the server withdrew after a list change is not silently
    // resolved to whatever now sits there.
    call(
        &runtime,
        "mcp__changes__echo",
        serde_json::json!({}),
        &cancel,
    )
    .await
    .unwrap();
    runtime.refresh(&cancel).await;
    let error = runtime
        .call(&echo, &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert!(error.message.contains("unknown tool"), "{}", error.message);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_late_initialization_result_is_discarded_after_a_denial() {
    let dir = workspace(serde_json::json!({"slow": stdio("slowinit")}));
    let trust = MemoryTrustStore::new();
    let runtime =
        Arc::new(McpRuntime::new(dir.path(), settings()).with_trust(Arc::clone(&trust) as Arc<_>));
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    runtime
        .decide("slow", TrustDecision::Approved)
        .await
        .unwrap();

    // Start it, then revoke while the handshake is still in flight.
    let starter = Arc::clone(&runtime);
    let token = cancel.clone();
    let starting = tokio::spawn(async move { starter.start(&token).await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    runtime.decide("slow", TrustDecision::Denied).await.unwrap();
    starting.await.unwrap();

    // The completed handshake belonged to the previous decision.
    let snapshots = runtime.snapshots().await;
    assert_eq!(state_of(&snapshots, "slow"), &McpServerState::Denied);
    assert_eq!(snapshot(&snapshots, "slow").tool_count, 0);
    assert!(runtime.tool_set().await.is_empty());
    assert!(
        runtime.child_pids().await.is_empty(),
        "a stale connection was installed"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn startup_concurrency_stays_within_the_configured_limit() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("concurrency.log");
    let mut entries = serde_json::Map::new();
    for index in 0..6 {
        entries.insert(
            format!("slow-{index}"),
            serde_json::json!({
                "command": FIXTURE,
                "args": ["slowinit"],
                "env": {"FIXTURE_CONCURRENCY": log.to_string_lossy()},
            }),
        );
    }
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({ "mcpServers": entries }).to_string(),
    )
    .unwrap();
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            max_concurrent_init: 2,
            ..settings()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    assert_eq!(snapshots.len(), 6);
    assert!(snapshots
        .iter()
        .all(|snapshot| snapshot.state == McpServerState::Ready));

    // Each server brackets its handshake, so the peak is measurable rather
    // than merely assumed from the fact that everything finished.
    let recorded = std::fs::read_to_string(&log).unwrap();
    let mut live = 0i32;
    let mut peak = 0i32;
    for line in recorded.lines() {
        match line.trim() {
            "start" => {
                live += 1;
                peak = peak.max(live);
            }
            "end" => live -= 1,
            _ => {}
        }
    }
    assert_eq!(recorded.lines().filter(|l| l.trim() == "start").count(), 6);
    assert!(
        peak <= 2,
        "peak concurrency was {peak}, limit was 2; log was:\n{recorded}"
    );
    assert!(
        peak >= 2,
        "the limit was never reached; peak was {peak}; log was:\n{recorded}"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn cancelling_a_call_tells_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let cancelled = dir.path().join("cancelled");
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {"hangs": {
            "command": FIXTURE,
            "args": ["slowcall"],
            "env": {"FIXTURE_CANCELLED": cancelled.to_string_lossy()},
        }}})
        .to_string(),
    )
    .unwrap();
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        canceller.cancel();
    });
    let error = call(&runtime, "mcp__hangs__echo", serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert_eq!(error.kind, gritt_core::ErrorKind::Cancelled);
    // Server-side proof that `notifications/cancelled` actually arrived.
    for _ in 0..100 {
        if cancelled.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        cancelled.exists(),
        "the server was never told about the cancellation"
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_server_that_stops_reading_cannot_block_shutdown() {
    let dir = workspace(serde_json::json!({"deaf": stdio("deaf")}));
    // A short handshake deadline, so a server that failed to connect shows up
    // as a fast failure rather than as a slow success.
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            init_timeout: Duration::from_secs(5),
            ..settings()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());
    let cancel = CancellationToken::new();
    // The handshake answers, then the server stops reading, so the pipe to it
    // fills and every later write parks.
    let snapshots = runtime.open(&cancel).await.unwrap();
    assert_eq!(
        state_of(&snapshots, "deaf"),
        &McpServerState::Ready,
        "the fixture must connect for this to test a blocked writer"
    );
    let pids = runtime.child_pids().await;
    assert_eq!(pids.len(), 1);
    let started = std::time::Instant::now();
    runtime.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "shutdown waited on a blocked write for {:?}",
        started.elapsed()
    );
    assert_all_gone(&pids, "the deaf server survived shutdown").await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_descendant_that_outlives_its_parent_is_cleaned_up_too() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("descendant.pid");
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {"spawner": {
            "command": FIXTURE,
            "args": ["descendant"],
            "env": {"FIXTURE_DESCENDANT": record.to_string_lossy()},
        }}})
        .to_string(),
    )
    .unwrap();
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            init_timeout: Duration::from_secs(5),
            ..settings()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());
    // The server exits before it ever handshakes, which is a failure; the
    // point is what it left behind.
    let snapshots = runtime.open(&CancellationToken::new()).await.unwrap();
    assert!(matches!(
        state_of(&snapshots, "spawner"),
        McpServerState::Failed { .. }
    ));
    for _ in 0..100 {
        if record.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let descendant: u32 = std::fs::read_to_string(&record)
        .expect("the descendant recorded its pid")
        .trim()
        .parse()
        .expect("a pid");
    runtime.shutdown().await;
    assert_all_gone(&[descendant], "a descendant outlived the runtime").await;
}

#[tokio::test]
async fn the_handshake_is_ordered_and_server_requests_are_answered() {
    let dir = tempfile::tempdir().unwrap();
    let initialized = dir.path().join("initialized");
    let ping_answered = dir.path().join("ping-answered");
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {"strict": {
            "command": FIXTURE,
            "args": ["strict"],
            "env": {
                "FIXTURE_INITIALIZED": initialized.to_string_lossy(),
                "FIXTURE_PING_ANSWERED": ping_answered.to_string_lossy(),
            },
        }}})
        .to_string(),
    )
    .unwrap();
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    // The fixture refuses `tools/list` that arrives before `initialized`, so
    // being ready at all proves the ordering held.
    assert_eq!(state_of(&snapshots, "strict"), &McpServerState::Ready);
    assert!(initialized.exists());
    assert_eq!(snapshot(&snapshots, "strict").tool_count, 2);
    // The fixture also sends a server-initiated `ping`. Answering it wrongly
    // would leave the connection in a state the next call would notice.
    let result = call(
        &runtime,
        "mcp__strict__echo",
        serde_json::json!({}),
        &cancel,
    )
    .await
    .unwrap();
    assert!(result.output.contains("echo"));
    runtime.shutdown().await;
}

// --- Round 2 review fixes ----------------------------------------------

/// A trust store whose answer can be held open, so a test can land a
/// lifecycle change in the middle of the await.
struct PausedTrust {
    release: tokio::sync::Semaphore,
    decision: gritt_core::mcp::TrustDecision,
}

impl PausedTrust {
    fn new(decision: gritt_core::mcp::TrustDecision) -> Arc<Self> {
        Arc::new(Self {
            release: tokio::sync::Semaphore::new(0),
            decision,
        })
    }

    fn release(&self) {
        self.release.add_permits(1);
    }
}

impl gritt_harness::mcp::trust::TrustStore for PausedTrust {
    fn decision<'a>(
        &'a self,
        _workspace: &'a str,
        _server: &'a str,
        _fingerprint: &'a str,
    ) -> gritt_core::session::BoxFuture<
        'a,
        gritt_core::Result<Option<gritt_core::mcp::TrustDecision>>,
    > {
        Box::pin(async move {
            // Forgotten rather than dropped: a dropped permit is returned to
            // the semaphore, which would let every later read straight
            // through and pause nothing.
            let permit = self.release.acquire().await.expect("paused trust");
            permit.forget();
            Ok(Some(self.decision))
        })
    }

    fn record<'a>(
        &'a self,
        _record: gritt_core::mcp::TrustRecord,
    ) -> gritt_core::session::BoxFuture<'a, gritt_core::Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// The pid a fixture recorded for itself, whether or not it ever connected.
async fn recorded_pid(path: &Path) -> u32 {
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the fixture never recorded its pid at {}", path.display());
}

#[tokio::test]
async fn rotating_a_token_keeps_redacting_what_the_running_server_still_holds() {
    // The definition names `${GRITT_TEST_ROTATED}` and never changes, so its
    // fingerprint does not change and the connection is retained across the
    // reload. The process keeps the value it was launched with.
    let first = "sk-rotation-first-8801";
    let second = "sk-rotation-second-8802";
    std::env::set_var("GRITT_TEST_ROTATED", first);
    let dir = workspace(serde_json::json!({"rotating": {
        "command": FIXTURE,
        "args": ["echo"],
        "env": {"FIXTURE_API_KEY": "${GRITT_TEST_ROTATED}"},
    }}));
    let runtime = runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();
    let before = runtime.child_pids().await;
    assert_eq!(before.len(), 1);

    // The environment rotates; the file does not.
    std::env::set_var("GRITT_TEST_ROTATED", second);
    runtime.reload().await.unwrap();
    assert_eq!(
        state_of(&runtime.snapshots().await, "rotating"),
        &McpServerState::Ready,
        "the unchanged definition should keep its connection"
    );
    assert_eq!(
        runtime.child_pids().await,
        before,
        "the process should not have been restarted"
    );

    // That process still echoes the first token, so that is the value that
    // has to be redacted.
    let result = call(
        &runtime,
        "mcp__rotating__leaky",
        serde_json::json!({}),
        &cancel,
    )
    .await
    .unwrap();
    assert!(
        !result.output.contains(first),
        "the retained connection leaked its own token: {}",
        result.output
    );
    assert!(result.output.contains("[redacted]"), "{}", result.output);

    let error = call(
        &runtime,
        "mcp__rotating__leaky",
        serde_json::json!({"text": "error"}),
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(!error.message.contains(first), "{}", error.message);
    runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_during_startup_takes_the_children_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let ready_pid = dir.path().join("ready.pid");
    let stalled_pid = dir.path().join("stalled.pid");
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {
            // One that finishes its handshake, and one that never answers.
            "ready": {"command": FIXTURE, "args": ["basic"],
                      "env": {"FIXTURE_PID": ready_pid.to_string_lossy()}},
            "stalled": {"command": FIXTURE, "args": ["silent"],
                        "env": {"FIXTURE_PID": stalled_pid.to_string_lossy()}},
        }})
        .to_string(),
    )
    .unwrap();
    let runtime = Arc::new(
        McpRuntime::new(
            dir.path(),
            McpRuntimeSettings {
                // Long enough that the stalled handshake is still running when
                // shutdown arrives.
                init_timeout: Duration::from_secs(120),
                max_concurrent_init: 4,
                ..settings()
            },
        )
        .with_trust(MemoryTrustStore::trust_all()),
    );
    let config = runtime.read_config().unwrap();
    runtime.load(&config).await.unwrap();

    let starter = Arc::clone(&runtime);
    let token = CancellationToken::new();
    let starting = tokio::spawn(async move { starter.start(&token).await });

    let ready = recorded_pid(&ready_pid).await;
    let stalled = recorded_pid(&stalled_pid).await;

    // Shutdown lands while `start` is still inside the stalled handshake.
    let began = std::time::Instant::now();
    runtime.shutdown().await;
    assert!(
        began.elapsed() < Duration::from_secs(60),
        "shutdown waited on the stalled handshake for {:?}",
        began.elapsed()
    );
    // Both children are gone, including the one that only ever existed
    // inside an in-flight initialization.
    assert_all_gone(
        &[ready, stalled],
        "a child outlived a shutdown during startup",
    )
    .await;
    assert!(runtime.child_pids().await.is_empty());
    // The interrupted start must not install anything afterwards.
    let snapshots = starting.await.unwrap();
    assert!(
        snapshots.iter().all(|snapshot| !snapshot.state.is_ready()),
        "a server was installed after shutdown: {snapshots:?}"
    );
    assert!(runtime.tool_set().await.is_empty());
}

#[tokio::test]
async fn a_lifecycle_change_during_a_slow_trust_read_wins() {
    let dir = workspace(serde_json::json!({"contested": stdio("basic")}));
    let trust = PausedTrust::new(gritt_core::mcp::TrustDecision::Approved);
    let runtime =
        Arc::new(McpRuntime::new(dir.path(), settings()).with_trust(Arc::clone(&trust) as Arc<_>));
    let cancel = CancellationToken::new();
    // The initial load also reads trust, so let that one through first.
    trust.release();
    runtime.open(&cancel).await.unwrap();
    assert_eq!(
        state_of(&runtime.snapshots().await, "contested"),
        &McpServerState::Ready
    );

    // A restart whose trust read is held open.
    let restarting = Arc::clone(&runtime);
    let token = cancel.clone();
    let restart = tokio::spawn(async move { restarting.restart("contested", &token).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // Meanwhile the server is stopped. The fingerprint is unchanged, so only
    // the generation can tell the pending Approved answer that it is stale.
    runtime.stop("contested").await.unwrap();
    trust.release();
    let outcome = restart.await.unwrap();

    assert!(
        outcome.is_err(),
        "a stale approval restarted a stopped server"
    );
    assert_eq!(
        state_of(&runtime.snapshots().await, "contested"),
        &McpServerState::Stopped,
        "the newer decision must stand"
    );
    assert!(runtime.child_pids().await.is_empty());
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_blocked_writer_neither_hangs_callers_nor_shutdown() {
    let dir = workspace(serde_json::json!({"deaf": stdio("deaf")}));
    let runtime = Arc::new(
        McpRuntime::new(
            dir.path(),
            McpRuntimeSettings {
                init_timeout: Duration::from_secs(5),
                // Long, so what ends these calls is the blocked write and the
                // saturated queue rather than the call deadline.
                call_timeout: Duration::from_secs(300),
                ..settings()
            },
        )
        .with_trust(MemoryTrustStore::trust_all()),
    );
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    assert_eq!(state_of(&snapshots, "deaf"), &McpServerState::Ready);
    let pids = runtime.child_pids().await;
    assert_eq!(pids.len(), 1);
    let frozen = runtime
        .tool_set()
        .await
        .lookup("mcp__deaf__echo")
        .cloned()
        .expect("the deaf fixture lists one tool before it stops reading");

    // Enough payload to fill the pipe to a server that is no longer reading,
    // and enough separate calls to saturate the command queue behind it.
    let payload = serde_json::json!({"text": "x".repeat(64 * 1024)});
    let mut calls = Vec::new();
    for _ in 0..200 {
        let runtime = Arc::clone(&runtime);
        let frozen = frozen.clone();
        let payload = payload.clone();
        let token = cancel.clone();
        calls.push(tokio::spawn(async move {
            runtime.call(&frozen, &payload, &token).await
        }));
    }

    // Nothing hangs: every call comes back with an error well inside the
    // call deadline, because the write and the queue both have their own.
    let settled = tokio::time::timeout(Duration::from_secs(120), async {
        let mut errors = Vec::new();
        for call in calls {
            if let Ok(Err(error)) = call.await {
                errors.push(error.message);
            }
        }
        errors
    })
    .await
    .expect("callers hung on a server that stopped reading");
    assert!(
        !settled.is_empty(),
        "a server that stopped reading should have failed these calls"
    );
    assert!(
        settled
            .iter()
            .any(|message| message.contains("stopped accepting messages")
                || message.contains("stopped reading")
                || message.contains("connection is closed")),
        "unexpected failures: {settled:?}"
    );

    // And shutdown does not wait on the parked write.
    let began = std::time::Instant::now();
    runtime.shutdown().await;
    assert!(
        began.elapsed() < Duration::from_secs(30),
        "shutdown waited on a blocked write for {:?}",
        began.elapsed()
    );
    assert_all_gone(&pids, "the deaf server survived shutdown").await;
}

/// A runtime pointed at a local Streamable HTTP endpoint.
fn http_runtime(dir: &Path) -> Arc<McpRuntime> {
    let transport: Arc<dyn gritt_provider::transport::HttpTransport> =
        Arc::new(gritt_provider::ReqwestTransport::new().unwrap());
    Arc::new(
        McpRuntime::new(dir, settings())
            .with_http_transport(transport)
            .with_trust(MemoryTrustStore::trust_all()),
    )
}

#[tokio::test]
async fn a_server_request_is_answered_with_a_compliant_post() {
    let endpoint = http_fixture::start(http_fixture::Mode::ServerRequests).await;
    let dir = workspace(serde_json::json!({"asks": {
        "type": "http",
        "url": endpoint.url,
    }}));
    let runtime = http_runtime(dir.path());
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.unwrap();
    // The server sent a `ping` on the discovery stream. Discovery still
    // finished, so the reply did not disturb the caller's own response.
    assert_eq!(state_of(&snapshots, "asks"), &McpServerState::Ready);
    assert_eq!(snapshot(&snapshots, "asks").tool_count, 1);

    // The answer arrived, as its own POST.
    let mut replies = Vec::new();
    for _ in 0..100 {
        replies = endpoint.replies.lock().unwrap().clone();
        if !replies.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(replies.len(), 1, "the server request was never answered");
    let (body, headers) = &replies[0];
    let answer: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(answer["id"], "srv-1");
    assert!(answer.get("result").is_some(), "{answer}");
    // Every client POST must offer both body shapes, this one included; a
    // strict endpoint rejects it otherwise and the originating call hangs.
    let accept = headers.get("accept").cloned().unwrap_or_default();
    assert!(
        accept.contains("application/json") && accept.contains("text/event-stream"),
        "the reply omitted the required Accept header: {accept:?}"
    );
    // It also carries the session and the negotiated revision, like any
    // other post-handshake request.
    assert_eq!(
        headers.get("mcp-session-id").map(String::as_str),
        Some("session-abc")
    );
    assert_eq!(
        headers.get("mcp-protocol-version").map(String::as_str),
        Some("2025-06-18")
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_stalled_notification_cannot_hold_up_shutdown() {
    let endpoint = http_fixture::start(http_fixture::Mode::StallNotifications).await;
    let dir = workspace(serde_json::json!({"stalls": {
        "type": "http",
        "url": endpoint.url,
    }}));
    let runtime = http_runtime(dir.path());
    let cancel = CancellationToken::new();
    runtime.open(&cancel).await.unwrap();

    // Cancelling a call sends `notifications/cancelled`, which this endpoint
    // accepts and then never answers.
    let frozen = runtime
        .tool_set()
        .await
        .lookup("mcp__stalls__fetch")
        .cloned()
        .unwrap();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let error = runtime
        .call(&frozen, &serde_json::json!({}), &cancel)
        .await
        .unwrap_err();
    assert_eq!(error.kind, gritt_core::ErrorKind::Cancelled);

    // The hung notification POST is owned, so shutdown cancels it rather
    // than waiting for an answer that never comes.
    let began = std::time::Instant::now();
    runtime.shutdown().await;
    assert!(
        began.elapsed() < Duration::from_secs(30),
        "shutdown waited on a stalled notification for {:?}",
        began.elapsed()
    );
}
