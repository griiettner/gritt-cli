//! Full-screen state and its reducers. Everything here is plain data so
//! the key handling, command dispatch, and transcript logic run under
//! `cargo test` without a terminal.
//!
//! The state is a client of the control plane and nothing more: it holds
//! the choices a user has made and the values the harness handed it. It
//! never resolves a model, reads a config file, or opens a session.

use std::cell::{Cell, RefCell};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gritt_core::connector::{
    ConnectorId, ConnectorModel, ConnectorModelDiscovery, ConnectorUpdateOutcome,
    ConnectorVersionCheck, UpdateAction, VersionCheckMode,
};
use gritt_core::event::{
    ApprovalDecision, ApprovalRequest, Event, EventKind, SessionStatus, Usage,
};
use gritt_core::mcp::{McpServerSnapshot, McpServerState, TrustDecision};
use gritt_core::provider::{ModelInfo, Protocol, ProviderProfile, ReasoningEffort};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{Phase, Session, SessionId};
use gritt_core::tool::native;
use ratatui::text::Line;

use super::command::{self, Command, Parsed};
use super::composer::Composer;
use super::picker::{ListStatus, Picker, PickerRow};
use super::sidebar::{self, SidebarModel, SidebarPlacement};
use super::theme::Theme;
use crate::changes::{ChangedFiles, FileDiff};
use crate::control::ControlPlane;
use crate::draft::{CatalogState, DraftError, DraftWarning, SessionDraft};
use crate::modes::print::describe_call;
use crate::policy::Decision;
use crate::setup::{ConfigDestination, CredentialState, ProfileSummary, SetupSubmission};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    /// The compact one-line form for a tool row, the whole message for
    /// everything else.
    pub text: String,
    /// Tool output, shown when `/details` is on.
    pub detail: Option<String>,
}

impl Entry {
    pub fn new(kind: EntryKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: sanitize(&text.into()),
            detail: None,
        }
    }
}

/// Replaces control characters that a terminal would act on with a
/// visible placeholder, so an escape sequence in tool or model output is
/// rendered rather than executed. Tabs and newlines are kept.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' | '\t' => c,
            '\u{1b}' => '␛',
            c if c.is_control() => '·',
            c => c,
        })
        .collect()
}

/// Which of the two main compositions is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Centered wordmark and composer, before the first submission.
    Home,
    /// Header, transcript, sidebar column, composer, footer.
    Conversation,
}

/// What has the keyboard when no overlay is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Composer,
    Transcript,
    Sidebar,
}

/// Kept for the existing runtime and PTY tests, which name the palette,
/// session list, and diff views.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Transcript,
    Palette,
    Sessions,
    Diff,
}

/// An `ask` waiting for the user. `preview` is the write diff.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub request: ApprovalRequest,
    pub decision: Decision,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StatusBar {
    pub profile: String,
    pub model: String,
    pub session: String,
    pub phase: String,
    pub usage: Usage,
    pub connection: String,
    pub workspace: String,
    pub effort: ReasoningEffort,
}

/// What the runtime should do after a key. Everything asynchronous is a
/// request here, never work the reducer does itself: the reducer stays
/// synchronous and testable, and the loop keeps drawing while it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Submit(String),
    Cancel,
    Quit,
    SetPhase(Phase),
    SetMode(gritt_core::session::ExecutionMode),
    Resume(SessionId),
    Approve(ApprovalDecision),
    RefreshSessions,
    /// Load a profile's model list. `selection` is the token the result
    /// must still match; a later profile change makes it stale.
    LoadCatalog {
        profile: String,
        selection: u64,
    },
    /// Load an installed agent's model catalog. `refresh` bypasses the
    /// short-lived cache.
    LoadConnectorCatalog {
        connector: ConnectorId,
        selection: u64,
        refresh: bool,
    },
    /// Persist the effort on the live native session.
    SetEffort(ReasoningEffort),
    /// Write the setup form through the injected `ProviderSetup`, then
    /// reload the configuration. The values are read from the form with
    /// [`App::take_setup_submission`] so no secret ever enters an action.
    SaveProfile,
    /// Leave the current session for a fresh draft. The session is kept.
    NewSession,
    /// A typed MCP runtime action from `/mcp`.
    Mcp(McpRequest),
    /// Re-read the MCP snapshots now, for the first `/mcp` open.
    RefreshMcp,
    /// Rescan workspace changes.
    ScanChanges,
    /// Open a read-only diff for a changed file.
    OpenFileDiff(String),
    /// Start a session on an installed agent, chosen explicitly from its
    /// detail view rather than by highlighting its row.
    SelectConnector(ConnectorId),
    /// Check the live connector session's CLI version. `mode` says
    /// whether a still-fresh cached answer is enough.
    LoadConnectorVersion {
        connector: ConnectorId,
        mode: VersionCheckMode,
    },
    /// Run an update the user approved in the modal overlay. The action
    /// is the one the overlay displayed; nothing is re-derived here.
    RunConnectorUpdate {
        connector: ConnectorId,
        action: UpdateAction,
    },
}

/// What `/mcp` asks the runtime to do. The runtime calls the same typed
/// API `gritt mcp trust` uses; nothing here is a parsed string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpRequest {
    /// Fetch a safe summary of what this server would run or connect to
    /// and put it in front of the user. Approving is a launch decision, so
    /// it goes through the same modal overlay a tool approval does rather
    /// than being granted by a highlighted row.
    RequestApproval {
        server: String,
    },
    Decide {
        server: String,
        decision: TrustDecision,
    },
    Restart {
        server: String,
    },
    Stop {
        server: String,
    },
    ReloadAll,
}

/// An installed external agent as the connection dialog sees it. The
/// connector owns its own model and permissions (ADR-010); this is only
/// what Gritt can honestly report about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSummary {
    pub id: ConnectorId,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    /// `None` when the agent does not report an auth state.
    pub authenticated: Option<bool>,
}

/// The catalog for the selected profile, as a picker shows it.
#[derive(Debug, Clone, Default)]
pub struct ModelCatalogView {
    pub profile: String,
    pub models: Vec<ModelInfo>,
    pub state: Option<CatalogState>,
    /// A refresh is in flight.
    pub loading: bool,
}

/// The catalog for the drafted connector, as the same picker shows it.
#[derive(Debug, Clone, Default)]
pub struct ConnectorCatalogView {
    pub connector: Option<ConnectorId>,
    pub models: Vec<ConnectorModel>,
    pub discovery: Option<ConnectorModelDiscovery>,
    pub loading: bool,
}

/// A kind of asynchronous work the interface can be waiting on.
///
/// The order is the display priority: when several are in flight, the
/// label shown is the first of these that is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Work {
    /// Opening, resuming, or replacing the session.
    Open,
    /// An MCP lifecycle action: a trust decision, a restart, a stop, or a
    /// reload. Detached, with its own cancellation token.
    Mcp,
    /// Reading a server's definition for a launch approval.
    ///
    /// A separate kind from the lifecycle action deliberately. The read is
    /// an ordinary cancellable request and the action that follows it is
    /// not, and sharing one kind let the request's bookkeeping end the
    /// label of a launch that was really running.
    McpDefinition,
    /// Writing a provider profile and re-reading the configuration.
    Setup,
    /// Loading a profile's model list.
    Catalog,
    /// Listing sessions.
    Sessions,
    /// Reading a file's diff.
    Diff,
    /// Checking an agent CLI's version against its published one.
    Version,
    /// Running an approved agent CLI update.
    Update,
}

/// Which searchable list an overlay is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Mode,
    Commands,
    Connect,
    Models,
    Effort,
    Sessions,
    Mcp,
    /// The actions available on one MCP server, opened from `/mcp`.
    McpActions,
    /// The changed files the sidebar lists, opened from the sidebar.
    Changes,
}

/// A supported provider preset: the endpoint and protocol Gritt already
/// knows, so setting one up asks only for a key. The values match the
/// shipped `config.toml` template; a provider that is not here is set up
/// as a custom endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPreset {
    pub name: &'static str,
    pub protocol: Protocol,
    pub base_url: &'static str,
}

pub const PRESETS: [ProviderPreset; 4] = [
    ProviderPreset {
        name: "openrouter",
        protocol: Protocol::ChatCompletions,
        base_url: "https://openrouter.ai/api/v1",
    },
    ProviderPreset {
        name: "openai",
        protocol: Protocol::Responses,
        base_url: "https://api.openai.com/v1",
    },
    ProviderPreset {
        name: "anthropic",
        protocol: Protocol::Messages,
        base_url: "https://api.anthropic.com",
    },
    ProviderPreset {
        name: "local",
        protocol: Protocol::ChatCompletions,
        base_url: "http://127.0.0.1:8080/v1",
    },
];

/// The environment variable Gritt looks for when a profile does not name
/// one: the profile name, upper-cased, with `_API_KEY` appended.
pub fn default_env_var(name: &str) -> String {
    format!("{}_API_KEY", name.to_ascii_uppercase().replace('-', "_"))
}

/// The protocol a preset of this name speaks, defaulting to the widely
/// compatible one for a custom endpoint.
fn preset_protocol(name: &str) -> Protocol {
    PRESETS
        .iter()
        .find(|preset| preset.name == name)
        .map(|preset| preset.protocol)
        .unwrap_or(Protocol::ChatCompletions)
}

/// Which field of the provider setup form has the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupField {
    Name,
    BaseUrl,
    EnvVar,
    Secret,
}

impl SetupField {
    pub const ORDER: [SetupField; 4] = [
        SetupField::Name,
        SetupField::BaseUrl,
        SetupField::EnvVar,
        SetupField::Secret,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SetupField::Name => "profile name",
            SetupField::BaseUrl => "endpoint",
            SetupField::EnvVar => "key variable",
            SetupField::Secret => "API key",
        }
    }

    /// The key field is echoed as dots and never reaches the transcript.
    pub fn is_secret(self) -> bool {
        self == SetupField::Secret
    }
}

/// The provider setup screens. The write itself is the injected
/// `ProviderSetup`; this only collects the fields.
#[derive(Clone)]
pub struct SetupForm {
    pub name: Composer,
    pub base_url: Composer,
    pub env_var: Composer,
    secret: Composer,
    pub field_index: usize,
    pub destination: ConfigDestination,
    /// The wire protocol the profile speaks. Cycled with Ctrl-T, because
    /// a preset sets it and a custom endpoint has to state it.
    pub protocol: Protocol,
    /// The outcome line shown after an attempted save.
    pub outcome: Option<String>,
    /// True while the write is in flight, so a second Enter cannot start
    /// a second one.
    pub saving: bool,
}

/// The derived `Debug` would have printed the key composer's buffer.
/// Nothing logs this today, but the type is reachable from `App`'s
/// `Debug`, and a secret that only one careless format string away from a
/// transcript is not a boundary. Only the length leaves.
impl std::fmt::Debug for SetupForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupForm")
            .field("name", &self.name.text())
            .field("base_url", &self.base_url.text())
            .field("env_var", &self.env_var.text())
            .field("secret_len", &self.secret_len())
            .field("field_index", &self.field_index)
            .field("destination", &self.destination)
            .field("protocol", &self.protocol)
            .field("outcome", &self.outcome)
            .field("saving", &self.saving)
            .finish()
    }
}

impl Default for SetupForm {
    fn default() -> Self {
        SetupForm::for_profile("")
    }
}

impl SetupForm {
    pub fn for_profile(name: &str) -> Self {
        Self {
            name: Composer::from_text(name),
            base_url: Composer::new(),
            env_var: Composer::from_text(if name.is_empty() {
                // A custom endpoint has no name yet, so there is no
                // variable to suggest; `profile_spec` derives one from
                // whatever name is typed.
                String::new()
            } else {
                default_env_var(name)
            }),
            secret: Composer::new(),
            // A named preset needs only its endpoint and key; a custom
            // endpoint starts at the name.
            field_index: usize::from(!name.is_empty()),
            destination: ConfigDestination::User,
            protocol: preset_protocol(name),
            outcome: None,
            saving: false,
        }
    }

    /// A form seeded from a supported preset: its endpoint and protocol
    /// are filled in and only the key is missing.
    pub fn for_preset(preset: &ProviderPreset) -> Self {
        let mut form = SetupForm::for_profile(preset.name);
        form.base_url = Composer::from_text(preset.base_url);
        form.protocol = preset.protocol;
        form.field_index = 3;
        form
    }

    pub fn field(&self) -> SetupField {
        SetupField::ORDER[self.field_index % SetupField::ORDER.len()]
    }

    /// The number of characters typed into the key field. The value
    /// itself has no accessor, so it cannot reach a transcript or a log.
    pub fn secret_len(&self) -> usize {
        self.secret.text().chars().count()
    }

    /// The focused field, for the test that proves the key never prints.
    #[cfg(test)]
    pub fn current_for_test(&mut self) -> &mut Composer {
        self.current()
    }

    fn current(&mut self) -> &mut Composer {
        match self.field() {
            SetupField::Name => &mut self.name,
            SetupField::BaseUrl => &mut self.base_url,
            SetupField::EnvVar => &mut self.env_var,
            SetupField::Secret => &mut self.secret,
        }
    }

    fn next_field(&mut self) {
        self.field_index = (self.field_index + 1) % SetupField::ORDER.len();
    }

    fn previous_field(&mut self) {
        self.field_index =
            (self.field_index + SetupField::ORDER.len() - 1) % SetupField::ORDER.len();
    }

