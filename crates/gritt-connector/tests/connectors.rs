//! Every external connector against the fake agent: fixture replay per
//! protocol, and the supervision paths (process exit, cancellation with
//! the child gone, timeout, missing executable, malformed output, PTY
//! transport, follow-up input through resume, and the health probes).

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use gritt_connector::process::is_alive;
use gritt_connector::protocols::{
    claude::ClaudeCode, codex::Codex, cursor::Cursor, opencode::OpenCode,
};
use gritt_connector::{ExternalConnector, Protocol, Timeouts};
use gritt_core::config::ConnectorSettings;
use gritt_core::connector::{AuthState, Connector, ConnectorId, TaskRequest, TaskState, Transport};
use gritt_core::event::{Event, EventKind, EventSource, StopReason};
use gritt_core::secret::Secret;
use gritt_core::session::SessionId;

fn fixture(connector: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(connector)
        .join(format!("{name}.jsonl"))
}

fn agent_script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-agent/agent.sh")
}

/// A wrapper that sets the fake agent's variables, so parallel tests never
/// share process environment.
struct Fake {
    dir: tempfile::TempDir,
    wrapper: PathBuf,
}

impl Fake {
    fn new(vars: &[(&str, String)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("agent");
        let mut script = String::from("#!/bin/sh\n");
        for (name, value) in vars {
            script.push_str(&format!(
                "{name}='{}'\nexport {name}\n",
                value.replace('\'', "'\\''")
            ));
        }
        script.push_str(&format!("exec '{}' \"$@\"\n", agent_script().display()));
        std::fs::write(&wrapper, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir, wrapper }
    }

    fn settings(&self, name: &str) -> ConnectorSettings {
        ConnectorSettings {
            executables: BTreeMap::from([(name.to_owned(), self.wrapper.display().to_string())]),
            ..ConnectorSettings::default()
        }
    }

    fn connector<P: Protocol>(&self, protocol: P, name: &str) -> ExternalConnector<P> {
        ExternalConnector::new(protocol, &self.settings(name)).with_timeouts(Timeouts {
            health: Duration::from_secs(5),
            startup: Duration::from_secs(5),
            idle: Duration::from_secs(5),
        })
    }

    fn request(&self, prompt: &str) -> TaskRequest {
        TaskRequest {
            session_id: SessionId("s-test".into()),
            prompt: prompt.into(),
            workspace: self.dir.path().to_path_buf(),
            continuation: None,
        }
    }
}

async fn collect(connector: &dyn Connector, request: TaskRequest) -> Vec<Event> {
    let mut stream = connector.start(request).await.expect("start");
    let mut events = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(20), stream.next())
        .await
        .expect("stream did not end")
    {
        events.push(item.expect("event"));
    }
    events
}

fn kinds(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .map(|event| match &event.kind {
            EventKind::TextDelta { .. } => "text".into(),
            EventKind::ReasoningSummary { .. } => "reasoning".into(),
            EventKind::ToolCall { call } => format!("tool_call:{}", call.name),
            EventKind::ToolResult { result } => format!(
                "tool_result:{}:{}",
                result.name,
                if result.is_error { "error" } else { "ok" }
            ),
            EventKind::ApprovalRequested { .. } => "approval_requested".into(),
            EventKind::ApprovalDecided { .. } => "approval_decided".into(),
            EventKind::Usage { .. } => "usage".into(),
            EventKind::StatusChanged { status } => format!("status:{status:?}").to_lowercase(),
            EventKind::Error { .. } => "error".into(),
            EventKind::Completed { stop_reason } => {
                format!("completed:{stop_reason:?}").to_lowercase()
            }
            EventKind::Cancelled => "cancelled".into(),
        })
        .collect()
}

fn text_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn no_status(kinds: Vec<String>) -> Vec<String> {
    kinds
        .into_iter()
        .filter(|k| !k.starts_with("status:"))
        .collect()
}

