//! Deterministic fixture state for the reviewable prototype.
//!
//! Every value here is invented. Nothing in this module opens a session,
//! reads a catalog, launches an MCP server, or touches a keychain, and the
//! interface labels the run `fixture` so a screenshot can never be mistaken
//! for live data. TKT-0019 replaces these builders with the control plane.

use chrono::{TimeZone, Utc};
use gritt_core::connector::ConnectorId;
use gritt_core::event::Usage;
use gritt_core::mcp::{McpServerSnapshot, McpServerState, McpTransportKind};
use gritt_core::provider::{ModelCapabilities, ModelInfo, Protocol, ReasoningEffort};
use gritt_core::session::{Phase, Session, SessionId, SessionKind};

use super::app::{AgentSummary, App, EntryKind, ModelCatalogView, StatusBar};
use super::sidebar::{
    ChangeSource, ChangeStatus, ChangedFile, ChangedFiles, CostSection, IntegrationsSection,
    UsageSection,
};
use super::theme::Theme;
use crate::draft::{CatalogState, SessionDraft};
use crate::setup::{CredentialState, ProfileSummary};

/// Which fixture screen to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureScreen {
    Home,
    Conversation,
}

impl FixtureScreen {
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "home" => Some(FixtureScreen::Home),
            "conversation" | "chat" => Some(FixtureScreen::Conversation),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FixtureScreen::Home => "home",
            FixtureScreen::Conversation => "conversation",
        }
    }
}

fn profiles() -> Vec<ProfileSummary> {
    vec![
        ProfileSummary {
            name: "openai".into(),
            protocol: Protocol::Responses,
            base_url: "https://api.openai.com/v1".into(),
            credential: CredentialState::Available,
            is_default: true,
        },
        ProfileSummary {
            name: "anthropic".into(),
            protocol: Protocol::Messages,
            base_url: "https://api.anthropic.com/v1".into(),
            credential: CredentialState::Available,
            is_default: false,
        },
        ProfileSummary {
            name: "openrouter".into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://openrouter.ai/api/v1".into(),
            // Drives the model-picker to provider-setup round trip.
            credential: CredentialState::Missing {
                env_var_name: "OPENROUTER_API_KEY".into(),
            },
            is_default: false,
        },
    ]
}

fn agents() -> Vec<AgentSummary> {
    vec![
        AgentSummary {
            id: ConnectorId::Codex,
            name: "codex".into(),
            installed: true,
            version: Some("0.48.0".into()),
            authenticated: Some(true),
        },
        AgentSummary {
            id: ConnectorId::ClaudeCode,
            name: "claude-code".into(),
            installed: true,
            version: Some("2.1.4".into()),
            authenticated: Some(false),
        },
        AgentSummary {
            id: ConnectorId::Cursor,
            name: "cursor-agent".into(),
            installed: false,
            version: None,
            authenticated: None,
        },
    ]
}

fn model(id: &str, display: &str, reasoning: Option<bool>) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        display_name: Some(display.into()),
        capabilities: ModelCapabilities {
            context_length: Some(400_000),
            tools: Some(true),
            reasoning,
            ..ModelCapabilities::default()
        },
        replaced_by: None,
        deprecated: false,
    }
}

fn models() -> Vec<ModelInfo> {
    let mut deprecated = model("openai/gpt-4o-2024-05-13", "GPT-4o (May 2024)", Some(false));
    deprecated.deprecated = true;
    deprecated.replaced_by = Some("openai/gpt-4o".into());
    vec![
        model("openai/gpt-5-nano", "GPT-5 nano", Some(true)),
        model("openai/gpt-5", "GPT-5", Some(true)),
        // A long name and a Unicode one, so wrapping and truncation are
        // visible in the snapshots.
        model(
            "openai/gpt-5-codex-preview-2026-08-31-long-identifier",
            "GPT-5 Codex preview (2026-08-31, long identifier for layout)",
            Some(true),
        ),
        model("openai/o4-mini-思考", "o4 mini 思考 · 推論", Some(true)),
        deprecated,
    ]
}

