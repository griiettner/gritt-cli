//! Owns the terminal and the event loop. The agent runs turns in a task
//! and talks to the loop over channels; keys come from a blocking reader
//! thread. The terminal is restored on exit and on panic.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TerminalEvent, KeyEventKind,
};
use crossterm::execute;
use gritt_core::connector::AuthState;
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event};
use gritt_core::mcp::McpServerSnapshot;
use gritt_core::session::{BoxFuture, Session, SessionStore};
use gritt_core::{Error, Result};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::app::{
    Action, AgentSummary, App, EntryKind, McpRequest, PendingApproval, StatusBar, Work,
};
use super::fixture::{self, FixtureScreen};
use super::render::draw;
use super::theme::Theme;
use crate::agent::{CancelHandle, TurnOutcome, Ui};
use crate::changes::{ChangedFiles, FileDiff, WorkspaceChanges};
use crate::control::{ControlPlane, DraftOpen, ProfileCatalog};
use crate::draft::SessionDraft;
use crate::driver::Driver;
use crate::policy::Decision;
use crate::CancellationToken;

enum UiMsg {
    Event {
        generation: u64,
        event: Event,
    },
    Approval {
        generation: u64,
        pending: PendingApproval,
        responder: oneshot::Sender<ApprovalDecision>,
    },
    Finished {
        generation: u64,
        agent: Box<dyn Driver>,
        result: Result<TurnOutcome>,
    },
    /// A profile's model list, stamped with the selection token it was
    /// requested under. A token that no longer matches is a late result
    /// for a provider the user has already left.
    Catalog {
        /// The request this answers. `selection` says which provider was
        /// chosen; this says which load was asked for, which is what
        /// decides whether it still owns the slot.
        operation: u64,
        selection: u64,
        profile: String,
        result: Result<ProfileCatalog>,
    },
    Sessions {
        operation: u64,
        sessions: Vec<Session>,
    },
    /// The installed agents, probed off the terminal event path because
    /// probing runs external processes.
    Agents(Vec<AgentSummary>),
    /// A draft that was opened or refused. `generation` is the sidebar
    /// generation the request started under; a driver for a session the
    /// user has already left is closed instead of adopted.
    Opened {
        /// The transition this answers. Only the transition the loop is
        /// still waiting for may be applied; anything else was superseded
        /// or cancelled and its driver is closed instead of adopted.
        operation: u64,
        generation: u64,
        result: Result<DraftOpen>,
    },
    Changes {
        generation: u64,
        changes: ChangedFiles,
    },
    Diff {
        operation: u64,
        diff: FileDiff,
    },
    /// Live MCP state, from the runtime's subscription rather than a poll.
    Mcp(Vec<McpServerSnapshot>),
    McpOutcome {
        /// The action this answers. A completion from an action that has
        /// already been superseded must not clear the current one's token
        /// or its loading line.
        operation: u64,
        result: Result<()>,
    },
    /// The workspace MCP runtime failed to open at startup, before any
    /// entry could be published. Without this the subscription delivers an
    /// empty list and the interface says no servers are configured, which
    /// is indistinguishable from a workspace that really has none.
    McpStartupFailed(String),
    /// A redacted definition for a first-use launch approval.
    McpApproval {
        operation: u64,
        server: String,
        definition: String,
    },
    /// Credential availability, computed off the event path because
    /// resolving one reaches the keychain.
    ///
    /// `generation` is the configuration this was computed against.
    /// Resolving a credential blocks for as long as the operating system
    /// wants, and setup can install a whole new configuration while it
    /// waits, so a result that outlived its own configuration is refused
    /// rather than applied (TKT-0020).
    Profiles {
        generation: u64,
        profiles: Vec<crate::setup::ProfileSummary>,
    },
    /// A plane rebuilt around re-read configuration, with the summaries
    /// that were computed against it, both off the event path.
    Reloaded {
        plane: Box<ControlPlane>,
        profiles: Vec<crate::setup::ProfileSummary>,
    },
    Setup {
        operation: u64,
        message: String,
        /// True when the write succeeded and the form should close.
        close: bool,
    },
}

struct ChannelUi {
    tx: mpsc::UnboundedSender<UiMsg>,
    /// The session generation this turn started under. Every message it
    /// sends carries it, so output from a session the user has left is
    /// refused rather than populating the one now on screen.
    generation: u64,
}

impl Ui for ChannelUi {
    fn event(&mut self, event: &Event) {
        let _ = self.tx.send(UiMsg::Event {
            generation: self.generation,
            event: event.clone(),
        });
    }

    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        decision: &'a Decision,
        preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        let (responder, receiver) = oneshot::channel();
        let pending = PendingApproval {
            request: request.clone(),
            decision: decision.clone(),
            preview: preview.map(str::to_owned),
        };
        let _ = self.tx.send(UiMsg::Approval {
            generation: self.generation,
            pending,
            responder,
        });
        Box::pin(async move { receiver.await.unwrap_or(ApprovalDecision::Denied) })
    }
}

/// The redraw cap. The plan asks for smooth updates at 30 fps rather than a
/// frame per event, which is wasted work when a stream produces faster than
/// a terminal can usefully show.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// How many queued messages one iteration may handle before drawing again.
///
/// Draining without a ceiling would let a fast producer hold the loop and
/// starve the key reader. This bounds the work between frames, not the
/// queue: whatever is left is handled on the next iteration.
const MAX_COALESCED_MESSAGES: usize = 4_096;

/// Reads terminal events on a thread until `stop` is set.
fn spawn_key_reader(stop: Arc<AtomicBool>) -> mpsc::UnboundedReceiver<TerminalEvent> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match event::poll(Duration::from_millis(100)) {
                Ok(true) => match event::read() {
                    Ok(event) => {
                        if tx.send(event).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
    rx
}

/// Enters the alternate screen and installs the panic hook that leaves it.
/// Bracketed paste is enabled here so a paste arrives as one event and
/// never as a stream of keys that could look like a command.
fn enter() -> Result<ratatui::DefaultTerminal> {
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
        ratatui::restore();
        previous_hook(info);
    }));
    let terminal = ratatui::try_init()
        .map_err(|error| Error::config(format!("cannot start the full-screen mode: {error}")))?;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    Ok(terminal)
}

fn leave() {
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    let _ = std::panic::take_hook();
}

/// The theme for this process, from `NO_COLOR` and `GRITT_THEME`.
fn theme_from_env() -> Theme {
    Theme::from_env(std::env::vars())
}

/// Runs the reviewable prototype: fixture state, no control plane, no
/// session, and no MCP server. The interface labels the run `fixture`.
pub async fn run_fixture(screen: FixtureScreen) -> Result<()> {
    let mut terminal = enter()?;
    let result = fixture_loop(screen, &mut terminal).await;
    leave();
    result
}

async fn fixture_loop(
    screen: FixtureScreen,
    terminal: &mut ratatui::DefaultTerminal,
) -> Result<()> {
    let mut app = fixture::screen(screen, theme_from_env());
    let stop = Arc::new(AtomicBool::new(false));
    let mut keys = spawn_key_reader(Arc::clone(&stop));
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let mut dirty = true;
    loop {
        // Only a wakeup that could have changed the screen draws one. The
        // tick is not one: nothing in the interface is drawn from a clock,
        // so a tick with no input behind it would redraw the same frame.
        if dirty {
            terminal
                .draw(|frame| draw(frame, &app))
                .map_err(|error| Error::config(format!("draw failed: {error}")))?;
            dirty = false;
        }
        let action = tokio::select! {
            Some(event) = keys.recv() => {
                dirty = true;
                match event {
                    TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
                    TerminalEvent::Paste(text) => {
                        app.on_paste(&text);
                        Action::None
                    }
                    TerminalEvent::Resize(columns, rows) => {
                        app.on_resize(columns, rows);
                        Action::None
                    }
                    _ => Action::None,
                }
            }
            _ = tick.tick() => Action::None,
        };
        match action {
            // A fixture run has no session, so a prompt is answered
            // locally and says so rather than pretending to stream.
            Action::Submit(prompt) => {
                app.push(
                    super::app::EntryKind::System,
                    format!("fixture: `{prompt}` was not sent. No session is open in this mode."),
                );
                app.running = false;
            }
            Action::Quit => break,
            // A fixture run has no setup service, and the interface says
            // so rather than implying a write happened.
            Action::SaveProfile => {
                app.setup_outcome(
                    "fixture: nothing was written. A real run saves the profile to the \
                     configuration and the key to the keychain."
                        .to_owned(),
                    false,
                );
            }
            _ => {}
        }
        if app.quit {
            break;
        }
    }
    stop.store(true, Ordering::SeqCst);
    Ok(())
}

/// Everything the loop shares with the tasks it spawns.
///
/// The plane sits behind an `Arc` so a task can hold it while the loop
/// keeps drawing, and so a configuration reload replaces the handle
/// instead of mutating a value a task is already using.
/// A session change that has been asked for and not yet answered.
///
/// The transition is *reserved*: the old driver is held here rather than
/// left idle, so a prompt submitted while it is in flight cannot start a
/// turn on the session being left. A failure puts the old driver back.
struct PendingOpen {
    operation: u64,
    /// The prompt that triggered a lazy open, run once the session exists
    /// and returned to the composer if it never does.
    prompt: Option<String>,
    /// The driver being replaced, restored if the open fails.
    previous: Option<Box<dyn Driver>>,
}

struct Runtime {
    plane: Arc<ControlPlane>,
    changes: Arc<WorkspaceChanges>,
    tx: mpsc::UnboundedSender<UiMsg>,
    /// The one cancellable background request. Starting another supersedes
    /// it, and Escape aborts it, so the loading line always describes work
    /// that is really running.
    work: Option<JoinHandle<()>>,
    /// What `work` is: its kind and the request it belongs to.
    ///
    /// The identity is the pair, not the kind. Matching on the kind alone
    /// let an older queued response retire a newer request of the same
    /// kind — discarding the newer one's cancellation handle while it was
    /// still running.
    work_active: Option<ActiveWork>,
    /// The task behind a session change, kept apart from `work`.
    ///
    /// A session change owns a reservation: the driver being replaced is
    /// held by it, and the interface refuses prompts until it resolves.
    /// Sharing the ordinary request slot let any later request — a session
    /// list, a catalog, a diff — abort the open while leaving the
    /// reservation behind, which blocked the interface with nothing left
    /// to unblock it.
    open_work: Option<JoinHandle<()>>,
    /// Monotonic id for cancellable operations whose result must be
    /// matched to the request that is still wanted.
    operations: u64,
    /// The session change in flight, if any.
    pending_open: Option<PendingOpen>,
    /// How many configurations have been installed. Starts at zero and
    /// moves only when a reload replaces the plane, so a profile lookup
    /// can say which configuration it answered for.
    config_generation: u64,
    /// The MCP operation in flight, identified so a late completion from
    /// a previous one cannot clear the current one's state.
    ///
    /// MCP work is never aborted. Its future owns child processes and
    /// launch slots, and dropping it would skip the shutdown that releases
    /// them; the token asks it to stop and its own cleanup runs.
    mcp: Option<McpOperation>,
}

/// One MCP action in flight.
struct McpOperation {
    id: u64,
    cancel: CancellationToken,
}

/// The ordinary background request that owns the shared slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveWork {
    kind: Work,
    operation: u64,
}

impl Runtime {
    /// Reserves the next operation id.
    fn next_operation(&mut self) -> u64 {
        self.operations += 1;
        self.operations
    }

    /// Replaces the session-change task. Only a caller that has already
    /// installed the matching reservation uses this.
    fn spawn_open(&mut self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        if let Some(previous) = self.open_work.take() {
            previous.abort();
        }
        self.open_work = Some(tokio::spawn(future));
    }

    /// Replaces the background request, aborting whatever it superseded.
    /// Answers whether this completion is the request that owns the slot,
    /// and retires it when it is.
    ///
    /// One decision covers both halves on purpose: retiring the ownership
    /// and applying the result must agree, and every handler asks this
    /// before it does either. `false` means the completion is stale —
    /// superseded, or cancelled — so nothing is retired and nothing is
    /// shown.
    ///
    /// The identity is the kind *and* the operation. On kind alone, an
    /// older queued response retired the newer request of the same kind
    /// and threw away its cancellation handle, leaving work that Escape
    /// and `/new` could no longer stop.
    #[must_use]
    fn finish_work(&mut self, app: &mut App, kind: Work, operation: u64) -> bool {
        let active = ActiveWork { kind, operation };
        if self.work_active != Some(active) {
            return false;
        }
        app.end_work(kind);
        self.work_active = None;
        self.work = None;
        true
    }