#[tokio::test]
async fn codex_fixtures_normalize() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("codex", "tool").display().to_string(),
    )]);
    let connector = fake.connector(Codex, "codex");
    let events = collect(&connector, fake.request("run it")).await;
    assert!(events.iter().all(|e| e.source
        == EventSource::Connector {
            id: ConnectorId::Codex
        }));
    assert_eq!(
        no_status(kinds(&events)),
        vec![
            "text",
            "tool_call:shell",
            "tool_result:shell:ok",
            "reasoning",
            "text",
            "usage",
            "completed:endturn"
        ]
    );
    assert_eq!(text_of(&events), "I will run that exact command now.DONE");
    let sequences: Vec<u64> = events.iter().map(|e| e.sequence).collect();
    assert_eq!(sequences, (0..events.len() as u64).collect::<Vec<_>>());
    let inspection = connector
        .inspect(&SessionId("s-test".into()))
        .await
        .unwrap();
    assert_eq!(inspection.external_id.as_deref(), Some("thread-codex-0002"));
    assert_eq!(inspection.state, TaskState::Completed);
    let continuation = connector
        .continuation_for(&SessionId("s-test".into()))
        .unwrap();
    assert_eq!(continuation.owner, "connector:codex");
    assert_eq!(continuation.state["external_id"], "thread-codex-0002");

    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("codex", "error").display().to_string(),
    )]);
    let events = collect(&fake.connector(Codex, "codex"), fake.request("x")).await;
    assert_eq!(no_status(kinds(&events)), vec!["error"]);
    assert!(
        matches!(&events.last().unwrap().kind, EventKind::Error { message, .. } if message.contains("not supported"))
    );
}

#[tokio::test]
async fn claude_fixtures_normalize() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("claude", "tool").display().to_string(),
    )]);
    let connector = fake.connector(ClaudeCode, "claude_code");
    let events = collect(&connector, fake.request("run it")).await;
    assert_eq!(
        no_status(kinds(&events)),
        vec![
            "text",
            "tool_call:Bash",
            "tool_result:Bash:ok",
            "text",
            "usage",
            "completed:endturn"
        ]
    );
    let usage = events.iter().find_map(|e| match &e.kind {
        EventKind::Usage { usage } => Some(*usage),
        _ => None,
    });
    assert_eq!(usage.unwrap().cached_input_tokens, Some(31180));
    assert_eq!(
        connector
            .inspect(&SessionId("s-test".into()))
            .await
            .unwrap()
            .external_id
            .as_deref(),
        Some("claude-session-0002")
    );
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("claude", "text").display().to_string(),
    )]);
    let events = collect(&fake.connector(ClaudeCode, "claude"), fake.request("x")).await;
    assert_eq!(text_of(&events), "PONG");
    // The rate limit line is unknown to the normalizer: a diagnostic, not a failure.
    assert!(events.iter().any(|e| e
        .diagnostic
        .as_ref()
        .is_some_and(|d| d.get("unknown_event").is_some())));
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("claude", "error").display().to_string(),
    )]);
    let events = collect(&fake.connector(ClaudeCode, "claude"), fake.request("x")).await;
    assert_eq!(no_status(kinds(&events)), vec!["usage", "error"]);
}

#[tokio::test]
async fn opencode_fixtures_normalize() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("opencode", "tool").display().to_string(),
    )]);
    let connector = fake.connector(OpenCode, "opencode");
    let events = collect(&connector, fake.request("run it")).await;
    assert_eq!(
        no_status(kinds(&events)),
        vec![
            "tool_call:bash",
            "tool_result:bash:ok",
            "usage",
            "text",
            "usage",
            "completed:endturn"
        ]
    );
    assert_eq!(
        connector
            .inspect(&SessionId("s-test".into()))
            .await
            .unwrap()
            .external_id
            .as_deref(),
        Some("ses_opencode_0002")
    );
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("opencode", "error").display().to_string(),
    )]);
    let events = collect(&fake.connector(OpenCode, "opencode"), fake.request("x")).await;
    assert_eq!(no_status(kinds(&events)), vec!["error"]);
    assert!(
        matches!(&events.last().unwrap().kind, EventKind::Error { message, .. } if message.contains("credentials"))
    );
}