/// Configured MCP entries in every state `/mcp` must account for.
pub fn mcp_servers() -> Vec<McpServerSnapshot> {
    vec![
        McpServerSnapshot {
            name: "gritt-local-memory".into(),
            state: McpServerState::Ready,
            transport: Some(McpTransportKind::Stdio),
            tool_count: 6,
            tools: vec!["memory_search".into(), "memory_write".into()],
            protocol_version: Some("2025-06-18".into()),
            server_version: Some("0.3.1".into()),
            fingerprint: "fixture-1".into(),
        },
        McpServerSnapshot {
            name: "turso-local-memory".into(),
            state: McpServerState::Starting,
            transport: Some(McpTransportKind::Stdio),
            tool_count: 0,
            tools: Vec::new(),
            protocol_version: None,
            server_version: None,
            fingerprint: "fixture-2".into(),
        },
        McpServerSnapshot {
            name: "unapproved-server".into(),
            state: McpServerState::AwaitingApproval,
            transport: Some(McpTransportKind::Stdio),
            tool_count: 0,
            tools: Vec::new(),
            protocol_version: None,
            server_version: None,
            fingerprint: "fixture-3".into(),
        },
        McpServerSnapshot {
            name: "broken-server".into(),
            state: McpServerState::Failed {
                reason: "the command exited with status 127".into(),
            },
            transport: Some(McpTransportKind::Stdio),
            tool_count: 0,
            tools: Vec::new(),
            protocol_version: None,
            server_version: None,
            fingerprint: "fixture-4".into(),
        },
        McpServerSnapshot {
            name: "legacy-sse".into(),
            state: McpServerState::UnsupportedTransport {
                reason: "standalone SSE is not supported; use Streamable HTTP".into(),
            },
            transport: None,
            tool_count: 0,
            tools: Vec::new(),
            protocol_version: None,
            server_version: None,
            fingerprint: "fixture-5".into(),
        },
    ]
}

fn session(name: &str, model: &str) -> Session {
    let at = Utc.with_ymd_and_hms(2026, 9, 5, 9, 30, 0).unwrap();
    Session {
        id: SessionId(format!("fixture-{name}")),
        name: name.into(),
        kind: SessionKind::Native {
            provider_profile: "openai".into(),
            model: model.into(),
            effort: ReasoningEffort::Medium,
        },
        phase: Phase::Coding,
        workspace: "/work/gritt".into(),
        created_at: at,
        updated_at: at,
        parent_id: None,
    }
}

/// The shared setup both screens start from.
fn base(theme: Theme) -> App {
    let mut app = App::new(StatusBar::default(), theme);
    app.fixture = Some("fixture".into());
    app.status.workspace = "/work/gritt".into();
    app.profiles = profiles();
    app.agents = agents();
    app.mcp = mcp_servers();
    app.sessions = vec![
        session("api-cleanup", "openai/gpt-5-nano"),
        session("docs-pass", "openai/gpt-5"),
    ];
    app.sidebar.session.workspace = Some("/work/gritt".into());
    app
}

/// The home screen: no connection yet, so the composer invites `/connect`
/// and the draft is empty.
pub fn home(theme: Theme) -> App {
    let mut app = base(theme);
    app.sidebar.session.activity = Some("no session yet".into());
    app.draft = SessionDraft::default();
    // A warm cache for the default profile, so the walkthrough can reach
    // the model picker without a network call. Selecting another profile
    // clears it, which is the dependent-reset behaviour.
    app.catalog = ModelCatalogView {
        profile: "openai".into(),
        models: models(),
        state: Some(CatalogState::Fresh {
            fetched_at: Utc.with_ymd_and_hms(2026, 9, 5, 8, 0, 0).unwrap(),
        }),
        loading: false,
    };
    app
}