    /// Gives up the session change in flight and clears everything that
    /// says one is happening.
    ///
    /// The reservation, its task, the transition flag, and the label are
    /// released together. Releasing them separately is what left
    /// "Opening…" on screen after a cancelled open, masking every later
    /// request behind a label for work that had stopped.
    fn release_pending_open(&mut self, app: &mut App) -> Option<PendingOpen> {
        if let Some(work) = self.open_work.take() {
            work.abort();
        }
        app.end_work(Work::Open);
        app.session_transition = false;
        self.pending_open.take()
    }

    /// Replaces the ordinary background request.
    ///
    /// Only this slot is touched: a session change lives in `open_work`
    /// and an MCP action is detached with its own token, so neither can be
    /// dropped by an unrelated request starting.
    fn spawn(
        &mut self,
        app: &mut App,
        kind: Work,
        operation: u64,
        label: impl Into<String>,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        if let Some(previous) = self.work_active.take() {
            app.end_work(previous.kind);
        }
        if let Some(previous) = self.work.take() {
            previous.abort();
        }
        self.work_active = Some(ActiveWork { kind, operation });
        app.begin_work(kind, label);
        self.work = Some(tokio::spawn(future));
    }

    /// Background work that is not the cancellable request: MCP delivery,
    /// change scans, and the agent probe. These are additive and never
    /// supersede the user's current request.
    fn spawn_detached(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        tokio::spawn(future);
    }

    /// Stops everything cancellable and reports whether there was any.
    ///
    /// Every kind is ended, not just the one whose label happens to be on
    /// screen: Escape means "stop what is running", and an MCP restart
    /// must stay cancellable even after an unrelated catalog response has
    /// arrived. MCP work is signalled rather than aborted, because its
    /// future owns a child process and a launch slot and aborting it would
    /// drop both before the shutdown that releases them.
    fn cancel_work(&mut self, app: &mut App) -> bool {
        let mut cancelled = false;
        if let Some(previous) = self.mcp.take() {
            previous.cancel.cancel();
            app.end_work(Work::Mcp);
            cancelled = true;
        }
        if let Some(previous) = self.work_active.take() {
            if let Some(work) = self.work.take() {
                work.abort();
            }
            app.end_work(previous.kind);
            cancelled = true;
        }
        cancelled
    }

    /// Starts an MCP action, signalling the one it replaces.
    ///
    /// Each action carries its own id and token, so a completion that
    /// arrives after another action started cannot clear the newer one's
    /// state, and the older one is always told to stop rather than being
    /// forgotten with its child still running.
    fn begin_mcp(&mut self) -> (u64, CancellationToken) {
        if let Some(previous) = self.mcp.take() {
            previous.cancel.cancel();
        }
        let id = self.next_operation();
        let cancel = CancellationToken::new();
        self.mcp = Some(McpOperation {
            id,
            cancel: cancel.clone(),
        });
        (id, cancel)
    }
}

/// The installed agents as the connection dialog shows them: what the
/// connector reports about itself, and nothing Gritt guessed.
async fn agent_summaries(plane: &ControlPlane) -> Vec<AgentSummary> {
    let mut out = Vec::new();
    for (id, info) in plane.infos().await {
        if id == gritt_core::connector::ConnectorId::Native {
            continue;
        }
        let (installed, version, authenticated) = match info {
            Ok(info) => (
                info.auth != AuthState::NotInstalled,
                info.version.clone(),
                match info.auth {
                    AuthState::Authenticated => Some(true),
                    AuthState::Unauthenticated => Some(false),
                    AuthState::NotInstalled | AuthState::Unknown => None,
                },
            ),
            // A probe that failed is reported as not installed with no
            // version, which is what Gritt honestly knows about it.
            Err(_) => (false, None, None),
        };
        out.push(AgentSummary {
            id,
            name: id.as_str().to_owned(),
            installed,
            version,
            authenticated,
        });
    }
    out
}

/// Takes a driver as the live session: history, identity, effort, and the
/// catalog figures behind usage and cost.
async fn adopt(app: &mut App, plane: &ControlPlane, agent: &dyn Driver) -> Result<()> {
    app.entries.clear();
    // The sidebar moves to a new generation here, so a scan or a catalog
    // load still in flight for the previous session cannot land on this one.
    app.sidebar.reset();
    app.status.usage = Default::default();
    let history = plane.builder.store.read_events(&agent.session().id).await?;
    app.load_history(&history);
    app.set_session(agent.session());
    let info = agent.info();
    app.status.profile = info.backend;
    app.status.model = info.detail;
    app.set_effective_effort(agent.effort());
    // A session that already has output is pinned to the provider and
    // model that produced it.
    app.session_pinned = !app.entries.is_empty();
    let model = plane
        .builder
        .catalog
        .model(&app.status.profile, &app.status.model);
    app.set_model_facts(model.as_ref());
    app.apply_mcp(match plane.builder.mcp() {
        Some(mcp) => mcp.snapshots().await,
        None => Vec::new(),
    });
    Ok(())
}

/// Runs the full-screen mode until the user quits.
///
/// `agent` is `None` for the lazy path: `gritt tui` with no named session
/// opens on a draft and creates the session when the first prompt is
/// submitted. Native and connector sessions share the loop, the approval
/// view, and the transcript.
/// `start_mcp` is the workspace MCP runtime to open, when this run is on
/// the native path. It is opened in the background rather than awaited, so
/// a server that never answers `initialize` cannot hold the first frame,
/// and a failure to open it is reported in the interface instead of on
/// stderr, which the alternate screen owns (TKT-0020).
pub async fn run_tui(
    plane: ControlPlane,
    agent: Option<Box<dyn Driver>>,
    draft: SessionDraft,
    start_mcp: Option<Arc<crate::mcp::McpRuntime>>,
) -> Result<()> {
    let theme = theme_from_env();
    let mut terminal = enter()?;
    let result = event_loop(plane, agent, draft, &mut terminal, theme, start_mcp).await;
    leave();
    result
}

async fn event_loop(
    plane: ControlPlane,
    agent: Option<Box<dyn Driver>>,
    draft: SessionDraft,
    terminal: &mut ratatui::DefaultTerminal,
    theme: Theme,
    start_mcp: Option<Arc<crate::mcp::McpRuntime>>,
) -> Result<()> {
    let mut app = App::new(StatusBar::default(), theme);
    app.draft = draft;
    app.status.workspace = plane.builder.workspace_root().display().to_string();
    app.sidebar.session.workspace = Some(app.status.workspace.clone());
    // Profile summaries are deliberately absent here. Resolving one
    // reaches `keys.key()` and therefore the keychain, which can block for
    // as long as the operating system wants; doing it before the first
    // draw would hold the entered terminal blank and deaf. `load_profiles`
    // fills them in from a blocking worker and the dialog updates when
    // they arrive.
    //
    // The flags above the configured defaults are the selection the home
    // screen shows before a session exists. It is a choice, not an open
    // connection, which is why nothing here opens anything.
    seed_draft(&mut app, &plane);
    let (ui_tx, ui_rx) = mpsc::unbounded_channel::<UiMsg>();
    let runtime = Runtime {
        plane: Arc::new(plane),
        changes: Arc::new(WorkspaceChanges::new(app.status.workspace.clone())),
        tx: ui_tx.clone(),
        work: None,
        work_active: None,
        open_work: None,
        operations: 0,
        pending_open: None,
        config_generation: 0,
        mcp: None,
    };
    let mut idle_agent: Option<Box<dyn Driver>> = None;
    if let Some(agent) = agent {
        adopt(&mut app, &runtime.plane, agent.as_ref()).await?;
        idle_agent = Some(agent);
    }
    // The baseline is taken before anything else can change the workspace,
    // so a change already present is labelled pre-existing rather than
    // attributed to this session.
    scan_changes(&runtime, app.sidebar.generation);
    probe_agents(&runtime);
    load_profiles(&runtime);
    subscribe_mcp(&runtime);
    open_mcp(&runtime, start_mcp);
    let stop = Arc::new(AtomicBool::new(false));
    let keys = spawn_key_reader(Arc::clone(&stop));
    let mut scheduler = Scheduler::new(app, runtime, idle_agent, ui_rx, keys);
    while !scheduler.app.quit {
        scheduler.step(terminal).await?;
    }
    stop.store(true, Ordering::SeqCst);
    scheduler.shutdown();
    Ok(())
}

/// What one scheduler step did. The benchmark reads these; the loop
/// ignores them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// A frame was put on the terminal.
    pub drew: bool,
    /// When that frame finished being written.
    ///
    /// This is when the user could see it. The step goes on to wait in
    /// `select!`, handle an action, and drain the queue, all of which can
    /// take longer than the frame did, so a caller measuring
    /// input-to-frame latency must use this instant and not the one after
    /// `step` returns (TKT-0020).
    pub drew_at: Option<std::time::Instant>,
    /// Messages taken off the queue and handled, including the drain.
    pub messages: usize,
    /// A terminal event was taken and handled.
    pub input: bool,
}

/// The loop, minus the terminal it draws to and the sources it reads from.
///
/// `event_loop` is a `while !quit { step() }` around this, and the
/// responsiveness benchmark drives the same `step` against a `TestBackend`
/// with an injected input channel. There is deliberately no second copy of
/// the scheduling rules: a benchmark that reimplements draw timing, drain
/// order, or input priority measures the reimplementation (TKT-0020).
struct Scheduler {
    app: App,
    runtime: Runtime,
    idle: Option<Box<dyn Driver>>,
    handle: Option<CancelHandle>,
    responder: Option<oneshot::Sender<ApprovalDecision>>,
    ui_rx: mpsc::UnboundedReceiver<UiMsg>,
    keys: mpsc::UnboundedReceiver<TerminalEvent>,
    tick: tokio::time::Interval,
    /// Whether the screen can differ from the last frame drawn. Every
    /// wakeup that carries input or a message sets it; the tick does not,
    /// because nothing in the interface is drawn from a clock. An idle
    /// session therefore draws nothing at all.
    dirty: bool,
    last_draw: tokio::time::Instant,
    /// The approval whose frame has been drawn, by install identity.
    ///
    /// The frame cap must never let a decision key answer a prompt the
    /// user has not seen. `y` approves a tool call or an MCP launch the
    /// moment it reaches the reducer, so a capped frame plus a keystroke
    /// already on its way is an approval nobody read.
    ///
    /// Identity rather than presence, because one iteration can answer A
    /// and install B during its drain: `pending` is `Some` before and
    /// after, so a flag would carry A's visibility onto B and let the next
    /// keystroke answer a prompt that was never drawn.
    drawn_approval: Option<u64>,
}

impl Scheduler {
    fn new(
        app: App,
        runtime: Runtime,
        idle: Option<Box<dyn Driver>>,
        ui_rx: mpsc::UnboundedReceiver<UiMsg>,
        keys: mpsc::UnboundedReceiver<TerminalEvent>,
    ) -> Self {
        Self {
            app,
            runtime,
            idle,
            handle: None,
            responder: None,
            ui_rx,
            keys,
            tick: tokio::time::interval(Duration::from_millis(50)),
            dirty: true,
            last_draw: tokio::time::Instant::now() - FRAME_INTERVAL,
            drawn_approval: None,
        }
    }

    /// True when a request is on `pending` whose own frame has not been
    /// drawn. Compares identity, so a request installed in place of an
    /// answered one is undrawn even though `pending` never became `None`.
    fn undrawn_approval(&self) -> bool {
        match self.app.pending_install() {
            Some(install) => self.drawn_approval != Some(install),
            None => false,
        }
    }

    fn shutdown(&mut self) {
        self.runtime.cancel_work(&mut self.app);
        if let Some(handle) = &self.handle {
            handle.cancel();
        }
    }