    /// The non-secret profile this form describes, or the field that is
    /// still empty. The key is not part of it: it travels separately and
    /// only to the keychain.
    pub fn profile_spec(&self) -> Result<ProviderProfile, SetupField> {
        let name = self.name.text().trim().to_owned();
        if name.is_empty() {
            return Err(SetupField::Name);
        }
        let base_url = self.base_url.text().trim().to_owned();
        if base_url.is_empty() {
            return Err(SetupField::BaseUrl);
        }
        // A blank variable is filled in from the name rather than
        // refused: `LOCAL_API_KEY` is what the user would have typed.
        let env_var = match self.env_var.text().trim() {
            "" => default_env_var(&name),
            typed => typed.to_owned(),
        };
        Ok(ProviderProfile {
            name: name.clone(),
            protocol: self.protocol,
            base_url,
            key: SecretRef::for_profile(&name, &env_var),
            aliases: Default::default(),
            fallback_model: None,
        })
    }

    fn cycle_protocol(&mut self) {
        self.protocol = match self.protocol {
            Protocol::ChatCompletions => Protocol::Responses,
            Protocol::Responses => Protocol::Messages,
            Protocol::Messages => Protocol::ChatCompletions,
        };
    }
}

/// A modal explanation with no choice to make beyond acknowledging it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// Set when the notice is the "this needs a new session" explanation.
    pub is_error: bool,
    /// When set, this notice is a detail view with something to accept:
    /// Enter selects that connector. A notice with `None` only closes, so
    /// nothing can be started by acknowledging an explanation.
    pub confirm: Option<ConnectorId>,
}

/// Everything that can sit above the main layout, most recent last.
#[derive(Debug, Clone)]
pub enum Overlay {
    Picker {
        kind: PickerKind,
        picker: Picker,
    },
    Setup(SetupForm),
    Notice(Notice),
    Help {
        scroll: usize,
    },
    /// A read-only diff for one changed file, opened from the sidebar.
    /// Nothing here can write: the harness produced the text.
    FileDiff {
        path: String,
        body: String,
        scroll: usize,
    },
    /// The narrow-terminal form of the sidebar. Closing it restores the
    /// focus and the scroll position it covered.
    Drawer {
        scroll: usize,
        restore_focus: Focus,
        restore_scroll: usize,
    },
}

impl Overlay {
    pub fn picker_kind(&self) -> Option<PickerKind> {
        match self {
            Overlay::Picker { kind, .. } => Some(*kind),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct LayoutCache {
    width: usize,
    revision: u64,
    lines: Vec<Line<'static>>,
}

#[derive(Debug)]
pub struct App {
    pub entries: Vec<Entry>,
    pub composer: Composer,
    pub status: StatusBar,
    pub pending: Option<PendingApproval>,
    /// How many approvals have been installed on `pending`, ever.
    ///
    /// The identity of the request on screen, not merely whether one is
    /// there. The loop must not let a decision key answer an approval that
    /// has not been drawn, and presence alone cannot tell one request from
    /// the next: answering A and installing B in the same iteration leaves
    /// `pending` `Some` throughout, so B would inherit A's visibility
    /// (TKT-0020).
    pending_installs: u64,
    pub view: View,
    pub overlays: Vec<Overlay>,
    pub focus: Focus,
    pub sessions: Vec<Session>,
    /// The first rendered transcript line on screen while the viewport is
    /// held. Held position is measured from the top, not the bottom: new
    /// output is appended below, so a top anchor keeps the same content on
    /// screen and a bottom offset would not.
    pub top: usize,
    pub diff_scroll: usize,
    /// True while the viewport sits at the bottom and follows streaming.
    pub follow: bool,
    /// Set when output arrived while the reader was scrolled up.
    pub new_output: bool,
    pub running: bool,
    pub quit: bool,
    pub theme: Theme,
    pub notice: Option<String>,
    /// `/details`: tool rows show their output.
    pub tool_details: bool,
    /// The user's `/sidebar` choice on a wide terminal.
    pub sidebar_enabled: bool,
    pub sidebar: SidebarModel,
    pub sidebar_scroll: usize,
    /// Fixture mode label, shown in the interface so a screenshot can
    /// never be mistaken for live data.
    pub fixture: Option<String>,
    /// Highlighted `/` suggestion.
    pub suggestion_index: usize,
    suggestions_dismissed: bool,
    /// The keyboard copy target, filled by Ctrl-Y.
    pub clipboard: Option<String>,
    // Session-draft state the pickers read and write.
    pub draft: SessionDraft,
    /// The last successful session's effort, shown as the selection while
    /// the draft names none. It stays out of the draft so the resolver can
    /// return it to the provider default where the model cannot take it.
    pub remembered_effort: Option<ReasoningEffort>,
    pub profiles: Vec<ProfileSummary>,
    pub agents: Vec<AgentSummary>,
    pub catalog: ModelCatalogView,
    /// True once the current session has history, which pins its provider
    /// and model until a new session is started.
    pub session_pinned: bool,
    pub mcp: Vec<McpServerSnapshot>,
    /// Bumped on every provider selection. An asynchronous catalog result
    /// carries the token it started under and is dropped when it no
    /// longer matches, so a slow list for the previous profile can never
    /// land under the current one.
    pub selection: u64,
    /// The asynchronous work in flight, by kind, with the label each one
    /// shows.
    ///
    /// A map and not one field. One kind finishing says nothing about
    /// another still running, and a single shared flag meant an ordinary
    /// catalog response could clear the line an MCP restart had put up,
    /// after which Escape had nothing to act on and the restart could not
    /// be cancelled at all.
    busy: std::collections::BTreeMap<Work, String>,
    /// The live session, `None` before the first prompt opens one.
    pub session_id: Option<SessionId>,
    /// Set for a connector session: the agent owns its model, effort, and
    /// permissions (ADR-010), so the native pickers are refused.
    pub connector: Option<ConnectorId>,
    /// Connector chosen from `/connect` before a session exists. The model
    /// picker loads this agent's catalog instead of a provider list.
    pub connector_choice: Option<ConnectorId>,
    /// Model chosen for that drafted connector. Kept off `draft.model` so
    /// a native session after `/new` cannot open with a connector id.
    pub connector_model: Option<String>,
    pub connector_catalog: ConnectorCatalogView,
    /// The live connector session's version check, once one has run.
    pub connector_version: Option<ConnectorVersionCheck>,
    /// The update the approval overlay is showing. Answering the overlay
    /// takes it, so a late key cannot run a command that was withdrawn.
    pub update_approval: Option<(ConnectorId, UpdateAction)>,
    /// The server `/mcp` opened an action list for.
    pub mcp_target: Option<String>,
    /// Set while the pending approval is an MCP server launch rather than
    /// a tool call, so answering it records a trust decision instead of
    /// answering a tool the agent is waiting on.
    pub mcp_approval: Option<String>,
    /// True between asking for a session and having one.
    ///
    /// The driver that answers is not known yet, so a prompt submitted
    /// now would run on the session being left. Submission and settings
    /// both wait, and Escape cancels the transition.
    pub session_transition: bool,
    /// File writes seen this turn, by call id, promoted to the changed
    /// list only when their result says the write succeeded.
    pending_writes: std::collections::BTreeMap<String, String>,
    /// Paths from successful native writes, for the runtime to record
    /// against the workspace observer.
    pub observed_writes: Vec<String>,
    /// Listed prices per million tokens for the active model, when the
    /// catalog reports them. Cost is shown only when both halves and the
    /// reported usage exist, and it is always labelled an estimate.
    pricing: Option<(f64, f64)>,
    stream_open: Option<EntryKind>,
    revision: u64,
    cache: RefCell<LayoutCache>,
    /// What the last frame measured: wrapped transcript lines, the height
    /// of the transcript area, and the terminal width. The reducer needs
    /// them to answer "is the viewport at the bottom" and "is the sidebar
    /// a column here", and only the renderer knows them.
    metrics: Cell<Metrics>,
    /// Frames drawn since this state was created.
    ///
    /// The responsiveness work (TKT-0020) needs to know how many frames a
    /// burst of events actually cost, and a deterministic harness cannot
    /// read that from a terminal. Feeding synthetic events is already
    /// possible through [`App::on_event`] and [`App::on_key`]; this is the
    /// other half of that seam. Nothing in the interface reads it.
    frames: Cell<u64>,
}

/// Measurements the last frame took, for reducers that need geometry.
#[derive(Debug, Clone, Copy, Default)]
pub struct Metrics {
    pub transcript_lines: usize,
    pub transcript_height: usize,
    pub terminal_width: u16,
}

impl App {
    pub fn new(status: StatusBar, theme: Theme) -> Self {
        Self {
            entries: Vec::new(),
            composer: Composer::new(),
            status,
            pending: None,
            pending_installs: 0,
            view: View::Transcript,
            overlays: Vec::new(),
            focus: Focus::Composer,
            sessions: Vec::new(),
            top: 0,
            diff_scroll: 0,
            follow: true,
            new_output: false,
            running: false,
            quit: false,
            theme,
            notice: None,
            tool_details: false,
            sidebar_enabled: true,
            sidebar: SidebarModel::default(),
            sidebar_scroll: 0,
            fixture: None,
            suggestion_index: 0,
            suggestions_dismissed: false,
            clipboard: None,
            draft: SessionDraft::default(),
            remembered_effort: None,
            profiles: Vec::new(),
            agents: Vec::new(),
            catalog: ModelCatalogView::default(),
            session_pinned: false,
            mcp: Vec::new(),
            selection: 0,
            busy: std::collections::BTreeMap::new(),
            session_id: None,
            connector: None,
            connector_choice: None,
            connector_model: None,
            connector_catalog: ConnectorCatalogView::default(),
            connector_version: None,
            update_approval: None,
            mcp_target: None,
            mcp_approval: None,
            session_transition: false,
            pricing: None,
            pending_writes: std::collections::BTreeMap::new(),
            observed_writes: Vec::new(),
            stream_open: None,
            revision: 0,
            cache: RefCell::new(LayoutCache::default()),
            metrics: Cell::new(Metrics::default()),
            frames: Cell::new(0),
        }
    }

    /// Frames drawn so far. See the field for why this exists.
    pub fn frames(&self) -> u64 {
        self.frames.get()
    }

    /// Counts a frame. Called once by the renderer per draw.
    pub fn count_frame(&self) {
        self.frames.set(self.frames.get() + 1);
    }

    /// The composition for this frame. The home screen is what an empty
    /// transcript shows, so `/new` returns to it.
    pub fn layout(&self) -> Layout {
        if self.entries.is_empty() {
            Layout::Home
        } else {
            Layout::Conversation
        }
    }

    pub fn is_connected(&self) -> bool {
        !self.status.profile.is_empty()
    }

    /// Where the sidebar goes at this terminal width.
    pub fn sidebar_placement(&self, width: u16) -> SidebarPlacement {
        let drawer = self
            .overlays
            .iter()
            .any(|overlay| matches!(overlay, Overlay::Drawer { .. }));
        sidebar::placement(width, self.sidebar_enabled, drawer)
    }

    /// What the last frame measured. Zero before the first draw.
    pub fn metrics(&self) -> Metrics {
        self.metrics.get()
    }

    /// Records the geometry of the frame being drawn.
    pub fn set_metrics(&self, metrics: Metrics) {
        self.metrics.set(metrics);
    }

    /// True when this terminal is wide enough for the sidebar column, so
    /// `/sidebar` toggles the column rather than opening a drawer.
    fn sidebar_fits_beside(&self) -> bool {
        self.metrics.get().terminal_width >= sidebar::SIDEBAR_MIN_TERMINAL_WIDTH
    }

    /// Whether the sidebar column is on screen, which is what decides
    /// whether focus may rest on it. The home layout is a centred
    /// wordmark and composer and never draws the column, however wide the
    /// terminal is, so width and the user's toggle are not enough.
    pub fn sidebar_column_visible(&self) -> bool {
        self.layout() == Layout::Conversation && self.sidebar_fits_beside() && self.sidebar_enabled
    }

    /// The home layout draws no transcript pane either, so focus has
    /// nowhere to go there but the composer.
    pub fn transcript_is_focusable(&self) -> bool {
        self.layout() == Layout::Conversation
    }

    /// Whether focus may rest on a pane at all.
    fn focus_is_available(&self, focus: Focus) -> bool {
        match focus {
            Focus::Composer => true,
            Focus::Transcript => self.transcript_is_focusable(),
            Focus::Sidebar => self.sidebar_column_visible(),
        }
    }

    /// The terminal changed size. The drawer and the focus are both tied
    /// to a placement that may no longer hold, so both are reconciled
    /// before the next key is read.
    pub fn on_resize(&mut self, width: u16, height: u16) {
        let previous = self.metrics.get();
        self.metrics.set(Metrics {
            terminal_width: width,
            transcript_height: height as usize,
            ..previous
        });
        self.reconcile_layout();
    }

    /// Drops state that the current terminal size cannot support.
    ///
    /// A drawer opened on a narrow terminal is not drawn once the column
    /// fits, so leaving it on the stack would let an invisible overlay
    /// swallow every key. Focus on a hidden sidebar would swallow the
    /// scroll keys the same way.
    pub fn reconcile_layout(&mut self) {
        if self.sidebar_fits_beside() {
            while let Some(position) = self
                .overlays
                .iter()
                .position(|overlay| matches!(overlay, Overlay::Drawer { .. }))
            {
                // The drawer becomes the column: the user asked to see
                // this information and the wide layout has room for it.
                self.close_drawer(position);
                self.sidebar_enabled = true;
            }
        }
        // Focus on a pane that is not drawn would send the scroll keys
        // somewhere invisible, so it returns to the composer.
        if !self.focus_is_available(self.focus) {
            self.focus = Focus::Composer;
        }
    }

    pub fn top_overlay(&self) -> Option<&Overlay> {
        self.overlays.last()
    }

    /// The highlighted `/` suggestion, or `None` when the list is empty.
    /// Indexing is checked: a completion or a backspace can shrink the
    /// list under a highlight that was valid a keystroke ago.
    pub fn highlighted_suggestion(&self) -> Option<&'static command::CommandSpec> {
        let suggestions = self.suggestions();
        suggestions
            .get(self.suggestion_index)
            .or_else(|| suggestions.first())
            .copied()
    }

    /// `/` suggestions, open only when nothing above them is.
    pub fn suggestions(&self) -> Vec<&'static command::CommandSpec> {
        if self.suggestions_dismissed || self.pending.is_some() || !self.overlays.is_empty() {
            return Vec::new();
        }
        if self.focus != Focus::Composer {
            return Vec::new();
        }
        match command::suggestion_query(self.composer.text()) {
            Some(query) => command::search(query),
            None => Vec::new(),
        }
    }

    // -- transcript -----------------------------------------------------

    pub fn push(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.entries.push(Entry::new(kind, text));
        self.stream_open =
            matches!(kind, EntryKind::Assistant | EntryKind::Reasoning).then_some(kind);
        self.touch();
    }

    fn push_entry(&mut self, entry: Entry) {
        self.stream_open =
            matches!(entry.kind, EntryKind::Assistant | EntryKind::Reasoning).then_some(entry.kind);
        self.entries.push(entry);
        self.touch();
    }

    fn touch(&mut self) {
        self.revision += 1;
        // A held viewport keeps its top anchor, so appended output cannot
        // push the lines the reader is looking at off the screen; it only
        // raises the indicator that says there is more below.
        self.new_output = !self.follow;
    }

    /// Replays stored events into the transcript on resume.
    pub fn load_history(&mut self, events: &[Event]) {
        for event in events {
            self.on_event(event);
        }
        self.stream_open = None;
        self.running = false;
        self.follow = true;
        self.top = 0;
        self.new_output = false;
    }

    pub fn on_event(&mut self, event: &Event) {
        match &event.kind {
            EventKind::TextDelta { text } | EventKind::ReasoningSummary { text } => {
                let kind = if matches!(event.kind, EventKind::ReasoningSummary { .. }) {
                    EntryKind::Reasoning
                } else {
                    EntryKind::Assistant
                };
                if self.stream_open == Some(kind) {
                    if let Some(last) = self.entries.last_mut() {
                        last.text.push_str(&sanitize(text));
                    }
                    self.touch();
                } else {
                    self.push(kind, text.clone());
                }
            }
            EventKind::ToolCall { call } => {
                // A write is remembered, not reported: only its result
                // proves the file changed, and the plan forbids claiming
                // a change Gritt did not observe succeeding.
                if call.name == native::FILE_WRITE {
                    if let Some(path) = call.arguments.get("path").and_then(|p| p.as_str()) {
                        self.pending_writes
                            .insert(call.id.0.clone(), path.to_owned());
                    }
                }
                self.push(
                    EntryKind::Tool,
                    format!("-> {}", describe_call(&call.name, &call.arguments)),
                );
            }
            EventKind::ToolResult { result } => {
                if let Some(path) = self.pending_writes.remove(&result.call_id.0) {
                    if !result.is_error {
                        self.observed_writes.push(path);
                    }
                }
                let first = result.output.lines().next().unwrap_or_default();
                let mut entry = Entry::new(
                    EntryKind::Tool,
                    format!(
                        "<- {} {} {}",
                        result.name,
                        if result.is_error { "error:" } else { "ok" },
                        if result.is_error { first } else { "" }
                    )
                    .trim_end()
                    .to_owned(),
                );
                if !result.output.is_empty() {
                    entry.detail = Some(sanitize(&result.output));
                }
                self.push_entry(entry);
            }
            EventKind::ApprovalRequested { request } => {
                self.push(
                    EntryKind::System,
                    format!(
                        "approval requested: {} on {}",
                        request.tool, request.resource
                    ),
                );
            }
            EventKind::ApprovalDecided { decision, .. } => {
                self.push(EntryKind::System, format!("{decision:?}"));
            }
            EventKind::Usage { usage } => {
                // A count the provider did not report is unknown, not
                // zero. Adding `unwrap_or(0)` would turn an unreported
                // total into a reported one and let a cost estimate be
                // computed from it.
                let total = &mut self.status.usage;
                if let Some(input) = usage.input_tokens {
                    total.input_tokens = Some(total.input_tokens.unwrap_or(0) + input);
                }
                if let Some(output) = usage.output_tokens {
                    total.output_tokens = Some(total.output_tokens.unwrap_or(0) + output);
                }
                if usage.input_tokens.is_none() || usage.output_tokens.is_none() {
                    // The totals can only be a floor from here on, so the
                    // estimate is withheld and the sidebar says why.
                    self.sidebar.usage.incomplete = true;
                }
                self.sidebar.usage.input_tokens = total.input_tokens;
                self.sidebar.usage.output_tokens = total.output_tokens;
                // The prompt tokens of one request are a fact about that
                // request. They are a lower bound on the context, not its
                // size, so they get their own label and never feed
                // occupancy.
                if let Some(prompt) = usage.input_tokens {
                    self.sidebar.usage.last_request_input = Some(prompt);
                }
                self.recompute_cost();
            }
            EventKind::StatusChanged { status } => {
                self.status.connection = format!("{status:?}");
                self.sidebar.session.activity = Some(format!("{status:?}").to_lowercase());
                if let Some(phase) = event
                    .diagnostic
                    .as_ref()
                    .and_then(|d| d.get("phase"))
                    .and_then(|p| p.as_str())
                {
                    self.status.phase = phase.to_owned();
                    self.sidebar.session.phase = Some(phase.to_owned());
                }
                if matches!(
                    status,
                    SessionStatus::Finished | SessionStatus::Failed | SessionStatus::Idle
                ) {
                    self.stream_open = None;
                }
            }
            EventKind::Error { message, .. } => self.push(EntryKind::Error, message.clone()),
            EventKind::Completed { .. } => self.stream_open = None,
            EventKind::Cancelled => self.push(EntryKind::System, "cancelled"),
        }
        if let Some(warning) = event
            .diagnostic
            .as_ref()
            .and_then(|d| d.get("capability_warning"))
        {
            self.push(
                EntryKind::System,
                format!(
                    "warning: provider did not report support for {}",
                    warning
                        .get("features")
                        .map(|f| f.to_string())
                        .unwrap_or_default()
                ),
            );
        }
    }

    /// Wrapped transcript lines for `width`, rebuilt only when the width
    /// or the transcript changed. Long sessions do not re-wrap on every
    /// frame.
    pub fn transcript_lines(
        &self,
        width: usize,
        render: impl Fn(&App, usize) -> Vec<Line<'static>>,
    ) -> Vec<Line<'static>> {
        {
            let cache = self.cache.borrow();
            if cache.width == width && cache.revision == self.revision && !cache.lines.is_empty() {
                return cache.lines.clone();
            }
        }
        let lines = render(self, width);
        let mut cache = self.cache.borrow_mut();
        cache.width = width;
        cache.revision = self.revision;
        cache.lines = lines.clone();
        lines
    }

