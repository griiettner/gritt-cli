//! REPL mode: a line loop over print mode with history, phase commands,
//! session listing, and resume. Cancellation of a running turn comes from
//! the binary's Ctrl-C handler through the agent's cancel handle.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

use gritt_core::session::{Phase, SessionStore};
use gritt_core::Result;

use crate::agent::{AgentBuilder, CancelHandle, NativeAgent, SessionSelector, TurnStatus};
use crate::modes::print::{PrintUi, PrintUiOptions};

/// Where the binary's Ctrl-C handler finds the running turn, if any.
pub type CancelSlot = Arc<Mutex<Option<CancelHandle>>>;

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

/// Runs the loop until `/quit` or end of input. Returns the agent so the
/// caller can inspect the final session.
pub async fn run_repl<I: BufRead, O: Write + Send, E: Write + Send>(
    builder: &AgentBuilder,
    mut agent: NativeAgent,
    input: &mut I,
    out: O,
    err: E,
    options: PrintUiOptions,
    cancel_slot: CancelSlot,
) -> Result<NativeAgent> {
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
        let mut line = String::new();
        let read = input.read_line(&mut line).unwrap_or(0);
        if read == 0 {
            break;
        }
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
                let sessions = builder.store.list().await?;
                let (out, _) = ui.parts_mut();
                for session in sessions {
                    let _ = writeln!(
                        out,
                        "{}  {}  {:?}  {}",
                        session.name,
                        &session.id.0[..8.min(session.id.0.len())],
                        session.phase,
                        session.updated_at.to_rfc3339()
                    );
                }
            }
            ReplCommand::Resume(name) => {
                match builder
                    .open(SessionSelector::Named(name.clone()), None, None, None)
                    .await
                {
                    Ok(resumed) => {
                        agent = resumed;
                        let events = builder.store.read_events(&agent.session().id).await?;
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
                        let (_, err) = ui.parts_mut();
                        let _ = writeln!(err, "error: {error}");
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
