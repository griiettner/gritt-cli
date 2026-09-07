//! Parser fixtures and fake-process coverage for each connector's own MCP
//! inventory (TKT-0026). Display only: nothing here adds, approves, or
//! connects to a server.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gritt_connector::protocols::claude::{parse_claude_mcp, ClaudeCode};
use gritt_connector::protocols::codex::{parse_codex_mcp, Codex};
use gritt_connector::protocols::cursor::Cursor;
use gritt_connector::protocols::opencode::{parse_opencode_mcp, OpenCode};
use gritt_connector::protocols::McpParseError;
use gritt_connector::{ExternalConnector, Protocol, Timeouts};
use gritt_core::config::ConnectorSettings;
use gritt_core::connector::{Connector, ConnectorId, ConnectorMcpDiscovery, ConnectorMcpStatus};
use gritt_core::secret::Secret;

fn fixture(connector: &str, name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp")
        .join(connector)
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn fixture_path(connector: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp")
        .join(connector)
        .join(name)
}

struct Fake {
    dir: tempfile::TempDir,
    wrapper: PathBuf,
}

impl Fake {
    fn new(vars: &[(&str, String)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("agent");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-agent/agent.sh");
        let mut text = String::from("#!/bin/sh\n");
        for (name, value) in vars {
            text.push_str(&format!(
                "{name}='{}'\nexport {name}\n",
                value.replace('\'', "'\\''")
            ));
        }
        text.push_str(&format!("exec '{}' \"$@\"\n", script.display()));
        std::fs::write(&wrapper, text).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir, wrapper }
    }

    fn workspace(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    fn connector<P: Protocol>(&self, protocol: P, name: &str) -> ExternalConnector<P> {
        let settings = ConnectorSettings {
            executables: BTreeMap::from([(name.to_owned(), self.wrapper.display().to_string())]),
            ..ConnectorSettings::default()
        };
        ExternalConnector::new(protocol, &settings).with_timeouts(Timeouts {
            health: Duration::from_secs(2),
            startup: Duration::from_secs(5),
            idle: Duration::from_secs(5),
        })
    }
}

fn dump(discovery: &ConnectorMcpDiscovery) -> String {
    serde_json::to_string(discovery).unwrap()
}

#[test]
fn codex_json_listing_keeps_names_and_status_and_drops_everything_secret() {
    let servers = parse_codex_mcp(&fixture("codex", "list.json")).unwrap();
    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "cognity",
            "computer-use",
            "node_repl",
            "playwright",
            "remote-design"
        ]
    );
    assert_eq!(servers[0].status, ConnectorMcpStatus::Enabled);
    assert_eq!(servers[0].transport.as_deref(), Some("stdio"));
    assert_eq!(
        servers[0].target.as_deref(),
        Some("/home/user/.cargo/bin/gritt-cognity")
    );
    assert_eq!(servers[1].status, ConnectorMcpStatus::Disabled);
    assert_eq!(servers[4].status, ConnectorMcpStatus::NeedsAuth);
    assert_eq!(servers[4].transport.as_deref(), Some("streamable_http"));
    let text = serde_json::to_string(&servers).unwrap();
    for never in [
        "sk-fixture-env-value-never-shown",
        "FAKE_REPL_TOKEN",
        "sk-fixture-header-secret",
        "Authorization",
        "DESIGN_TOKEN",
        "@playwright/mcp@latest",
    ] {
        assert!(!text.contains(never), "{never} leaked into {text}");
    }
}

#[test]
fn claude_text_listing_reads_each_status_line() {
    let servers = parse_claude_mcp(&fixture("claude", "list.txt")).unwrap();
    assert_eq!(servers.len(), 4);
    assert_eq!(servers[0].name, "claude.ai Google Drive");
    assert_eq!(servers[0].status, ConnectorMcpStatus::Connected);
    assert_eq!(servers[0].transport.as_deref(), Some("http"));
    assert_eq!(servers[1].name, "gritt");
    assert_eq!(servers[1].status, ConnectorMcpStatus::PendingApproval);
    assert_eq!(
        servers[1].detail.as_deref(),
        Some("run `claude` to approve")
    );
    assert_eq!(
        servers[1].target.as_deref(),
        Some(".agents/gritt-agent mcp serve")
    );
    assert_eq!(servers[2].name, "broken-demo");
    assert_eq!(servers[2].status, ConnectorMcpStatus::Failed);
    assert_eq!(servers[3].status, ConnectorMcpStatus::NeedsAuth);
    assert_eq!(
        parse_claude_mcp(&fixture("claude", "empty.txt")).unwrap(),
        vec![]
    );
    assert_eq!(
        parse_claude_mcp(&fixture("claude", "malformed.txt")),
        Err(McpParseError::Malformed)
    );
    assert_eq!(parse_claude_mcp(""), Err(McpParseError::Malformed));
}