    /// True when the cached wrap is reusable, for the cache test.
    pub fn layout_cache_hit(&self, width: usize) -> bool {
        let cache = self.cache.borrow();
        cache.width == width && cache.revision == self.revision && !cache.lines.is_empty()
    }

    pub fn request_approval(&mut self, pending: PendingApproval) {
        self.diff_scroll = 0;
        self.mcp_approval = None;
        self.install_pending(pending);
    }

    /// Puts a request on screen and stamps it with a fresh identity.
    fn install_pending(&mut self, pending: PendingApproval) {
        self.pending = Some(pending);
        self.pending_installs = self.pending_installs.wrapping_add(1);
    }

    /// The identity of the approval currently on screen, if any.
    ///
    /// A caller that has drawn this value has drawn *this* request. The
    /// next install changes it, so nothing carries over.
    pub fn pending_install(&self) -> Option<u64> {
        self.pending.as_ref().map(|_| self.pending_installs)
    }

    /// Puts a first-use MCP launch in front of the user, with the
    /// redacted definition the harness produced.
    ///
    /// The same modal overlay a tool approval uses: reading a workspace
    /// file does not authorize running what it names, so the executable
    /// and its arguments are shown before anything starts.
    /// Puts an update command in front of the user through the same modal
    /// overlay a tool call uses. The vector shown is the vector that runs.
    pub fn request_update_approval(
        &mut self,
        connector: ConnectorId,
        action: UpdateAction,
        check: &ConnectorVersionCheck,
    ) {
        self.diff_scroll = 0;
        let preview = ControlPlane::connector_version_lines(check).join("\n");
        self.install_pending(PendingApproval {
            request: ApprovalRequest {
                id: gritt_core::event::ApprovalId(format!(
                    "connector-update:{}",
                    connector.as_str()
                )),
                tool: "connector_update".into(),
                resource: action.display(),
                reason: format!(
                    "updates {} through {}; runs exactly this command",
                    connector.as_str(),
                    action.source.label()
                ),
                call_id: None,
            },
            decision: Decision {
                outcome: gritt_core::policy::PolicyOutcome::Ask,
                reason: "an update changes the installed agent CLI".into(),
                destructive: false,
                rule: None,
            },
            preview: Some(preview),
        });
        self.update_approval = Some((connector, action));
    }

    /// Records a version check for the live connector session. A result
    /// for a connector the user has already left is dropped.
    pub fn apply_connector_version(
        &mut self,
        connector: ConnectorId,
        check: ConnectorVersionCheck,
    ) -> bool {
        if self.connector != Some(connector) {
            return false;
        }
        self.sidebar.model.version = Some(version_summary(&check));
        self.notice = Some(check.describe());
        self.connector_version = Some(check);
        true
    }

    /// Records an update's outcome and the version check that followed a
    /// successful one.
    pub fn apply_connector_update(
        &mut self,
        connector: ConnectorId,
        outcome: ConnectorUpdateOutcome,
    ) -> bool {
        if self.connector != Some(connector) {
            return false;
        }
        match outcome {
            ConnectorUpdateOutcome::Updated { recheck, .. } => {
                let text = recheck.describe();
                self.apply_connector_version(connector, *recheck);
                self.notice = Some(format!("{} updated; {text}", connector.as_str()));
            }
            other => self.notice = Some(other.describe()),
        }
        true
    }

    pub fn request_mcp_approval(&mut self, server: String, definition: String) {
        self.diff_scroll = 0;
        self.install_pending(PendingApproval {
            request: ApprovalRequest {
                id: gritt_core::event::ApprovalId(format!("mcp-launch:{server}")),
                tool: "mcp_server_launch".into(),
                resource: format!("mcp:{server}"),
                reason: "a workspace file names this server; running it needs approval".into(),
                call_id: None,
            },
            decision: Decision {
                outcome: gritt_core::policy::PolicyOutcome::Ask,
                reason: "first use of this exact server definition".into(),
                destructive: false,
                rule: None,
            },
            preview: Some(definition),
        });
        self.mcp_approval = Some(server);
    }

    // -- picker construction -------------------------------------------

    /// The grouped connection dialog: configured provider profiles and
    /// installed agents, each row saying which it is.
    pub fn connection_picker(&self) -> Picker {
        let mut rows = Vec::new();
        for profile in &self.profiles {
            let credential = match &profile.credential {
                CredentialState::Available => "key available".to_owned(),
                CredentialState::Missing { env_var_name } => {
                    format!("no key; set {env_var_name} or run setup")
                }
            };
            let catalog = match &self.catalog.state {
                Some(state) if self.catalog.profile == profile.name => catalog_word(state),
                _ => "catalog not loaded",
            };
            rows.push(
                PickerRow::new(format!("profile:{}", profile.name), profile.name.clone())
                    .group("AI providers")
                    .detail(profile.base_url.clone())
                    .badge(protocol_word(profile.protocol).to_owned())
                    .note(format!("{credential} · {catalog}"))
                    .current(self.draft.profile.as_deref() == Some(profile.name.as_str())),
            );
        }
        for agent in &self.agents {
            let auth = match agent.authenticated {
                Some(true) => "signed in",
                Some(false) => "not signed in",
                None => "auth state not reported",
            };
            let mut row = PickerRow::new(format!("agent:{}", agent.name), agent.name.clone())
                .group("Installed agents")
                .detail(
                    agent
                        .version
                        .clone()
                        .unwrap_or_else(|| "version unknown".into()),
                )
                .badge(if agent.installed {
                    "installed"
                } else {
                    "not installed"
                })
                // ADR-010: the agent keeps its own model, effort, and
                // permissions. Gritt does not offer to change them.
                .note(format!("{auth} · Managed by agent"));
            if !agent.installed {
                row = row.unavailable("install this agent to select it");
            }
            rows.push(row);
        }
        // A supported provider with no profile yet is offered as setup,
        // so `/connect` works with nothing configured at all.
        for preset in PRESETS.iter() {
            if self.profiles.iter().any(|p| p.name == preset.name) {
                continue;
            }
            rows.push(
                PickerRow::new(
                    format!("preset:{}", preset.name),
                    format!("Set up {}…", preset.name),
                )
                .group("Add a provider")
                .detail(preset.base_url.to_owned())
                .badge(protocol_word(preset.protocol).to_owned())
                .note("not configured yet"),
            );
        }
        rows.push(
            PickerRow::new("custom", "Custom endpoint…")
                .group("Add a provider")
                .detail("any OpenAI-compatible, Responses, or Messages endpoint")
                .badge("custom"),
        );
        Picker::new("Connect", rows)
            .with_hint("Selecting does not install software or start a sign-in")
    }

