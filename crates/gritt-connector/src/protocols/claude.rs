//! Claude Code connector. Targets `claude` 2.1.x: `claude -p
//! --output-format stream-json --verbose` prints one JSON message per
//! line (`system`, `assistant`, `user`, `result`, `rate_limit_event`,
//! `stream_event`), and `--resume <session_id>` continues a session.
//! Approvals are not relayed: in print mode Claude Code applies its own
//! permission mode, which the user selects through `extra_args`.

use std::collections::HashMap;

use gritt_core::connector::{AuthState, ConnectorCapabilities, ConnectorId, TaskRequest};
use gritt_core::event::{EventKind, SessionStatus, StopReason};
use gritt_core::ErrorKind;

use super::{model_flag, number, render, text, tool_call, tool_result};
use crate::health::ProbeOutput;
use crate::supervise::{usage, Normalized, Normalizer, Protocol};

pub struct ClaudeCode;

impl Protocol for ClaudeCode {
    fn id(&self) -> ConnectorId {
        ConnectorId::ClaudeCode
    }

    fn executable(&self) -> &'static str {
        "claude"
    }

    fn vendor_install(&self) -> Option<crate::install::VendorInstall> {
        Some(crate::install::VendorInstall {
            installer: "Claude Code native installer",
            markers: &[".local/share/claude/", ".claude/local/"],
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
        Some(vec!["auth".into(), "status".into()])
    }

    fn auth_state(&self, probe: &ProbeOutput) -> AuthState {
        let parsed: Option<serde_json::Value> = serde_json::from_str(probe.stdout.trim()).ok();
        match parsed.and_then(|v| v.get("loggedIn").and_then(|b| b.as_bool())) {
            Some(true) => AuthState::Authenticated,
            Some(false) => AuthState::Unauthenticated,
            None => {
                let text = probe.stdout.to_ascii_lowercase();
                if text.contains("\"loggedin\": true") || text.contains("logged in: true") {
                    AuthState::Authenticated
                } else if text.contains("not logged in") {
                    AuthState::Unauthenticated
                } else {
                    AuthState::Unknown
                }
            }
        }
    }

    fn task_args(&self, request: &TaskRequest, external_id: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "-p".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
        ];
        if let Some(id) = external_id {
            args.push("--resume".into());
            args.push(id.to_owned());
        }
        args.extend(model_flag(request.model.as_deref()));
        args.push(request.prompt.clone());
        args
    }

    fn normalizer(&self) -> Box<dyn Normalizer> {
        Box::new(ClaudeNormalizer::default())
    }
}

#[derive(Default)]
pub struct ClaudeNormalizer {
    session_id: Option<String>,
    terminal: bool,
    tool_names: HashMap<String, String>,
}

fn content_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| match item.get("text").and_then(|t| t.as_str()) {
                Some(text) => text.to_owned(),
                None => render(item),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => render(other),
    }
}

impl Normalizer for ClaudeNormalizer {
    fn message(&mut self, value: serde_json::Value) -> Vec<Normalized> {
        let Some(kind) = text(&value, "type") else {
            // The final `result` line of some versions has no `type`.
            if value.get("stop_reason").is_some() && value.get("session_id").is_some() {
                return self.result(&value);
            }
            return vec![Normalized::unknown(&value)];
        };
        if let Some(id) = text(&value, "session_id") {
            if self.session_id.is_none() {
                self.session_id = Some(id);
            }
        }
        match kind.as_str() {
            "system" => {
                let subtype = text(&value, "subtype").unwrap_or_default();
                if subtype == "init" {
                    vec![Normalized::with(
                        EventKind::StatusChanged {
                            status: SessionStatus::Connecting,
                        },
                        serde_json::json!({
                            "session_id": self.session_id,
                            "model": text(&value, "model"),
                            "permission_mode": text(&value, "permissionMode"),
                            "tools": value.get("tools").map(|t| t.as_array().map(|a| a.len()).unwrap_or(0)),
                        }),
                    )]
                } else {
                    vec![Normalized::with(
                        EventKind::StatusChanged {
                            status: SessionStatus::Streaming,
                        },
                        serde_json::json!({ "system": subtype }),
                    )]
                }
            }
            "assistant" => {
                let mut events = Vec::new();
                let content = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .cloned()
                    .unwrap_or_default();
                for block in content {
                    match text(&block, "type").as_deref() {
                        Some("text") => events.push(Normalized::new(EventKind::TextDelta {
                            text: text(&block, "text").unwrap_or_default(),
                        })),
                        Some("thinking") => {
                            events.push(Normalized::new(EventKind::ReasoningSummary {
                                text: text(&block, "thinking").unwrap_or_default(),
                            }))
                        }
                        Some("tool_use") => {
                            let id = text(&block, "id").unwrap_or_default();
                            let name = text(&block, "name").unwrap_or_default();
                            self.tool_names.insert(id.clone(), name.clone());
                            events.push(Normalized::new(EventKind::StatusChanged {
                                status: SessionStatus::RunningTool,
                            }));
                            events.push(Normalized::new(tool_call(
                                &id,
                                &name,
                                block
                                    .get("input")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            )));
                        }
                        _ => events.push(Normalized::unknown(&block)),
                    }
                }
                events
            }
            "user" => {
                let mut events = Vec::new();
                let content = value
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .cloned()
                    .unwrap_or_default();
                for block in content {
                    if text(&block, "type").as_deref() != Some("tool_result") {
                        continue;
                    }
                    let id = text(&block, "tool_use_id").unwrap_or_default();
                    let name = self
                        .tool_names
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| "tool".into());
                    let is_error = block
                        .get("is_error")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    events.push(Normalized::new(tool_result(
                        &id,
                        &name,
                        content_text(block.get("content").unwrap_or(&serde_json::Value::Null)),
                        is_error,
                    )));
                }
                events
            }
            "result" => self.result(&value),
            "stream_event" => Vec::new(),
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

impl ClaudeNormalizer {
    fn result(&mut self, value: &serde_json::Value) -> Vec<Normalized> {
        self.terminal = true;
        let mut events = Vec::new();
        if let Some(used) = value.get("usage") {
            events.push(Normalized::new(usage(
                number(used, "input_tokens"),
                number(used, "output_tokens"),
                used.get("output_tokens_details")
                    .and_then(|d| number(d, "thinking_tokens")),
                number(used, "cache_read_input_tokens"),
            )));
        }
        let subtype = text(value, "subtype").unwrap_or_default();
        let is_error = value
            .get("is_error")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
            || subtype.starts_with("error");
        let diagnostic = serde_json::json!({
            "session_id": self.session_id,
            "subtype": subtype,
            "duration_ms": number(value, "duration_ms"),
            "num_turns": number(value, "num_turns"),
            "total_cost_usd": value.get("total_cost_usd"),
            "terminal_reason": text(value, "terminal_reason"),
        });
        if is_error {
            events.push(Normalized::with(
                EventKind::Error {
                    error_kind: ErrorKind::Connector,
                    message: text(value, "result")
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| format!("claude ended with {subtype}")),
                },
                diagnostic,
            ));
        } else {
            let stop = match text(value, "stop_reason").as_deref() {
                Some("max_tokens") => StopReason::MaxTokens,
                Some("end_turn") | Some("stop_sequence") | None => StopReason::EndTurn,
                Some(_) => StopReason::Other,
            };
            events.push(Normalized::with(
                EventKind::Completed { stop_reason: stop },
                diagnostic,
            ));
        }
        events
    }
}
