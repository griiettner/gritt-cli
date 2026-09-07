//! Cursor CLI connector. Targets the documented `cursor-agent -p
//! --output-format stream-json` interface (one JSON message per line:
//! `system`, `user`, `assistant`, `thinking`, `tool_call`, `result`) and
//! `--resume <session_id>`. Cursor's CLI is not installed on the
//! development machine, so this mapping was written from the published
//! format and is covered by hand-authored fixtures only. `info()` reports
//! `NotInstalled` until the executable is on PATH.

use std::collections::HashMap;

use gritt_core::connector::{
    AuthState, ConnectorCapabilities, ConnectorId, ConnectorModel, TaskRequest,
};
use gritt_core::event::{EventKind, SessionStatus, StopReason};
use gritt_core::tool::native;
use gritt_core::ErrorKind;

use super::{model_flag, number, render, text, tool_call, tool_result, ModelParseError};
use crate::health::ProbeOutput;
use crate::models::strip_ansi;
use crate::supervise::{Normalized, Normalizer, Protocol};

pub struct Cursor;

impl Protocol for Cursor {
    fn id(&self) -> ConnectorId {
        ConnectorId::Cursor
    }

    /// Only the documented name. A bare `agent` on PATH belongs to other
    /// tools and must not be mistaken for Cursor.
    fn executable(&self) -> &'static str {
        "cursor-agent"
    }

    fn vendor_install(&self) -> Option<crate::install::VendorInstall> {
        Some(crate::install::VendorInstall {
            installer: "Cursor CLI installer",
            markers: &[".local/share/cursor-agent/"],
            update_args: &["update"],
        })
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
        Some(vec!["status".into()])
    }

    fn auth_state(&self, probe: &ProbeOutput) -> AuthState {
        let text = format!("{}\n{}", probe.stdout, probe.stderr).to_ascii_lowercase();
        if text.contains("not logged in") || text.contains("not authenticated") {
            AuthState::Unauthenticated
        } else if text.contains("logged in") || text.contains("authenticated") {
            AuthState::Authenticated
        } else {
            AuthState::Unknown
        }
    }

    fn task_args(&self, request: &TaskRequest, external_id: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
        ];
        if let Some(id) = external_id {
            args.push("--resume".into());
            args.push(id.to_owned());
        }
        args.extend(model_flag(request.model.as_deref()));
        args.push(request.prompt.clone());
        args
    }

    /// The published reference documents `mcp list` as an interactive
    /// menu, not a machine-readable listing, so nothing is scraped.
    fn mcp_list_unsupported_reason(&self) -> String {
        "cursor-agent mcp list opens an interactive menu; no machine-readable listing is documented"
            .to_owned()
    }

    fn model_list_args(&self, _refresh: bool) -> Option<Vec<String>> {
        Some(vec!["--list-models".into()])
    }

    fn model_list_source(&self) -> &'static str {
        "cursor-agent --list-models"
    }

    fn parse_models(
        &self,
        stdout: &str,
        _stderr: &str,
    ) -> std::result::Result<Vec<ConnectorModel>, ModelParseError> {
        parse_cursor_models(stdout)
    }

    fn normalizer(&self) -> Box<dyn Normalizer> {
        Box::new(CursorNormalizer::default())
    }
}

/// `cursor-agent --list-models` prints one model per line. Markers such as
/// `(default)` and `(current)` are labels, not part of the id.
pub fn parse_cursor_models(
    stdout: &str,
) -> std::result::Result<Vec<ConnectorModel>, ModelParseError> {
    let text = strip_ansi(stdout);
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("available")
            || lower.starts_with("models")
            || lower.starts_with("usage:")
            || lower.starts_with("error")
        {
            continue;
        }
        let stripped = line.trim_start_matches(['*', '-', '•', '>']).trim();
        let mut tokens = stripped.split_whitespace();
        let Some(first) = tokens.next() else {
            continue;
        };
        let id = first.trim_end_matches([':', ',']).to_owned();
        if id.is_empty() || !looks_like_model_id(&id) {
            continue;
        }
        let rest: Vec<&str> = tokens.collect();
        let label = rest
            .iter()
            .filter(|token| {
                let t = token.trim_matches(['(', ')']).to_ascii_lowercase();
                t != "default" && t != "current" && t != "preview"
            })
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        out.push(ConnectorModel {
            id,
            display_label: (!label.is_empty()).then_some(label),
        });
    }
    if out.is_empty() {
        return Err(ModelParseError::Malformed);
    }
    Ok(out)
}