#[tokio::test]
async fn cursor_fixtures_normalize() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("cursor", "tool").display().to_string(),
    )]);
    let connector = fake.connector(Cursor, "cursor");
    let events = collect(&connector, fake.request("run it")).await;
    assert_eq!(
        no_status(kinds(&events)),
        vec![
            "reasoning",
            "tool_call:file_read",
            "tool_result:file_read:ok",
            "text",
            "completed:endturn"
        ]
    );
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("cursor", "error").display().to_string(),
    )]);
    let events = collect(&fake.connector(Cursor, "cursor"), fake.request("x")).await;
    assert_eq!(no_status(kinds(&events)), vec!["error"]);
}

#[tokio::test]
async fn malformed_lines_and_unknown_events_are_diagnostics_not_failures() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("codex", "malformed").display().to_string(),
    )]);
    let events = collect(&fake.connector(Codex, "codex"), fake.request("x")).await;
    assert!(events.iter().any(|e| e
        .diagnostic
        .as_ref()
        .is_some_and(|d| d.get("malformed_line").is_some())));
    assert!(events.iter().any(|e| e
        .diagnostic
        .as_ref()
        .is_some_and(|d| d.get("unknown_event").is_some())));
    assert_eq!(text_of(&events), "still fine");
    assert!(matches!(
        events.last().unwrap().kind,
        EventKind::Completed {
            stop_reason: StopReason::EndTurn
        }
    ));
}

#[tokio::test]
async fn a_nonzero_exit_without_a_terminal_event_is_an_error() {
    let fake = Fake::new(&[
        ("FAKE_AGENT_STDERR", "fatal: something broke".into()),
        ("FAKE_AGENT_EXIT", "3".into()),
    ]);
    let events = collect(&fake.connector(Codex, "codex"), fake.request("x")).await;
    let last = events.last().unwrap();
    assert!(
        matches!(&last.kind, EventKind::Error { message, .. } if message.contains("status 3") && message.contains("something broke"))
    );
    assert_eq!(last.diagnostic.as_ref().unwrap()["exit"], 3);
    let inspection = fake
        .connector(Codex, "codex")
        .inspect(&SessionId("s-test".into()))
        .await;
    // A fresh connector has no such session; the one that ran does.
    assert!(inspection.is_err());
}

#[tokio::test]
async fn a_clean_exit_without_a_terminal_event_completes() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("codex", "text").display().to_string(),
    )]);
    // Only the first three lines: no turn.completed.
    let partial = fake.dir.path().join("partial.jsonl");
    let lines: Vec<String> = std::fs::read_to_string(fixture("codex", "text"))
        .unwrap()
        .lines()
        .take(3)
        .map(str::to_owned)
        .collect();
    std::fs::write(&partial, lines.join("\n") + "\n").unwrap();
    let fake = Fake::new(&[("FAKE_AGENT_FIXTURE", partial.display().to_string())]);
    let events = collect(&fake.connector(Codex, "codex"), fake.request("x")).await;
    assert_eq!(text_of(&events), "PONG");
    assert!(matches!(
        events.last().unwrap().kind,
        EventKind::Completed {
            stop_reason: StopReason::Other
        }
    ));
}

#[tokio::test]
async fn cancellation_kills_the_agent_and_its_children() {
    let fake = Fake::new(&[
        (
            "FAKE_AGENT_FIXTURE",
            fixture("codex", "text").display().to_string(),
        ),
        ("FAKE_AGENT_LINE_DELAY", "30".into()),
    ]);
    let connector = Arc::new(fake.connector(Codex, "codex"));
    let session = SessionId("s-test".into());
    let mut stream = connector.start(fake.request("x")).await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    let pid = first.diagnostic.as_ref().unwrap()["pid"].as_u64().unwrap() as u32;
    assert!(is_alive(pid).await);
    // Wait for the first fixture line so the child is in its sleep.
    let _ = stream.next().await;
    connector.cancel(&session).await.unwrap();
    let mut rest = Vec::new();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("stream ends after cancel")
    {
        rest.push(item.unwrap());
    }
    assert!(matches!(rest.last().unwrap().kind, EventKind::Cancelled));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!is_alive(pid).await, "agent {pid} still alive after cancel");
    assert_eq!(
        connector.inspect(&session).await.unwrap().state,
        TaskState::Cancelled
    );
}

