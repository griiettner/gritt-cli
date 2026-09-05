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

use super::app::{Action, AgentSummary, App, EntryKind, McpRequest, PendingApproval, StatusBar};
use super::fixture::{self, FixtureScreen};
use super::render::draw;
use super::theme::Theme;
use crate::agent::{CancelHandle, TurnOutcome, Ui};
use crate::changes::{ChangedFiles, FileDiff, WorkspaceChanges};
use crate::control::{ControlPlane, DraftOpen, ProfileCatalog};
use crate::draft::SessionDraft;
use crate::driver::Driver;
use crate::policy::Decision;

enum UiMsg {
    Event(Event),
    Approval {
        pending: PendingApproval,
        responder: oneshot::Sender<ApprovalDecision>,
    },
    Finished {
        agent: Box<dyn Driver>,
        result: Result<TurnOutcome>,
    },
    /// A profile's model list, stamped with the selection token it was
    /// requested under. A token that no longer matches is a late result
    /// for a provider the user has already left.
    Catalog {
        selection: u64,
        profile: String,
        result: Result<ProfileCatalog>,
    },
    Sessions(Vec<Session>),
    /// The installed agents, probed off the terminal event path because
    /// probing runs external processes.
    Agents(Vec<AgentSummary>),
    /// A draft that was opened or refused. `generation` is the sidebar
    /// generation the request started under; a driver for a session the
    /// user has already left is closed instead of adopted.
    Opened {
        generation: u64,
        result: Result<DraftOpen>,
        /// The prompt that triggered the lazy open, run once the session
        /// exists. `None` for `/resume` and `/sessions`.
        prompt: Option<String>,
    },
    Changes {
        generation: u64,
        changes: ChangedFiles,
    },
    Diff(FileDiff),
    /// Live MCP state, from the runtime's subscription rather than a poll.
    Mcp(Vec<McpServerSnapshot>),
    McpOutcome(Result<()>),
    Setup {
        message: String,
        /// True when the write succeeded and the form should close.
        close: bool,
        /// True when the configuration should be re-read afterwards.
        reload: bool,
    },
}

struct ChannelUi {
    tx: mpsc::UnboundedSender<UiMsg>,
}