fn looks_like_model_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let rest_ok = id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '/');
    let has_separator =
        id.contains('-') || id.contains('/') || id.contains('.') || id.contains('_');
    (first.is_ascii_alphanumeric() || first == '_' || first == '.') && rest_ok && has_separator
}

#[derive(Default)]
pub struct CursorNormalizer {
    session_id: Option<String>,
    terminal: bool,
    tool_names: HashMap<String, String>,
}

/// Cursor wraps each call in an object keyed by its kind, such as
/// `readToolCall`, `writeToolCall`, or `shellToolCall`.
fn call_shape(
    tool_call: &serde_json::Value,
) -> Option<(String, serde_json::Value, Option<serde_json::Value>)> {
    let object = tool_call.as_object()?;
    let (key, body) = object.iter().next()?;
    let name = match key.as_str() {
        "readToolCall" => native::FILE_READ.to_owned(),
        "writeToolCall" | "editToolCall" => native::FILE_WRITE.to_owned(),
        "shellToolCall" => native::SHELL.to_owned(),
        other => other.trim_end_matches("ToolCall").to_owned(),
    };
    Some((
        name,
        body.get("args").cloned().unwrap_or(serde_json::Value::Null),
        body.get("result").cloned(),
    ))
}

impl Normalizer for CursorNormalizer {
    fn message(&mut self, value: serde_json::Value) -> Vec<Normalized> {
        if let Some(id) = text(&value, "session_id") {
            if self.session_id.is_none() {
                self.session_id = Some(id);
            }
        }
        let Some(kind) = text(&value, "type") else {
            return vec![Normalized::unknown(&value)];
        };
        match kind.as_str() {
            "system" => vec![Normalized::with(
                EventKind::StatusChanged {
                    status: SessionStatus::Connecting,
                },
                serde_json::json!({ "session_id": self.session_id, "model": text(&value, "model"), "subtype": text(&value, "subtype") }),
            )],
            "user" => Vec::new(),
            "thinking" => vec![Normalized::new(EventKind::ReasoningSummary {
                text: text(&value, "text").unwrap_or_default(),
            })],
            "assistant" => value
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|block| {
                    text(&block, "text").map(|text| Normalized::new(EventKind::TextDelta { text }))
                })
                .collect(),
            "tool_call" => {
                let id = text(&value, "call_id").unwrap_or_default();
                let subtype = text(&value, "subtype").unwrap_or_default();
                let Some((name, args, result)) = value.get("tool_call").and_then(call_shape) else {
                    return vec![Normalized::unknown(&value)];
                };
                match subtype.as_str() {
                    "started" => {
                        self.tool_names.insert(id.clone(), name.clone());
                        vec![
                            Normalized::new(EventKind::StatusChanged {
                                status: SessionStatus::RunningTool,
                            }),
                            Normalized::new(tool_call(&id, &name, args)),
                        ]
                    }
                    "completed" => {
                        let name = self.tool_names.remove(&id).unwrap_or(name);
                        let result = result.unwrap_or(serde_json::Value::Null);
                        let is_error = result.get("error").is_some_and(|e| !e.is_null())
                            || result
                                .get("success")
                                .and_then(|s| s.as_bool())
                                .is_some_and(|ok| !ok);
                        vec![Normalized::new(tool_result(
                            &id,
                            &name,
                            render(&result),
                            is_error,
                        ))]
                    }
                    _ => vec![Normalized::unknown(&value)],
                }
            }
            "result" => {
                self.terminal = true;
                let subtype = text(&value, "subtype").unwrap_or_default();
                let diagnostic = serde_json::json!({
                    "session_id": self.session_id,
                    "subtype": subtype,
                    "duration_ms": number(&value, "duration_ms"),
                });
                if value
                    .get("is_error")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false)
                    || subtype.starts_with("error")
                {
                    vec![Normalized::with(
                        EventKind::Error {
                            error_kind: ErrorKind::Connector,
                            message: text(&value, "result")
                                .filter(|t| !t.is_empty())
                                .unwrap_or_else(|| format!("cursor ended with {subtype}")),
                        },
                        diagnostic,
                    )]
                } else {
                    vec![Normalized::with(
                        EventKind::Completed {
                            stop_reason: StopReason::EndTurn,
                        },
                        diagnostic,
                    )]
                }
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