#[tokio::test]
async fn dropping_the_stream_kills_the_agent() {
    let fake = Fake::new(&[("FAKE_AGENT_SLEEP", "30".into())]);
    let connector = fake.connector(Codex, "codex");
    let mut stream = connector.start(fake.request("x")).await.unwrap();
    let first = stream.next().await.unwrap().unwrap();
    let pid = first.diagnostic.as_ref().unwrap()["pid"].as_u64().unwrap() as u32;
    drop(stream);
    for _ in 0..50 {
        if !is_alive(pid).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("agent {pid} survived the dropped stream");
}

#[tokio::test]
async fn an_idle_agent_times_out() {
    let fake = Fake::new(&[
        (
            "FAKE_AGENT_FIXTURE",
            fixture("codex", "text").display().to_string(),
        ),
        ("FAKE_AGENT_LINE_DELAY", "30".into()),
    ]);
    let connector =
        ExternalConnector::new(Codex, &fake.settings("codex")).with_timeouts(Timeouts {
            health: Duration::from_secs(5),
            startup: Duration::from_secs(5),
            idle: Duration::from_millis(500),
        });
    let events = collect(&connector, fake.request("x")).await;
    let last = events.last().unwrap();
    assert!(
        matches!(&last.kind, EventKind::Error { message, .. } if message.contains("no output for"))
    );
    let pid = events[0].diagnostic.as_ref().unwrap()["pid"]
        .as_u64()
        .unwrap() as u32;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!is_alive(pid).await);
}

#[tokio::test]
async fn a_missing_executable_is_reported_not_fatal() {
    let settings = ConnectorSettings {
        executables: BTreeMap::from([("codex".to_owned(), "/definitely/not/codex".to_owned())]),
        ..ConnectorSettings::default()
    };
    let connector = ExternalConnector::new(Codex, &settings);
    let info = connector.info().await.unwrap();
    assert_eq!(info.auth, AuthState::NotInstalled);
    assert_eq!(info.version, None);
    let error = connector
        .start(TaskRequest {
            session_id: SessionId("s".into()),
            prompt: "x".into(),
            workspace: std::env::temp_dir(),
            continuation: None,
        })
        .await
        .err()
        .expect("start fails");
    assert!(error.message.contains("not installed"));
}

#[tokio::test]
async fn health_probes_report_version_and_auth() {
    let fake = Fake::new(&[]);
    let info = fake.connector(Codex, "codex").info().await.unwrap();
    assert_eq!(info.version.as_deref(), Some("1.0.0"));
    assert_eq!(info.auth, AuthState::Authenticated);
    assert_eq!(info.transport, Transport::MachineReadable);
    assert!(!info.capabilities.approvals);
    let fake = Fake::new(&[("FAKE_AGENT_AUTH", "Not logged in".into())]);
    assert_eq!(
        fake.connector(Codex, "codex").info().await.unwrap().auth,
        AuthState::Unauthenticated
    );
    let fake = Fake::new(&[("FAKE_AGENT_AUTH", "{\"loggedIn\": false}".into())]);
    assert_eq!(
        fake.connector(ClaudeCode, "claude")
            .info()
            .await
            .unwrap()
            .auth,
        AuthState::Unauthenticated
    );
    let fake = Fake::new(&[("FAKE_AGENT_AUTH", "2 credentials".into())]);
    assert_eq!(
        fake.connector(OpenCode, "opencode")
            .info()
            .await
            .unwrap()
            .auth,
        AuthState::Authenticated
    );
    let fake = Fake::new(&[("FAKE_AGENT_AUTH", "0 credentials".into())]);
    assert_eq!(
        fake.connector(OpenCode, "opencode")
            .info()
            .await
            .unwrap()
            .auth,
        AuthState::Unknown
    );
}