#[test]
fn opencode_boxed_listing_reads_status_hint_and_target() {
    let servers = parse_opencode_mcp(&fixture("opencode", "list.txt")).unwrap();
    assert_eq!(servers.len(), 3);
    assert_eq!(servers[0].name, "demo-local");
    assert_eq!(servers[0].status, ConnectorMcpStatus::Failed);
    assert_eq!(
        servers[0].detail.as_deref(),
        Some("MCP error -32000: Connection closed")
    );
    assert_eq!(
        servers[0].target.as_deref(),
        Some("echo hi --token sk-fixture-arg-secret"),
        "the parser hands the raw line on; the connector redacts it"
    );
    assert_eq!(servers[0].transport.as_deref(), Some("stdio"));
    assert_eq!(servers[1].name, "demo-remote");
    assert_eq!(servers[1].status, ConnectorMcpStatus::Disabled);
    assert_eq!(
        servers[1].target.as_deref(),
        Some("https://example.invalid/mcp")
    );
    assert_eq!(servers[1].transport.as_deref(), Some("http"));
    assert_eq!(servers[2].name, "memory");
    assert_eq!(servers[2].status, ConnectorMcpStatus::Connected);
    assert_eq!(servers[2].detail.as_deref(), Some("OAuth"));
    assert_eq!(
        parse_opencode_mcp(&fixture("opencode", "empty.txt")).unwrap(),
        vec![]
    );
    assert_eq!(
        parse_opencode_mcp(&fixture("opencode", "malformed.txt")),
        Err(McpParseError::Malformed)
    );
}

#[test]
fn codex_listing_survives_a_leading_diagnostic_line_and_trailing_text() {
    let noisy = format!(
        "[2026-09-06T12:00:00] warning: config migrated\n{}\ndone\n",
        fixture("codex", "list.json")
    );
    let servers = parse_codex_mcp(&noisy).unwrap();
    assert_eq!(servers.len(), 5);
}

#[test]
fn codex_rejects_malformed_documents() {
    assert_eq!(
        parse_codex_mcp(&fixture("codex", "malformed.json")),
        Err(McpParseError::Malformed)
    );
    assert_eq!(parse_codex_mcp(""), Err(McpParseError::Malformed));
    assert_eq!(
        parse_codex_mcp("[{\"enabled\":true}]"),
        Err(McpParseError::Malformed)
    );
}

#[tokio::test]
async fn a_current_inventory_is_redacted_before_it_is_kept() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_MCP_FILE",
        fixture_path("codex", "list.json").display().to_string(),
    )]);
    let connector = fake
        .connector(Codex, "codex")
        .with_secrets(vec![Secret::new("gritt-cognity")]);
    let outcome = connector.discover_mcp_inventory(fake.workspace()).await;
    let ConnectorMcpDiscovery::Current { inventory } = &outcome else {
        panic!("expected a current inventory, got {outcome:?}");
    };
    assert_eq!(inventory.connector, ConnectorId::Codex);
    assert_eq!(inventory.source, "codex mcp list --json");
    assert_eq!(inventory.servers.len(), 5);
    let remote = &inventory.servers[4];
    assert_eq!(
        remote.target.as_deref(),
        Some("https://design.example.invalid/mcp"),
        "the query string carried a token and must be gone"
    );
    assert_eq!(
        inventory.servers[0].target.as_deref(),
        Some("/home/user/.cargo/bin/[redacted]"),
        "a known secret is redacted out of a command path"
    );
    let text = dump(&outcome);
    for never in [
        "sk-fixture",
        "FAKE_REPL_TOKEN",
        "Authorization",
        "DESIGN_TOKEN",
        "gritt-cognity",
    ] {
        assert!(!text.contains(never), "{never} leaked into {text}");
    }
}

#[tokio::test]
async fn claude_command_lines_lose_credential_option_values() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_MCP_FILE",
        fixture_path("claude", "list.txt").display().to_string(),
    )]);
    let connector = fake.connector(ClaudeCode, "claude");
    let outcome = connector.discover_mcp_inventory(fake.workspace()).await;
    let inventory = outcome.inventory().expect("current");
    assert_eq!(inventory.source, "claude mcp list");
    assert_eq!(
        inventory.servers[2].target.as_deref(),
        Some("/opt/mcp/server --api-key [redacted] --port 8080")
    );
    assert!(!dump(&outcome).contains("sk-fixture"));
}

#[tokio::test]
async fn opencode_listing_runs_through_the_same_redaction() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_MCP_FILE",
        fixture_path("opencode", "list.txt").display().to_string(),
    )]);
    let connector = fake.connector(OpenCode, "opencode");
    let outcome = connector.discover_mcp_inventory(fake.workspace()).await;
    let inventory = outcome.inventory().expect("current");
    assert_eq!(inventory.source, "opencode mcp list");
    assert_eq!(
        inventory.servers[0].target.as_deref(),
        Some("echo hi --token [redacted]")
    );
    assert!(!dump(&outcome).contains("sk-fixture"));
}

