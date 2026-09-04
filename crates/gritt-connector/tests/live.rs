//! Live smoke tests against the installed Codex and Claude Code CLIs.
//! Gated by `GRITT_LIVE_CONNECTOR_TESTS=1`; never required for a pass.

use std::time::Duration;

use futures::StreamExt;
use gritt_connector::protocols::{claude::ClaudeCode, codex::Codex};
use gritt_connector::ExternalConnector;
use gritt_core::config::ConnectorSettings;
use gritt_core::connector::{AuthState, Connector, TaskRequest};
use gritt_core::event::EventKind;
use gritt_core::session::SessionId;

fn gated() -> bool {
    std::env::var("GRITT_LIVE_CONNECTOR_TESTS").is_ok_and(|v| v == "1")
}

async fn smoke(connector: &dyn Connector, name: &str) {
    let info = connector.info().await.expect("info");
    eprintln!("{name}: version={:?} auth={:?}", info.version, info.auth);
    if info.auth == AuthState::NotInstalled {
        eprintln!("{name} is not installed; skipping");
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(workspace.path())
        .status()
        .ok();
    let started = std::time::Instant::now();
    let mut stream = connector
        .start(TaskRequest {
            session_id: SessionId(format!("live-{name}")),
            prompt: "Reply with the single word PONG.".into(),
            workspace: workspace.path().to_path_buf(),
            continuation: None,
        })
        .await
        .expect("start");
    let mut text = String::new();
    let mut completed = false;
    while let Some(item) = tokio::time::timeout(Duration::from_secs(180), stream.next())
        .await
        .expect("live agent timed out")
    {
        let event = item.expect("event");
        match &event.kind {
            EventKind::TextDelta { text: t } => text.push_str(t),
            EventKind::Completed { .. } => completed = true,
            EventKind::Error { message, .. } => panic!("{name} failed: {message}"),
            _ => {}
        }
    }
    eprintln!(
        "{name}: completed in {:?}, text={text:?}",
        started.elapsed()
    );
    assert!(completed, "{name} did not complete");
    assert!(!text.trim().is_empty(), "{name} produced no text");
}

#[tokio::test]
async fn codex_live_smoke() {
    if !gated() {
        eprintln!("GRITT_LIVE_CONNECTOR_TESTS is not set; skipping");
        return;
    }
    smoke(
        &ExternalConnector::new(Codex, &ConnectorSettings::default()),
        "codex",
    )
    .await;
}

#[tokio::test]
async fn claude_live_smoke() {
    if !gated() {
        eprintln!("GRITT_LIVE_CONNECTOR_TESTS is not set; skipping");
        return;
    }
    smoke(
        &ExternalConnector::new(ClaudeCode, &ConnectorSettings::default()),
        "claude",
    )
    .await;
}