    /// One iteration: draw what is owed, take one wakeup, handle it, then
    /// handle everything else already waiting.
    async fn step<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut ratatui::Terminal<B>,
    ) -> Result<Step> {
        let mut outcome = Step::default();

        // The frame cap yields to an approval that has not been drawn.
        // Every other frame waits its turn; this one is what the next
        // keystroke answers.
        let due = self.last_draw + FRAME_INTERVAL;
        let owed = self.undrawn_approval();
        if self.dirty && (owed || tokio::time::Instant::now() >= due) {
            terminal
                .draw(|frame| draw(frame, &self.app))
                .map_err(|error| Error::config(format!("draw failed: {error}")))?;
            self.dirty = false;
            self.last_draw = tokio::time::Instant::now();
            outcome.drew = true;
            // Wall-clock, taken here rather than by the caller: everything
            // after this point in the step happens once the frame is
            // already on screen.
            outcome.drew_at = Some(std::time::Instant::now());
            self.drawn_approval = self.app.pending_install();
        }

        let action = tokio::select! {
            // Input first, deliberately. Under a stream the message branch
            // is always ready, and an unbiased `select!` would put a
            // keypress behind a random share of the backlog. A person
            // cannot type fast enough to starve the queue, and the drain
            // below runs on every iteration regardless.
            biased;
            Some(event) = self.keys.recv() => {
                self.dirty = true;
                outcome.input = true;
                if self.undrawn_approval() {
                    // Unreachable while the draw above precedes this
                    // select, and kept so that it stays unreachable: a
                    // key cannot answer a prompt that has never been on
                    // screen, whatever later reordering does to the draw.
                    Action::None
                } else {
                    match event {
                        TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => {
                            self.app.on_key(key)
                        }
                        TerminalEvent::Paste(text) => {
                            self.app.on_paste(&text);
                            Action::None
                        }
                        TerminalEvent::Resize(columns, rows) => {
                            // Placement depends on the width; a drawer or
                            // a focused sidebar may no longer exist.
                            self.app.on_resize(columns, rows);
                            Action::None
                        }
                        _ => Action::None,
                    }
                }
            }
            // A frame that is owed but capped. Without this branch the
            // loop would sit in `recv` until the next message or tick and
            // leave a stale screen up for as long as that took.
            _ = tokio::time::sleep_until(due), if self.dirty => Action::None,
            Some(msg) = self.ui_rx.recv() => {
                self.dirty = true;
                outcome.messages += 1;
                on_message(
                    &mut self.app,
                    &mut self.runtime,
                    &mut self.idle,
                    &mut self.handle,
                    &mut self.responder,
                    msg,
                )
                .await?
            }
            _ = self.tick.tick() => Action::None,
        };
        on_action(
            &mut self.app,
            &mut self.runtime,
            &mut self.idle,
            &mut self.handle,
            &mut self.responder,
            action,
        )
        .await?;

        // Coalescing. One message per frame cannot keep up with a stream:
        // a turn emits a text delta per token through this same channel,
        // so drawing after each one caps throughput at the frame rate and
        // lets the queue grow without bound. Measured before this landed,
        // the loop drained 69 messages a second against 1,000 produced and
        // finished holding 8,862 (TKT-0020).
        //
        // Everything already waiting is handled before the next draw, in
        // order, by the same handler. Nothing is dropped: no approval, tool
        // result, or final event is skipped, only the intermediate frames
        // between them. The batch is bounded so a fast producer cannot hold
        // the loop and starve the key reader.
        let mut coalesced = 0usize;
        while coalesced < MAX_COALESCED_MESSAGES && !self.app.quit {
            let Ok(msg) = self.ui_rx.try_recv() else {
                break;
            };
            coalesced += 1;
            outcome.messages += 1;
            self.dirty = true;
            let action = on_message(
                &mut self.app,
                &mut self.runtime,
                &mut self.idle,
                &mut self.handle,
                &mut self.responder,
                msg,
            )
            .await?;
            on_action(
                &mut self.app,
                &mut self.runtime,
                &mut self.idle,
                &mut self.handle,
                &mut self.responder,
                action,
            )
            .await?;
        }
        Ok(outcome)
    }
}

/// Fills the draft's empty fields from the configured defaults and shows
/// the result. With nothing configured the fields stay empty and the home
/// screen keeps saying `/connect` is where to start.
fn seed_draft(app: &mut App, plane: &ControlPlane) {
    let config = &plane.builder.config;
    if app.draft.profile.is_none() {
        app.draft.profile = config.default_profile.clone();
    }
    if app.draft.model.is_none() {
        app.draft.model = config.default_model.clone();
    }
    app.status.profile = app.draft.profile.clone().unwrap_or_default();
    app.status.model = app.draft.model.clone().unwrap_or_default();
    app.status.effort = app.draft.effort.unwrap_or_default();
    app.status.phase = match app
        .draft
        .phase
        .unwrap_or(gritt_core::session::Phase::Planning)
    {
        gritt_core::session::Phase::Planning => "planning".into(),
        gritt_core::session::Phase::Coding => "coding".into(),
    };
    if let Some(profile) = &app.draft.profile {
        app.sidebar.model.backend = Some(profile.clone());
    }
    app.sidebar.model.model = app.draft.model.clone();
    app.sidebar.session.phase = Some(app.status.phase.clone());
    if let (Some(profile), Some(model)) = (&app.draft.profile, &app.draft.model) {
        let info = plane.builder.catalog.model(profile, model);
        app.set_model_facts(info.as_ref());
    }
}

/// Rescans the workspace under the current sidebar generation.
fn scan_changes(runtime: &Runtime, generation: u64) {
    record_and_scan(runtime, generation, Vec::new());
}

/// Records the writes a turn observed and rescans, entirely in the
/// background.
///
/// Nothing here is awaited by a handler. The generation is captured now
/// and travels with the result, so a scan for a session the user has left
/// is refused when it lands rather than being prevented by blocking the
/// loop until it finishes.
fn record_and_scan(runtime: &Runtime, generation: u64, writes: Vec<String>) {
    let changes = Arc::clone(&runtime.changes);
    let tx = runtime.tx.clone();
    runtime.spawn_detached(async move {
        for path in writes {
            changes.record_write(path).await;
        }
        changes.capture_baseline().await;
        let scanned = changes.scan().await;
        let _ = tx.send(UiMsg::Changes {
            generation,
            changes: scanned,
        });
    });
}

/// Resolves credential availability for every configured profile.
///
/// `keys.key()` reaches the keychain, which can block for as long as the
/// operating system wants, so this is blocking work and never runs on the
/// loop.
fn load_profiles(runtime: &Runtime) {
    let plane = Arc::clone(&runtime.plane);
    let tx = runtime.tx.clone();
    // Captured with the plane it is about to read. Whatever this returns
    // describes that configuration and no later one.
    let generation = runtime.config_generation;
    runtime.spawn_detached(async move {
        if let Ok(profiles) = tokio::task::spawn_blocking(move || plane.profile_summaries()).await {
            let _ = tx.send(UiMsg::Profiles {
                generation,
                profiles,
            });
        }
    });
}

fn probe_agents(runtime: &Runtime) {
    let plane = Arc::clone(&runtime.plane);
    let tx = runtime.tx.clone();
    runtime.spawn_detached(async move {
        let _ = tx.send(UiMsg::Agents(agent_summaries(&plane).await));
    });
}

/// Starts the workspace MCP runtime without blocking the first frame.
///
/// Approved servers launch, and every state change arrives through the
/// subscription. A failure to read or parse `.mcp.json` happens before any
/// entry is published, so it would otherwise be invisible: the subscription
/// would deliver an empty list and the sidebar would say no servers are
/// configured. That case becomes a message instead.
fn open_mcp(runtime: &Runtime, mcp: Option<Arc<crate::mcp::McpRuntime>>) {
    let Some(mcp) = mcp else { return };
    let tx = runtime.tx.clone();
    runtime.spawn_detached(async move {
        if let Err(error) = mcp.open(&CancellationToken::new()).await {
            let _ = tx.send(UiMsg::McpStartupFailed(error.message));
        }
    });
}

/// Forwards MCP lifecycle changes into the loop.
///
/// The runtime publishes the whole snapshot list on every change, so this
/// is a subscription and not a poll: nothing here wakes up on a timer, and
/// a lagged receiver resynchronises on the next message.
fn subscribe_mcp(runtime: &Runtime) {
    let Some(mcp) = runtime.plane.builder.mcp().cloned() else {
        return;
    };
    let tx = runtime.tx.clone();
    let mut updates = mcp.subscribe();
    runtime.spawn_detached(async move {
        let _ = tx.send(UiMsg::Mcp(mcp.snapshots().await));
        loop {
            match updates.recv().await {
                Ok(snapshots) => {
                    if tx.send(UiMsg::Mcp(snapshots)).is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Every message is the whole truth, so the next one
                    // repairs the gap. Nothing is replayed.
                    continue;
                }
                Err(_) => break,
            }
        }
    });
}