    /// The model picker for the drafted profile. A profile with no key
    /// gets a setup row, which is the `/models` to provider-setup round
    /// trip. A drafted connector uses the same picker with that agent's
    /// catalog.
    pub fn model_picker(&self) -> Picker {
        if let Some(id) = self.connector_choice {
            return self.connector_model_picker(id);
        }
        let mut rows = Vec::new();
        let profile = self.draft.profile.clone().unwrap_or_default();
        if let Some(summary) = self.profiles.iter().find(|p| p.name == profile) {
            if let CredentialState::Missing { env_var_name } = &summary.credential {
                rows.push(
                    PickerRow::new("__setup__", format!("Set up {profile}…"))
                        .detail(format!("no key found; {env_var_name} is unset"))
                        .badge("setup"),
                );
            }
        }
        for model in &self.catalog.models {
            let label = model
                .display_name
                .clone()
                .unwrap_or_else(|| model.id.clone());
            let mut row = PickerRow::new(model.id.clone(), label)
                .detail(model.id.clone())
                .badge(profile.clone())
                .current(self.draft.model.as_deref() == Some(model.id.as_str()));
            if model.deprecated {
                row = row.note(match &model.replaced_by {
                    Some(replacement) => format!("deprecated; replaced by {replacement}"),
                    None => "deprecated".into(),
                });
            }
            rows.push(row);
        }
        let status = if self.catalog.loading {
            ListStatus::Loading {
                what: profile.clone(),
            }
        } else {
            match &self.catalog.state {
                Some(CatalogState::Stale { .. }) => ListStatus::Failed {
                    reason: "the refresh failed; showing the last cached list".into(),
                    cached: true,
                },
                Some(CatalogState::Missing { reason }) => ListStatus::Failed {
                    reason: reason.clone(),
                    cached: false,
                },
                Some(CatalogState::RefreshFailed { reason }) => ListStatus::Failed {
                    reason: reason.clone(),
                    cached: false,
                },
                _ => ListStatus::Ready,
            }
        };
        Picker::new(format!("Models · {profile}"), rows)
            .with_hint(match &self.catalog.state {
                Some(state) => catalog_word(state).to_owned(),
                None => "catalog not loaded".to_owned(),
            })
            .with_status(status)
    }

    fn connector_model_picker(&self, id: ConnectorId) -> Picker {
        let mut rows = vec![PickerRow::new("__default__", "Agent default")
            .detail("the CLI chooses its own model")
            .badge(id.as_str().to_owned())
            .current(self.connector_model.is_none())];
        for model in &self.connector_catalog.models {
            let label = model
                .display_label
                .clone()
                .unwrap_or_else(|| model.id.clone());
            rows.push(
                PickerRow::new(model.id.clone(), label)
                    .detail(model.id.clone())
                    .badge(id.as_str().to_owned())
                    .current(self.connector_model.as_deref() == Some(model.id.as_str())),
            );
        }
        let status = if self.connector_catalog.loading {
            ListStatus::Loading {
                what: format!("{} models", id.as_str()),
            }
        } else {
            match &self.connector_catalog.discovery {
                Some(ConnectorModelDiscovery::CachedStale { reason, .. }) => ListStatus::Failed {
                    reason: reason.clone(),
                    cached: true,
                },
                Some(ConnectorModelDiscovery::Unavailable { reason, .. })
                | Some(ConnectorModelDiscovery::Unsupported { reason, .. })
                | Some(ConnectorModelDiscovery::CommandFailure { reason, .. })
                | Some(ConnectorModelDiscovery::MalformedOutput { reason, .. }) => {
                    ListStatus::Failed {
                        reason: reason.clone(),
                        cached: false,
                    }
                }
                _ => ListStatus::Ready,
            }
        };
        let hint = match &self.connector_catalog.discovery {
            Some(outcome) => outcome.describe(),
            None => format!("{} catalog not loaded", id.as_str()),
        };
        Picker::new(format!("Models · {}", id.as_str()), rows)
            .with_hint(hint)
            .with_status(status)
    }

    /// The effort picker: `Provider default` plus only the levels the
    /// adapter has a verified mapping for on this model.
    pub fn effort_picker(&self) -> Picker {
        let protocol = self
            .draft
            .profile
            .as_deref()
            .and_then(|name| self.profiles.iter().find(|p| p.name == name))
            .map(|profile| profile.protocol);
        let capabilities = self
            .draft
            .model
            .as_deref()
            .and_then(|id| self.catalog.models.iter().find(|model| model.id == id))
            .map(|model| &model.capabilities);
        let selected = self
            .draft
            .effort
            .or(self.remembered_effort)
            .unwrap_or_default();
        let mut rows = vec![PickerRow::new("auto", "Provider default")
            .detail("no explicit effort is sent")
            .badge("default")
            .current(selected == ReasoningEffort::Auto)];
        for level in ReasoningEffort::EXPLICIT {
            let mut row = PickerRow::new(level.as_str(), level.as_str())
                .badge(level.as_str().to_owned())
                .current(selected == level);
            match protocol {
                Some(protocol) => {
                    // One rule for the adapter and the picker: the
                    // provider crate decides, the TUI only displays.
                    if let gritt_provider::effort::EffortSupport::Unsupported(reason) =
                        gritt_provider::effort::effort_support(protocol, capabilities, level)
                    {
                        row = row.unavailable(format!(
                            "this model does not support {}",
                            reason.describe()
                        ));
                    }
                }
                None => row = row.unavailable("choose a provider first"),
            }
            rows.push(row);
        }
        Picker::new("Effort", rows)
            .with_hint("Effort applies to native turns and can change between them")
    }

    pub fn session_picker(&self) -> Picker {
        let rows: Vec<PickerRow> = self
            .sessions
            .iter()
            .map(|session| {
                PickerRow::new(session.id.0.clone(), session.name.clone())
                    .detail(session.updated_at.format("%Y-%m-%d %H:%M").to_string())
                    .badge(match session.phase {
                        Phase::Planning => "planning".into(),
                        Phase::Coding => "coding".to_owned(),
                    })
                    .current(session.name == self.status.session)
            })
            .collect();
        Picker::new("Sessions", rows).with_hint("Enter resumes; the draft is kept")
    }

    pub fn mcp_picker(&self) -> Picker {
        let rows: Vec<PickerRow> = self
            .mcp
            .iter()
            .map(|server| {
                let word = sidebar::mcp_state_word(&server.state);
                // Every row stays selectable, including a failed or
                // unapproved one: approving and restarting are exactly
                // what a user opens `/mcp` for.
                PickerRow::new(server.name.clone(), server.name.clone())
                    .badge(word.to_owned())
                    .detail(if server.state.is_ready() {
                        format!("{} tools", server.tool_count)
                    } else {
                        String::new()
                    })
                    .note(server.state.explain())
            })
            .collect();
        Picker::new("MCP servers", rows)
            .with_hint("Enter opens the actions for a server; every configured entry is listed")
    }

    /// The actions available on one MCP server. Which ones apply depends
    /// on its state, and an inapplicable one says why rather than being
    /// hidden.
    pub fn mcp_actions_picker(&self, server: &str) -> Picker {
        let snapshot = self.mcp.iter().find(|entry| entry.name == server);
        let state = snapshot.map(|entry| entry.state.clone());
        let approved = !matches!(
            state,
            Some(McpServerState::AwaitingApproval) | Some(McpServerState::Denied) | None
        );
        let configured = !matches!(
            state,
            Some(McpServerState::Invalid { .. })
                | Some(McpServerState::UnsupportedTransport { .. })
                | None
        );
        let mut rows = Vec::new();
        let mut row = PickerRow::new("approve", "Approve this server")
            .detail("launch it now and remember this exact definition")
            .badge("trust");
        if !configured {
            row = row.unavailable("this entry cannot run as configured");
        } else if approved {
            row = row.unavailable("already approved");
        }
        rows.push(row);
        let mut row = PickerRow::new("deny", "Deny this server")
            .detail("stop it now and refuse it until the decision is forgotten")
            .badge("trust");
        if !configured {
            row = row.unavailable("this entry cannot run as configured");
        }
        rows.push(row);
        let mut row = PickerRow::new("restart", "Restart")
            .detail("close the connection and connect again")
            .badge("lifecycle");
        if !approved || !configured {
            row = row.unavailable("approve it first");
        }
        rows.push(row);
        let mut row = PickerRow::new("stop", "Stop")
            .detail("end the connection; the entry stays listed as stopped")
            .badge("lifecycle");
        if !approved || !configured {
            row = row.unavailable("this server is not running");
        }
        rows.push(row);
        rows.push(
            PickerRow::new("reload", "Reload .mcp.json")
                .detail("validate the file and apply it to every server")
                .badge("lifecycle"),
        );
        Picker::new(format!("MCP · {server}"), rows).with_hint(match &state {
            Some(state) => state.explain().to_string(),
            None => "this server is no longer configured".to_owned(),
        })
    }

    /// The changed files the sidebar lists, as a searchable list. Enter
    /// opens a read-only diff.
    pub fn changes_picker(&self) -> Picker {
        let rows: Vec<PickerRow> = self
            .sidebar
            .changed_files
            .files()
            .iter()
            .map(|file| {
                PickerRow::new(file.path.clone(), file.path.clone())
                    .badge(file.status.label().to_owned())
                    .note(if file.pre_existing {
                        "already changed when this session opened".to_owned()
                    } else {
                        String::new()
                    })
            })
            .collect();
        let hint = match &self.sidebar.changed_files {
            ChangedFiles::Unavailable { reason } => reason.clone(),
            ChangedFiles::Observed { source, .. } => source
                .caveat()
                .unwrap_or("read-only: opening a file shows its diff")
                .to_owned(),
        };
        Picker::new("Changed files", rows).with_hint(hint)
    }

    fn command_picker(&self) -> Picker {
        let rows: Vec<PickerRow> = command::COMMANDS
            .iter()
            .map(|spec| {
                PickerRow::new(spec.name, format!("/{}", spec.name))
                    .detail(spec.summary.to_owned())
                    .badge(spec.shortcut.unwrap_or_default().to_owned())
            })
            .collect();
        Picker::new("Commands", rows).with_hint("The same registry as / and the shortcuts")
    }

    fn mode_picker(&self) -> Picker {
        use gritt_core::session::ExecutionMode;
        Picker::new(
            "Execution mode",
            ExecutionMode::ALL
                .into_iter()
                .map(|mode| {
                    PickerRow::new(mode.as_str(), mode.label())
                        .detail(match mode {
                            ExecutionMode::Planning => "Read files only. No writes, shell, or MCP.",
                            ExecutionMode::Supervised => {
                                "Use configured policy and approval prompts."
                            }
                            ExecutionMode::AutoApprove => {
                                "Approve prompts. Keep denials and file limits."
                            }
                            ExecutionMode::FullAccess => {
                                "Files anywhere; commands without policy checks."
                            }
                        })
                        .current(
                            self.status
                                .phase
                                .parse::<ExecutionMode>()
                                .unwrap_or_default()
                                == mode,
                        )
                })
                .collect(),
        )
        .with_hint("Between turns. Resume resets elevated access.")
    }

    pub fn set_effective_mode(&mut self, mode: Option<gritt_core::session::ExecutionMode>) {
        self.draft.mode = mode;
        if let Some(mode) = mode {
            self.draft.phase = Some(mode.phase());
            self.status.phase = mode.as_str().into();
            self.sidebar.session.phase = Some(mode.label().into());
        }
    }

    // -- command dispatch ----------------------------------------------

    fn open_picker(&mut self, kind: PickerKind) {
        let picker = match kind {
            PickerKind::Mode => self.mode_picker(),
            PickerKind::Commands => self.command_picker(),
            PickerKind::Connect => self.connection_picker(),
            PickerKind::Models => self.model_picker(),
            PickerKind::Effort => self.effort_picker(),
            PickerKind::Sessions => self.session_picker(),
            PickerKind::Mcp => self.mcp_picker(),
            PickerKind::McpActions => {
                self.mcp_actions_picker(self.mcp_target.clone().unwrap_or_default().as_str())
            }
            PickerKind::Changes => self.changes_picker(),
        };
        self.overlays.push(Overlay::Picker { kind, picker });
    }

    /// The next pane in the cycle that is actually on screen. On home
    /// that is the composer every time, because nothing else is drawn.
    fn next_focus(&self, forward: bool) -> Focus {
        const ORDER: [Focus; 3] = [Focus::Composer, Focus::Transcript, Focus::Sidebar];
        let at = ORDER
            .iter()
            .position(|pane| *pane == self.focus)
            .unwrap_or(0);
        for step in 1..=ORDER.len() {
            let next = if forward {
                (at + step) % ORDER.len()
            } else {
                (at + ORDER.len() - step) % ORDER.len()
            };
            if self.focus_is_available(ORDER[next]) {
                return ORDER[next];
            }
        }
        Focus::Composer
    }

