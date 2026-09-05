//! REPL mode: a line loop over print mode with history, phase commands,
//! session listing, and resume. Cancellation of a running turn comes from
//! the binary's Ctrl-C handler through the agent's cancel handle.

use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gritt_core::event::ApprovalDecision;
use gritt_core::session::{Phase, SessionStore};
use gritt_core::Result;

use crate::agent::{CancelHandle, SessionSelector, TurnStatus, Ui};
use crate::control::ControlPlane;
use crate::driver::Driver;
use crate::modes::print::{read_yes_no, PrintUi, PrintUiOptions, Prompter};

/// Where the binary's Ctrl-C handler finds the running turn, if any.
pub type CancelSlot = Arc<Mutex<Option<CancelHandle>>>;

/// The one owner of an input stream. A reader thread forwards lines over
/// a channel; the REPL loop and the approval prompter both take lines
/// from here, so neither can hold the underlying reader's lock while the
/// other waits on it. Cloning shares the same stream.
#[derive(Clone)]
pub struct LineInput {
    lines: Arc<Mutex<Receiver<String>>>,
}

impl LineInput {
    /// Starts the reader thread over `reader`. Lines keep their trailing
    /// newline. The thread ends at end of input or a read error.
    pub fn from_reader<R: BufRead + Send + 'static>(mut reader: R) -> Self {
        let (tx, rx): (SyncSender<String>, Receiver<String>) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            }
        });
        Self {
            lines: Arc::new(Mutex::new(rx)),
        }
    }

    /// Blocks for the next line; `None` at end of input.
    pub fn next_line(&self) -> Option<String> {
        self.lines.lock().expect("line input").recv().ok()
    }

    /// Blocks for the next line, giving up with `None` once `give_up`
    /// returns true (polled every 100 ms) or at end of input. A cancelled
    /// approval stops waiting this way, so the line the user types next
    /// reaches the loop instead of answering a question that is gone.
    pub fn next_line_until(&self, give_up: impl Fn() -> bool) -> Option<String> {
        let lines = self.lines.lock().expect("line input");
        loop {
            match lines.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => return Some(line),
                Err(RecvTimeoutError::Timeout) => {
                    if give_up() {
                        return None;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}

/// Answers approvals from the shared line input. `prompt` shows the
/// question first. The running turn's cancel handle is captured when the
/// question is asked, not read from the slot while waiting: the loop
/// clears the slot as soon as a cancelled turn returns, and a reader that
/// only consulted the slot could miss the cancellation and hold the input
/// for good. No handle at all means the turn is already over, so the
/// answer is a denial without waiting.
pub fn line_prompter(
    input: LineInput,
    slot: CancelSlot,
    prompt: impl Fn() + Send + Sync + 'static,
) -> Prompter {
    Arc::new(move |_, _, _| {
        prompt();
        let Some(handle) = slot.lock().expect("cancel slot").clone() else {
            return ApprovalDecision::Denied;
        };
        match input.next_line_until(|| handle.is_cancelled()) {
            Some(line) => read_yes_no(&mut std::io::Cursor::new(line.into_bytes())),
            None => ApprovalDecision::Denied,
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplCommand {
    Prompt(String),
    Plan,
    Code,
    Sessions,
    Resume(String),
    History,
    Help,
    Quit,
    Empty,
    Unknown(String),
}

pub fn parse_command(line: &str) -> ReplCommand {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ReplCommand::Empty;
    }
    if !trimmed.starts_with('/') {
        return ReplCommand::Prompt(trimmed.to_owned());
    }
    let mut parts = trimmed.splitn(2, ' ');
    let command = parts.next().unwrap_or_default();
    let argument = parts.next().map(str::trim).unwrap_or_default();
    match command {
        "/plan" => ReplCommand::Plan,
        "/code" => ReplCommand::Code,
        "/sessions" => ReplCommand::Sessions,
        "/resume" if !argument.is_empty() => ReplCommand::Resume(argument.to_owned()),
        "/history" => ReplCommand::History,
        "/help" => ReplCommand::Help,
        "/quit" | "/exit" => ReplCommand::Quit,
        other => ReplCommand::Unknown(other.to_owned()),
    }
}

pub const HELP: &str = "commands: /plan  /code  /sessions  /resume NAME  /history  /help  /quit\n\
Ctrl-C cancels a running turn; a second Ctrl-C at the prompt quits.";

/// Runs the loop until `/quit` or end of input. Returns the driver so the
/// caller can inspect the final session. Native and connector sessions
/// run through the same loop.
pub async fn run_repl<O: Write + Send, E: Write + Send>(
    plane: &ControlPlane,
    mut agent: Box<dyn Driver>,
    input: &LineInput,
    out: O,
    err: E,
    options: PrintUiOptions,
    cancel_slot: CancelSlot,
) -> Result<Box<dyn Driver>> {
    let mut ui = PrintUi::new(out, err, options);
    let mut history: Vec<String> = Vec::new();
    loop {
        {
            let (out, _) = ui.parts_mut();
            let _ = write!(
                out,
                "[{} {}] > ",
                agent.session().name,
                match agent.phase() {
                    Phase::Planning => "plan",
                    Phase::Coding => "code",
                }
            );
            let _ = out.flush();
        }
        let reader = input.clone();
        let Ok(Some(line)) = tokio::task::spawn_blocking(move || reader.next_line()).await else {
            break;
        };
        match parse_command(&line) {
            ReplCommand::Empty => {}
            ReplCommand::Quit => break,
            ReplCommand::Help => {
                let (out, _) = ui.parts_mut();
                let _ = writeln!(out, "{HELP}");
            }
            ReplCommand::History => {
                let (out, _) = ui.parts_mut();
                for (index, entry) in history.iter().enumerate() {
                    let _ = writeln!(out, "{:>3}  {entry}", index + 1);
                }
            }
            ReplCommand::Plan => {
                agent.set_phase(Phase::Planning).await?;
                let (out, _) = ui.parts_mut();
                let _ = writeln!(out, "phase: planning");
            }
            ReplCommand::Code => {
                agent.set_phase(Phase::Coding).await?;
                let (out, _) = ui.parts_mut();
                let _ = writeln!(out, "phase: coding");
            }
            ReplCommand::Sessions => {
                let sessions = plane.builder.store.list().await?;
                let (out, _) = ui.parts_mut();
                for session in sessions {
                    let _ = writeln!(
                        out,
                        "{}  {}  {:?}  {}  {}",
                        session.name,
                        &session.id.0[..8.min(session.id.0.len())],
                        session.phase,
                        match &session.kind {
                            gritt_core::session::SessionKind::Native {
                                provider_profile,
                                model,
                                ..
                            } => format!("{provider_profile}/{model}"),
                            gritt_core::session::SessionKind::Connector { id } =>
                                id.as_str().to_owned(),
                        },
                        session.updated_at.to_rfc3339()
                    );
                }
            }
            ReplCommand::Resume(name) => {
                match plane
                    .open(SessionSelector::Named(name.clone()), None, None, None, None)
                    .await
                {
                    Ok(resumed) => {
                        agent = resumed;
                        let events = plane
                            .builder
                            .store
                            .read_events(&Driver::session(agent.as_ref()).id)
                            .await?;
                        let (out, _) = ui.parts_mut();
                        let _ = writeln!(out, "resumed `{name}` ({} events)", events.len());
                    }
                    Err(error) => {
                        let (_, err) = ui.parts_mut();
                        let _ = writeln!(err, "error: {error}");
                    }
                }
            }
            ReplCommand::Unknown(command) => {
                let (_, err) = ui.parts_mut();
                let _ = writeln!(err, "unknown command {command}; try /help");
            }
            ReplCommand::Prompt(prompt) => {
                history.push(prompt.clone());
                *cancel_slot.lock().expect("cancel slot") = Some(agent.handle());
                let turn = agent.run_turn(&prompt, &mut ui).await;
                *cancel_slot.lock().expect("cancel slot") = None;
                match turn {
                    Ok(outcome) => {
                        ui.finish();
                        if outcome.status != TurnStatus::Completed {
                            let (_, err) = ui.parts_mut();
                            let _ = writeln!(err, "turn {:?}", outcome.status);
                        }
                    }
                    Err(error) => {
                        ui.finish();
                        // A failed turn already showed its error event;
                        // only an output failure has nothing on screen yet.
                        if ui.output_error().is_some() {
                            return Err(error);
                        }
                    }
                }
            }
        }
    }
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_input_forwards_lines_and_gives_up_on_request() {
        let input = LineInput::from_reader(std::io::Cursor::new(b"one\ntwo\n".to_vec()));
        assert_eq!(input.next_line().as_deref(), Some("one\n"));
        let shared = input.clone();
        assert_eq!(shared.next_line_until(|| false).as_deref(), Some("two\n"));
        assert_eq!(input.next_line(), None);

        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(1);
        let waiting = LineInput {
            lines: Arc::new(Mutex::new(rx)),
        };
        let started = std::time::Instant::now();
        assert_eq!(waiting.next_line_until(|| true), None);
        assert!(started.elapsed() < Duration::from_secs(2));
        tx.send("late\n".into()).unwrap();
        assert_eq!(waiting.next_line().as_deref(), Some("late\n"));
    }

    #[test]
    fn commands_parse() {
        assert_eq!(parse_command("  "), ReplCommand::Empty);
        assert_eq!(parse_command("hello"), ReplCommand::Prompt("hello".into()));
        assert_eq!(parse_command("/plan"), ReplCommand::Plan);
        assert_eq!(parse_command("/code"), ReplCommand::Code);
        assert_eq!(
            parse_command("/resume work"),
            ReplCommand::Resume("work".into())
        );
        assert_eq!(
            parse_command("/resume"),
            ReplCommand::Unknown("/resume".into())
        );
        assert_eq!(parse_command("/exit"), ReplCommand::Quit);
        assert_eq!(parse_command("/nope"), ReplCommand::Unknown("/nope".into()));
    }
}