#[tokio::test]
async fn follow_up_input_resumes_the_external_thread() {
    let args_file = tempfile::tempdir().unwrap();
    let args_path = args_file.path().join("args.txt");
    let fake = Fake::new(&[
        (
            "FAKE_AGENT_FIXTURE",
            fixture("codex", "text").display().to_string(),
        ),
        ("FAKE_AGENT_ARGS_FILE", args_path.display().to_string()),
    ]);
    let connector = fake.connector(Codex, "codex");
    let session = SessionId("s-test".into());
    let _ = collect(&connector, fake.request("first")).await;
    let first_args = std::fs::read_to_string(&args_path).unwrap();
    assert!(first_args.contains("exec\n--json\n"));
    assert!(first_args.ends_with("first\n"));
    assert!(!first_args.contains("resume"));
    connector
        .send_input(&session, "second".into())
        .await
        .unwrap();
    assert_eq!(
        connector.inspect(&session).await.unwrap().state,
        TaskState::AwaitingInput
    );
    let mut stream = connector.resume(&session).await.unwrap();
    while let Some(item) = stream.next().await {
        item.unwrap();
    }
    let second_args = std::fs::read_to_string(&args_path).unwrap();
    assert!(second_args.contains("resume\n--json\n"), "{second_args}");
    assert!(
        second_args.contains("thread-codex-0001\nsecond\n"),
        "{second_args}"
    );
    // A continuation handed back through the request resumes too.
    let mut request = fake.request("third");
    request.continuation = connector.continuation_for(&session);
    let fresh = fake.connector(Codex, "codex");
    let _ = collect(&fresh, request).await;
    let third_args = std::fs::read_to_string(&args_path).unwrap();
    assert!(
        third_args.contains("thread-codex-0001\nthird\n"),
        "{third_args}"
    );
    assert!(connector.resume(&session).await.is_err(), "no input queued");
}

#[tokio::test]
async fn approvals_are_reported_as_unsupported() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("codex", "text").display().to_string(),
    )]);
    let connector = fake.connector(Codex, "codex");
    let _ = collect(&connector, fake.request("x")).await;
    let error = connector
        .answer_approval(
            &SessionId("s-test".into()),
            gritt_core::event::ApprovalId("a".into()),
            gritt_core::event::ApprovalDecision::Approved,
        )
        .await
        .err()
        .unwrap();
    assert!(error.message.contains("cannot answer"));
}

#[tokio::test]
async fn secrets_are_redacted_from_events_and_diagnostics() {
    let leaky = tempfile::tempdir().unwrap();
    let path = leaky.path().join("leak.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n{\"type\":\"item.completed\",\"item\":{\"id\":\"i\",\"type\":\"agent_message\",\"text\":\"key is sk-leaked-value\"}}\n{\"type\":\"weird\",\"note\":\"sk-leaked-value\"}\n{\"type\":\"turn.completed\",\"usage\":{}}\n",
    )
    .unwrap();
    let fake = Fake::new(&[("FAKE_AGENT_FIXTURE", path.display().to_string())]);
    let connector = fake
        .connector(Codex, "codex")
        .with_secrets(vec![Secret::new("sk-leaked-value")]);
    let events = collect(&connector, fake.request("x")).await;
    let dump = serde_json::to_string(&events).unwrap();
    assert!(!dump.contains("sk-leaked-value"), "{dump}");
    assert!(dump.contains("[redacted]"));
}

#[tokio::test]
async fn pty_transport_reads_the_same_fixture() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_FIXTURE",
        fixture("codex", "text").display().to_string(),
    )]);
    let mut settings = fake.settings("codex");
    settings.pty = vec!["codex".into()];
    let connector = ExternalConnector::new(Codex, &settings).with_timeouts(Timeouts {
        health: Duration::from_secs(5),
        startup: Duration::from_secs(10),
        idle: Duration::from_secs(10),
    });
    assert_eq!(connector.transport(), Transport::Pty);
    let events = collect(&connector, fake.request("x")).await;
    assert_eq!(events[0].diagnostic.as_ref().unwrap()["transport"], "pty");
    assert_eq!(text_of(&events), "PONG");
    assert!(matches!(
        events.last().unwrap().kind,
        EventKind::Completed {
            stop_reason: StopReason::EndTurn
        }
    ));
}

#[test]
fn connector_names_parse() {
    use gritt_connector::parse_connector_id;
    assert_eq!(parse_connector_id("claude"), Some(ConnectorId::ClaudeCode));
    assert_eq!(
        parse_connector_id("claude-code"),
        Some(ConnectorId::ClaudeCode)
    );
    assert_eq!(parse_connector_id("OpenCode"), Some(ConnectorId::OpenCode));
    assert_eq!(parse_connector_id("native"), Some(ConnectorId::Native));
    assert_eq!(parse_connector_id("grok"), None);
}