    /// Runs a registry command. Every entry point — `/` submission, the
    /// palette, and a shortcut — comes through here.
    pub fn dispatch(&mut self, cmd: Command, argument: Option<String>) -> Action {
        self.notice = None;
        self.suggestions_dismissed = false;
        // An approval or a running turn owns the session: the plan keeps
        // both modal, so a settings command explains the refusal instead
        // of changing a draft the runtime would reject.
        if changes_settings(cmd) && !self.settings_are_editable() {
            self.notice = Some(if self.pending.is_some() {
                "answer the approval first; settings cannot change during it".into()
            } else {
                "a turn is running; Esc cancels it before settings change".to_owned()
            });
            return Action::None;
        }
        // ADR-010: an external agent owns its model, effort, and
        // permissions. The native pickers do not apply to its session and
        // say so rather than pretending to change anything.
        if let (Some(id), true) = (self.connector, is_native_setting(cmd)) {
            self.notice = Some(format!(
                "this session runs on {}; its model, effort, and permissions are managed by the agent",
                id.as_str()
            ));
            return Action::None;
        }
        match cmd {
            Command::Mode => {
                if let Some(argument) = argument {
                    return match argument.parse() {
                        Ok(mode) => Action::SetMode(mode),
                        Err(reason) => {
                            self.notice = Some(reason);
                            Action::None
                        }
                    };
                }
                self.open_picker(PickerKind::Mode);
                Action::None
            }
            Command::Connect => {
                self.open_picker(PickerKind::Connect);
                Action::None
            }
            Command::Version => {
                let Some(id) = self.connector else {
                    self.notice = Some(
                        "native sessions have no agent CLI to check; /connect to an agent first"
                            .into(),
                    );
                    return Action::None;
                };
                Action::LoadConnectorVersion {
                    connector: id,
                    mode: VersionCheckMode::Refresh,
                }
            }
            Command::Update => {
                let Some(id) = self.connector else {
                    self.notice = Some(
                        "native sessions have no agent CLI to update; /connect to an agent first"
                            .into(),
                    );
                    return Action::None;
                };
                let Some(check) = self.connector_version.clone() else {
                    self.notice = Some(format!("checking {} first", id.as_str()));
                    return Action::LoadConnectorVersion {
                        connector: id,
                        mode: VersionCheckMode::Cached,
                    };
                };
                match check.status().and_then(|status| status.update.clone()) {
                    Some(action) => {
                        self.request_update_approval(id, action, &check);
                        Action::None
                    }
                    None => {
                        let why = check
                            .status()
                            .and_then(|status| status.next_step.clone())
                            .unwrap_or_else(|| check.describe());
                        self.notice = Some(format!("no update to run: {why}"));
                        Action::None
                    }
                }
            }
            Command::Models => {
                if let Some(id) = self.connector_choice {
                    self.open_picker(PickerKind::Models);
                    return self.request_connector_catalog(id, false);
                }
                if self.draft.profile.is_none() {
                    self.notice = Some("choose a provider with /connect first".into());
                    self.open_picker(PickerKind::Connect);
                    return Action::None;
                }
                self.open_picker(PickerKind::Models);
                self.request_catalog()
            }
            Command::Effort => {
                self.open_picker(PickerKind::Effort);
                Action::None
            }
            // The displayed phase is not changed here. The runtime applies
            // the change and calls `set_session`, which is the only place
            // the shown phase moves; a refused change shows nothing.
            Command::Plan => Action::SetPhase(Phase::Planning),
            Command::Code => Action::SetPhase(Phase::Coding),
            Command::Sessions => {
                self.view = View::Sessions;
                self.open_picker(PickerKind::Sessions);
                if let Some(name) = argument {
                    if let Some(Overlay::Picker { picker, .. }) = self.overlays.last_mut() {
                        for c in name.chars() {
                            picker.type_char(c);
                        }
                    }
                }
                Action::RefreshSessions
            }
            Command::New => {
                // A fresh draft, not a deleted session: the transcript
                // view is cleared and the composer draft is kept. The
                // runtime releases the previous driver on the action.
                self.start_new_draft();
                self.notice = Some("new draft; the previous session is still listed".into());
                Action::NewSession
            }
            Command::Details => {
                self.tool_details = !self.tool_details;
                self.revision += 1;
                Action::None
            }
            Command::Sidebar => {
                self.toggle_sidebar();
                Action::None
            }
            Command::Mcp => {
                self.mcp_target = None;
                self.open_picker(PickerKind::Mcp);
                Action::RefreshMcp
            }
            Command::Help => {
                self.overlays.push(Overlay::Help { scroll: 0 });
                Action::None
            }
            Command::Quit => {
                self.quit = true;
                Action::Quit
            }
        }
    }

    /// Records that work of this kind has started.
    pub fn begin_work(&mut self, kind: Work, label: impl Into<String>) {
        self.busy.insert(kind, label.into());
    }

    /// Records that work of this kind has ended. Ending a kind that was
    /// not running is not an error: a cancelled operation and its own
    /// completion can both arrive.
    pub fn end_work(&mut self, kind: Work) {
        self.busy.remove(&kind);
    }

    /// The label to show near the composer, highest priority first.
    pub fn loading(&self) -> Option<&str> {
        self.busy.values().next().map(String::as_str)
    }

    /// Whether anything is in flight. This is what Escape acts on, so it
    /// cannot be cleared by an unrelated kind finishing.
    pub fn is_busy(&self) -> bool {
        !self.busy.is_empty()
    }

    /// Whether work of this kind is in flight.
    pub fn is_working_on(&self, kind: Work) -> bool {
        self.busy.contains_key(&kind)
    }

    /// Whether settings may change right now.
    ///
    /// A session transition counts: the choices would be applied against a
    /// driver that is about to be replaced.
    pub fn settings_are_editable(&self) -> bool {
        !self.running && self.pending.is_none() && !self.session_transition
    }

    /// Takes a freshly loaded session list. When the session picker is
    /// already open, its rows are rebuilt in place so the list the user is
    /// looking at fills in, keeping the query typed so far and the row
    /// highlighted before the load landed.
    pub fn load_sessions(&mut self, sessions: Vec<Session>) {
        self.sessions = sessions;
        let rows = self.session_picker().rows().to_vec();
        for overlay in self.overlays.iter_mut() {
            if let Overlay::Picker {
                kind: PickerKind::Sessions,
                picker,
            } = overlay
            {
                picker.replace_rows(rows.clone());
            }
        }
    }

    /// `/sidebar`. At 110 columns or more the column is simply shown or
    /// hidden; below that the same information opens as a drawer, which
    /// restores the focus and scroll it covered when it closes.
    fn toggle_sidebar(&mut self) {
        if let Some(position) = self
            .overlays
            .iter()
            .position(|overlay| matches!(overlay, Overlay::Drawer { .. }))
        {
            self.close_drawer(position);
            return;
        }
        if self.sidebar_fits_beside() {
            self.sidebar_enabled = !self.sidebar_enabled;
            self.sidebar_scroll = 0;
            if !self.sidebar_enabled && self.focus == Focus::Sidebar {
                self.focus = Focus::Composer;
            }
            return;
        }
        self.overlays.push(Overlay::Drawer {
            scroll: self.sidebar_scroll,
            restore_focus: self.focus,
            restore_scroll: self.top,
        });
    }

    fn close_drawer(&mut self, position: usize) {
        if let Overlay::Drawer {
            restore_focus,
            restore_scroll,
            ..
        } = self.overlays.remove(position)
        {
            self.focus = restore_focus;
            self.top = restore_scroll;
        }
    }

    // -- the reducer ----------------------------------------------------