#[tokio::test]
async fn the_fake_default_listing_never_keeps_args_or_env() {
    let fake = Fake::new(&[]);
    let outcome = fake
        .connector(Codex, "codex")
        .discover_mcp_inventory(fake.workspace())
        .await;
    let inventory = outcome.inventory().expect("current");
    assert_eq!(inventory.servers[0].name, "fake-server");
    assert_eq!(inventory.servers[0].target.as_deref(), Some("fake-mcp"));
    let text = dump(&outcome);
    assert!(!text.contains("sk-fake"), "{text}");
    assert!(!text.contains("FAKE_TOKEN"), "{text}");
}

/// The listing runs in the session workspace, where an agent's
/// project-scoped servers live, not in Gritt's own directory.
#[tokio::test]
async fn the_listing_runs_in_the_session_workspace() {
    let fake = Fake::new(&[("FAKE_AGENT_MCP_FILE", "./mcp-in-workspace.json".into())]);
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("mcp-in-workspace.json"),
        r#"[{"name":"project-only","enabled":true,"transport":{"type":"stdio","command":"srv"},"auth_status":"unsupported"}]"#,
    )
    .unwrap();
    let outcome = fake
        .connector(Codex, "codex")
        .discover_mcp_inventory(workspace.path().to_path_buf())
        .await;
    let inventory = outcome.inventory().expect("current");
    assert_eq!(inventory.servers[0].name, "project-only");
}

#[tokio::test]
async fn an_empty_listing_is_current_and_empty_not_an_error() {
    let fake = Fake::new(&[(
        "FAKE_AGENT_MCP_FILE",
        fixture_path("opencode", "empty.txt").display().to_string(),
    )]);
    let outcome = fake
        .connector(OpenCode, "opencode")
        .discover_mcp_inventory(fake.workspace())
        .await;
    let inventory = outcome.inventory().expect("current");
    assert!(inventory.servers.is_empty());
    assert!(outcome.describe().contains("no MCP servers"));
}

#[tokio::test]
async fn missing_executable_is_unavailable() {
    let settings = ConnectorSettings {
        executables: BTreeMap::from([(
            "codex".to_owned(),
            "/definitely/not/an/installed/codex".into(),
        )]),
        ..ConnectorSettings::default()
    };
    let outcome = ExternalConnector::new(Codex, &settings)
        .discover_mcp_inventory(std::env::temp_dir())
        .await;
    assert!(
        matches!(
            outcome,
            ConnectorMcpDiscovery::Unavailable {
                connector: ConnectorId::Codex,
                ..
            }
        ),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn cursor_is_unsupported_with_its_documented_reason() {
    let fake = Fake::new(&[]);
    let outcome = fake
        .connector(Cursor, "cursor")
        .discover_mcp_inventory(fake.workspace())
        .await;
    let ConnectorMcpDiscovery::Unsupported { connector, reason } = &outcome else {
        panic!("expected unsupported, got {outcome:?}");
    };
    assert_eq!(*connector, ConnectorId::Cursor);
    assert!(reason.contains("interactive menu"), "{reason}");
    assert!(outcome.describe().contains("does not list"));
}

#[tokio::test]
async fn command_failure_timeout_and_malformed_output_are_each_typed() {
    let failing = Fake::new(&[("FAKE_AGENT_MCP_EXIT", "1".into())]);
    let outcome = failing
        .connector(Codex, "codex")
        .discover_mcp_inventory(failing.workspace())
        .await;
    assert!(
        matches!(outcome, ConnectorMcpDiscovery::CommandFailure { .. }),
        "{outcome:?}"
    );
    assert!(outcome.describe().contains("failed"));

    let slow = Fake::new(&[("FAKE_AGENT_MCP_SLEEP", "5".into())]);
    let started = std::time::Instant::now();
    let outcome = slow
        .connector(Codex, "codex")
        .discover_mcp_inventory(failing.workspace())
        .await;
    assert!(
        matches!(outcome, ConnectorMcpDiscovery::TimedOut { .. }),
        "{outcome:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "the health timeout bounds the listing"
    );

    let malformed = Fake::new(&[(
        "FAKE_AGENT_MCP_FILE",
        fixture_path("codex", "malformed.json")
            .display()
            .to_string(),
    )]);
    let outcome = malformed
        .connector(Codex, "codex")
        .discover_mcp_inventory(failing.workspace())
        .await;
    assert!(
        matches!(outcome, ConnectorMcpDiscovery::MalformedOutput { .. }),
        "{outcome:?}"
    );
}
