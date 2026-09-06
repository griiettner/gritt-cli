//! Live smoke tests against the installed Codex and Claude Code CLIs.
//! Gated by `GRITT_LIVE_CONNECTOR_TESTS=1`; never required for a pass.

use std::time::Duration;

use futures::StreamExt;
use gritt_connector::protocols::{claude::ClaudeCode, codex::Codex, opencode::OpenCode};
use gritt_connector::ExternalConnector;
use gritt_core::config::ConnectorSettings;
use gritt_core::connector::{
    AuthState, Connector, ConnectorId, ConnectorModelDiscovery, TaskRequest,
};
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
            model: None,
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

/// Resume through the connector's continuation path: a first turn records
/// the Codex thread id, a second turn passes it back and must see the
/// earlier context.
#[tokio::test]
async fn codex_live_resume() {
    if !gated() {
        eprintln!("GRITT_LIVE_CONNECTOR_TESTS is not set; skipping");
        return;
    }
    let connector = ExternalConnector::new(Codex, &ConnectorSettings::default());
    let info = connector.info().await.expect("info");
    if info.auth == AuthState::NotInstalled {
        eprintln!("codex is not installed; skipping");
        return;
    }
    let workspace = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(workspace.path())
        .status()
        .ok();
    let session_id = SessionId("live-codex-resume".into());
    let started = std::time::Instant::now();
    let turn = |prompt: &str, continuation| {
        let request = TaskRequest {
            session_id: session_id.clone(),
            prompt: prompt.to_owned(),
            workspace: workspace.path().to_path_buf(),
            continuation,
            model: None,
        };
        let connector = &connector;
        async move {
            let mut stream = connector.start(request).await.expect("start");
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
                    EventKind::Error { message, .. } => panic!("codex failed: {message}"),
                    _ => {}
                }
            }
            assert!(completed, "codex did not complete");
            text
        }
    };
    let first = turn(
        "Remember the code word MARMALADE. Reply with the single word OK.",
        None,
    )
    .await;
    eprintln!("codex resume: first turn text={first:?}");
    let continuation = connector
        .continuation_for(&session_id)
        .expect("codex reported a thread id to resume");
    let second = turn(
        "Reply with only the code word you were asked to remember.",
        Some(continuation),
    )
    .await;
    eprintln!(
        "codex resume: second turn text={second:?} after {:?}",
        started.elapsed()
    );
    assert!(
        second.to_ascii_uppercase().contains("MARMALADE"),
        "the resumed thread lost its context: {second:?}"
    );
}

#[tokio::test]
async fn live_codex_model_listing() {
    if !gated() {
        eprintln!("GRITT_LIVE_CONNECTOR_TESTS is not set; skipping");
        return;
    }
    let connector = ExternalConnector::new(Codex, &ConnectorSettings::default());
    let info = connector.info().await.expect("info");
    if info.auth == AuthState::NotInstalled {
        eprintln!("codex is not installed; skipping live model listing");
        return;
    }
    let outcome = connector.discover_models(true).await;
    eprintln!("codex models: {}", outcome.describe());
    assert!(
        outcome.catalog().is_some(),
        "codex should return a catalog, got {outcome:?}"
    );
    assert_eq!(outcome.connector(), ConnectorId::Codex);
}

#[tokio::test]
async fn live_claude_model_listing_is_unsupported() {
    if !gated() {
        eprintln!("GRITT_LIVE_CONNECTOR_TESTS is not set; skipping");
        return;
    }
    let connector = ExternalConnector::new(ClaudeCode, &ConnectorSettings::default());
    let info = connector.info().await.expect("info");
    if info.auth == AuthState::NotInstalled {
        eprintln!("claude is not installed; skipping live model listing");
        return;
    }
    let outcome = connector.discover_models(true).await;
    eprintln!("claude models: {}", outcome.describe());
    assert!(
        matches!(outcome, ConnectorModelDiscovery::Unsupported { .. }),
        "claude has no documented listing command, got {outcome:?}"
    );
}

#[tokio::test]
async fn live_opencode_model_listing() {
    if !gated() {
        eprintln!("GRITT_LIVE_CONNECTOR_TESTS is not set; skipping");
        return;
    }
    let connector = ExternalConnector::new(OpenCode, &ConnectorSettings::default());
    let info = connector.info().await.expect("info");
    if info.auth == AuthState::NotInstalled {
        eprintln!("opencode is not installed; skipping live model listing");
        return;
    }
    let outcome = connector.discover_models(true).await;
    eprintln!("opencode models: {}", outcome.describe());
    assert!(
        outcome.catalog().is_some(),
        "opencode should return a catalog, got {outcome:?}"
    );
    assert_eq!(outcome.connector(), ConnectorId::OpenCode);
}