/// Takes one message from a task or the agent loop. Every asynchronous
/// result is checked against the token it started under before it is
/// allowed to change what is on screen.
async fn on_message(
    app: &mut App,
    runtime: &mut Runtime,
    idle_agent: &mut Option<Box<dyn Driver>>,
    handle: &mut Option<CancelHandle>,
    responder: &mut Option<oneshot::Sender<ApprovalDecision>>,
    msg: UiMsg,
) -> Result<Action> {
    match msg {
        UiMsg::Event { generation, event } => {
            // Output from a session the user has left may not populate the
            // one on screen.
            if app.sidebar.accepts(generation) {
                app.on_event(&event);
            }
        }
        UiMsg::Approval {
            generation,
            pending,
            responder: sender,
        } => {
            if !app.sidebar.accepts(generation) {
                // The turn belongs to a session that is gone. Denying is
                // the only safe answer, and it is answered here rather
                // than shown, so no key can approve it later.
                let _ = sender.send(ApprovalDecision::Denied);
                return Ok(Action::None);
            }
            *responder = Some(sender);
            app.request_approval(pending);
        }
        UiMsg::Finished {
            generation,
            agent,
            result,
        } => {
            if !app.sidebar.accepts(generation) {
                // A turn that finished on a session the user has left.
                // Its driver is dropped rather than restored as the idle
                // one, which would have put the old session back.
                drop(agent);
                return Ok(Action::None);
            }
            app.running = false;
            *handle = None;
            if let Err(error) = result {
                app.push(EntryKind::Error, error.message);
            }
            // A turn can only pin the session it ran in.
            app.session_pinned = true;
            app.set_effective_effort(agent.effort());
            *idle_agent = Some(agent);
            // Tools may have written; the sidebar refreshes after a turn
            // without blocking the frame that reported it finished.
            //
            // Recording a write stats the file and a scan runs `git`, both
            // under the same bounded blocking pool. Awaiting either here
            // would stop drawing and key handling — including Escape —
            // whenever those workers were busy, which is exactly when a
            // turn has just finished writing files.
            record_and_scan(runtime, app.sidebar.generation, app.take_observed_writes());
            if let Some(mcp) = runtime.plane.builder.mcp().cloned() {
                let tx = runtime.tx.clone();
                runtime.spawn_detached(async move {
                    let snapshots = mcp.refresh(&crate::CancellationToken::new()).await;
                    let _ = tx.send(UiMsg::Mcp(snapshots));
                });
            }
        }
        UiMsg::Catalog {
            operation,
            selection,
            profile,
            result,
        } => {
            if !runtime.finish_work(app, Work::Catalog, operation) {
                return Ok(Action::None);
            }
            match result {
                Ok(catalog) => {
                    app.apply_catalog(selection, &profile, catalog.models, catalog.state);
                }
                Err(error) => {
                    app.catalog_failed(selection, &profile, error.message);
                }
            }
        }
        UiMsg::Sessions {
            operation,
            sessions,
        } => {
            if !runtime.finish_work(app, Work::Sessions, operation) {
                return Ok(Action::None);
            }
            app.load_sessions(sessions);
        }
        UiMsg::Agents(agents) => {
            app.agents = agents;
            // The connection dialog may already be open; its rows fill in
            // where the user is looking rather than after they close it.
            app.refresh_connection_picker();
        }
        UiMsg::Opened {
            operation,
            generation,
            result,
        } => {
            // Only the transition the loop is still waiting for may be
            // applied. A cancelled or superseded one has already given the
            // prompt back and put the previous driver where it belongs, so
            // this driver is closed instead of adopted.
            let matches = runtime
                .pending_open
                .as_ref()
                .is_some_and(|pending| pending.operation == operation);
            if !matches || !app.sidebar.accepts(generation) {
                if let Ok(DraftOpen::Opened { driver, .. }) = result {
                    drop(driver);
                }
                if matches {
                    // The reservation is still installed but its session
                    // is gone, so nothing else will ever release it. It
                    // goes now, with its label and its driver.
                    if let Some(pending) = runtime.release_pending_open(app) {
                        *idle_agent = pending.previous;
                    }
                    app.running = false;
                }
                return Ok(Action::None);
            }
            let pending = runtime.release_pending_open(app).expect("checked above");
            match result {
                Ok(DraftOpen::Opened {
                    driver, warnings, ..
                }) => {
                    // The driver being replaced goes away only now that
                    // its replacement exists.
                    drop(pending.previous);
                    adopt(app, &runtime.plane, driver.as_ref()).await?;
                    app.show_draft_warnings(&warnings);
                    *idle_agent = Some(driver);
                    scan_changes(runtime, app.sidebar.generation);
                    if let Some(prompt) = pending.prompt {
                        // The transcript entry was pushed on submit and the
                        // history reload cleared it; put it back with the
                        // turn that is about to run.
                        app.push(EntryKind::User, prompt.clone());
                        app.running = true;
                        return Ok(Action::Submit(prompt));
                    }
                }
                Ok(DraftOpen::Rejected { errors, .. }) => {
                    // The draft is kept: one field is wrong, not the run.
                    // The session that was open stays open.
                    *idle_agent = pending.previous;
                    if let Some(prompt) = pending.prompt {
                        app.undo_submission(&prompt);
                    }
                    app.show_draft_errors(&errors);
                }
                Err(error) => {
                    *idle_agent = pending.previous;
                    if let Some(prompt) = pending.prompt {
                        app.undo_submission(&prompt);
                    }
                    app.running = false;
                    app.push(EntryKind::Error, error.message);
                }
            }
        }
        UiMsg::Changes {
            generation,
            changes,
        } => {
            app.apply_changes(generation, changes);
        }
        UiMsg::Diff { operation, diff } => {
            // A diff that was superseded or cancelled may not open an
            // overlay: by now the reader has moved on, possibly to another
            // session entirely.
            if !runtime.finish_work(app, Work::Diff, operation) {
                return Ok(Action::None);
            }
            app.show_file_diff(diff);
        }
        UiMsg::Mcp(snapshots) => app.apply_mcp(snapshots),
        UiMsg::McpStartupFailed(reason) => {
            // The reason is a configuration error: parse failures and
            // credential-shaped refusals name the field or the variable and
            // never its value (ADR-008), so it is shown as it is.
            app.push(EntryKind::Error, format!("MCP configuration: {reason}"));
            app.notice = Some(format!("MCP configuration: {reason}"));
        }
        UiMsg::McpOutcome { operation, result } => {
            let current = runtime
                .mcp
                .as_ref()
                .is_some_and(|active| active.id == operation);
            if !current {
                // A superseded or cancelled action finished its cleanup.
                // Its outcome is not this operation's, so it clears
                // neither the loading line nor the live token.
                return Ok(Action::None);
            }
            runtime.mcp = None;
            app.end_work(Work::Mcp);
            if let Err(error) = result {
                app.notice = Some(error.message);
            }
        }
        UiMsg::McpApproval {
            operation,
            server,
            definition,
        } => {
            if !runtime.finish_work(app, Work::McpDefinition, operation) {
                return Ok(Action::None);
            }
            // The definition was asked for when nothing was running. If a
            // turn or another approval started while it was being read,
            // showing it now would let the answer authorize a launch
            // during that turn, which is exactly what the mutation guard
            // exists to prevent. It is refused rather than deferred: the
            // user can reopen `/mcp` when the turn ends, and a queued
            // approval for a definition read minutes ago is worse.
            if !app.settings_are_editable() {
                app.notice = Some(format!(
                    "{server} was not approved: a turn started while its definition was read"
                ));
                return Ok(Action::None);
            }
            app.request_mcp_approval(server, definition);
        }
        UiMsg::Profiles {
            generation,
            profiles,
        } => {
            // A lookup that started before a reload describes the
            // configuration that reload replaced. Applying it would drop
            // the profile setup has just added: the new provider vanishes
            // from `/connect`, and `/effort` stops offering explicit
            // levels because it can no longer find the provider's protocol
            // in this list. Reopening a picker does not re-read profiles,
            // so nothing would put it back (TKT-0020).
            if generation != runtime.config_generation {
                return Ok(Action::None);
            }
            app.profiles = profiles;
            app.refresh_connection_picker();
        }
        UiMsg::Reloaded { plane, profiles } => {
            // A saved profile is only usable once the running configuration
            // has it. The rebuilt plane shares every handle, so nothing
            // already open is disturbed.
            runtime.plane = Arc::new(*plane);
            // Everything computed against the previous configuration is
            // now stale, including any profile lookup still blocked on the
            // keychain.
            runtime.config_generation = runtime.config_generation.wrapping_add(1);
            app.profiles = profiles;
            app.refresh_connection_picker();
        }
        UiMsg::Setup {
            operation,
            message,
            close,
        } => {
            if !runtime.finish_work(app, Work::Setup, operation) {
                return Ok(Action::None);
            }
            return Ok(app.setup_outcome(message, close));
        }
    }
    Ok(Action::None)
}