    /// Overlay priority, top first: an approval, then the overlay stack,
    /// then `/` suggestions, then the focused pane.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // A resize may have invalidated the drawer or the focused pane
        // since the last key; neither may capture this one.
        self.reconcile_layout();
        if self.pending.is_some() {
            return self.approval_key(key);
        }
        if !self.overlays.is_empty() {
            return self.overlay_key(key);
        }
        // Terminals report Shift+Tab as BackTab or as Tab with SHIFT.
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && (key.code == KeyCode::BackTab
                || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT)))
        {
            use gritt_core::session::ExecutionMode;
            let next = match self
                .status
                .phase
                .parse::<ExecutionMode>()
                .unwrap_or_default()
            {
                ExecutionMode::Planning => ExecutionMode::Supervised,
                ExecutionMode::Supervised => ExecutionMode::AutoApprove,
                ExecutionMode::AutoApprove => ExecutionMode::FullAccess,
                ExecutionMode::FullAccess => ExecutionMode::Planning,
            };
            return self.dispatch(Command::Mode, Some(next.as_str().into()));
        }
        if !self.suggestions().is_empty() {
            if let Some(action) = self.suggestion_key(key) {
                return action;
            }
        }
        self.main_key(key)
    }

    /// Bracketed paste from the runtime. Pasted text is always text.
    pub fn on_paste(&mut self, pasted: &str) {
        if self.pending.is_some() {
            return;
        }
        match self.overlays.last_mut() {
            Some(Overlay::Picker { picker, .. }) => picker.paste(pasted),
            Some(Overlay::Setup(form)) => {
                let first = pasted.lines().next().unwrap_or_default().to_owned();
                form.current().insert_paste(&first);
            }
            Some(_) => {}
            None => {
                self.composer.insert_paste(pasted);
                self.suggestion_index = 0;
                // A pasted line beginning with `/` must not open the
                // suggestion list and must not run on submit.
                self.suggestions_dismissed = self.composer.is_multiline();
            }
        }
    }

    /// Answers the pending approval. An MCP launch decision is recorded
    /// as trust; anything else answers the tool call the agent is waiting
    /// on. Both leave through this one path, so a late key cannot answer
    /// an approval that has already been taken away.
    fn answer_approval(&mut self, approved: bool) -> Action {
        self.pending = None;
        self.view = View::Transcript;
        if let Some((connector, action)) = self.update_approval.take() {
            if approved {
                return Action::RunConnectorUpdate { connector, action };
            }
            self.notice = Some(format!(
                "{} update declined; nothing was run",
                connector.as_str()
            ));
            return Action::None;
        }
        if let Some(server) = self.mcp_approval.take() {
            return Action::Mcp(McpRequest::Decide {
                server,
                decision: if approved {
                    TrustDecision::Approved
                } else {
                    TrustDecision::Denied
                },
            });
        }
        Action::Approve(if approved {
            ApprovalDecision::Approved
        } else {
            ApprovalDecision::Denied
        })
    }

    fn approval_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.answer_approval(true),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.answer_approval(false),
            KeyCode::Char('d') => {
                self.view = if self.view == View::Diff {
                    View::Transcript
                } else {
                    View::Diff
                };
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.diff_scroll = self.diff_scroll.saturating_add(1);
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.diff_scroll = self.diff_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::PageDown => {
                self.diff_scroll = self.diff_scroll.saturating_add(10);
                Action::None
            }
            KeyCode::PageUp => {
                self.diff_scroll = self.diff_scroll.saturating_sub(10);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn suggestion_key(&mut self, key: KeyEvent) -> Option<Action> {
        let count = self.suggestions().len();
        if count == 0 {
            return None;
        }
        // The highlight can only be stale, never out of range: it is
        // clamped here and every edit below resets it.
        self.suggestion_index = self.suggestion_index.min(count - 1);
        match key.code {
            KeyCode::Down => {
                self.suggestion_index = (self.suggestion_index + 1) % count;
                Some(Action::None)
            }
            KeyCode::Up => {
                self.suggestion_index = (self.suggestion_index + count - 1) % count;
                Some(Action::None)
            }
            KeyCode::Esc => {
                self.suggestions_dismissed = true;
                Some(Action::None)
            }
            KeyCode::Tab => {
                // Tab completes the highlighted suggestion instead of
                // moving focus while the list is open. Completing narrows
                // the list, so the highlight returns to its first row.
                let name = self.highlighted_suggestion()?.name;
                self.composer.set_text(format!("/{name}"));
                self.suggestion_index = 0;
                Some(Action::None)
            }
            KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                let command = self.highlighted_suggestion()?.command;
                self.composer.clear();
                self.suggestion_index = 0;
                Some(self.dispatch(command, None))
            }
            _ => None,
        }
    }

    fn overlay_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            let position = self.overlays.len() - 1;
            if matches!(self.overlays[position], Overlay::Drawer { .. }) {
                self.close_drawer(position);
            } else {
                self.overlays.pop();
            }
            return Action::None;
        }
        match self.overlays.last_mut() {
            Some(Overlay::Help { scroll }) => {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    KeyCode::Enter => {
                        self.overlays.pop();
                    }
                    _ => {}
                }
                Action::None
            }
            Some(Overlay::FileDiff { scroll, .. }) => {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    KeyCode::PageDown => *scroll += 10,
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                    _ => {}
                }
                Action::None
            }
            Some(Overlay::Drawer { scroll, .. }) => {
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                    _ => {}
                }
                self.sidebar_scroll = match self.overlays.last() {
                    Some(Overlay::Drawer { scroll, .. }) => *scroll,
                    _ => 0,
                };
                Action::None
            }
            Some(Overlay::Notice(notice)) => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
                    let confirm = notice.confirm;
                    self.overlays.pop();
                    // Only a detail view carries a confirmation, so an
                    // ordinary explanation still just closes.
                    if let Some(id) = confirm {
                        return Action::SelectConnector(id);
                    }
                }
                Action::None
            }
            Some(Overlay::Setup(_)) => self.setup_key(key),
            Some(Overlay::Picker { .. }) => self.picker_key(key, ctrl),
            None => Action::None,
        }
    }

    fn setup_key(&mut self, key: KeyEvent) -> Action {
        let Some(Overlay::Setup(form)) = self.overlays.last_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Tab | KeyCode::Down => form.next_field(),
            KeyCode::BackTab | KeyCode::Up => form.previous_field(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.current().insert_char(c)
            }
            KeyCode::Backspace => form.current().backspace(),
            KeyCode::Left => form.current().move_left(false),
            KeyCode::Right => form.current().move_right(false),
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.cycle_protocol()
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // The project file is never the default: writing a
                // profile there changes the workspace for everyone.
                form.destination = match form.destination {
                    ConfigDestination::User => ConfigDestination::Project,
                    ConfigDestination::Project => ConfigDestination::User,
                };
            }
            KeyCode::Enter => {
                if form.field() != SetupField::Secret {
                    form.next_field();
                    return Action::None;
                }
                if form.saving {
                    return Action::None;
                }
                match form.profile_spec() {
                    Ok(_) => {
                        form.saving = true;
                        form.outcome = Some("saving…".to_owned());
                        // A fixture run has no setup service; the runtime
                        // answers with the read-only outcome and the form
                        // shows it, so nothing here claims a write.
                        return Action::SaveProfile;
                    }
                    Err(missing) => {
                        form.field_index = SetupField::ORDER
                            .iter()
                            .position(|field| *field == missing)
                            .unwrap_or(0);
                        form.outcome = Some(format!("the {} is required", missing.label()));
                    }
                }
            }
            _ => {}
        }
        Action::None
    }

    fn picker_key(&mut self, key: KeyEvent, ctrl: bool) -> Action {
        let Some(kind) = self.overlays.last().and_then(Overlay::picker_kind) else {
            return Action::None;
        };
        if ctrl && matches!(key.code, KeyCode::Char('r')) && kind == PickerKind::Models {
            if let Some(id) = self.connector_choice {
                return self.request_connector_catalog(id, true);
            }
        }
        let Some(Overlay::Picker { picker, .. }) = self.overlays.last_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Down => picker.move_down(),
            KeyCode::Up => picker.move_up(),
            KeyCode::PageDown => picker.page_down(10),
            KeyCode::PageUp => picker.page_up(10),
            KeyCode::Char('n') if ctrl => picker.move_down(),
            KeyCode::Char('p') if ctrl => picker.move_up(),
            KeyCode::Backspace => picker.backspace(),
            // Typing filters, including j and k.
            KeyCode::Char(c) if !ctrl => picker.type_char(c),
            KeyCode::Tab => picker.move_down(),
            KeyCode::Enter => return self.choose(kind),
            _ => {}
        }
        Action::None
    }

    /// Applies the highlighted row of the top picker.
    fn choose(&mut self, kind: PickerKind) -> Action {
        let Some(Overlay::Picker { picker, .. }) = self.overlays.last() else {
            return Action::None;
        };
        let Some(row) = picker.choose() else {
            // An unavailable row explains itself instead of doing nothing.
            if let Some(row) = picker.selected() {
                self.notice = Some(row.availability.reason().to_owned());
            }
            return Action::None;
        };
        let id = row.id.clone();
        match kind {
            PickerKind::Commands => {
                self.overlays.pop();
                match command::lookup(&id) {
                    Some(command) => self.dispatch(command, None),
                    None => Action::None,
                }
            }
            PickerKind::Connect => {
                self.overlays.pop();
                if let Some(name) = id.strip_prefix("preset:") {
                    // A supported provider that is not configured yet:
                    // setup opens with its endpoint and protocol filled in.
                    if let Some(preset) = PRESETS.iter().find(|preset| preset.name == name) {
                        self.overlays
                            .push(Overlay::Setup(SetupForm::for_preset(preset)));
                    }
                    return Action::None;
                }
                if id == "custom" {
                    self.overlays
                        .push(Overlay::Setup(SetupForm::for_profile("")));
                    return Action::None;
                }
                if let Some(profile) = id.strip_prefix("profile:") {
                    self.connector_choice = None;
                    self.connector_model = None;
                    self.select_profile(profile);
                    self.overlays.pop();
                    self.open_picker(PickerKind::Models);
                    return self.request_catalog();
                } else if let Some(agent) = id.strip_prefix("agent:") {
                    if let Some(connector) = connector_id(agent) {
                        self.connector_choice = Some(connector);
                        self.connector_model = None;
                        self.overlays.pop();
                        let action = self.request_connector_catalog(connector, false);
                        self.open_picker(PickerKind::Models);
                        return action;
                    }
                }
                Action::None
            }
            PickerKind::Models => {
                if id == "__setup__" {
                    // The round trip: setup opens above the model picker,
                    // so closing it returns with the search preserved.
                    let profile = self.draft.profile.clone().unwrap_or_default();
                    self.overlays
                        .push(Overlay::Setup(SetupForm::for_profile(&profile)));
                    return Action::None;
                }
                self.overlays.pop();
                if let Some(connector) = self.connector_choice {
                    if id == "__default__" {
                        self.connector_model = None;
                    } else {
                        self.connector_model = Some(id);
                    }
                    return Action::SelectConnector(connector);
                }
                self.select_model(&id);
                Action::None
            }
            PickerKind::Effort => {
                self.overlays.pop();
                match id.parse::<ReasoningEffort>() {
                    Ok(effort) => {
                        self.draft.effort = Some(effort);
                        self.status.effort = effort;
                        self.sidebar.model.effort = Some(effort.label().to_owned());
                        // Effort is a session setting and can change
                        // between turns, so a live session persists it
                        // rather than waiting for a new one.
                        if self.session_id.is_some() {
                            return Action::SetEffort(effort);
                        }
                        Action::None
                    }
                    Err(_) => Action::None,
                }
            }
            PickerKind::Mode => {
                self.overlays.pop();
                self.dispatch(Command::Mode, Some(id))
            }
            PickerKind::Sessions => {
                self.overlays.pop();
                self.view = View::Transcript;
                if self.running || self.session_transition {
                    self.notice = Some("finish or cancel the work in flight first".into());
                    return Action::None;
                }
                Action::Resume(SessionId(id))
            }
            PickerKind::Mcp => {
                self.mcp_target = Some(id);
                self.open_picker(PickerKind::McpActions);
                Action::None
            }
            PickerKind::McpActions => {
                let Some(server) = self.mcp_target.clone() else {
                    self.overlays.pop();
                    return Action::None;
                };
                // Every action here changes what the running agent can
                // call. Like the other settings, they wait for a turn or
                // an approval to finish rather than changing the tool set
                // underneath it.
                if !self.settings_are_editable() {
                    self.notice = Some(if self.pending.is_some() {
                        "answer the approval first; MCP servers cannot change during it".into()
                    } else {
                        "a turn is running; Esc cancels it before MCP servers change".to_owned()
                    });
                    return Action::None;
                }
                self.overlays.pop();
                match id.as_str() {
                    // Approving launches a program. What is being trusted
                    // has to be visible first, so this asks for the
                    // definition and the modal overlay, not for the grant.
                    "approve" => Action::Mcp(McpRequest::RequestApproval { server }),
                    "deny" => Action::Mcp(McpRequest::Decide {
                        server,
                        decision: TrustDecision::Denied,
                    }),
                    "restart" => Action::Mcp(McpRequest::Restart { server }),
                    "stop" => Action::Mcp(McpRequest::Stop { server }),
                    "reload" => Action::Mcp(McpRequest::ReloadAll),
                    _ => Action::None,
                }
            }
            PickerKind::Changes => {
                self.overlays.pop();
                Action::OpenFileDiff(id)
            }
        }
    }

    /// Asks the runtime for the selected profile's model list when the
    /// list on screen is not that profile's. The token travels with the
    /// request so a late answer for a superseded profile is dropped.
    fn request_catalog(&mut self) -> Action {
        let Some(profile) = self.draft.profile.clone() else {
            return Action::None;
        };
        if self.catalog.profile == profile && self.catalog.state.is_some() {
            return Action::None;
        }
        self.catalog.profile = profile.clone();
        self.catalog.loading = true;
        Action::LoadCatalog {
            profile,
            selection: self.selection,
        }
    }

    fn request_connector_catalog(&mut self, id: ConnectorId, refresh: bool) -> Action {
        self.selection = self.selection.wrapping_add(1);
        self.connector_catalog.connector = Some(id);
        self.connector_catalog.loading = true;
        if refresh {
            self.connector_catalog.discovery = None;
        }
        self.refresh_open_picker();
        Action::LoadConnectorCatalog {
            connector: id,
            selection: self.selection,
            refresh,
        }
    }

    /// A connector catalog arrived. `selection` is the token the load
    /// started under.
    pub fn apply_connector_catalog(
        &mut self,
        selection: u64,
        connector: ConnectorId,
        discovery: ConnectorModelDiscovery,
    ) -> bool {
        if selection != self.selection || self.connector_choice != Some(connector) {
            return false;
        }
        let models = discovery
            .catalog()
            .map(|catalog| catalog.models.clone())
            .unwrap_or_default();
        self.connector_catalog = ConnectorCatalogView {
            connector: Some(connector),
            models,
            discovery: Some(discovery),
            loading: false,
        };
        self.refresh_open_picker();
        true
    }

    /// Whether a pinned session refuses this (provider, model) pair, and
    /// says so.
    ///
    /// The check is on the pair, not on either half: the driver keeps
    /// running the provider and model its transcript was produced under,
    /// so changing *either* would leave the interface displaying settings
    /// the driver is not using. A model of the same name under another
    /// provider is a different model, which a model-only comparison would
    /// have missed.
    fn refuses_pinned_change(&mut self, profile: &str, model: Option<&str>) -> bool {
        if !self.session_pinned || self.status.model.is_empty() {
            return false;
        }
        let same_profile = profile == self.status.profile;
        let same_model = model.is_none_or(|model| model == self.status.model);
        if same_profile && same_model {
            return false;
        }
        let wanted = match model {
            Some(model) => format!("{model} on {profile}"),
            None => profile.to_owned(),
        };
        self.overlays.push(Overlay::Notice(Notice {
            title: "Changing this needs a new session".into(),
            body: format!(
                "This session is pinned to {} on {}. Gritt cannot move its stored transcript \
                 and continuation state to {wanted}. Run /new to start a session on the new \
                 choice; this one stays in /sessions and your composer draft is kept.",
                self.status.model, self.status.profile
            ),
            is_error: false,
            confirm: None,
        }));
        true
    }

    /// Selecting a provider clears the model, because a model belongs to
    /// the profile it was chosen under.
    pub fn select_profile(&mut self, profile: &str) {
        // Nothing below this line may run on a pinned session: the draft,
        // the catalog, the sidebar's provider, and the effort would all
        // move away from what the driver is really using.
        if self.refuses_pinned_change(profile, None) {
            return;
        }
        let had_model = self.draft.model.clone();
        self.draft = self.draft.clone().with_profile(profile);
        if had_model.is_some() && self.draft.model.is_none() {
            self.notice = Some(format!(
                "the model was cleared: it belonged to another provider, not {profile}"
            ));
        }
        if self.catalog.profile != profile {
            // The previous profile's list is not this profile's list, and
            // a load already in flight for it must not land here.
            self.selection += 1;
            self.catalog = ModelCatalogView {
                profile: profile.to_owned(),
                ..ModelCatalogView::default()
            };
        }
        self.sidebar.model.backend = Some(profile.to_owned());
        self.sidebar.model.model = None;
        self.revalidate_effort();
    }

    /// Selecting a model revalidates the effort against it and, on a
    /// session that already has history, explains that the change needs a
    /// new session instead of silently discarding context.
    pub fn select_model(&mut self, model: &str) {
        // The profile the model would be chosen under, which is what makes
        // an identically named model on another provider a change.
        let profile = self
            .draft
            .profile
            .clone()
            .unwrap_or_else(|| self.status.profile.clone());
        if self.refuses_pinned_change(&profile, Some(model)) {
            return;
        }
        self.draft = self.draft.clone().with_model(model);
        self.sidebar.model.model = Some(model.to_owned());
        self.revalidate_effort();
    }

    /// Drops an explicit effort the newly selected model cannot take.
    fn revalidate_effort(&mut self) {
        let Some(effort) = self.draft.effort else {
            return;
        };
        if !effort.is_explicit() {
            return;
        }
        let picker = self.effort_picker();
        let still_valid = picker
            .rows()
            .iter()
            .any(|row| row.id == effort.as_str() && row.availability.is_available());
        if !still_valid {
            self.draft.effort = Some(ReasoningEffort::Auto);
            self.status.effort = ReasoningEffort::Auto;
            self.sidebar.model.effort = Some(ReasoningEffort::Auto.label().into());
            let explanation = format!(
                "effort returned to the model default: {effort} is not available on this model"
            );
            // A profile change can clear the model and the effort at once;
            // both reasons are shown rather than one overwriting the other.
            self.notice = Some(match self.notice.take() {
                Some(existing) => format!("{existing}; {explanation}"),
                None => explanation,
            });
        }
    }

    fn main_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => {
                if self.running {
                    Action::Cancel
                } else {
                    self.quit = true;
                    Action::Quit
                }
            }
            (KeyCode::Char('q'), true) => {
                self.quit = true;
                Action::Quit
            }
            (KeyCode::Char('p'), true) => {
                self.view = View::Palette;
                self.open_picker(PickerKind::Commands);
                Action::None
            }
            (KeyCode::Char('s'), true) => self.dispatch(Command::Sessions, None),
            (KeyCode::Char('j'), true) => {
                self.composer.insert_newline();
                Action::None
            }
            (KeyCode::Char('y'), true) => {
                // The keyboard copy path: no mouse, no OS clipboard crate.
                self.clipboard = Some(match self.focus {
                    Focus::Transcript => self
                        .entries
                        .iter()
                        .map(|entry| entry.text.clone())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => self.composer.copy_target().to_owned(),
                });
                let count = self.clipboard.as_ref().map(String::len).unwrap_or(0);
                self.notice = Some(format!("copied {count} bytes to the Gritt buffer"));
                Action::None
            }
            (KeyCode::Char('a'), true) => {
                self.composer.select_all();
                Action::None
            }
            (KeyCode::Char('w'), true) => {
                self.composer.delete_word_back();
                self.suggestion_index = 0;
                Action::None
            }
            (KeyCode::Char('u'), true) => {
                self.composer.delete_to_line_start();
                self.suggestion_index = 0;
                Action::None
            }
            (KeyCode::Char('g'), true) => {
                self.follow_latest();
                Action::None
            }
            (KeyCode::Esc, _) => {
                // Nothing is open here: Escape cancels a running turn, the
                // asynchronous work the loading line is showing, or a
                // session change that is still in flight. The transition
                // is checked on its own: its loading line can have been
                // replaced by another request's, and without this the
                // interface would have no way back.
                if self.running || self.is_busy() || self.session_transition {
                    Action::Cancel
                } else {
                    self.notice = None;
                    Action::None
                }
            }
            // Focus only stops on panes that are actually on screen, so
            // Tab can never park the keyboard on a hidden sidebar.
            (KeyCode::Tab, _) => {
                self.focus = self.next_focus(true);
                Action::None
            }
            (KeyCode::Enter, _) => {
                // Shift-Enter and Alt-Enter insert a newline where the
                // terminal reports them; Ctrl-J always does. Ctrl-M is
                // never bound on its own: terminals encode it as Enter.
                if shift || alt {
                    self.composer.insert_newline();
                    return Action::None;
                }
                // The sidebar's changed files open as the same searchable
                // list every other selection uses, so reaching one needs
                // no key that would otherwise type into the composer.
                if self.focus == Focus::Sidebar {
                    self.open_picker(PickerKind::Changes);
                    return Action::ScanChanges;
                }
                self.submit()
            }
            (KeyCode::Backspace, _) => {
                self.composer.backspace();
                self.suggestion_index = 0;
                Action::None
            }
            (KeyCode::Delete, _) => {
                self.composer.delete_forward();
                self.suggestion_index = 0;
                Action::None
            }
            (KeyCode::Left, true) => {
                self.composer.move_word_left(shift);
                Action::None
            }
            (KeyCode::Right, true) => {
                self.composer.move_word_right(shift);
                Action::None
            }
            (KeyCode::Left, false) => {
                self.composer.move_left(shift);
                Action::None
            }
            (KeyCode::Right, false) => {
                self.composer.move_right(shift);
                Action::None
            }
            (KeyCode::Up, _) => {
                // The sidebar column scrolls on its own; the transcript
                // pane scrolls outright; from the composer, a move with
                // nowhere to go scrolls the transcript instead.
                if self.focus == Focus::Sidebar && self.sidebar_column_visible() {
                    self.sidebar_scroll = self.sidebar_scroll.saturating_sub(1);
                } else if self.focus == Focus::Transcript || !self.composer.move_up(shift) {
                    self.scroll_up(1);
                }
                Action::None
            }
            (KeyCode::Down, _) => {
                if self.focus == Focus::Sidebar && self.sidebar_column_visible() {
                    self.sidebar_scroll = self.sidebar_scroll.saturating_add(1);
                } else if self.focus == Focus::Transcript || !self.composer.move_down(shift) {
                    self.scroll_down(1);
                }
                Action::None
            }
            (KeyCode::Home, _) => {
                self.composer.move_line_start(shift);
                Action::None
            }
            (KeyCode::End, _) => {
                self.composer.move_line_end(shift);
                Action::None
            }
            (KeyCode::PageUp, _) => {
                if self.focus == Focus::Sidebar && self.sidebar_column_visible() {
                    self.sidebar_scroll = self.sidebar_scroll.saturating_sub(10);
                } else {
                    self.scroll_up(10);
                }
                Action::None
            }
            (KeyCode::PageDown, _) => {
                if self.focus == Focus::Sidebar && self.sidebar_column_visible() {
                    self.sidebar_scroll = self.sidebar_scroll.saturating_add(10);
                } else {
                    self.scroll_down(10);
                }
                Action::None
            }
            (KeyCode::Char(c), false) => {
                self.composer.insert_char(c);
                self.suggestions_dismissed = false;
                self.suggestion_index = 0;
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Enter. A command runs locally; anything else becomes a prompt.
    fn submit(&mut self) -> Action {
        let text = self.composer.text().to_owned();
        match command::parse(&text) {
            Parsed::Command { command, argument } => {
                self.composer.clear();
                self.dispatch(command, argument)
            }
            Parsed::Unknown(name) => {
                // The input is kept so the typo can be corrected.
                self.notice = Some(format!("unknown command /{name}; /help lists them"));
                Action::None
            }
            Parsed::Prompt(prompt) => {
                if self.session_transition {
                    // The driver this would run on is being replaced. The
                    // draft stays in the composer.
                    self.notice = Some("the session is still opening; Esc cancels it".into());
                    return Action::None;
                }
                if self.running {
                    self.notice = Some("a turn is running; Esc cancels it".into());
                    return Action::None;
                }
                if prompt.trim().is_empty() {
                    return Action::None;
                }
                self.composer.clear();
                self.push(EntryKind::User, prompt.clone());
                self.running = true;
                self.stream_open = None;
                self.follow_latest();
                Action::Submit(prompt)
            }
        }
    }

    /// Puts a draft back after a submission that could not be sent.
    pub fn restore_draft(&mut self, draft: &str) {
        self.composer.set_text(draft);
    }

    /// The top line the viewport would show if it were following the
    /// bottom, from the last frame's measurements.
    fn bottom_top(&self) -> usize {
        let metrics = self.metrics.get();
        metrics
            .transcript_lines
            .saturating_sub(metrics.transcript_height)
    }

    /// Scrolling up holds the viewport: streaming stops following and the
    /// anchor stays on the same content while output is appended below.
    pub fn scroll_up(&mut self, lines: usize) {
        if self.follow {
            self.top = self.bottom_top();
        }
        self.top = self.top.saturating_sub(lines);
        self.follow = false;
    }

    pub fn scroll_down(&mut self, lines: usize) {
        if self.follow {
            return;
        }
        self.top = self.top.saturating_add(lines);
        if self.top >= self.bottom_top() {
            self.follow_latest();
        }
    }

    /// Return to latest: the viewport goes back to the bottom and follows
    /// streaming again.
    pub fn follow_latest(&mut self) {
        self.follow = true;
        self.top = self.bottom_top();
        self.new_output = false;
    }

    /// The transcript lines visible in an area `width` by `height`, and
    /// the index of the first one. The renderer draws exactly this, and a
    /// test can assert on the same thing the reader sees.
    pub fn visible_transcript(
        &self,
        width: usize,
        height: usize,
        render: impl Fn(&App, usize) -> Vec<Line<'static>>,
    ) -> (usize, Vec<Line<'static>>) {
        let lines = self.transcript_lines(width, render);
        let metrics = Metrics {
            transcript_lines: lines.len(),
            transcript_height: height,
            terminal_width: self.metrics.get().terminal_width,
        };
        self.metrics.set(metrics);
        let bottom = lines.len().saturating_sub(height);
        let start = if self.follow {
            bottom
        } else {
            self.top.min(bottom)
        };
        let end = (start + height).min(lines.len());
        (start, lines[start..end].to_vec())
    }

    // -- what the runtime hands back ------------------------------------

    /// `/new`: clears the presentation and the session identity, keeping
    /// the composer draft and the provider/model choices so the next
    /// prompt opens a session on the same selection.
    pub fn start_new_draft(&mut self) {
        self.entries.clear();
        self.revision += 1;
        self.top = 0;
        self.follow = true;
        self.new_output = false;
        self.session_pinned = false;
        self.session_id = None;
        self.connector = None;
        self.connector_choice = None;
        self.connector_model = None;
        self.connector_catalog = ConnectorCatalogView::default();
        self.connector_version = None;
        self.update_approval = None;
        self.running = false;
        self.pending = None;
        self.mcp_approval = None;
        self.session_transition = false;
        self.pending_writes.clear();
        self.observed_writes.clear();
        self.status.session.clear();
        self.status.usage = Usage::default();
        // The sidebar's generation moves here, so anything still in
        // flight for the session just left is refused when it lands.
        self.sidebar.reset();
        self.draft.name = None;
    }

    /// A profile's model list arrived. `selection` is the token the load
    /// started under: a list for a profile the user has already moved on
    /// from is dropped instead of replacing the current one.
    ///
    /// Returns whether the result was accepted, which is what the
    /// late-result tests assert on.
    pub fn apply_catalog(
        &mut self,
        selection: u64,
        profile: &str,
        models: Vec<ModelInfo>,
        state: CatalogState,
    ) -> bool {
        if selection != self.selection || self.draft.profile.as_deref() != Some(profile) {
            return false;
        }
        self.catalog = ModelCatalogView {
            profile: profile.to_owned(),
            models,
            state: Some(state),
            loading: false,
        };
        // The model may no longer be offered by the list that just
        // arrived, and the effort may no longer be supported by it.
        self.revalidate_effort();
        self.refresh_open_picker();
        true
    }

    /// A catalog load failed outright (a storage or configuration error
    /// rather than a provider refusal). The picker shows the reason and
    /// stops claiming to be loading.
    pub fn catalog_failed(&mut self, selection: u64, profile: &str, reason: String) -> bool {
        if selection != self.selection || self.draft.profile.as_deref() != Some(profile) {
            return false;
        }
        self.catalog.loading = false;
        self.catalog.state = Some(CatalogState::RefreshFailed { reason });
        self.refresh_open_picker();
        true
    }

    /// Live MCP state from the runtime's subscription. The sidebar shows
    /// every configured entry; `/mcp` rebuilds in place so a list the
    /// user is looking at changes under them rather than going stale.
    pub fn apply_mcp(&mut self, snapshots: Vec<McpServerSnapshot>) {
        self.mcp = snapshots;
        self.sidebar.integrations.mcp = Some(self.mcp.clone());
        // These servers are Gritt's, whatever kind of session is open:
        // they come from Gritt's runtime and Gritt's `.mcp.json`. An
        // external agent owns its own MCP clients (ADR-010) and does not
        // report their state, so that is named as unknown beside them
        // rather than this list being relabelled as the agent's.
        self.sidebar.integrations.mcp_owner = Some("Gritt".to_owned());
        self.sidebar.integrations.connector_mcp = self.connector.map(|id| id.as_str().to_owned());
        self.refresh_open_picker();
    }

    /// A workspace change scan landed. `generation` is the sidebar
    /// generation the scan started under; a scan for the previous session
    /// is dropped.
    pub fn apply_changes(&mut self, generation: u64, changes: ChangedFiles) -> bool {
        if !self.sidebar.accepts(generation) {
            return false;
        }
        self.sidebar.changed_files = changes;
        self.refresh_open_picker();
        true
    }

    /// Paths from successful native writes since the last call, for the
    /// runtime to record against the workspace observer.
    pub fn take_observed_writes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.observed_writes)
    }

    /// Shows a read-only diff, or the reason there is none.
    pub fn show_file_diff(&mut self, diff: FileDiff) {
        match diff {
            FileDiff::Text { path, body } => self.overlays.push(Overlay::FileDiff {
                path,
                body,
                scroll: 0,
            }),
            FileDiff::Unavailable { path, reason } => self.overlays.push(Overlay::Notice(Notice {
                title: path,
                body: reason,
                is_error: true,
                confirm: None,
            })),
        }
    }

    /// The setup form's values, taken once. The key leaves the form here
    /// and is not kept anywhere else.
    pub fn take_setup_submission(&mut self) -> Option<SetupSubmission> {
        let Some(Overlay::Setup(form)) = self
            .overlays
            .iter_mut()
            .rev()
            .find(|overlay| matches!(overlay, Overlay::Setup(_)))
        else {
            return None;
        };
        let profile = form.profile_spec().ok()?;
        let typed = form.secret.text().to_owned();
        form.secret.clear();
        Some(SetupSubmission {
            profile,
            secret: (!typed.is_empty()).then(|| Secret::new(typed)),
            destination: form.destination,
        })
    }

    /// The outcome of a setup write, shown on the form. `close` follows a
    /// success: the flow returns to the picker underneath with its search
    /// and the composer draft intact.
    /// Returns the work the outcome created: after a successful write the
    /// profile's catalog has to be loaded again, because the one cached
    /// before the credential existed is a completed failure and reopening
    /// `/models` would show it rather than retry.
    pub fn setup_outcome(&mut self, message: String, close: bool) -> Action {
        let position = self
            .overlays
            .iter()
            .rposition(|overlay| matches!(overlay, Overlay::Setup(_)));
        let Some(position) = position else {
            return Action::None;
        };
        if !close {
            if let Overlay::Setup(form) = &mut self.overlays[position] {
                form.saving = false;
                form.outcome = Some(message);
            }
            return Action::None;
        }
        let profile = match &self.overlays[position] {
            Overlay::Setup(form) => form.name.text().trim().to_owned(),
            _ => String::new(),
        };
        // Only the form leaves. The picker underneath keeps its query and
        // its highlight, and the composer draft was never touched.
        self.overlays.remove(position);
        self.notice = Some(message);
        if profile.is_empty() {
            return Action::None;
        }
        // The credential changed, so the state cached under the old one is
        // no longer an answer about this profile.
        if self.catalog.profile == profile {
            self.selection += 1;
            self.catalog = ModelCatalogView {
                profile: profile.clone(),
                ..ModelCatalogView::default()
            };
        }
        // Writing a profile is always allowed; selecting it is not. A
        // pinned session's driver keeps its own provider and model, so
        // adopting the new one here would show a selection the driver is
        // not using. The save stands and the explanation says what to do
        // with it.
        if self.draft.profile.as_deref() != Some(profile.as_str()) {
            if self.refuses_pinned_change(&profile, None) {
                self.refresh_open_picker();
                return Action::None;
            }
            self.draft = self.draft.clone().with_profile(&profile);
            self.sidebar.model.backend = Some(profile.clone());
        }
        // A model picker underneath is for the profile that was just set
        // up when the round trip came from `/models`; either way it is
        // rebuilt from current state before it is looked at again.
        self.refresh_open_picker();
        self.request_catalog()
    }

    /// A draft that could not open. The draft is kept, so correcting one
    /// field and submitting again is all it takes.
    pub fn show_draft_errors(&mut self, errors: &[DraftError]) {
        self.running = false;
        let Some(error) = errors.first() else { return };
        let (title, body) = describe_draft_error(error);
        self.overlays.push(Overlay::Notice(Notice {
            title,
            body,
            is_error: true,
            confirm: None,
        }));
    }

    /// Startup notes as transcript lines. A skipped profile is the one
    /// note that is also raised as a notice, because the session runs on
    /// a provider other than the one the draft named.
    pub fn show_draft_warnings(&mut self, warnings: &[DraftWarning]) {
        for warning in warnings {
            let text = warning.to_string();
            if let DraftWarning::ProfileSkipped(_) = warning {
                self.notice = Some(text.clone());
            }
            self.push(EntryKind::System, text);
        }
    }

    /// The catalog's figures for the active model. Both are optional and
    /// an absent one leaves its section unavailable rather than zero.
    pub fn set_model_facts(&mut self, model: Option<&ModelInfo>) {
        let capabilities = model.map(|model| &model.capabilities);
        self.sidebar.usage.context_limit = capabilities.and_then(|c| c.context_length);
        self.pricing = capabilities.and_then(|c| {
            match (c.input_price_per_million, c.output_price_per_million) {
                (Some(input), Some(output)) => Some((input, output)),
                _ => None,
            }
        });
        self.recompute_cost();
    }

    /// The session cost estimate, only when reported usage and listed
    /// prices for this model both exist. Never a billed amount.
    fn recompute_cost(&mut self) {
        let Some((input_price, output_price)) = self.pricing else {
            self.sidebar.cost = Default::default();
            return;
        };
        let usage = &self.sidebar.usage;
        // An estimate needs complete usage for the whole session. A turn
        // that reported one half leaves the totals a floor, and pricing a
        // floor would understate the cost without saying so.
        if usage.incomplete {
            self.sidebar.cost = Default::default();
            return;
        }
        let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) else {
            self.sidebar.cost = Default::default();
            return;
        };
        let estimate = (input as f64 / 1_000_000.0) * input_price
            + (output as f64 / 1_000_000.0) * output_price;
        self.sidebar.cost.estimate_usd = Some(estimate);
        self.sidebar.cost.scope = Some("this session, at listed prices".to_owned());
    }

    /// Puts a failed submission back: the prompt returns to the composer
    /// and the entry it produced leaves the transcript, so a rejected
    /// draft costs nothing typed.
    pub fn undo_submission(&mut self, prompt: &str) {
        if self
            .entries
            .last()
            .is_some_and(|entry| entry.kind == EntryKind::User && entry.text == sanitize(prompt))
        {
            self.entries.pop();
            self.revision += 1;
        }
        self.running = false;
        self.restore_draft(prompt);
    }

    /// The effort the live driver reports, which is the effective one.
    /// `None` is a connector session, where the agent owns it.
    pub fn set_effective_effort(&mut self, effort: Option<ReasoningEffort>) {
        match effort {
            Some(effort) => {
                self.status.effort = effort;
                self.sidebar.model.effort = Some(effort.label().to_owned());
                self.draft.effort = Some(effort);
            }
            None => self.sidebar.model.effort = None,
        }
    }

    /// Rebuilds the connection dialog if it is the picker on screen, for
    /// results that change what it can offer: credential availability and
    /// the installed-agent probe.
    pub fn refresh_connection_picker(&mut self) {
        if self.top_overlay().and_then(Overlay::picker_kind) == Some(PickerKind::Connect) {
            let rows = self.connection_picker().rows().to_vec();
            if let Some(Overlay::Picker { picker, .. }) = self.overlays.last_mut() {
                picker.replace_rows(rows);
            }
        }
    }

    /// Rebuilds the rows of the picker on screen from current state, so
    /// an asynchronous result fills in the list the user is looking at.
    fn refresh_open_picker(&mut self) {
        let Some(kind) = self.overlays.last().and_then(Overlay::picker_kind) else {
            return;
        };
        let rows = match kind {
            PickerKind::Mode => self.mode_picker(),
            PickerKind::Models => self.model_picker(),
            PickerKind::Effort => self.effort_picker(),
            PickerKind::Connect => self.connection_picker(),
            PickerKind::Mcp => self.mcp_picker(),
            PickerKind::McpActions => {
                self.mcp_actions_picker(self.mcp_target.clone().unwrap_or_default().as_str())
            }
            PickerKind::Changes => self.changes_picker(),
            PickerKind::Commands | PickerKind::Sessions => return,
        };
        if let Some(Overlay::Picker { picker, .. }) = self.overlays.last_mut() {
            picker.replace_contents(rows);
        }
    }

    pub fn set_session(&mut self, session: &Session) {
        self.session_id = Some(session.id.clone());
        self.status.session = session.name.clone();
        self.status.workspace = session.workspace.display().to_string();
        self.status.phase = match session.phase {
            Phase::Planning => "planning".into(),
            Phase::Coding => "coding".into(),
        };
        self.sidebar.session.name = Some(session.name.clone());
        self.sidebar.session.workspace = Some(self.status.workspace.clone());
        self.sidebar.session.phase = Some(self.status.phase.clone());
        match &session.kind {
            gritt_core::session::SessionKind::Native {
                provider_profile,
                model,
                effort,
            } => {
                self.status.profile = provider_profile.clone();
                self.status.model = model.clone();
                self.status.effort = *effort;
                self.sidebar.model.backend = Some(provider_profile.clone());
                self.sidebar.model.model = Some(model.clone());
                self.sidebar.model.effort = Some(effort.label().to_owned());
                self.sidebar.model.managed_by_agent = false;
                self.sidebar.model.version = None;
                self.connector_version = None;
                self.connector = None;
                // A session with stored history is pinned to the provider
                // and model its transcript was produced under.
                self.draft.profile = Some(provider_profile.clone());
                self.draft.model = Some(model.clone());
                self.draft.effort = Some(*effort);
            }
            gritt_core::session::SessionKind::Connector { id, model } => {
                self.connector = Some(*id);
                self.status.profile = id.as_str().to_owned();
                self.status.model = model.clone().unwrap_or_default();
                self.sidebar.model.backend = Some(id.as_str().to_owned());
                self.sidebar.model.model = model.clone();
                // ADR-010: the connector owns effort and permissions. A
                // model Gritt passed at launch is shown, not guessed.
                self.sidebar.model.managed_by_agent = true;
                self.sidebar.model.version = None;
                self.connector_version = None;
            }
        }
    }
}

