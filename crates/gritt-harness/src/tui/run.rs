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
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event};
use gritt_core::session::{BoxFuture, SessionStore};
use gritt_core::{Error, Result};
use tokio::sync::{mpsc, oneshot};

use super::app::{Action, App, PendingApproval, StatusBar};
use super::fixture::{self, FixtureScreen};
use super::render::draw;
use super::theme::Theme;
use crate::agent::{CancelHandle, SessionSelector, TurnOutcome, Ui};
use crate::control::ControlPlane;
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

fn status_for(agent: &dyn Driver) -> StatusBar {
    let mut status = StatusBar::default();
    let mut app = App::new(StatusBar::default(), Theme::default());
    app.set_session(agent.session());
    let info = agent.info();
    status.profile = info.backend;
    status.model = info.detail;
    status.session = app.status.session;
    status.phase = app.status.phase;
    status.workspace = app.status.workspace;
    status.effort = app.status.effort;
    status
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
            _ => {}
        }
        if app.quit {
            break;
        }
    }
    stop.store(true, Ordering::SeqCst);
    Ok(())
}

/// Runs the full-screen mode until the user quits. Native and connector
/// sessions share the loop, the approval view, and the transcript.
pub async fn run_tui(plane: &ControlPlane, agent: Box<dyn Driver>) -> Result<()> {
    let theme = theme_from_env();
    let mut terminal = enter()?;
    let result = event_loop(plane, agent, &mut terminal, theme).await;
    leave();
    result
}

async fn event_loop(
    plane: &ControlPlane,
    agent: Box<dyn Driver>,
    terminal: &mut ratatui::DefaultTerminal,
    theme: Theme,
) -> Result<()> {
    let mut app = App::new(status_for(agent.as_ref()), theme);
    let history = plane.builder.store.read_events(&agent.session().id).await?;
    app.load_history(&history);
    let mut idle_agent: Option<Box<dyn Driver>> = Some(agent);
    let mut handle: Option<CancelHandle> = None;
    let mut responder: Option<oneshot::Sender<ApprovalDecision>> = None;
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiMsg>();
    let stop = Arc::new(AtomicBool::new(false));
    let mut keys = spawn_key_reader(Arc::clone(&stop));
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    loop {
        terminal
            .draw(|frame| draw(frame, &app))
            .map_err(|error| Error::config(format!("draw failed: {error}")))?;
        let action = tokio::select! {
            Some(msg) = ui_rx.recv() => {
                match msg {
                    UiMsg::Event(event) => {
                        app.on_event(&event);
                        Action::None
                    }
                    UiMsg::Approval { pending, responder: sender } => {
                        responder = Some(sender);
                        app.request_approval(pending);
                        Action::None
                    }
                    UiMsg::Finished { agent, result } => {
                        app.running = false;
                        handle = None;
                        if let Err(error) = result {
                            app.push(super::app::EntryKind::Error, error.message);
                        }
                        idle_agent = Some(agent);
                        Action::None
                    }
                }
            }
            Some(event) = keys.recv() => {
                match event {
                    TerminalEvent::Key(key) if key.kind != KeyEventKind::Release => app.on_key(key),
                    TerminalEvent::Paste(text) => {
                        app.on_paste(&text);
                        Action::None
                    }
                    TerminalEvent::Resize(_, _) => Action::None,
                    _ => Action::None,
                }
            }
            _ = tick.tick() => Action::None,
        };
        match action {
            Action::None => {}
            Action::Quit => break,
            Action::Cancel => {
                if let Some(handle) = &handle {
                    handle.cancel();
                }
                // A pending approval is denied by the loop on cancel; drop
                // the view and its responder so a late key cannot answer it.
                app.pending = None;
                responder = None;
            }
            Action::Approve(decision) => {
                if let Some(sender) = responder.take() {
                    let _ = sender.send(decision);
                }
            }
            Action::Submit(prompt) => {
                if let Some(mut agent) = idle_agent.take() {
                    app.sidebar.session.activity = Some("running".into());
                    handle = Some(agent.handle());
                    let tx = ui_tx.clone();
                    tokio::spawn(async move {
                        let mut ui = ChannelUi { tx: tx.clone() };
                        let result = agent.run_turn(&prompt, &mut ui).await;
                        let _ = tx.send(UiMsg::Finished { agent, result });
                    });
                } else {
                    app.running = false;
                    // The draft survives a submission that could not be sent.
                    app.restore_draft(&prompt);
                    app.notice = Some("no idle agent; your draft was kept".into());
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
                    app.notice = Some("finish the running turn first".into());
                }
            }
            Action::RefreshSessions => {
                // Into the picker that is already open, not just into the
                // list behind it: the overlay opens before the store
                // answers, so it has to fill in where the user is looking.
                app.load_sessions(plane.builder.store.list().await.unwrap_or_default());
            }
            Action::Resume(id) => {
                match plane
                    .open(SessionSelector::Id(id), None, None, None, None)
                    .await
                {
                    Ok(agent) => {
                        app.entries.clear();
                        // No value may survive from the previous driver.
                        app.sidebar.reset();
                        let history = plane.builder.store.read_events(&agent.session().id).await?;
                        app.load_history(&history);
                        app.set_session(agent.session());
                        let info = agent.info();
                        app.status.profile = info.backend;
                        app.status.model = info.detail;
                        idle_agent = Some(agent);
                    }
                    Err(error) => app.push(super::app::EntryKind::Error, error.message),
                }
            }
        }
        if app.quit {
            break;
        }
    }
    stop.store(true, Ordering::SeqCst);
    if let Some(handle) = &handle {
        handle.cancel();
    }
    Ok(())
}
