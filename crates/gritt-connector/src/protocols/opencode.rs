//! OpenCode connector. Targets `opencode` 1.15.x: `opencode run --format
//! json` prints one JSON event per line (`step_start`, `text`,
//! `reasoning`, `tool_use`, `step_finish`, `error`), each carrying the
//! `sessionID`; `--session <id>` continues a session. Approvals are not
//! relayed: `run` applies OpenCode's own permission configuration.

use std::collections::HashSet;

use gritt_core::connector::{
    AuthState, ConnectorCapabilities, ConnectorId, ConnectorModel, TaskRequest,
};
use gritt_core::event::{EventKind, SessionStatus, StopReason};
use gritt_core::ErrorKind;

use super::{model_flag, number, render, text, tool_call, tool_result, ModelParseError};
use crate::health::ProbeOutput;
use crate::models::strip_ansi;
use crate::supervise::{usage, Normalized, Normalizer, Protocol};

pub struct OpenCode;

impl Protocol for OpenCode {
    fn id(&self) -> ConnectorId {
        ConnectorId::OpenCode
    }

    fn executable(&self) -> &'static str {
        "opencode"
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            follow_up_input: true,
            approvals: false,
            cancel: true,
            resume: true,
            inspect: true,
            structured_events: true,
        }
    }

    fn auth_probe_args(&self) -> Option<Vec<String>> {
        Some(vec!["auth".into(), "list".into()])
    }

    /// `auth list` counts stored credentials. Zero is `Unknown`, not
    /// `Unauthenticated`: OpenCode can also take providers from its own
    /// config and environment.
    fn auth_state(&self, probe: &ProbeOutput) -> AuthState {
        let text = format!("{}\n{}", probe.stdout, probe.stderr);
        let count = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|pair| pair[1].starts_with("credential"))
            .and_then(|pair| pair[0].parse::<u64>().ok());
        match count {
            Some(0) | None => AuthState::Unknown,
            Some(_) => AuthState::Authenticated,
        }
    }

    fn task_args(&self, request: &TaskRequest, external_id: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "run".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--dir".to_owned(),
            request.workspace.display().to_string(),
        ];
        if let Some(id) = external_id {
            args.push("--session".into());
            args.push(id.to_owned());
        }
        args.extend(model_flag(request.model.as_deref()));
        args.push(request.prompt.clone());
        args
    }

    fn model_list_args(&self, refresh: bool) -> Option<Vec<String>> {
        let mut args = vec!["models".to_owned()];
        if refresh {
            args.push("--refresh".into());
        }
        Some(args)
    }

    fn model_list_source(&self) -> &'static str {
        "opencode models"
    }

    fn parse_models(
        &self,
        stdout: &str,
        _stderr: &str,
    ) -> std::result::Result<Vec<ConnectorModel>, ModelParseError> {
        parse_opencode_models(stdout)
    }

    fn normalizer(&self) -> Box<dyn Normalizer> {
        Box::new(OpenCodeNormalizer::default())
    }
}

/// `opencode models` prints one `provider/id` line. JSON blobs from
/// `--verbose` are skipped so a display name is only taken from the id.
pub fn parse_opencode_models(
    stdout: &str,
) -> std::result::Result<Vec<ConnectorModel>, ModelParseError> {
    let text = strip_ansi(stdout);
    let mut out = Vec::new();
    let mut depth = 0usize;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        depth += line.chars().filter(|c| *c == '{').count();
        depth = depth.saturating_sub(line.chars().filter(|c| *c == '}').count());
        if depth > 0 || line.starts_with('{') || line.starts_with('}') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("warning") || lower.starts_with("error") {
            continue;
        }
        if let Some((provider, model)) = line.split_once('/') {
            if provider
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && !model.is_empty()
                && !model.contains(' ')
            {
                out.push(ConnectorModel {
                    id: line.to_owned(),
                    display_label: None,
                });
            }
        }
    }
    if out.is_empty() && text.chars().any(|c| !c.is_whitespace()) {
        return Err(ModelParseError::Malformed);
    }
    Ok(out)
}

