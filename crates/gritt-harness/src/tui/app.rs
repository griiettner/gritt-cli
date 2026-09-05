//! Full-screen state and its reducers. Everything here is plain data so
//! the key handling, command dispatch, and transcript logic run under
//! `cargo test` without a terminal.
//!
//! The state is a client of the control plane and nothing more: it holds
//! the choices a user has made and the values the harness handed it. It
//! never resolves a model, reads a config file, or opens a session.

use std::cell::{Cell, RefCell};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gritt_core::connector::ConnectorId;
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
use crate::draft::{CatalogState, DraftError, DraftWarning, SessionDraft};
use crate::modes::print::describe_call;
use crate::policy::Decision;
use crate::setup::{ConfigDestination, CredentialState, ProfileSummary};

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
    Resume(SessionId),
    Approve(ApprovalDecision),
    RefreshSessions,
    /// Load a profile's model list. `selection` is the token the result
    /// must still match; a later profile change makes it stale.
    LoadCatalog {
        profile: String,
        selection: u64,
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
}

/// What `/mcp` asks the runtime to do. The runtime calls the same typed
/// API `gritt mcp trust` uses; nothing here is a parsed string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpRequest {
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

/// Which searchable list an overlay is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
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

/// The provider setup screens. In this step it is an overlay over fixture
/// state: nothing here writes a config file or a keychain entry.
#[derive(Debug, Clone)]
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
            env_var: Composer::from_text(format!(
                "{}_API_KEY",
                name.to_ascii_uppercase().replace('-', "_")
            )),
            secret: Composer::new(),
            field_index: 1,
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
        let env_var = self.env_var.text().trim().to_owned();
        if env_var.is_empty() {
            return Err(SetupField::EnvVar);
        }
        Ok(ProviderProfile {
            name: name.clone(),
            protocol: self.protocol,
            base_url,
            key: SecretRef::for_profile(&name, &env_var),
            aliases: Default::default(),
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

/// The profile spec and the key the setup form collected, handed to the
/// runtime once. Taking it clears the key from the form, so a typed value
/// exists in one place at a time and never in an `Action`.
pub struct SetupSubmission {
    pub profile: ProviderProfile,
    /// `None` when the user left the key blank, which is allowed: the
    /// variable may already be set in the environment.
    pub secret: Option<Secret>,
    pub destination: ConfigDestination,
}

/// A modal explanation with no choice to make beyond acknowledging it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// Set when the notice is the "this needs a new session" explanation.
    pub is_error: bool,
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
    /// What asynchronous work is in flight, shown near the composer. The
    /// interface stays usable while it runs.
    pub loading: Option<String>,
    /// The live session, `None` before the first prompt opens one.
    pub session_id: Option<SessionId>,
    /// Set for a connector session: the agent owns its model, effort, and
    /// permissions (ADR-010), so the native pickers are refused.
    pub connector: Option<ConnectorId>,
    /// The server `/mcp` opened an action list for.
    pub mcp_target: Option<String>,
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
    assistant_open: bool,
    revision: u64,
    cache: RefCell<LayoutCache>,
    /// What the last frame measured: wrapped transcript lines, the height
    /// of the transcript area, and the terminal width. The reducer needs
    /// them to answer "is the viewport at the bottom" and "is the sidebar
    /// a column here", and only the renderer knows them.
    metrics: Cell<Metrics>,
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
            profiles: Vec::new(),
            agents: Vec::new(),
            catalog: ModelCatalogView::default(),
            session_pinned: false,
            mcp: Vec::new(),
            selection: 0,
            loading: None,
            session_id: None,
            connector: None,
            mcp_target: None,
            pricing: None,
            pending_writes: std::collections::BTreeMap::new(),
            observed_writes: Vec::new(),
            assistant_open: false,
            revision: 0,
            cache: RefCell::new(LayoutCache::default()),
            metrics: Cell::new(Metrics::default()),
        }
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
        self.assistant_open = kind == EntryKind::Assistant;
        self.touch();
    }

    fn push_entry(&mut self, entry: Entry) {
        self.assistant_open = entry.kind == EntryKind::Assistant;
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
        self.assistant_open = false;
        self.running = false;
        self.follow = true;
        self.top = 0;
        self.new_output = false;
    }

    pub fn on_event(&mut self, event: &Event) {
        match &event.kind {
            EventKind::TextDelta { text } => {
                if self.assistant_open {
                    if let Some(last) = self.entries.last_mut() {
                        last.text.push_str(&sanitize(text));
                    }
                    self.touch();
                } else {
                    self.push(EntryKind::Assistant, text.clone());
                }
            }
            EventKind::ReasoningSummary { text } => self.push(EntryKind::Reasoning, text.clone()),
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
                let total = &mut self.status.usage;
                total.input_tokens =
                    Some(total.input_tokens.unwrap_or(0) + usage.input_tokens.unwrap_or(0));
                total.output_tokens =
                    Some(total.output_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0));
                self.sidebar.usage.input_tokens = total.input_tokens;
                self.sidebar.usage.output_tokens = total.output_tokens;
                // The prompt tokens of the most recent request are the
                // tokens that were in the model's context for it. The
                // cumulative totals above are not, and the sidebar keeps
                // the two apart.
                if let Some(prompt) = usage.input_tokens {
                    self.sidebar.usage.context_tokens = Some(prompt);
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
                    self.assistant_open = false;
                }
            }
            EventKind::Error { message, .. } => self.push(EntryKind::Error, message.clone()),
            EventKind::Completed { .. } => self.assistant_open = false,
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
        self.pending = Some(pending);
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
    /// trip.
    pub fn model_picker(&self) -> Picker {
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

    /// The effort picker: `Model default` plus only the levels the
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
        let selected = self.draft.effort.unwrap_or_default();
        let mut rows = vec![PickerRow::new("auto", "Model default")
            .detail("no explicit effort is sent")
            .badge("auto")
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

    // -- command dispatch ----------------------------------------------

    fn open_picker(&mut self, kind: PickerKind) {
        let picker = match kind {
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
                "this session runs on {}; its model and effort are managed by the agent",
                id.as_str()
            ));
            return Action::None;
        }
        match cmd {
            Command::Connect => {
                self.open_picker(PickerKind::Connect);
                Action::None
            }
            Command::Models => {
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

    /// Whether settings may change right now.
    pub fn settings_are_editable(&self) -> bool {
        !self.running && self.pending.is_none()
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

    fn approval_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.pending = None;
                self.view = View::Transcript;
                Action::Approve(ApprovalDecision::Approved)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending = None;
                self.view = View::Transcript;
                Action::Approve(ApprovalDecision::Denied)
            }
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
            Some(Overlay::Notice(_)) => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) {
                    self.overlays.pop();
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
        let Some(Overlay::Picker { kind, picker }) = self.overlays.last_mut() else {
            return Action::None;
        };
        let kind = *kind;
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
                    self.select_profile(profile);
                    self.open_picker(PickerKind::Models);
                    return self.request_catalog();
                } else if let Some(agent) = id.strip_prefix("agent:") {
                    self.overlays.push(Overlay::Notice(Notice {
                        title: agent.to_string(),
                        body: format!(
                            "{agent} runs its own harness. Gritt supervises it and relays its \
                             approvals; its model and effort are managed by the agent and are \
                             not set here."
                        ),
                        is_error: false,
                    }));
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
                self.select_model(&id);
                Action::None
            }
            PickerKind::Effort => {
                self.overlays.pop();
                match id.parse::<ReasoningEffort>() {
                    Ok(effort) => {
                        self.draft.effort = Some(effort);
                        self.status.effort = effort;
                        self.sidebar.model.effort = Some(effort.as_str().to_owned());
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
            PickerKind::Sessions => {
                self.overlays.pop();
                self.view = View::Transcript;
                if self.running {
                    self.notice = Some("finish or cancel the running turn first".into());
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
                self.overlays.pop();
                match id.as_str() {
                    "approve" => Action::Mcp(McpRequest::Decide {
                        server,
                        decision: TrustDecision::Approved,
                    }),
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

    /// Selecting a provider clears the model, because a model belongs to
    /// the profile it was chosen under.
    pub fn select_profile(&mut self, profile: &str) {
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
        if self.session_pinned && self.status.model != model && !self.status.model.is_empty() {
            self.overlays.push(Overlay::Notice(Notice {
                title: "Changing the model needs a new session".into(),
                body: format!(
                    "This session is pinned to {} on {}. Gritt cannot move its stored \
                     transcript and continuation state to {model}. Run /new to start a \
                     session on the new model; this one stays in /sessions and your \
                     composer draft is kept.",
                    self.status.model, self.status.profile
                ),
                is_error: false,
            }));
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
            self.sidebar.model.effort = Some("auto".into());
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
                // Nothing is open here: Escape cancels a running turn, or
                // the asynchronous work the loading line is showing.
                if self.running || self.loading.is_some() {
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
            (KeyCode::BackTab, _) => {
                self.focus = self.next_focus(false);
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
                self.assistant_open = false;
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
        self.running = false;
        self.pending = None;
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
        self.sidebar.integrations.mcp_owner = self.connector.map(|id| id.as_str().to_owned());
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
    pub fn setup_outcome(&mut self, message: String, close: bool) {
        let position = self
            .overlays
            .iter()
            .rposition(|overlay| matches!(overlay, Overlay::Setup(_)));
        let Some(position) = position else { return };
        if close {
            self.overlays.remove(position);
            self.notice = Some(message);
            return;
        }
        if let Overlay::Setup(form) = &mut self.overlays[position] {
            form.saving = false;
            form.outcome = Some(message);
        }
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
        }));
    }

    pub fn show_draft_warnings(&mut self, warnings: &[DraftWarning]) {
        for warning in warnings {
            let text = match warning {
                DraftWarning::ModelNotInCatalog { profile, model } => {
                    format!("{model} is not in {profile}'s list; its capabilities are unreported")
                }
                DraftWarning::DeprecatedModelRemapped { from, to } => {
                    format!("{from} is deprecated; using {to}")
                }
            };
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
                self.sidebar.model.effort = Some(effort.as_str().to_owned());
                self.draft.effort = Some(effort);
            }
            None => self.sidebar.model.effort = None,
        }
    }

    /// Rebuilds the rows of the picker on screen from current state, so
    /// an asynchronous result fills in the list the user is looking at.
    fn refresh_open_picker(&mut self) {
        let Some(kind) = self.overlays.last().and_then(Overlay::picker_kind) else {
            return;
        };
        let rows = match kind {
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
        let rows = rows.rows().to_vec();
        if let Some(Overlay::Picker { picker, .. }) = self.overlays.last_mut() {
            picker.replace_rows(rows);
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
                self.sidebar.model.effort = Some(effort.as_str().to_owned());
                self.sidebar.model.managed_by_agent = false;
                self.connector = None;
                // A session with stored history is pinned to the provider
                // and model its transcript was produced under.
                self.draft.profile = Some(provider_profile.clone());
                self.draft.model = Some(model.clone());
                self.draft.effort = Some(*effort);
            }
            gritt_core::session::SessionKind::Connector { id } => {
                self.connector = Some(*id);
                self.status.profile = id.as_str().to_owned();
                self.status.model.clear();
                self.sidebar.model.backend = Some(id.as_str().to_owned());
                self.sidebar.model.model = None;
                // ADR-010: the connector owns these, so the sidebar says
                // so rather than showing Gritt's native values.
                self.sidebar.model.managed_by_agent = true;
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
            | Command::Plan
            | Command::Code
            | Command::New
    )
}

/// Settings the native provider owns. A connector session refuses these
/// because the external agent owns its own model, effort, and permissions.
fn is_native_setting(command: Command) -> bool {
    matches!(
        command,
        Command::Connect | Command::Models | Command::Effort
    )
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
    }
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