/// A conversation with a streamed answer, a compact tool row, a populated
/// sidebar, and a session pinned to its model.
pub fn conversation(theme: Theme) -> App {
    let mut app = base(theme);
    let session = session("api-cleanup", "openai/gpt-5-nano");
    app.set_session(&session);
    app.set_effective_mode(Some(gritt_core::session::ExecutionMode::Supervised));
    app.session_pinned = true;
    app.draft = SessionDraft::default()
        .with_profile("openai")
        .with_model("openai/gpt-5-nano")
        .with_effort(ReasoningEffort::Medium);
    app.catalog = ModelCatalogView {
        profile: "openai".into(),
        models: models(),
        state: Some(CatalogState::Fresh {
            fetched_at: Utc.with_ymd_and_hms(2026, 9, 5, 8, 0, 0).unwrap(),
        }),
        loading: false,
    };
    app.status.effort = ReasoningEffort::Medium;
    app.status.connection = "Streaming".into();
    app.status.usage = Usage {
        input_tokens: Some(12_480),
        output_tokens: Some(3_210),
        ..Usage::default()
    };

    app.push(EntryKind::User, "Tidy the public API of the store module.");
    app.push(
        EntryKind::Reasoning,
        "Reading the module, then the callers.",
    );
    app.push(
        EntryKind::Assistant,
        "I read `store/mod.rs` and found three exported helpers that no caller \
         outside the crate uses. I will make them private and keep the trait \
         surface unchanged. 世界 renders at its real width.",
    );
    let mut tool = super::app::Entry::new(EntryKind::Tool, "-> file_read store/mod.rs");
    tool.detail = Some(
        "pub fn open() -> Result<Store>\npub fn migrate() -> Result<()>\npub(crate) fn seed()"
            .into(),
    );
    app.entries.push(tool);
    app.push(EntryKind::Tool, "<- file_read ok");
    // An escape sequence in model output is shown, never executed.
    app.push(
        EntryKind::System,
        "tool output contained \u{1b}[31m and was rendered as text",
    );

    app.sidebar.session.name = Some("api-cleanup".into());
    app.sidebar.session.activity = Some("streaming".into());
    app.sidebar.usage = UsageSection {
        input_tokens: Some(12_480),
        output_tokens: Some(3_210),
        // A fixture may show occupancy because it states its own source;
        // the live path leaves it unavailable until one exists.
        context_tokens: Some(31_500),
        context_limit: Some(400_000),
        last_request_input: Some(9_100),
        incomplete: false,
    };
    app.sidebar.cost = CostSection {
        estimate_usd: Some(0.0412),
        scope: Some("this session, listed prices".into()),
    };
    app.sidebar.changed_files = ChangedFiles::Observed {
        source: ChangeSource::Git,
        files: vec![
            ChangedFile {
                path: "crates/gritt-harness/src/store/mod.rs".into(),
                status: ChangeStatus::Modified,
                pre_existing: false,
            },
            ChangedFile {
                path: "README.md".into(),
                status: ChangeStatus::Modified,
                pre_existing: true,
            },
        ],
    };
    app.sidebar.integrations = IntegrationsSection {
        mcp: Some(mcp_servers()),
        mcp_owner: Some("Gritt".into()),
        connector_mcp: None,
    };
    app
}

/// Builds the requested fixture screen.
pub fn screen(screen: FixtureScreen, theme: Theme) -> App {
    match screen {
        FixtureScreen::Home => home(theme),
        FixtureScreen::Conversation => conversation(theme),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Layout;

    #[test]
    fn the_fixture_screens_are_labelled_and_open_nothing() {
        let home = home(Theme::default());
        assert_eq!(home.fixture.as_deref(), Some("fixture"));
        assert_eq!(home.layout(), Layout::Home);
        assert!(!home.is_connected());
        let conversation = conversation(Theme::default());
        assert_eq!(conversation.layout(), Layout::Conversation);
        assert!(conversation.is_connected());
        assert!(conversation.session_pinned);
    }

    #[test]
    fn the_fixture_covers_every_mcp_state_the_overlay_must_account_for() {
        let states: Vec<&str> = mcp_servers()
            .iter()
            .map(|server| crate::tui::sidebar::mcp_state_word(&server.state))
            .collect();
        assert!(states.contains(&"ready"));
        assert!(states.contains(&"starting"));
        assert!(states.contains(&"awaiting approval"));
        assert!(states.contains(&"failed"));
        assert!(states.contains(&"unsupported"));
    }

    #[test]
    fn screen_names_round_trip() {
        assert_eq!(FixtureScreen::parse("home"), Some(FixtureScreen::Home));
        assert_eq!(
            FixtureScreen::parse("Conversation"),
            Some(FixtureScreen::Conversation)
        );
        assert_eq!(FixtureScreen::parse("nope"), None);
        assert_eq!(FixtureScreen::Home.name(), "home");
    }
}