/// Performs one reducer action. Anything that could wait becomes a task,
/// so the draw loop is never blocked by storage, a catalog, or a server.
async fn on_action(
    app: &mut App,
    runtime: &mut Runtime,
    idle_agent: &mut Option<Box<dyn Driver>>,
    handle: &mut Option<CancelHandle>,
    responder: &mut Option<oneshot::Sender<ApprovalDecision>>,
    action: Action,
) -> Result<()> {
    match action {
        Action::None | Action::Quit => {}
        Action::Cancel => {
            if let Some(handle) = handle.as_ref() {
                handle.cancel();
            }
            // A pending approval is denied by the loop on cancel; drop the
            // view and its responder so a late key cannot answer it.
            app.pending = None;
            app.mcp_approval = None;
            *responder = None;
            // Cancellation is derived from what is really running, not
            // from the label on screen: an unrelated completion could
            // clear that label and leave a live operation uncancellable.
            let cancelled_work = runtime.cancel_work(app);
            // A cancelled session change has no driver coming and will
            // never produce a `Finished`, so the loop has to end the
            // running state itself. Without this the first lazy open being
            // cancelled leaves every later prompt and setting refused.
            if let Some(pending) = runtime.release_pending_open(app) {
                *idle_agent = pending.previous;
                if let Some(prompt) = pending.prompt {
                    app.undo_submission(&prompt);
                }
                app.running = false;
                app.notice = Some("cancelled; your draft was kept".into());
            } else if cancelled_work {
                app.notice = Some("cancelled".into());
            }
        }
        Action::Approve(decision) => {
            if let Some(sender) = responder.take() {
                let _ = sender.send(decision);
            }
        }
        Action::Submit(prompt) => {
            if runtime.pending_open.is_some() {
                // A session change is in flight; the driver this would run
                // on is about to be replaced.
                app.running = false;
                app.restore_draft(&prompt);
                app.notice = Some("the session is still opening; your draft was kept".into());
            } else if let Some(mut agent) = idle_agent.take() {
                app.sidebar.session.activity = Some("running".into());
                *handle = Some(agent.handle());
                let tx = runtime.tx.clone();
                let generation = app.sidebar.generation;
                tokio::spawn(async move {
                    let mut ui = ChannelUi {
                        tx: tx.clone(),
                        generation,
                    };
                    let result = agent.run_turn(&prompt, &mut ui).await;
                    let _ = tx.send(UiMsg::Finished {
                        generation,
                        agent,
                        result,
                    });
                });
            } else if app.running && app.session_id.is_none() {
                // The lazy path: no session exists yet, so the first
                // prompt is what creates one from the draft.
                app.begin_work(Work::Open, "opening the session");
                open_draft(app, runtime, Some(prompt), None);
            } else {
                app.running = false;
                // The draft survives a submission that could not be sent.
                app.restore_draft(&prompt);
                app.notice = Some("a turn is already running; your draft was kept".into());
            }
        }
        Action::SetPhase(phase) => {
            if let Some(agent) = idle_agent.as_mut() {
                agent.set_phase(phase).await?;
                app.set_session(agent.session());
                let info = agent.info();
                app.status.profile = info.backend;
                app.status.model = info.detail;
            } else {
                app.draft.phase = Some(phase);
                app.status.phase = match phase {
                    gritt_core::session::Phase::Planning => "planning".into(),
                    gritt_core::session::Phase::Coding => "coding".into(),
                };
                app.sidebar.session.phase = Some(app.status.phase.clone());
            }
        }
        Action::RefreshSessions => {
            let plane = Arc::clone(&runtime.plane);
            let tx = runtime.tx.clone();
            let operation = runtime.next_operation();
            runtime.spawn(
                app,
                Work::Sessions,
                operation,
                "loading sessions",
                async move {
                    let sessions = plane.builder.store.list().await.unwrap_or_default();
                    let _ = tx.send(UiMsg::Sessions {
                        operation,
                        sessions,
                    });
                },
            );
        }
        Action::Resume(id) => {
            if handle.is_some() || runtime.pending_open.is_some() {
                app.notice = Some("finish or cancel the work in flight first".into());
                return Ok(());
            }
            // Resuming keeps the stored profile, model, and effort: the
            // request only names the session.
            app.begin_work(Work::Open, "resuming");
            app.session_transition = true;
            let named = id.0.clone();
            let plane = Arc::clone(&runtime.plane);
            let tx = runtime.tx.clone();
            let generation = app.sidebar.generation;
            let operation = runtime.next_operation();
            // The driver being replaced is held by the transition, so a
            // prompt cannot start a turn on the session being left.
            runtime.pending_open = Some(PendingOpen {
                operation,
                prompt: None,
                previous: idle_agent.take(),
            });
            runtime.spawn_open(async move {
                let result = resume_by_id(&plane, &named).await;
                let _ = tx.send(UiMsg::Opened {
                    operation,
                    generation,
                    result,
                });
            });
        }
        Action::SelectConnector(id) => {
            if handle.is_some() || runtime.pending_open.is_some() {
                app.notice = Some("finish or cancel the work in flight first".into());
                return Ok(());
            }
            app.begin_work(Work::Open, format!("starting {}", id.as_str()));
            app.session_transition = true;
            let plane = Arc::clone(&runtime.plane);
            let tx = runtime.tx.clone();
            let generation = app.sidebar.generation;
            let operation = runtime.next_operation();
            runtime.pending_open = Some(PendingOpen {
                operation,
                prompt: None,
                previous: idle_agent.take(),
            });
            runtime.spawn_open(async move {
                // An external agent owns its own model and effort, so this
                // is the general control-plane operation, not a draft.
                let result = plane
                    .open(
                        crate::agent::SessionSelector::New { name: None },
                        Some(id),
                        None,
                        None,
                        None,
                    )
                    .await
                    .map(|driver| DraftOpen::Opened {
                        driver,
                        catalog: crate::draft::CatalogState::Skipped,
                        warnings: Vec::new(),
                    });
                let _ = tx.send(UiMsg::Opened {
                    operation,
                    generation,
                    result,
                });
            });
        }
        Action::LoadCatalog { profile, selection } => {
            let plane = Arc::clone(&runtime.plane);
            let tx = runtime.tx.clone();
            let label = format!("loading {profile} models");
            let operation = runtime.next_operation();
            runtime.spawn(app, Work::Catalog, operation, label, async move {
                let result = plane.catalog(&profile).await;
                let _ = tx.send(UiMsg::Catalog {
                    operation,
                    selection,
                    profile,
                    result,
                });
            });
        }
        Action::SetEffort(effort) => {
            if let Some(agent) = idle_agent.as_mut() {
                match agent.set_effort(effort).await? {
                    crate::driver::EffortOutcome::Applied { effort } => {
                        app.set_effective_effort(Some(effort))
                    }
                    crate::driver::EffortOutcome::ManagedByConnector { id } => {
                        app.notice = Some(format!("{} manages its own effort", id.as_str()));
                        app.set_effective_effort(None);
                    }
                    crate::driver::EffortOutcome::Unsupported { effort, reason } => {
                        app.notice = Some(format!(
                            "{effort} is not supported here: {}",
                            reason.describe()
                        ));
                        app.set_effective_effort(agent.effort());
                    }
                }
            }
        }
        Action::SaveProfile => {
            let Some(submission) = app.take_setup_submission() else {
                return Ok(());
            };
            let plane = Arc::clone(&runtime.plane);
            let tx = runtime.tx.clone();
            // The write, the configuration reload, and the credential
            // resolution behind the summaries are all blocking file and
            // system calls. `keyring::Entry::get_password()` in particular
            // can block for as long as the operating system wants, so none
            // of it runs on the loop.
            let operation = runtime.next_operation();
            // The configuration this save starts from. If it reloads, the
            // `Reloaded` below installs a newer one; if it does not, the
            // profiles it computed still belong to this generation and are
            // refused should anything else have reloaded meanwhile.
            let generation = runtime.config_generation;
            runtime.spawn(
                app,
                Work::Setup,
                operation,
                "saving the profile",
                async move {
                    let outcome = tokio::task::spawn_blocking(move || {
                        let (message, ok) =
                            crate::setup::apply_setup(plane.setup().as_ref(), submission);
                        if !ok {
                            return (message, false, None);
                        }
                        let reloaded = plane.reloaded();
                        let profiles = match &reloaded {
                            Some(plane) => plane.profile_summaries(),
                            None => plane.profile_summaries(),
                        };
                        (message, true, Some((reloaded, profiles)))
                    })
                    .await;
                    let (message, close, reloaded) = match outcome {
                        Ok(outcome) => outcome,
                        Err(_) => ("the setup task did not finish".to_owned(), false, None),
                    };
                    if let Some((plane, profiles)) = reloaded {
                        if let Some(plane) = plane {
                            let _ = tx.send(UiMsg::Reloaded {
                                plane: Box::new(plane),
                                profiles,
                            });
                        } else {
                            let _ = tx.send(UiMsg::Profiles {
                                generation,
                                profiles,
                            });
                        }
                    }
                    let _ = tx.send(UiMsg::Setup {
                        operation,
                        message,
                        close,
                    });
                },
            );
        }
        Action::NewSession => {
            // The previous session is not deleted; its driver is released
            // so the next prompt opens a fresh one from the draft.
            *idle_agent = None;
            *handle = None;
            *responder = None;
            runtime.cancel_work(app);
            drop(runtime.release_pending_open(app));
            scan_changes(runtime, app.sidebar.generation);
        }
        Action::Mcp(request) => {
            let Some(mcp) = runtime.plane.builder.mcp().cloned() else {
                app.notice = Some("no MCP runtime is configured for this workspace".into());
                return Ok(());
            };
            let tx = runtime.tx.clone();
            if let McpRequest::RequestApproval { server } = request {
                // Fetching the definition changes nothing; it only asks
                // what approving would run.
                let operation = runtime.next_operation();
                runtime.spawn(
                    app,
                    Work::McpDefinition,
                    operation,
                    "reading the server definition",
                    async move {
                        let definition = mcp
                            .definition_summary(&server)
                            .await
                            .unwrap_or_else(|| "this entry cannot run as configured".to_owned());
                        let _ = tx.send(UiMsg::McpApproval {
                            operation,
                            server,
                            definition,
                        });
                    },
                );
                return Ok(());
            }
            // The mutation guard is enforced here as well as in the
            // reducer: the decision may have been taken from an approval
            // overlay that opened before a turn started.
            if !app.settings_are_editable() {
                app.notice =
                    Some("a turn or an approval is active; the MCP change was not applied".into());
                return Ok(());
            }
            app.begin_work(Work::Mcp, "applying the MCP change");
            // The token is kept by the loop, with an id. Cancelling signals
            // it and lets the operation's own cleanup shut the child down
            // and release its launch slot; aborting the future would drop
            // both and leave the process alive until the application
            // exits. The id is what stops a completion from a superseded
            // action clearing this one's state.
            let (operation, cancel) = runtime.begin_mcp();
            runtime.spawn_detached(async move {
                let result = match request {
                    McpRequest::RequestApproval { .. } => Ok(()),
                    McpRequest::Decide { server, decision } => {
                        match mcp.decide(&server, decision).await {
                            // Approving records the decision; the launch
                            // is the separate step, as `gritt mcp trust`
                            // does it.
                            Ok(()) => {
                                mcp.start(&cancel).await;
                                Ok(())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    McpRequest::Restart { server } => mcp.restart(&server, &cancel).await,
                    McpRequest::Stop { server } => mcp.stop(&server).await,
                    McpRequest::ReloadAll => match mcp.reload().await {
                        Ok(()) => {
                            mcp.start(&cancel).await;
                            Ok(())
                        }
                        Err(error) => Err(error),
                    },
                };
                let _ = tx.send(UiMsg::McpOutcome { operation, result });
            });
        }
        Action::ScanChanges => scan_changes(runtime, app.sidebar.generation),
        Action::OpenFileDiff(path) => {
            let changes = Arc::clone(&runtime.changes);
            let tx = runtime.tx.clone();
            let operation = runtime.next_operation();
            runtime.spawn(app, Work::Diff, operation, "reading the diff", async move {
                let diff = changes.diff(&path).await;
                let _ = tx.send(UiMsg::Diff { operation, diff });
            });
        }
        Action::RefreshMcp => {
            if let Some(mcp) = runtime.plane.builder.mcp().cloned() {
                let tx = runtime.tx.clone();
                runtime.spawn_detached(async move {
                    let _ = tx.send(UiMsg::Mcp(mcp.snapshots().await));
                });
            } else {
                // An inventory that was checked and found empty is the one
                // case that may say "none".
                app.apply_mcp(Vec::new());
            }
        }
    }
    Ok(())
}

/// Opens the draft on screen, carrying the prompt that triggered it.
///
/// The transition is reserved before the request goes out, so a prompt
/// submitted while it is in flight is refused rather than run on the
/// driver that is about to be replaced.
fn open_draft(
    app: &mut App,
    runtime: &mut Runtime,
    prompt: Option<String>,
    previous: Option<Box<dyn Driver>>,
) {
    let draft = app.draft.clone();
    let plane = Arc::clone(&runtime.plane);
    let tx = runtime.tx.clone();
    let generation = app.sidebar.generation;
    let operation = runtime.next_operation();
    app.session_transition = true;
    runtime.pending_open = Some(PendingOpen {
        operation,
        prompt,
        previous,
    });
    runtime.spawn_open(async move {
        let result = plane.open_draft(draft).await;
        let _ = tx.send(UiMsg::Opened {
            operation,
            generation,
            result,
        });
    });
}

/// Resumes by session id.
///
/// A native session goes through the draft operation, so it gets the same
/// pinning and effort rules as any other. A connector session cannot:
/// draft validation rejects it by design, because an external agent owns
/// its own model and effort. That one goes through the general
/// control-plane operation, which is what the eager path always used.
async fn resume_by_id(plane: &ControlPlane, id: &str) -> Result<DraftOpen> {
    let id = gritt_core::session::SessionId(id.to_owned());
    let Some(session) = plane.builder.store.get(&id).await? else {
        return Err(Error::storage(format!("session `{}` is gone", id.0)));
    };
    if matches!(
        session.kind,
        gritt_core::session::SessionKind::Connector { .. }
    ) {
        let driver = plane
            .open(
                crate::agent::SessionSelector::Id(id),
                None,
                None,
                None,
                None,
            )
            .await?;
        return Ok(DraftOpen::Opened {
            driver,
            catalog: crate::draft::CatalogState::Skipped,
            warnings: Vec::new(),
        });
    }
    plane
        .open_draft(SessionDraft::default().with_name(session.name))
        .await
}

/// The real scheduler, driven from a test (TKT-0020).
///
/// `Scheduler::step` is the loop. This exposes it against a `TestBackend`
/// with an injected input channel, so the responsiveness benchmark measures
/// the production draw timing, drain order, and input priority rather than a
/// copy of them. It adds no scheduling of its own: every method either
/// forwards to `step` or hands over one of the channels the loop reads.
///
/// Not part of the supported API.
#[doc(hidden)]
pub struct LoopHarness {
    scheduler: Scheduler,
    ui_tx: mpsc::UnboundedSender<UiMsg>,
    keys_tx: mpsc::UnboundedSender<TerminalEvent>,
}

#[doc(hidden)]
impl LoopHarness {
    /// The loop's state as `event_loop` builds it, minus the terminal.
    pub fn new(plane: ControlPlane, workspace: &std::path::Path, app: App) -> Self {
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let (keys_tx, keys_rx) = mpsc::unbounded_channel();
        let runtime = Runtime {
            plane: Arc::new(plane),
            changes: Arc::new(WorkspaceChanges::new(workspace)),
            tx: ui_tx.clone(),
            work: None,
            work_active: None,
            open_work: None,
            operations: 0,
            pending_open: None,
            config_generation: 0,
            mcp: None,
        };
        Self {
            scheduler: Scheduler::new(app, runtime, None, ui_rx, keys_rx),
            ui_tx,
            keys_tx,
        }
    }

    pub fn app(&self) -> &App {
        &self.scheduler.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.scheduler.app
    }

    /// One iteration of the production loop.
    pub async fn step<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut ratatui::Terminal<B>,
    ) -> Result<Step> {
        self.scheduler.step(terminal).await
    }

    /// The production `Ui` a turn streams through. A producer holding this
    /// sends exactly what a real turn sends, on the channel the real loop
    /// drains.
    pub fn channel_ui(&self, generation: u64) -> Box<dyn Ui> {
        Box::new(ChannelUi {
            tx: self.ui_tx.clone(),
            generation,
        })
    }

    /// The channel the key reader thread writes to in production. Input
    /// queued here waits its turn exactly as a real keystroke does.
    pub fn input(&self) -> mpsc::UnboundedSender<TerminalEvent> {
        self.keys_tx.clone()
    }

    /// Messages waiting to be handled, read from the channel itself rather
    /// than modelled.
    pub fn queue_depth(&self) -> usize {
        self.scheduler.ui_rx.len()
    }

    /// Starts the real MCP lifecycle subscription, the way `event_loop`
    /// does, so snapshots arrive as messages on the same queue.
    pub fn subscribe_mcp(&mut self) {
        subscribe_mcp(&self.scheduler.runtime);
    }

    /// Installs a driver so a cancel has a running turn to act on.
    pub fn set_driver(&mut self, driver: Box<dyn Driver>) {
        self.scheduler.handle = Some(driver.handle());
        self.scheduler.idle = Some(driver);
    }
}

#[cfg(test)]
mod tests {
    //! The loop's two handlers, driven directly.
    //!
    //! Session transitions and turn completion are the places where two
    //! asynchronous results can overwrite each other, and neither is
    //! reachable from a reducer test: the reducer never holds a driver.
    //! These build the real `Runtime` and call the real handlers.

    use super::*;
    use crate::agent::ApprovalMode;
    use crate::store::{DatabaseLocation, Store};
    use crate::telemetry::Telemetry;
    use crate::tools::{ProcessRegistry, Workspace};
    use gritt_core::config::Config;
    use gritt_core::provider::{Protocol, ProviderProfile};
    use gritt_core::secret::{Secret, SecretRef};
    use gritt_core::session::{Phase, Session, SessionId, SessionKind};
    use gritt_provider::models::ModelCatalog;
    use gritt_provider::{FixtureTransport, StaticKey};

    /// A driver that answers questions and never runs anything. Enough to
    /// prove which driver the loop would have used.
    struct StubDriver {
        session: Session,
        turns: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Driver for StubDriver {
        fn session(&self) -> &Session {
            &self.session
        }
        fn phase(&self) -> Phase {
            self.session.phase
        }
        fn set_phase(&mut self, _phase: Phase) -> gritt_core::session::BoxFuture<'_, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn handle(&self) -> CancelHandle {
            CancelHandle::new(crate::CancellationToken::new(), ProcessRegistry::new())
        }
        fn run_turn<'a>(
            &'a mut self,
            _prompt: &'a str,
            _ui: &'a mut dyn crate::agent::Ui,
        ) -> gritt_core::session::BoxFuture<'a, Result<TurnOutcome>> {
            self.turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(TurnOutcome {
                    status: crate::agent::TurnStatus::Completed,
                    text: String::new(),
                    usage: Default::default(),
                    tool_calls: 0,
                    error: None,
                })
            })
        }
        fn info(&self) -> crate::driver::DriverInfo {
            crate::driver::DriverInfo {
                backend: "openrouter".into(),
                detail: "openai/gpt-5-nano".into(),
            }
        }
        fn effort(&self) -> Option<gritt_core::provider::ReasoningEffort> {
            Some(Default::default())
        }
        fn set_effort(
            &mut self,
            _effort: gritt_core::provider::ReasoningEffort,
        ) -> gritt_core::session::BoxFuture<'_, Result<crate::driver::EffortOutcome>> {
            Box::pin(async {
                Ok(crate::driver::EffortOutcome::Applied {
                    effort: Default::default(),
                })
            })
        }
    }

    /// A scheduler over the same `Runtime` the handler tests build, with an
    /// injected input channel and a `TestBackend`.
    async fn scheduler() -> (
        Scheduler,
        mpsc::UnboundedSender<UiMsg>,
        mpsc::UnboundedSender<TerminalEvent>,
        tempfile::TempDir,
    ) {
        let h = harness().await;
        let (keys_tx, keys_rx) = mpsc::unbounded_channel();
        let ui_tx = h.runtime.tx.clone();
        let mut app = h.app;
        app.on_resize(120, 40);
        (
            Scheduler::new(app, h.runtime, None, h.rx, keys_rx),
            ui_tx,
            keys_tx,
            h._dir,
        )
    }

    fn screen(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// A decision key must never answer an approval that has not been drawn.
    ///
    /// `y` approves the moment it reaches the reducer. The frame cap added
    /// in TKT-0020 can suppress the frame after an approval arrives, so
    /// without the guard a keystroke arriving in that window approves a
    /// tool call or an MCP launch that was never on screen. Before the cap
    /// the loop drew on every iteration and the question could not arise.
    ///
    /// The clock is paused, so the cap is engaged deterministically rather
    /// than by racing it: no time passes between the first frame and the
    /// approval, so the cap owes nothing and would suppress the frame.
    #[tokio::test(start_paused = true)]
    async fn an_approval_is_drawn_before_a_decision_key_is_accepted() {
        let (mut scheduler, ui_tx, keys_tx, _dir) = scheduler().await;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();

        // A first frame, which arms the cap. The message is only there so
        // the step has a ready branch and does not advance the clock.
        ui_tx.send(UiMsg::Agents(Vec::new())).unwrap();
        let first = scheduler.step(&mut terminal).await.unwrap();
        assert!(first.drew, "the first frame should be free");

        // The approval arrives inside the cap window.
        let (responder, mut decision) = oneshot::channel();
        ui_tx
            .send(UiMsg::Approval {
                generation: scheduler.app.sidebar.generation,
                pending: PendingApproval {
                    request: ApprovalRequest {
                        id: gritt_core::event::ApprovalId("a1".into()),
                        tool: "shell".into(),
                        resource: "rm -rf /tmp/x".into(),
                        reason: "destructive".into(),
                        call_id: None,
                    },
                    decision: Decision {
                        outcome: gritt_core::policy::PolicyOutcome::Ask,
                        reason: "destructive".into(),
                        destructive: true,
                        rule: None,
                    },
                    preview: None,
                },
                responder,
            })
            .unwrap();
        let handled = scheduler.step(&mut terminal).await.unwrap();
        assert!(
            !handled.drew,
            "the cap should have suppressed this frame; the test is not exercising it"
        );
        assert!(
            scheduler.app.pending.is_some(),
            "the approval was not taken off the queue"
        );

        // The keystroke lands in exactly that window.
        keys_tx
            .send(TerminalEvent::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('y'),
                crossterm::event::KeyModifiers::NONE,
            )))
            .unwrap();

        let mut shown = false;
        let mut answered_unseen = false;
        let mut delivered = false;
        for _ in 0..8 {
            scheduler.step(&mut terminal).await.unwrap();
            if screen(&terminal).contains("approve?") {
                shown = true;
            }
            if decision.try_recv().is_ok() {
                delivered = true;
                if !shown {
                    answered_unseen = true;
                }
                break;
            }
        }
        assert!(
            !answered_unseen,
            "a decision was sent for an approval that had not been drawn"
        );
        assert!(shown, "the approval was never drawn");
        // The guard delays the key, it does not eat it: once the prompt has
        // been drawn the keystroke must still answer it.
        assert!(delivered, "the decision never reached its responder");
    }

    /// A second approval installed in place of an answered one must be
    /// drawn on its own account.
    ///
    /// Answering A and installing B can happen in the same iteration: the
    /// turn that receives A's decision enqueues B from another worker, and
    /// the drain handles it before the next frame. `pending` is `Some`
    /// throughout, so tracking presence rather than identity would carry
    /// A's visibility onto B and let the next keystroke answer a prompt
    /// that had never been drawn.
    #[tokio::test(start_paused = true)]
    async fn a_second_approval_does_not_inherit_the_first_one_s_visibility() {
        let (mut scheduler, ui_tx, keys_tx, _dir) = scheduler().await;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();

        // A is installed and drawn, then answered.
        let (responder_a, mut decision_a) = oneshot::channel();
        ui_tx
            .send(approval_message(
                scheduler.app.sidebar.generation,
                "a1",
                "rm -rf /tmp/a",
                responder_a,
            ))
            .unwrap();
        for _ in 0..4 {
            scheduler.step(&mut terminal).await.unwrap();
            if screen(&terminal).contains("/tmp/a") {
                break;
            }
        }
        assert!(screen(&terminal).contains("/tmp/a"), "A was never drawn");
        let drawn_for_a = scheduler.drawn_approval;

        // One step now answers A and installs B: the key is taken first,
        // and B's message is handled by the same step's drain. `pending`
        // is `Some` before and after, which is the transition that made
        // presence-tracking wrong.
        let (responder_b, mut decision_b) = oneshot::channel();
        keys_tx
            .send(TerminalEvent::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('y'),
                crossterm::event::KeyModifiers::NONE,
            )))
            .unwrap();
        ui_tx
            .send(approval_message(
                scheduler.app.sidebar.generation,
                "b1",
                "rm -rf /tmp/b",
                responder_b,
            ))
            .unwrap();
        let step = scheduler.step(&mut terminal).await.unwrap();
        assert!(step.input, "the step did not answer A");
        assert!(step.messages > 0, "the step did not install B");
        assert!(
            decision_a.try_recv().is_ok(),
            "A's own decision never reached its responder"
        );
        assert!(
            scheduler.app.pending.is_some(),
            "B is not on screen; the transition under test did not happen"
        );

        // The next decision key is already on its way, as it would be from
        // a user still answering A.
        keys_tx
            .send(TerminalEvent::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('y'),
                crossterm::event::KeyModifiers::NONE,
            )))
            .unwrap();

        let mut shown_b = false;
        let mut answered_unseen = false;
        let mut delivered_b = false;
        for _ in 0..8 {
            scheduler.step(&mut terminal).await.unwrap();
            if screen(&terminal).contains("/tmp/b") {
                shown_b = true;
            }
            if decision_b.try_recv().is_ok() {
                delivered_b = true;
                if !shown_b {
                    answered_unseen = true;
                }
                break;
            }
        }
        assert_ne!(
            scheduler.drawn_approval, drawn_for_a,
            "B inherited A's visibility"
        );
        assert!(
            !answered_unseen,
            "a decision was sent for a second approval that had not been drawn"
        );
        assert!(shown_b, "the second approval was never drawn");
        assert!(
            delivered_b,
            "the second decision never reached its responder"
        );
    }

    /// One `step` draws before it takes input, so a frame cannot show a
    /// keystroke handled in the same step.
    ///
    /// The load benchmark relies on exactly this to attribute
    /// input-to-frame latency: a frame drawn at the top of a step completes
    /// only the input handled in earlier steps. If the order here ever
    /// changes, that attribution becomes wrong and this fails first.
    #[tokio::test(start_paused = true)]
    async fn a_step_draws_before_it_handles_input() {
        let (mut scheduler, ui_tx, keys_tx, _dir) = scheduler().await;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        // Something to draw, and a key waiting behind it.
        ui_tx.send(UiMsg::Agents(Vec::new())).unwrap();
        scheduler.step(&mut terminal).await.unwrap();
        scheduler.app.composer.clear();

        keys_tx
            .send(TerminalEvent::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('Z'),
                crossterm::event::KeyModifiers::NONE,
            )))
            .unwrap();
        // Make a frame owed so this step both draws and takes the key.
        scheduler.dirty = true;
        scheduler.last_draw = tokio::time::Instant::now() - FRAME_INTERVAL;
        let step = scheduler.step(&mut terminal).await.unwrap();

        assert!(step.drew, "the step did not draw");
        assert!(step.input, "the step did not take the key");
        assert!(
            !screen(&terminal).contains('Z'),
            "the frame drawn by this step already showed the key it handled afterwards"
        );
        assert!(
            scheduler.app.composer.text().contains('Z'),
            "the key was not applied at all"
        );
    }

    /// `Step::drew_at` is taken when the frame is written, not when the
    /// step returns.
    ///
    /// After drawing, a step waits in `select!` and then drains. That time
    /// is scheduler time with the frame already on screen, and the load
    /// benchmark's input-to-frame numbers are wrong by exactly that much if
    /// they use the later instant. Real time here, not a paused clock: the
    /// point is to observe the gap.
    #[tokio::test]
    async fn a_step_records_when_its_frame_was_drawn_not_when_it_returned() {
        let (mut scheduler, ui_tx, _keys_tx, _dir) = scheduler().await;
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
        // The first frame, then a step with nothing queued to spend the
        // interval's immediate first tick. After that a tick really waits.
        ui_tx.send(UiMsg::Agents(Vec::new())).unwrap();
        scheduler.step(&mut terminal).await.unwrap();
        scheduler.step(&mut terminal).await.unwrap();

        // Owe a frame, and leave both queues empty so the step draws and
        // then waits out the 50 ms tick with nothing to do.
        scheduler.dirty = true;
        scheduler.last_draw = tokio::time::Instant::now() - FRAME_INTERVAL;
        let step = scheduler.step(&mut terminal).await.unwrap();
        let returned_at = std::time::Instant::now();

        assert!(step.drew, "the step did not draw");
        let drew_at = step.drew_at.expect("a drawn step records when");
        let after_the_frame = returned_at.duration_since(drew_at);
        assert!(
            after_the_frame >= Duration::from_millis(20),
            "the step returned {after_the_frame:?} after its frame; this test needs it to \
             wait, so it is not proving anything"
        );
    }

    fn summary(name: &str) -> crate::setup::ProfileSummary {
        crate::setup::ProfileSummary {
            name: name.into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://example.invalid/v1".into(),
            credential: crate::setup::CredentialState::Available,
            is_default: false,
        }
    }

    /// A profile lookup that started before a reload must not replace the
    /// list that reload installed.
    ///
    /// Startup resolves every profile's credential, which reaches the
    /// keychain and blocks for as long as the operating system wants. A
    /// user who runs `/connect` and saves a provider in that window gets a
    /// reload with the new profile in it. If the startup lookup then lands,
    /// applying it drops the provider that was just configured: it
    /// disappears from `/connect`, and `/effort` stops offering explicit
    /// levels because it can no longer find the provider's protocol in the
    /// list. Reopening a picker does not re-read profiles, so nothing puts
    /// it back.
    ///
    /// The order here is the defect's order: the refreshed configuration is
    /// applied first, and the older startup response is delivered after it.
    #[tokio::test]
    async fn a_profile_lookup_from_before_a_reload_is_refused() {
        let mut h = harness().await;
        // What startup found: only the profile that was configured then.
        let startup_generation = h.runtime.config_generation;
        let stale = vec![summary("openrouter")];

        // Setup saves a new provider and the plane is rebuilt around the
        // re-read configuration.
        let reloaded = (*h.runtime.plane).clone();
        h.message(UiMsg::Reloaded {
            plane: Box::new(reloaded),
            profiles: vec![summary("openrouter"), summary("anthropic")],
        })
        .await;
        assert_eq!(
            h.app.profiles.len(),
            2,
            "the reload did not install its own list"
        );

        // The startup lookup finally returns, describing the configuration
        // that reload replaced.
        h.message(UiMsg::Profiles {
            generation: startup_generation,
            profiles: stale,
        })
        .await;

        let names: Vec<&str> = h.app.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["openrouter", "anthropic"],
            "a profile lookup from before the reload replaced the list it installed"
        );

        // And a lookup issued against the current configuration still
        // applies, so the guard delays nothing that is still true.
        h.message(UiMsg::Profiles {
            generation: h.runtime.config_generation,
            profiles: vec![summary("anthropic")],
        })
        .await;
        assert_eq!(
            h.app.profiles.len(),
            1,
            "a current profile lookup was refused"
        );
    }

    fn approval_message(
        generation: u64,
        id: &str,
        resource: &str,
        responder: oneshot::Sender<ApprovalDecision>,
    ) -> UiMsg {
        UiMsg::Approval {
            generation,
            pending: PendingApproval {
                request: ApprovalRequest {
                    id: gritt_core::event::ApprovalId(id.into()),
                    tool: "shell".into(),
                    resource: resource.into(),
                    reason: "destructive".into(),
                    call_id: None,
                },
                decision: Decision {
                    outcome: gritt_core::policy::PolicyOutcome::Ask,
                    reason: "destructive".into(),
                    destructive: true,
                    rule: None,
                },
                preview: None,
            },
            responder,
        }
    }

    fn session(name: &str) -> Session {
        let now = chrono::Utc::now();
        Session {
            name: name.into(),
            id: SessionId(format!("id-{name}")),
            kind: SessionKind::Native {
                provider_profile: "openrouter".into(),
                model: "openai/gpt-5-nano".into(),
                effort: Default::default(),
            },
            phase: Phase::Coding,
            workspace: std::path::PathBuf::from("/tmp"),
            created_at: now,
            updated_at: now,
            parent_id: None,
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        app: App,
        runtime: Runtime,
        idle: Option<Box<dyn Driver>>,
        handle: Option<CancelHandle>,
        responder: Option<oneshot::Sender<ApprovalDecision>>,
        rx: mpsc::UnboundedReceiver<UiMsg>,
    }

    async fn harness() -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
                .await
                .unwrap(),
        );
        let mut config = Config::default();
        config.profiles.insert(
            "openrouter".into(),
            ProviderProfile {
                name: "openrouter".into(),
                protocol: Protocol::ChatCompletions,
                base_url: "https://openrouter.ai/api/v1".into(),
                key: SecretRef::for_profile("openrouter", "OPENROUTER_API_KEY"),
                aliases: Default::default(),
            },
        );
        config.default_profile = Some("openrouter".into());
        config.default_model = Some("openai/gpt-5-nano".into());
        let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
        let builder = crate::agent::AgentBuilder {
            config,
            store,
            telemetry,
            keys: Arc::new(StaticKey(Secret::new("k"))),
            transport: Arc::new(FixtureTransport::new(Vec::new(), 17)),
            catalog: ModelCatalog::new(),
            cache: None,
            workspace: Workspace::open(dir.path()).unwrap(),
            approval: ApprovalMode::DenyAll,
            mcp: None,
        };
        let (tx, rx) = mpsc::unbounded_channel();
        let plane = ControlPlane::native(Arc::new(builder));
        let runtime = Runtime {
            plane: Arc::new(plane),
            changes: Arc::new(WorkspaceChanges::new(dir.path())),
            tx,
            work: None,
            work_active: None,
            open_work: None,
            operations: 0,
            pending_open: None,
            config_generation: 0,
            mcp: None,
        };
        Harness {
            _dir: dir,
            app: App::new(StatusBar::default(), Theme::default()),
            runtime,
            idle: None,
            handle: None,
            responder: None,
            rx,
        }
    }

    impl Harness {
        async fn act(&mut self, action: Action) {
            on_action(
                &mut self.app,
                &mut self.runtime,
                &mut self.idle,
                &mut self.handle,
                &mut self.responder,
                action,
            )
            .await
            .unwrap();
        }

        async fn message(&mut self, msg: UiMsg) -> Action {
            on_message(
                &mut self.app,
                &mut self.runtime,
                &mut self.idle,
                &mut self.handle,
                &mut self.responder,
                msg,
            )
            .await
            .unwrap()
        }
    }

    /// Finding 1: a resume reserves the transition, so a prompt submitted
    /// while it is in flight cannot start a turn on the session being
    /// left, and the driver that arrives cannot be undone by the old one.
    #[tokio::test]
    async fn a_prompt_during_a_resume_cannot_run_on_the_session_being_left() {
        let mut h = harness().await;
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        h.idle = Some(Box::new(StubDriver {
            session: session("old"),
            turns: Arc::clone(&turns),
        }));
        h.app.set_session(&session("old"));

        h.act(Action::Resume(SessionId("id-new".into()))).await;
        assert!(h.runtime.pending_open.is_some(), "no transition reserved");
        assert!(h.app.session_transition);
        assert!(
            h.idle.is_none(),
            "the driver being replaced was left available to a turn"
        );

        // A prompt submitted now must not reach the old driver.
        h.app.running = true;
        h.act(Action::Submit("do a thing".into())).await;
        assert_eq!(
            turns.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a turn ran on the session being left"
        );
        assert_eq!(h.app.composer.text(), "do a thing", "the draft was lost");
        assert!(!h.app.running);
    }

    /// Finding 1: a result for a transition that is no longer wanted is
    /// dropped, and untagged turn output cannot populate the new session.
    #[tokio::test]
    async fn a_superseded_open_and_a_stale_turn_are_both_refused() {
        let mut h = harness().await;
        h.act(Action::Resume(SessionId("id-a".into()))).await;
        let stale_operation = h.runtime.pending_open.as_ref().unwrap().operation;
        // The first one is cancelled the way the user cancels it, then a
        // second is asked for. Nothing is cleared by hand.
        h.act(Action::Cancel).await;
        h.act(Action::Resume(SessionId("id-b".into()))).await;
        let current = h.runtime.pending_open.as_ref().unwrap().operation;
        assert_ne!(stale_operation, current);

        // The first one answers. It must not be adopted.
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        h.message(UiMsg::Opened {
            operation: stale_operation,
            generation: h.app.sidebar.generation,
            result: Ok(DraftOpen::Opened {
                driver: Box::new(StubDriver {
                    session: session("stale"),
                    turns: Arc::clone(&turns),
                }),
                catalog: crate::draft::CatalogState::Skipped,
                warnings: Vec::new(),
            }),
        })
        .await;
        assert!(h.idle.is_none(), "a superseded driver was adopted");
        assert!(
            h.runtime.pending_open.is_some(),
            "the transition still wanted was cancelled by a stale answer"
        );

        // Output from a session that is gone cannot populate the one on
        // screen, and its driver is not restored as the idle one.
        let stale_generation = h.app.sidebar.generation;
        h.app.sidebar.reset();
        h.message(UiMsg::Event {
            generation: stale_generation,
            event: gritt_core::event::Event {
                session_id: SessionId("id-a".into()),
                sequence: 1,
                timestamp: chrono::Utc::now(),
                source: gritt_core::event::EventSource::Native,
                kind: gritt_core::event::EventKind::TextDelta {
                    text: "from the old session".into(),
                },
                diagnostic: None,
            },
        })
        .await;
        assert!(
            h.app.entries.is_empty(),
            "output from a session that was left reached the transcript"
        );
        h.message(UiMsg::Finished {
            generation: stale_generation,
            agent: Box::new(StubDriver {
                session: session("stale"),
                turns,
            }),
            result: Ok(TurnOutcome {
                status: crate::agent::TurnStatus::Completed,
                text: String::new(),
                usage: Default::default(),
                tool_calls: 0,
                error: None,
            }),
        })
        .await;
        assert!(h.idle.is_none(), "the previous driver was restored");
    }

    /// Finding 2: cancelling the first lazy open gives the prompt back and
    /// leaves the interface usable, and the result it was waiting for can
    /// no longer land.
    #[tokio::test]
    async fn cancelling_the_first_session_open_does_not_strand_the_interface() {
        let mut h = harness().await;
        // What `Action::Submit` does with no session: the reducer has
        // already pushed the entry and set `running`.
        h.app.push(EntryKind::User, "first prompt");
        h.app.running = true;
        h.act(Action::Submit("first prompt".into())).await;
        let operation = h.runtime.pending_open.as_ref().unwrap().operation;
        assert!(h.app.session_transition);

        h.act(Action::Cancel).await;
        assert!(!h.app.running, "the interface stayed in a running state");
        assert!(!h.app.session_transition);
        assert!(h.runtime.pending_open.is_none());
        assert!(
            !h.app.is_working_on(Work::Open),
            "the cancelled open left its label behind"
        );
        assert!(h.app.loading().is_none());
        assert_eq!(h.app.composer.text(), "first prompt", "the prompt was lost");
        assert!(
            h.app.entries.is_empty(),
            "the cancelled prompt stayed in the transcript"
        );
        // Settings and prompts work again.
        assert!(h.app.settings_are_editable());

        // A result queued before the cancellation cannot open a session.
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        h.message(UiMsg::Opened {
            operation,
            generation: h.app.sidebar.generation,
            result: Ok(DraftOpen::Opened {
                driver: Box::new(StubDriver {
                    session: session("late"),
                    turns,
                }),
                catalog: crate::draft::CatalogState::Skipped,
                warnings: Vec::new(),
            }),
        })
        .await;
        assert!(h.idle.is_none(), "a cancelled open still adopted a driver");
        assert!(h.app.session_id.is_none());
    }

    /// A refused draft restores the driver that was open, so a failed
    /// switch does not cost the session the user already had.
    #[tokio::test]
    async fn a_refused_transition_puts_the_previous_session_back() {
        let mut h = harness().await;
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        h.idle = Some(Box::new(StubDriver {
            session: session("old"),
            turns,
        }));
        h.act(Action::Resume(SessionId("id-new".into()))).await;
        assert!(h.idle.is_none());
        let operation = h.runtime.pending_open.as_ref().unwrap().operation;
        h.message(UiMsg::Opened {
            operation,
            generation: h.app.sidebar.generation,
            result: Ok(DraftOpen::Rejected {
                errors: vec![crate::draft::DraftError::MissingModel],
                catalog: None,
            }),
        })
        .await;
        assert!(
            h.idle.is_some(),
            "a refused switch left the interface with no session"
        );
        assert!(!h.app.session_transition);
    }

    /// Finding 3: cancelling MCP work signals its token instead of
    /// dropping the future that owns the child process.
    #[tokio::test]
    async fn cancelling_mcp_work_signals_it_rather_than_aborting_it() {
        let mut h = harness().await;
        let (_id, token) = h.runtime.begin_mcp();
        h.app.begin_work(Work::Mcp, "applying the MCP change");
        h.act(Action::Cancel).await;
        assert!(
            token.is_cancelled(),
            "the MCP operation was dropped without being told to stop"
        );
        assert!(h.runtime.mcp.is_none());
        let _ = h.rx.try_recv();
    }
    /// Round 2, finding 1: another request during a pending resume must
    /// not strand the reservation.
    ///
    /// `/sessions` is the realistic one, because the picker opens before
    /// the store answers and its response clears the loading line the
    /// resume had put up. The open must survive it, and Escape must still
    /// reach the transition afterwards.
    #[tokio::test]
    async fn a_session_list_during_a_resume_leaves_the_transition_recoverable() {
        let mut h = harness().await;
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        h.idle = Some(Box::new(StubDriver {
            session: session("old"),
            turns: Arc::clone(&turns),
        }));
        h.act(Action::Resume(SessionId("id-new".into()))).await;
        let operation = h.runtime.pending_open.as_ref().unwrap().operation;

        // The real sequence `/sessions` produces.
        h.act(Action::RefreshSessions).await;
        let sessions_op = h
            .runtime
            .work_active
            .expect("the session list owns the slot")
            .operation;
        h.message(UiMsg::Sessions {
            operation: sessions_op,
            sessions: Vec::new(),
        })
        .await;

        assert!(
            h.runtime.pending_open.is_some(),
            "the session list aborted the open and left its reservation behind"
        );
        assert_eq!(
            h.runtime.pending_open.as_ref().unwrap().operation,
            operation,
            "the reservation was replaced by the session list"
        );
        assert!(h.app.session_transition);
        // The session list ends its own label and no other. The transition
        // keeps saying it is outstanding, which is what Escape acts on.
        assert!(!h.app.is_working_on(Work::Sessions));
        assert!(
            h.app.is_working_on(Work::Open),
            "the session list ended the resume's label as well as its own"
        );
        assert_eq!(
            h.app.on_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE
            )),
            Action::Cancel,
            "Escape ignored a session change that was still in flight"
        );
        h.act(Action::Cancel).await;
        assert!(h.runtime.pending_open.is_none());
        assert!(!h.app.session_transition);
        assert!(
            h.idle.is_some(),
            "cancelling the transition did not give the session back"
        );
        assert!(h.app.settings_are_editable());
        // The label goes with the reservation. Leaving it behind would
        // keep "resuming…" on screen and mask every later request, since
        // it is the highest-priority kind.
        assert!(
            !h.app.is_working_on(Work::Open),
            "the cancelled resume left its label behind"
        );
        assert!(h.app.loading().is_none());
    }

    /// Round 2, finding 4: overlapping MCP actions each own their token,
    /// and a completion from a superseded one changes nothing.
    #[tokio::test]
    async fn overlapping_mcp_actions_keep_their_own_tokens_and_completions() {
        let mut h = harness().await;
        let (first, first_token) = h.runtime.begin_mcp();
        h.app.begin_work(Work::Mcp, "applying the MCP change");
        let (second, second_token) = h.runtime.begin_mcp();
        assert_ne!(first, second);
        assert!(
            first_token.is_cancelled(),
            "the superseded operation was never told to stop"
        );
        assert!(!second_token.is_cancelled());

        // The first one finishes its cleanup afterwards. It must not clear
        // the second's token or its loading line.
        h.message(UiMsg::McpOutcome {
            operation: first,
            result: Ok(()),
        })
        .await;
        assert!(
            h.runtime.mcp.is_some(),
            "a late completion took the live operation's token"
        );
        assert!(
            h.app.loading().is_some(),
            "a late completion cleared the live operation's label"
        );
        // Escape can still reach the operation that is really running.
        h.act(Action::Cancel).await;
        assert!(second_token.is_cancelled());
        assert!(h.runtime.mcp.is_none());
    }

    /// Round 2, finding 6: a definition read before a turn started cannot
    /// put an approval in front of the user during that turn.
    #[tokio::test]
    async fn a_definition_arriving_after_a_turn_started_is_refused() {
        let mut h = harness().await;
        // The turn starts while the definition is being read.
        h.app.running = true;
        let definition_op = h.runtime.next_operation();
        h.runtime.spawn(
            &mut h.app,
            Work::McpDefinition,
            definition_op,
            "reading the server definition",
            async {},
        );
        h.message(UiMsg::McpApproval {
            operation: definition_op,
            server: "probe".into(),
            definition: "run: /usr/bin/probe".into(),
        })
        .await;
        assert!(
            h.app.pending.is_none(),
            "a launch approval opened during a running turn"
        );
        assert!(h.app.mcp_approval.is_none());
        assert!(h
            .app
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("a turn started"));

        // And the decision itself is refused if it reaches the runtime
        // during a turn anyway.
        h.act(Action::Mcp(McpRequest::Decide {
            server: "probe".into(),
            decision: gritt_core::mcp::TrustDecision::Approved,
        }))
        .await;
        assert!(
            h.runtime.mcp.is_none(),
            "an MCP mutation started during a running turn"
        );
    }
    /// A `git` that blocks, so the bounded worker pool can be filled on
    /// purpose.
    struct BlockingGit {
        delay: std::time::Duration,
    }

    impl crate::changes::GitRunner for BlockingGit {
        fn run(
            &self,
            _root: &std::path::Path,
            _args: &[&str],
        ) -> std::io::Result<std::process::Output> {
            std::thread::sleep(self.delay);
            Err(std::io::Error::other("not a repository"))
        }
    }

    /// Round 3, finding 1: a turn finishing must not wait for the
    /// workspace observer.
    ///
    /// Recording a write stats the file and a scan runs `git`, both under
    /// the same bounded pool. If the handler awaited them, a turn that had
    /// just written files — the moment the pool is busiest — would stop
    /// drawing and stop reading keys, Escape included.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn a_turn_finishing_does_not_wait_for_the_workspace_observer() {
        let mut h = harness().await;
        h.runtime.changes = Arc::new(WorkspaceChanges::with_git(
            "/tmp/ws",
            // Long enough that awaiting it would blow the 500 ms budget
            // below by a wide margin, short enough not to hold the test
            // binary's blocking pool open at exit.
            Arc::new(BlockingGit {
                delay: std::time::Duration::from_secs(2),
            }),
        ));
        // Fill every blocking slot the observer is allowed to use.
        for _ in 0..4 {
            scan_changes(&h.runtime, h.app.sidebar.generation);
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        // A turn that wrote a file finishes now.
        h.app.push(EntryKind::Tool, "-> file_write notes.txt");
        h.app.observed_writes.push("notes.txt".into());
        let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let finished = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            h.message(UiMsg::Finished {
                generation: h.app.sidebar.generation,
                agent: Box::new(StubDriver {
                    session: session("live"),
                    turns,
                }),
                result: Ok(TurnOutcome {
                    status: crate::agent::TurnStatus::Completed,
                    text: String::new(),
                    usage: Default::default(),
                    tool_calls: 0,
                    error: None,
                }),
            }),
        )
        .await;
        assert!(
            finished.is_ok(),
            "the turn-completion handler waited for the blocked workspace observer"
        );
        assert!(!h.app.running, "the turn was not marked finished");
        assert!(h.idle.is_some(), "the driver was not returned");
        assert!(
            h.app.observed_writes.is_empty(),
            "the write was not handed to the observer"
        );
    }

    /// Round 3, finding 2: an unrelated completion must not take away the
    /// ability to cancel what is still running.
    ///
    /// The reported sequence: a slow `/models` load, its picker closed,
    /// then an MCP restart. The catalog response arrives and clears its
    /// own label; the restart is still running and must still be
    /// cancellable.
    #[tokio::test]
    async fn a_catalog_response_cannot_disable_cancelling_an_mcp_restart() {
        let mut h = harness().await;
        // A slow catalog load, started from `/models`.
        h.act(Action::LoadCatalog {
            profile: "openrouter".into(),
            selection: h.app.selection,
        })
        .await;
        assert!(h.app.is_working_on(Work::Catalog));

        // An MCP action starts while it is still in flight.
        let (_id, token) = h.runtime.begin_mcp();
        h.app.begin_work(Work::Mcp, "applying the MCP change");

        // The catalog answers. It clears its own label and nothing else.
        let catalog_op = h
            .runtime
            .work_active
            .expect("the catalog owns the slot")
            .operation;
        h.message(UiMsg::Catalog {
            operation: catalog_op,
            selection: h.app.selection,
            profile: "openrouter".into(),
            result: Err(gritt_core::Error::config("no")),
        })
        .await;
        assert!(!h.app.is_working_on(Work::Catalog));
        assert!(
            h.app.is_working_on(Work::Mcp),
            "an unrelated completion cleared the MCP operation's label"
        );

        // Escape still produces a cancellation, and it reaches the MCP
        // operation rather than finding nothing to do.
        assert_eq!(
            h.app.on_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE
            )),
            Action::Cancel,
            "Escape had nothing to cancel while an MCP action was running"
        );
        h.act(Action::Cancel).await;
        assert!(
            token.is_cancelled(),
            "the MCP operation was never told to stop"
        );
        assert!(h.runtime.mcp.is_none());
        assert!(!h.app.is_busy());
    }
    /// Round 4, finding 1: approving a server must stay cancellable after
    /// an unrelated request has come and gone.
    ///
    /// The full sequence the review named, definition read included: read
    /// the definition, approve, start the launch, open `/models`, let the
    /// catalog finish, then Escape. The launch is detached and still
    /// running throughout; nothing in that sequence may end its label.
    #[tokio::test]
    async fn an_approved_launch_stays_cancellable_across_a_catalog_request() {
        let mut h = harness().await;
        // The definition read is ordinary work and owns the shared slot.
        let definition_op = h.runtime.next_operation();
        h.runtime.spawn(
            &mut h.app,
            Work::McpDefinition,
            definition_op,
            "reading the server definition",
            async {},
        );
        // It answers, which must retire its ownership as well as its label.
        h.message(UiMsg::McpApproval {
            operation: definition_op,
            server: "probe".into(),
            definition: "run: /usr/bin/probe".into(),
        })
        .await;
        assert!(!h.app.is_working_on(Work::McpDefinition));
        assert!(
            h.runtime.work_active.is_none(),
            "a finished request stayed installed as the owner of the slot"
        );
        assert!(h.app.pending.is_some(), "no approval overlay was shown");

        // Approving starts the launch: detached, with its own token.
        let action = h.app.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('y'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(action, Action::Mcp(McpRequest::Decide { .. })));
        let (_id, token) = h.runtime.begin_mcp();
        h.app.begin_work(Work::Mcp, "applying the MCP change");

        // An unrelated request starts and finishes while it runs.
        h.act(Action::LoadCatalog {
            profile: "openrouter".into(),
            selection: h.app.selection,
        })
        .await;
        let catalog_op = h
            .runtime
            .work_active
            .expect("the catalog owns the slot")
            .operation;
        h.message(UiMsg::Catalog {
            operation: catalog_op,
            selection: h.app.selection,
            profile: "openrouter".into(),
            result: Err(gritt_core::Error::config("no")),
        })
        .await;
        assert!(
            h.app.is_working_on(Work::Mcp),
            "the catalog request ended the live launch's label"
        );

        // Escape still reaches the launch.
        assert_eq!(
            h.app.on_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE
            )),
            Action::Cancel,
            "Escape had nothing to cancel while a launch was running"
        );
        h.act(Action::Cancel).await;
        assert!(token.is_cancelled(), "the launch was never told to stop");
        assert!(h.runtime.mcp.is_none());
        assert!(!h.app.is_busy());
    }
    /// Round 5: an older queued response must not retire a newer request
    /// of the same kind, and a cancelled request must not open its result
    /// later.
    ///
    /// The reported sequence, deterministically: diff A queues its result,
    /// diff B starts before it is handled, A's completion arrives, then
    /// `/new`. B has to stay cancellable through A's completion, and
    /// neither diff may open an overlay over the fresh draft.
    #[tokio::test]
    async fn a_queued_diff_cannot_retire_the_one_that_replaced_it() {
        let mut h = harness().await;

        // Diff A owns the slot and its result is on its way.
        h.act(Action::OpenFileDiff("a.txt".into())).await;
        let a = h
            .runtime
            .work_active
            .expect("diff A owns the slot")
            .operation;

        // Diff B supersedes it before A's response is handled.
        h.act(Action::OpenFileDiff("b.txt".into())).await;
        let b = h
            .runtime
            .work_active
            .expect("diff B owns the slot")
            .operation;
        assert_ne!(a, b);
        assert!(h.app.is_working_on(Work::Diff));

        // A's queued completion arrives. It is not the request that owns
        // the slot, so it changes nothing and shows nothing.
        h.message(UiMsg::Diff {
            operation: a,
            diff: crate::changes::FileDiff::Text {
                path: "a.txt".into(),
                body: "@@ -1 +1 @@\n-old\n+new\n".into(),
            },
        })
        .await;
        assert!(
            h.app.overlays.is_empty(),
            "a superseded diff opened its overlay"
        );
        assert_eq!(
            h.runtime.work_active.map(|active| active.operation),
            Some(b),
            "an older completion retired the request that replaced it"
        );
        assert!(
            h.app.is_working_on(Work::Diff),
            "an older completion ended the live request's label"
        );

        // `/new` must be able to cancel B.
        h.act(Action::NewSession).await;
        assert!(
            h.runtime.work_active.is_none(),
            "the live diff survived /new"
        );
        assert!(!h.app.is_working_on(Work::Diff));
        assert!(!h.app.is_busy());

        // And B's own result, arriving afterwards, cannot open a diff over
        // the fresh draft.
        h.message(UiMsg::Diff {
            operation: b,
            diff: crate::changes::FileDiff::Text {
                path: "b.txt".into(),
                body: "@@ -1 +1 @@\n-old\n+new\n".into(),
            },
        })
        .await;
        assert!(
            h.app.overlays.is_empty(),
            "a cancelled diff opened its overlay over the new draft"
        );
    }
}