/// Commands that change the session draft or the transcript the runtime
/// owns, and so must wait for an active turn or approval to finish.
fn changes_settings(command: Command) -> bool {
    matches!(
        command,
        Command::Connect
            | Command::Models
            | Command::Effort
            | Command::Mode
            | Command::Plan
            | Command::Code
            | Command::New
            | Command::Update
    )
}

/// Settings the native provider owns. A connector session refuses these
/// because the external agent owns its own model, effort, and permissions.
fn is_native_setting(command: Command) -> bool {
    matches!(
        command,
        Command::Connect | Command::Models | Command::Effort | Command::Mode
    )
}

/// The sidebar's one-line version state for an agent CLI.
pub fn version_summary(check: &ConnectorVersionCheck) -> String {
    use gritt_core::connector::{VersionComparison, VersionFreshness};
    match check {
        ConnectorVersionCheck::NotInstalled { .. } => "not installed".to_owned(),
        ConnectorVersionCheck::Unsupported { .. } => "no version check".to_owned(),
        _ => {
            let status = check.status().expect("status for a checked outcome");
            let installed = status.installed.as_deref().unwrap_or("unknown");
            let stale = match status.freshness {
                VersionFreshness::Stale => ", stale",
                VersionFreshness::Current => "",
            };
            match (status.comparison, &status.latest) {
                (VersionComparison::Outdated, Some(latest)) => {
                    format!("{installed} (latest {latest}{stale}; /update)")
                }
                (VersionComparison::Current, _) => format!("{installed} (current{stale})"),
                (VersionComparison::Newer, Some(latest)) => {
                    format!("{installed} (newer than {latest}{stale})")
                }
                (_, Some(latest)) => format!("{installed} (latest {latest}?{stale})"),
                (_, None) => format!("{installed} (latest unknown)"),
            }
        }
    }
}