impl Ui for ChannelUi {
    fn event(&mut self, event: &Event) {
        let _ = self.tx.send(UiMsg::Event(event.clone()));
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
        let _ = self.tx.send(UiMsg::Approval { pending, responder });
        Box::pin(async move { receiver.await.unwrap_or(ApprovalDecision::Denied) })
    }
}

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
    loop {
        terminal
            .draw(|frame| draw(frame, &app))
            .map_err(|error| Error::config(format!("draw failed: {error}")))?;
        let action = tokio::select! {
            Some(event) = keys.recv() => match event {
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
            },
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
struct Runtime {
    plane: Arc<ControlPlane>,
    changes: Arc<WorkspaceChanges>,
    tx: mpsc::UnboundedSender<UiMsg>,
    /// The one cancellable background request. Starting another supersedes
    /// it, and Escape aborts it, so the loading line always describes work
    /// that is really running.
    work: Option<JoinHandle<()>>,
}

impl Runtime {
    /// Replaces the background request, aborting whatever it superseded.
    fn spawn(&mut self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        if let Some(previous) = self.work.take() {
            previous.abort();
        }
        self.work = Some(tokio::spawn(future));
    }

    /// Background work that is not the cancellable request: MCP delivery,
    /// change scans, and the agent probe. These are additive and never
    /// supersede the user's current request.
    fn spawn_detached(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        tokio::spawn(future);
    }

    fn cancel_work(&mut self) {
        if let Some(work) = self.work.take() {
            work.abort();
        }
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
pub async fn run_tui(
    plane: ControlPlane,
    agent: Option<Box<dyn Driver>>,
    draft: SessionDraft,
) -> Result<()> {
    let theme = theme_from_env();
    let mut terminal = enter()?;
    let result = event_loop(plane, agent, draft, &mut terminal, theme).await;
    leave();
    result
}

async fn event_loop(
    plane: ControlPlane,
    agent: Option<Box<dyn Driver>>,
    draft: SessionDraft,
    terminal: &mut ratatui::DefaultTerminal,
    theme: Theme,
) -> Result<()> {
    let mut app = App::new(StatusBar::default(), theme);
    app.draft = draft;
    app.status.workspace = plane.builder.workspace_root().display().to_string();
    app.sidebar.session.workspace = Some(app.status.workspace.clone());
    app.profiles = plane.profile_summaries();
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiMsg>();
    let mut runtime = Runtime {
        plane: Arc::new(plane),
        changes: Arc::new(WorkspaceChanges::new({
            let plane = &app.status.workspace;
            plane.clone()
        })),
        tx: ui_tx.clone(),
        work: None,
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
    subscribe_mcp(&runtime);
    let mut handle: Option<CancelHandle> = None;
    let mut responder: Option<oneshot::Sender<ApprovalDecision>> = None;
    let stop = Arc::new(AtomicBool::new(false));
    let mut keys = spawn_key_reader(Arc::clone(&stop));
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    loop {
        terminal
            .draw(|frame| draw(frame, &app))
            .map_err(|error| Error::config(format!("draw failed: {error}")))?;
        let action = tokio::select! {
            Some(msg) = ui_rx.recv() => {
                on_message(&mut app, &mut runtime, &mut idle_agent, &mut handle, &mut responder, msg).await?
            }
            Some(event) = keys.recv() => {
                match event {
                    TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
                    TerminalEvent::Paste(text) => {
                        app.on_paste(&text);
                        Action::None
                    }
                    TerminalEvent::Resize(columns, rows) => {
                        // Placement depends on the width; a drawer or a
                        // focused sidebar may no longer exist.
                        app.on_resize(columns, rows);
                        Action::None
                    }
                    _ => Action::None,
                }
            }
            _ = tick.tick() => Action::None,
        };
        on_action(
            &mut app,
            &mut runtime,
            &mut idle_agent,
            &mut handle,
            &mut responder,
            action,
        )
        .await?;
        if app.quit {
            break;
        }
    }
    stop.store(true, Ordering::SeqCst);
    runtime.cancel_work();
    if let Some(handle) = &handle {
        handle.cancel();
    }
    Ok(())
}

/// Rescans the workspace under the current sidebar generation.
fn scan_changes(runtime: &Runtime, generation: u64) {
    let changes = Arc::clone(&runtime.changes);
    let tx = runtime.tx.clone();
    runtime.spawn_detached(async move {
        changes.capture_baseline().await;
        let scanned = changes.scan().await;
        let _ = tx.send(UiMsg::Changes {
            generation,
            changes: scanned,
        });
    });
}

fn probe_agents(runtime: &Runtime) {
    let plane = Arc::clone(&runtime.plane);
    let tx = runtime.tx.clone();
    runtime.spawn_detached(async move {
        let _ = tx.send(UiMsg::Agents(agent_summaries(&plane).await));
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
        UiMsg::Event(event) => app.on_event(&event),
        UiMsg::Approval {
            pending,
            responder: sender,
        } => {
            *responder = Some(sender);
            app.request_approval(pending);
        }
        UiMsg::Finished { agent, result } => {
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
            for path in app.take_observed_writes() {
                runtime.changes.record_write(path).await;
            }
            scan_changes(runtime, app.sidebar.generation);
            if let Some(mcp) = runtime.plane.builder.mcp().cloned() {
                let tx = runtime.tx.clone();
                runtime.spawn_detached(async move {
                    let snapshots = mcp.refresh(&crate::CancellationToken::new()).await;
                    let _ = tx.send(UiMsg::Mcp(snapshots));
                });
            }
        }
        UiMsg::Catalog {
            selection,
            profile,
            result,
        } => {
            app.loading = None;
            match result {
                Ok(catalog) => {
                    app.apply_catalog(selection, &profile, catalog.models, catalog.state);
                }
                Err(error) => {
                    app.catalog_failed(selection, &profile, error.message);
                }
            }
        }
        UiMsg::Sessions(sessions) => {
            app.loading = None;
            app.load_sessions(sessions);
        }
        UiMsg::Agents(agents) => {
            app.agents = agents;
            // The connection dialog may already be open; its rows fill in
            // where the user is looking rather than after they close it.
            if app.top_overlay().and_then(super::app::Overlay::picker_kind)
                == Some(super::app::PickerKind::Connect)
            {
                let rows = app.connection_picker().rows().to_vec();
                if let Some(super::app::Overlay::Picker { picker, .. }) = app.overlays.last_mut() {
                    picker.replace_rows(rows);
                }
            }
        }
        UiMsg::Opened {
            generation,
            result,
            prompt,
        } => {
            app.loading = None;
            if !app.sidebar.accepts(generation) {
                // The user left this session while it was opening. The
                // driver is dropped rather than adopted over the current
                // one, and the prompt is not sent anywhere.
                return Ok(Action::None);
            }
            match result {
                Ok(DraftOpen::Opened {
                    driver,
                    catalog: _,
                    warnings,
                }) => {
                    adopt(app, &runtime.plane, driver.as_ref()).await?;
                    app.show_draft_warnings(&warnings);
                    *idle_agent = Some(driver);
                    scan_changes(runtime, app.sidebar.generation);
                    if let Some(prompt) = prompt {
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
                    if let Some(prompt) = prompt {
                        app.undo_submission(&prompt);
                    }
                    app.show_draft_errors(&errors);
                }
                Err(error) => {
                    if let Some(prompt) = prompt {
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
        UiMsg::Diff(diff) => {
            app.loading = None;
            app.show_file_diff(diff);
        }
        UiMsg::Mcp(snapshots) => app.apply_mcp(snapshots),
        UiMsg::McpOutcome(result) => {
            app.loading = None;
            if let Err(error) = result {
                app.notice = Some(error.message);
            }
        }
        UiMsg::Setup {
            message,
            close,
            reload,
        } => {
            app.loading = None;
            if reload {
                // A saved profile is only usable once the running
                // configuration has it. Rebuilding the plane shares every
                // handle, so nothing already open is disturbed.
                if let Some(reloaded) = runtime.plane.reloaded() {
                    runtime.plane = Arc::new(reloaded);
                }
                app.profiles = runtime.plane.profile_summaries();
            }
            app.setup_outcome(message, close);
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
            *responder = None;
            if app.loading.take().is_some() {
                runtime.cancel_work();
                app.notice = Some("cancelled".into());
            }
        }
        Action::Approve(decision) => {
            if let Some(sender) = responder.take() {
                let _ = sender.send(decision);
            }
        }
        Action::Submit(prompt) => {
            if let Some(mut agent) = idle_agent.take() {
                app.sidebar.session.activity = Some("running".into());
                *handle = Some(agent.handle());
                let tx = runtime.tx.clone();
                tokio::spawn(async move {
                    let mut ui = ChannelUi { tx: tx.clone() };
                    let result = agent.run_turn(&prompt, &mut ui).await;
                    let _ = tx.send(UiMsg::Finished { agent, result });
                });
            } else if app.running && app.session_id.is_none() {
                // The lazy path: no session exists yet, so the first
                // prompt is what creates one from the draft.
                app.loading = Some("opening the session".into());
                open_draft(app, runtime, Some(prompt));
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
            app.loading = Some("loading sessions".into());
            runtime.spawn(async move {
                let sessions = plane.builder.store.list().await.unwrap_or_default();
                let _ = tx.send(UiMsg::Sessions(sessions));
            });
        }
        Action::Resume(id) => {
            // Resuming keeps the stored profile, model, and effort: the
            // draft only names the session.
            app.loading = Some("resuming".into());
            let draft = SessionDraft::default();
            let named = id.0.clone();
            let plane = Arc::clone(&runtime.plane);
            let tx = runtime.tx.clone();
            let generation = app.sidebar.generation;
            runtime.spawn(async move {
                let result = resume_by_id(&plane, &named, draft).await;
                let _ = tx.send(UiMsg::Opened {
                    generation,
                    result,
                    prompt: None,
                });
            });
        }
        Action::LoadCatalog { profile, selection } => {
            app.loading = Some(format!("loading {profile} models"));
            let plane = Arc::clone(&runtime.plane);
            let tx = runtime.tx.clone();
            runtime.spawn(async move {
                let result = plane.catalog(&profile).await;
                let _ = tx.send(UiMsg::Catalog {
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
            app.loading = Some("saving the profile".into());
            let setup = Arc::clone(runtime.plane.setup());
            let tx = runtime.tx.clone();
            // Config and keychain writes are blocking file and system
            // calls; they never run on the loop.
            runtime.spawn(async move {
                let message =
                    tokio::task::spawn_blocking(move || write_profile(setup.as_ref(), submission))
                        .await
                        .unwrap_or_else(|_| ("the setup task did not finish".to_owned(), false));
                let _ = tx.send(UiMsg::Setup {
                    message: message.0,
                    close: message.1,
                    reload: message.1,
                });
            });
        }
        Action::NewSession => {
            // The previous session is not deleted; its driver is released
            // so the next prompt opens a fresh one from the draft.
            *idle_agent = None;
            *handle = None;
            *responder = None;
            runtime.cancel_work();
            app.loading = None;
            scan_changes(runtime, app.sidebar.generation);
        }
        Action::Mcp(request) => {
            let Some(mcp) = runtime.plane.builder.mcp().cloned() else {
                app.notice = Some("no MCP runtime is configured for this workspace".into());
                return Ok(());
            };
            app.loading = Some("applying the MCP change".into());
            let tx = runtime.tx.clone();
            runtime.spawn(async move {
                let cancel = crate::CancellationToken::new();
                let result = match request {
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
                let _ = tx.send(UiMsg::McpOutcome(result));
            });
        }
        Action::ScanChanges => scan_changes(runtime, app.sidebar.generation),
        Action::OpenFileDiff(path) => {
            app.loading = Some("reading the diff".into());
            let changes = Arc::clone(&runtime.changes);
            let tx = runtime.tx.clone();
            runtime.spawn(async move {
                let diff = changes.diff(&path).await;
                let _ = tx.send(UiMsg::Diff(diff));
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
fn open_draft(app: &mut App, runtime: &mut Runtime, prompt: Option<String>) {
    let draft = app.draft.clone();
    let plane = Arc::clone(&runtime.plane);
    let tx = runtime.tx.clone();
    let generation = app.sidebar.generation;
    runtime.spawn(async move {
        let result = plane.open_draft(draft).await;
        let _ = tx.send(UiMsg::Opened {
            generation,
            result,
            prompt,
        });
    });
}

/// Resumes by session id through the same draft operation, so a resumed
/// session gets the same pinning and effort rules as any other.
async fn resume_by_id(
    plane: &ControlPlane,
    id: &str,
    mut draft: SessionDraft,
) -> Result<DraftOpen> {
    let session = plane
        .builder
        .store
        .get(&gritt_core::session::SessionId(id.to_owned()))
        .await?;
    let Some(session) = session else {
        return Err(Error::storage(format!("session `{id}` is gone")));
    };
    draft.name = Some(session.name);
    plane.open_draft(draft).await
}

/// Writes a profile and, when one was typed, its key. Returns the line to
/// show and whether the flow may close.
fn write_profile(
    setup: &dyn crate::setup::ProviderSetup,
    submission: super::app::SetupSubmission,
) -> (String, bool) {
    use crate::setup::{CredentialStoreOutcome, ProfileSaveOutcome};
    let saved = setup.save_profile(&submission.profile, submission.destination);
    let (message, ok) = match saved {
        ProfileSaveOutcome::Saved {
            path, shadowed_by, ..
        } => (
            match shadowed_by {
                Some(layer) => format!(
                    "saved to {}, but a {:?} configuration already defines this profile and wins",
                    path.display(),
                    layer
                ),
                None => format!("saved to {}", path.display()),
            },
            true,
        ),
        ProfileSaveOutcome::Invalid { problem } => {
            (format!("the profile is not valid: {problem:?}"), false)
        }
        ProfileSaveOutcome::Unavailable { reason } => (reason, false),
        ProfileSaveOutcome::Failed { message } => (message, false),
    };
    if !ok {
        return (message, false);
    }
    let Some(secret) = submission.secret else {
        return (
            format!(
                "{message}; no key was typed, so {} must be set in the environment",
                submission.profile.key.env_var_name
            ),
            true,
        );
    };
    match setup.store_credential(&submission.profile, secret) {
        CredentialStoreOutcome::Stored { .. } => {
            (format!("{message}; the key went to the keychain"), true)
        }
        CredentialStoreOutcome::KeychainUnavailable {
            env_var_name,
            message: reason,
            ..
        } => (
            format!(
                "{message}, but the keychain is unavailable ({reason}). Export {env_var_name} instead."
            ),
            // The profile exists; only the key did not land. The flow
            // closes so the profile is usable once the variable is set.
            true,
        ),
        CredentialStoreOutcome::Unavailable { reason } => {
            (format!("{message}, but no keychain writer is available: {reason}"), true)
        }
    }
}
