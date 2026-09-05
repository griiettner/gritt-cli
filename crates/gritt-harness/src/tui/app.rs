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
use gritt_core::mcp::McpServerSnapshot;
use gritt_core::provider::{ModelInfo, Protocol, ReasoningEffort};
use gritt_core::session::{Phase, Session, SessionId};
use ratatui::text::Line;

use super::command::{self, Command, Parsed};
use super::composer::Composer;
use super::picker::{ListStatus, Picker, PickerRow};
use super::sidebar::{self, SidebarModel, SidebarPlacement};
use super::theme::Theme;
use crate::draft::{CatalogState, SessionDraft};
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

/// What the runtime should do after a key.
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
    /// The outcome line shown after an attempted save.
    pub outcome: Option<String>,
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
            outcome: None,
        }
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
    /// whether focus may rest on it.
    pub fn sidebar_column_visible(&self) -> bool {
        self.sidebar_fits_beside() && self.sidebar_enabled
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
        if self.focus == Focus::Sidebar && !self.sidebar_column_visible() {
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
                self.push(
                    EntryKind::Tool,
                    format!("-> {}", describe_call(&call.name, &call.arguments)),
                );
            }
            EventKind::ToolResult { result } => {
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
                let mut row = PickerRow::new(server.name.clone(), server.name.clone())
                    .badge(word.to_owned())
                    .detail(if server.state.is_ready() {
                        format!("{} tools", server.tool_count)
                    } else {
                        String::new()
                    })
                    .note(server.state.explain());
                if !server.state.is_ready() {
                    row = row.unavailable(word.to_owned());
                }
                row
            })
            .collect();
        Picker::new("MCP servers", rows)
            .with_hint("Every configured entry is listed, whatever its state")
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
        };
        self.overlays.push(Overlay::Picker { kind, picker });
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
                Action::None
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
                // view is cleared and the composer draft is kept.
                self.entries.clear();
                self.revision += 1;
                self.top = 0;
                self.follow = true;
                self.new_output = false;
                self.session_pinned = false;
                self.sidebar.reset();
                self.notice = Some("new draft; the previous session is still listed".into());
                Action::None
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
                self.open_picker(PickerKind::Mcp);
                Action::None
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
            KeyCode::Enter => {
                if form.field() == SetupField::Secret {
                    // Fixture mode: nothing is written. The real writes
                    // are the injected `ProviderSetup` service.
                    let name = form.name.text().to_owned();
                    form.outcome = Some(format!(
                        "fixture: nothing was written. A real run would save `{name}` \
                         to the user config and the key to the keychain."
                    ));
                } else {
                    form.next_field();
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
                if let Some(profile) = id.strip_prefix("profile:") {
                    self.select_profile(profile);
                    self.open_picker(PickerKind::Models);
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
                if let Ok(effort) = id.parse::<ReasoningEffort>() {
                    self.draft.effort = Some(effort);
                    self.status.effort = effort;
                    self.sidebar.model.effort = Some(effort.as_str().to_owned());
                }
                Action::None
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
            PickerKind::Mcp => Action::None,
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
            // The previous profile's list is not this profile's list.
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
                // Nothing is open here: Escape cancels a running turn.
                if self.running {
                    Action::Cancel
                } else {
                    self.notice = None;
                    Action::None
                }
            }
            // Focus only stops on panes that are actually on screen, so
            // Tab can never park the keyboard on a hidden sidebar.
            (KeyCode::Tab, _) => {
                self.focus = match self.focus {
                    Focus::Composer => Focus::Transcript,
                    Focus::Transcript if self.sidebar_column_visible() => Focus::Sidebar,
                    Focus::Transcript | Focus::Sidebar => Focus::Composer,
                };
                Action::None
            }
            (KeyCode::BackTab, _) => {
                self.focus = match self.focus {
                    Focus::Composer if self.sidebar_column_visible() => Focus::Sidebar,
                    Focus::Composer => Focus::Transcript,
                    Focus::Transcript => Focus::Composer,
                    Focus::Sidebar => Focus::Transcript,
                };
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

    pub fn set_session(&mut self, session: &Session) {
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
            }
            gritt_core::session::SessionKind::Connector { id } => {
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