/// A typed draft rejection as a modal explanation. The interface never
/// parses an error string; it matches on the value the control plane
/// returned.
pub fn describe_draft_error(error: &DraftError) -> (String, String) {
    match error {
        DraftError::MissingProfile => (
            "Choose a provider".into(),
            "No provider is selected and no default is configured. /connect lists the \
             configured profiles and offers to set up a new one."
                .into(),
        ),
        DraftError::UnknownProfile { profile } => (
            "Unknown provider".into(),
            format!("`{profile}` is not in the configuration. /connect lists what is."),
        ),
        DraftError::MissingModel => (
            "Choose a model".into(),
            "No model is selected and no default is configured. /models lists the \
             selected provider's catalog."
                .into(),
        ),
        DraftError::ModelOutsideProfile {
            model,
            model_profile,
            profile,
        } => (
            "That model belongs to another provider".into(),
            format!("{model} resolves under {model_profile}, not {profile}. Choose a model from {profile}'s list, or change the provider first."),
        ),
        DraftError::ModelResolution { model, message } => (
            "That model could not be resolved".into(),
            format!("{model}: {message}"),
        ),
        DraftError::EffortUnsupported {
            model,
            effort,
            reason,
            ..
        } => (
            "That effort is not supported here".into(),
            format!("{model} has no safe mapping for {effort}: {}. Effort returns to the model default.", reason.describe()),
        ),
        DraftError::SessionPinned {
            name,
            profile,
            model,
            ..
        } => (
            "Changing the model needs a new session".into(),
            format!("`{name}` is pinned to {model} on {profile}. Gritt cannot move its stored transcript and continuation state. Run /new to start a session on the new choice; this one stays in /sessions and your composer draft is kept."),
        ),
        DraftError::ConnectorSession { name, connector } => (
            "That session runs on an agent".into(),
            format!("`{name}` runs on {}, which manages its own model, effort, and permissions. The native pickers do not apply to it.", connector.as_str()),
        ),
        DraftError::OtherWorkspace { name, workspace } => (
            "That session belongs to another workspace".into(),
            format!("`{name}` was created in {}. Sessions do not move between workspaces.", workspace.display()),
        ),
        DraftError::NoUsableProfile { skipped } => (
            "No provider could start the session".into(),
            format!(
                "Every profile in the fallback order was skipped:\n{}\n/connect lists the profiles; keys and endpoints are checked again on the next prompt.",
                skipped
                    .iter()
                    .map(|entry| format!("  {entry}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        ),
    }
}

/// The connector a row label names, or `None` when it is not one Gritt
/// knows. Row ids are built from the same list, so this cannot drift.
fn connector_id(name: &str) -> Option<ConnectorId> {
    ConnectorId::ORDER
        .into_iter()
        .find(|id| id.as_str() == name && *id != ConnectorId::Native)
}

fn protocol_word(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::ChatCompletions => "chat completions",
        Protocol::Responses => "responses",
        Protocol::Messages => "messages",
    }
}

/// The one-line catalog state shown in a picker.
pub fn catalog_word(state: &CatalogState) -> &'static str {
    match state {
        CatalogState::Fresh { .. } => "catalog fresh",
        CatalogState::Stale { .. } => "catalog stale; using the last cached list",
        CatalogState::Missing { .. } => "no catalog; capabilities unreported",
        CatalogState::RefreshFailed { .. } => "refresh failed; no list in use",
        CatalogState::Skipped => "catalog loading is off for this run",
    }
}

#[cfg(test)]
mod tests;