#[derive(Default)]
pub struct OpenCodeNormalizer {
    session_id: Option<String>,
    terminal: bool,
    called: HashSet<String>,
}

impl Normalizer for OpenCodeNormalizer {
    fn message(&mut self, value: serde_json::Value) -> Vec<Normalized> {
        if let Some(id) = text(&value, "sessionID") {
            if self.session_id.is_none() {
                self.session_id = Some(id);
            }
        }
        let Some(kind) = text(&value, "type") else {
            return vec![Normalized::unknown(&value)];
        };
        let part = value
            .get("part")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        match kind.as_str() {
            "step_start" => vec![Normalized::with(
                EventKind::StatusChanged {
                    status: SessionStatus::Streaming,
                },
                serde_json::json!({ "session_id": self.session_id }),
            )],
            "text" => vec![Normalized::new(EventKind::TextDelta {
                text: text(&part, "text").unwrap_or_default(),
            })],
            "reasoning" => vec![Normalized::new(EventKind::ReasoningSummary {
                text: text(&part, "text").unwrap_or_default(),
            })],
            "tool_use" => {
                let id = text(&part, "callID").unwrap_or_default();
                let name = text(&part, "tool").unwrap_or_else(|| "tool".into());
                let state = part
                    .get("state")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let status = text(&state, "status").unwrap_or_default();
                let mut events = Vec::new();
                if self.called.insert(id.clone()) {
                    events.push(Normalized::new(EventKind::StatusChanged {
                        status: SessionStatus::RunningTool,
                    }));
                    events.push(Normalized::new(tool_call(
                        &id,
                        &name,
                        state
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )));
                }
                match status.as_str() {
                    "completed" => events.push(Normalized::with(
                        tool_result(
                            &id,
                            &name,
                            render(state.get("output").unwrap_or(&serde_json::Value::Null)),
                            false,
                        ),
                        serde_json::json!({ "title": text(&state, "title"), "metadata": state.get("metadata") }),
                    )),
                    "error" => events.push(Normalized::new(tool_result(
                        &id,
                        &name,
                        render(state.get("error").unwrap_or(&serde_json::Value::Null)),
                        true,
                    ))),
                    _ => {}
                }
                events
            }
            "step_finish" => {
                let mut events = Vec::new();
                if let Some(tokens) = part.get("tokens") {
                    events.push(Normalized::new(usage(
                        number(tokens, "input"),
                        number(tokens, "output"),
                        number(tokens, "reasoning"),
                        tokens.get("cache").and_then(|c| number(c, "read")),
                    )));
                }
                let reason = text(&part, "reason").unwrap_or_default();
                match reason.as_str() {
                    "tool-calls" | "tool_calls" => {}
                    "error" => {
                        self.terminal = true;
                        events.push(Normalized::with(
                            EventKind::Error {
                                error_kind: ErrorKind::Connector,
                                message: "opencode step failed".into(),
                            },
                            serde_json::json!({ "part": part }),
                        ));
                    }
                    other => {
                        self.terminal = true;
                        let stop = match other {
                            "length" => StopReason::MaxTokens,
                            "stop" | "end-turn" | "end_turn" | "" => StopReason::EndTurn,
                            _ => StopReason::Other,
                        };
                        events.push(Normalized::with(
                            EventKind::Completed { stop_reason: stop },
                            serde_json::json!({ "session_id": self.session_id, "reason": other, "cost": part.get("cost") }),
                        ));
                    }
                }
                events
            }
            "error" => {
                self.terminal = true;
                let message = value
                    .get("error")
                    .and_then(|e| text(e, "message").or_else(|| e.as_str().map(str::to_owned)))
                    .or_else(|| text(&part, "message"))
                    .or_else(|| text(&value, "message"))
                    .unwrap_or_else(|| "opencode reported an error".into());
                vec![Normalized::with(
                    EventKind::Error {
                        error_kind: ErrorKind::Connector,
                        message,
                    },
                    serde_json::json!({ "raw": value }),
                )]
            }
            _ => vec![Normalized::unknown(&value)],
        }
    }

    fn external_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    fn terminal_seen(&self) -> bool {
        self.terminal
    }
}
