//! Opt-in smoke check against this repository's own `.mcp.json`.
//!
//! Fake servers prove the runtime's behavior; they cannot prove that a real
//! server on this machine still speaks the protocol Gritt implements. This
//! check does exactly that and nothing more: it initializes each configured
//! server whose executable is present and lists its tools. It never calls a
//! tool.
//!
//! That is the limit of what it promises. A server may do whatever it likes
//! during its own startup, including writing to its own storage, and this
//! check neither prevents nor observes that; it only guarantees that Gritt
//! issues no `tools/call`.
//!
//! Run it with `GRITT_LIVE_MCP_TESTS=1`. Without that variable it skips, and
//! a skip is reported as a skip rather than a pass.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gritt_core::mcp::{
    parse_mcp_config, McpEntry, McpRuntimeSettings, McpServerState, McpTransport,
};
use gritt_harness::mcp::trust::MemoryTrustStore;
use gritt_harness::mcp::{stdio, McpRuntime};
use gritt_harness::CancellationToken;

/// A one-line rendering of a state and its reason, for the failure message.
fn describe(state: &McpServerState) -> String {
    let reason = state.reason();
    if reason.is_empty() {
        format!("{state:?}")
    } else {
        reason.to_owned()
    }
}

fn repository_root() -> PathBuf {
    // `crates/gritt-harness` -> the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repository root")
}

/// True when the entry's command can actually be run here.
fn executable_available(workspace: &Path, command: &str) -> bool {
    let program = stdio::resolve_program(workspace, command);
    if program.components().count() > 1 || program.is_absolute() {
        return program.is_file();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(&program).is_file())
}

#[tokio::test]
async fn every_configured_server_is_reachable_or_explained() {
    if std::env::var("GRITT_LIVE_MCP_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set GRITT_LIVE_MCP_TESTS=1 to run the live MCP smoke check");
        return;
    }
    let root = repository_root();
    let path = root.join(".mcp.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("skipped: {} does not exist", path.display());
        return;
    };
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let config =
        parse_mcp_config(&text, &env, ".mcp.json").expect("the workspace .mcp.json parses");
    println!("live MCP smoke check: {} entry(ies)", config.len());

    // Entries whose executable is missing are reported separately: an
    // unavailable program is a fact about this machine, not a failure of
    // the runtime.
    let mut unavailable = Vec::new();
    for entry in &config.entries {
        if let McpEntry::Server(server) = entry {
            if let McpTransport::Stdio { command, .. } = &server.transport {
                if !executable_available(&root, command) {
                    unavailable.push(server.name.clone());
                }
            }
        }
    }

    let runtime = McpRuntime::new(
        &root,
        McpRuntimeSettings {
            init_timeout: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(2),
            ..McpRuntimeSettings::default()
        },
    )
    // Running this check is the approval; nothing is written to the store.
    .with_trust(MemoryTrustStore::trust_all())
    .with_http_transport(std::sync::Arc::new(
        gritt_provider::ReqwestTransport::new().unwrap(),
    ));
    let cancel = CancellationToken::new();
    let snapshots = runtime.open(&cancel).await.expect("the runtime opens");
    assert_eq!(
        snapshots.len(),
        config.len(),
        "every configured entry must be accounted for"
    );
    let mut failures = Vec::new();
    for snapshot in &snapshots {
        let unavailable = unavailable.contains(&snapshot.name);
        println!(
            "  {:<24} {:<22} tools={} protocol={} {}",
            snapshot.name,
            format!("{:?}", snapshot.state)
                .split_whitespace()
                .next()
                .unwrap_or("?")
                .to_lowercase(),
            snapshot.tool_count,
            snapshot.protocol_version.as_deref().unwrap_or("-"),
            if unavailable {
                "(executable not available on this machine)"
            } else {
                snapshot.state.reason()
            }
        );
        // Every entry has a visible state, and nothing is left waiting: the
        // check approved them all up front.
        assert!(
            !matches!(snapshot.state, McpServerState::AwaitingApproval),
            "{} was never attempted",
            snapshot.name
        );
        if snapshot.state.is_ready() {
            assert!(
                snapshot.protocol_version.is_some(),
                "{} is ready without a negotiated revision",
                snapshot.name
            );
        }
        // An entry whose executable is present, on a transport Gritt
        // supports, is expected to work. Letting that pass silently would
        // make a green run meaningless.
        let expected_to_work = !unavailable
            && !matches!(
                snapshot.state,
                McpServerState::Invalid { .. } | McpServerState::UnsupportedTransport { .. }
            );
        if expected_to_work && !snapshot.state.is_ready() {
            failures.push(format!("{}: {}", snapshot.name, describe(&snapshot.state)));
        }
    }
    assert!(
        failures.is_empty(),
        "available servers failed to initialize: {}",
        failures.join("; ")
    );
    if !unavailable.is_empty() {
        println!("  unavailable executables: {}", unavailable.join(", "));
    }
    // No tool is called anywhere in this check.
    runtime.shutdown().await;
    assert!(runtime.child_pids().await.is_empty());
}
