//! Codex CLI connector. Targets `codex` 0.153.x: `codex exec --json`
//! prints one JSON event per line (`thread.started`, `turn.started`,
//! `item.started`, `item.updated`, `item.completed`, `turn.completed`,
//! `turn.failed`, `error`), and `codex exec resume <thread_id> --json`
//! continues a thread. Approvals are not exposed in headless mode: Codex
//! runs under its own configured sandbox and approval policy.

use std::collections::HashMap;

use gritt_core::connector::{
    AuthState, ConnectorCapabilities, ConnectorId, ConnectorMcpServer, ConnectorMcpStatus,
    ConnectorModel, TaskRequest,
};
use gritt_core::event::{EventKind, SessionStatus, StopReason};
use gritt_core::tool::native;
use gritt_core::ErrorKind;

use super::{
    model_flag, number, render, text, tool_call, tool_result, McpParseError, ModelParseError,
};
use crate::health::ProbeOutput;
use crate::supervise::{usage, Normalized, Normalizer, Protocol};

pub struct Codex;

/// `codex debug models` prints `{"models":[{"slug":"...","display_name":"..."}]}`.
pub fn parse_codex_models(
    stdout: &str,
) -> std::result::Result<Vec<ConnectorModel>, ModelParseError> {
    let body = stdout.trim();
    let start = body.find('{').ok_or(ModelParseError::Malformed)?;
    let value: serde_json::Value =
        serde_json::from_str(&body[start..]).map_err(|_| ModelParseError::Malformed)?;
    let models = value
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or(ModelParseError::Malformed)?;
    let mut out = Vec::new();
    for model in models {
        let id = text(model, "slug")
            .or_else(|| text(model, "id"))
            .or_else(|| text(model, "model"))
            .ok_or(ModelParseError::Malformed)?;
        if id.is_empty() {
            return Err(ModelParseError::Malformed);
        }
        let display_label = text(model, "display_name").or_else(|| text(model, "displayName"));
        out.push(ConnectorModel { id, display_label });
    }
    Ok(out)
}

/// `codex mcp list --json` prints an array of `{"name","enabled",
/// "disabled_reason","transport":{"type","command"|"url",...},
/// "auth_status"}`. The listing reads configuration and runs no live
/// check, so an enabled server is `Enabled`, not `Connected`. Only the
/// name, transport type, command or URL, enabled flag, disabled reason,
/// and auth status are read: `args`, `env`, `env_vars`, headers, and
/// bearer token variables are never taken from the document.
pub fn parse_codex_mcp(
    stdout: &str,
) -> std::result::Result<Vec<ConnectorMcpServer>, McpParseError> {
    // The array is the first line-leading `[` that parses as JSON: a
    // diagnostic line printed before it may begin with a bracket of its
    // own, and anything after the array is ignored.
    let value = stdout
        .match_indices('[')
        .map(|(index, _)| index)
        .filter(|index| {
            let before = &stdout[..*index];
            before.trim().is_empty() || before.trim_end_matches([' ', '\t']).ends_with('\n')
        })
        .find_map(|index| {
            serde_json::Deserializer::from_str(&stdout[index..])
                .into_iter::<serde_json::Value>()
                .next()
                .and_then(Result::ok)
                .filter(serde_json::Value::is_array)
        })
        .ok_or(McpParseError::Malformed)?;
    let entries = value.as_array().ok_or(McpParseError::Malformed)?;
    let mut out = Vec::new();
    for entry in entries {
        let name = text(entry, "name").ok_or(McpParseError::Malformed)?;
        if name.is_empty() {
            return Err(McpParseError::Malformed);
        }
        let enabled = entry
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let transport = entry.get("transport");
        let kind = transport.and_then(|t| text(t, "type"));
        let target = transport.and_then(|t| text(t, "command").or_else(|| text(t, "url")));
        let auth = text(entry, "auth_status")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let (status, detail) = if !enabled {
            (ConnectorMcpStatus::Disabled, text(entry, "disabled_reason"))
        } else if auth.contains("not_logged_in") || auth.contains("not logged in") {
            (
                ConnectorMcpStatus::NeedsAuth,
                Some("not logged in".to_owned()),
            )
        } else {
            (ConnectorMcpStatus::Enabled, None)
        };
        out.push(ConnectorMcpServer {
            name,
            transport: kind,
            target,
            status,
            detail,
        });
    }
    Ok(out)
}

impl Protocol for Codex {
    fn id(&self) -> ConnectorId {
        ConnectorId::Codex
    }

    fn executable(&self) -> &'static str {
        "codex"
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
        Some(vec!["login".into(), "status".into()])
    }

    fn auth_state(&self, probe: &ProbeOutput) -> AuthState {
        let text = format!("{}\n{}", probe.stdout, probe.stderr).to_ascii_lowercase();
        if text.contains("not logged in") {
            AuthState::Unauthenticated
        } else if text.contains("logged in") {
            AuthState::Authenticated
        } else {
            AuthState::Unknown
        }
    }

    fn model_list_args(&self, _refresh: bool) -> Option<Vec<String>> {
        Some(vec!["debug".into(), "models".into()])
    }

    fn model_list_source(&self) -> &'static str {
        "codex debug models"
    }

    fn parse_models(
        &self,
        stdout: &str,
        _stderr: &str,
    ) -> std::result::Result<Vec<ConnectorModel>, ModelParseError> {
        parse_codex_models(stdout)
    }

    fn mcp_list_args(&self) -> Option<Vec<String>> {
        Some(vec!["mcp".into(), "list".into(), "--json".into()])
    }

    fn mcp_list_source(&self) -> &'static str {
        "codex mcp list --json"
    }

    fn parse_mcp_inventory(
        &self,
        stdout: &str,
        _stderr: &str,
    ) -> std::result::Result<Vec<ConnectorMcpServer>, McpParseError> {
        parse_codex_mcp(stdout)
    }

    fn task_args(&self, request: &TaskRequest, external_id: Option<&str>) -> Vec<String> {
        let mut args = vec!["exec".to_owned()];
        match external_id {
            Some(id) => {
                args.push("resume".into());
                args.push("--json".into());
                args.push("--skip-git-repo-check".into());
                args.push(id.to_owned());
            }
            None => {
                args.push("--json".into());
                args.push("--skip-git-repo-check".into());
                args.push("-C".into());
                args.push(request.workspace.display().to_string());
            }
        }
        args.extend(model_flag(request.model.as_deref()));
        args.push(request.prompt.clone());
        args
    }

    fn normalizer(&self) -> Box<dyn Normalizer> {
        Box::new(CodexNormalizer::default())
    }
}

#[derive(Default)]
pub struct CodexNormalizer {
    thread_id: Option<String>,
    terminal: bool,
    /// Item ids whose tool call was already emitted on `item.started`.
    started: HashMap<String, String>,
}

impl CodexNormalizer {
    fn item_call(
        &mut self,
        item: &serde_json::Value,
    ) -> Option<(String, String, serde_json::Value)> {
        let id = text(item, "id")?;
        let kind = text(item, "type")?;
        let (name, arguments) = match kind.as_str() {
            "command_execution" => (
                native::SHELL.to_owned(),
                serde_json::json!({ "command": text(item, "command").unwrap_or_default() }),
            ),
            "file_change" => (
                native::FILE_WRITE.to_owned(),
                serde_json::json!({ "changes": item.get("changes").cloned().unwrap_or(serde_json::Value::Null) }),
            ),
            "mcp_tool_call" => (
                format!(
                    "mcp:{}.{}",
                    text(item, "server").unwrap_or_default(),
                    text(item, "tool").unwrap_or_default()
                ),
                item.get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            ),
            "web_search" => (
                "web_search".to_owned(),
                serde_json::json!({ "query": text(item, "query").unwrap_or_default() }),
            ),
            _ => return None,
        };
        Some((id, name, arguments))
    }

    fn item_result(&self, item: &serde_json::Value, name: &str) -> Option<(String, bool)> {
        let kind = text(item, "type")?;
        Some(match kind.as_str() {
            "command_execution" => {
                let output = text(item, "aggregated_output").unwrap_or_default();
                let code = item.get("exit_code").and_then(|v| v.as_i64());
                let failed = text(item, "status").is_some_and(|s| s == "failed")
                    || code.is_some_and(|c| c != 0);
                (
                    if code.is_some_and(|c| c != 0) {
                        format!("{output}\n[exit code {}]", code.unwrap_or_default())
                    } else {
                        output
                    },
                    failed,
                )
            }
            "file_change" => (
                render(item.get("changes").unwrap_or(&serde_json::Value::Null)),
                text(item, "status").is_some_and(|s| s == "failed"),
            ),
            "mcp_tool_call" => {
                let error = item.get("error");
                (
                    render(
                        error
                            .or_else(|| item.get("result"))
                            .unwrap_or(&serde_json::Value::Null),
                    ),
                    error.is_some_and(|e| !e.is_null())
                        || text(item, "status").is_some_and(|s| s == "failed"),
                )
            }
            "web_search" => (format!("search: {name}"), false),
            _ => return None,
        })
    }
}

impl Normalizer for CodexNormalizer {
    fn message(&mut self, value: serde_json::Value) -> Vec<Normalized> {
        let Some(kind) = text(&value, "type") else {
            return vec![Normalized::unknown(&value)];
        };
        match kind.as_str() {
            "thread.started" => {
                self.thread_id = text(&value, "thread_id");
                vec![Normalized::with(
                    EventKind::StatusChanged {
                        status: SessionStatus::Connecting,
                    },
                    serde_json::json!({ "thread_id": self.thread_id }),
                )]
            }
            "turn.started" => vec![Normalized::new(EventKind::StatusChanged {
                status: SessionStatus::Streaming,
            })],
            "item.started" => {
                let Some(item) = value.get("item") else {
                    return vec![Normalized::unknown(&value)];
                };
                match self.item_call(item) {
                    Some((id, name, arguments)) => {
                        self.started.insert(id.clone(), name.clone());
                        vec![
                            Normalized::new(EventKind::StatusChanged {
                                status: SessionStatus::RunningTool,
                            }),
                            Normalized::with(
                                tool_call(&id, &name, arguments),
                                serde_json::json!({ "item_type": text(item, "type") }),
                            ),
                        ]
                    }
                    None => Vec::new(),
                }
            }
            "item.updated" => Vec::new(),
            "item.completed" => {
                let Some(item) = value.get("item") else {
                    return vec![Normalized::unknown(&value)];
                };
                let item_type = text(item, "type").unwrap_or_default();
                match item_type.as_str() {
                    "agent_message" => vec![Normalized::new(EventKind::TextDelta {
                        text: text(item, "text").unwrap_or_default(),
                    })],
                    "reasoning" => vec![Normalized::new(EventKind::ReasoningSummary {
                        text: text(item, "text").unwrap_or_default(),
                    })],
                    "error" => {
                        self.terminal = true;
                        vec![Normalized::with(
                            EventKind::Error {
                                error_kind: ErrorKind::Connector,
                                message: text(item, "message")
                                    .unwrap_or_else(|| "codex reported an error".into()),
                            },
                            serde_json::json!({ "item": item }),
                        )]
                    }
                    "todo_list" => vec![Normalized::with(
                        EventKind::StatusChanged {
                            status: SessionStatus::Streaming,
                        },
                        serde_json::json!({ "todo_list": item.get("items") }),
                    )],
                    _ => {
                        let mut events = Vec::new();
                        let Some((id, name, arguments)) = self.item_call(item) else {
                            return vec![Normalized::unknown(&value)];
                        };
                        let name = match self.started.remove(&id) {
                            Some(name) => name,
                            None => {
                                events.push(Normalized::new(tool_call(&id, &name, arguments)));
                                name
                            }
                        };
                        if let Some((output, is_error)) = self.item_result(item, &name) {
                            events.push(Normalized::with(
                                tool_result(&id, &name, output, is_error),
                                serde_json::json!({ "item_type": item_type, "status": text(item, "status") }),
                            ));
                        }
                        events
                    }
                }
            }
            "turn.completed" => {
                self.terminal = true;
                let mut events = Vec::new();
                if let Some(used) = value.get("usage") {
                    events.push(Normalized::new(usage(
                        number(used, "input_tokens"),
                        number(used, "output_tokens"),
                        number(used, "reasoning_output_tokens"),
                        number(used, "cached_input_tokens"),
                    )));
                }
                events.push(Normalized::with(
                    EventKind::Completed {
                        stop_reason: StopReason::EndTurn,
                    },
                    serde_json::json!({ "thread_id": self.thread_id }),
                ));
                events
            }
            "turn.failed" | "error" => {
                self.terminal = true;
                let message = value
                    .get("error")
                    .and_then(|e| text(e, "message"))
                    .or_else(|| text(&value, "message"))
                    .unwrap_or_else(|| "codex turn failed".into());
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
        self.thread_id.clone()
    }

    fn terminal_seen(&self) -> bool {
        self.terminal
    }
}
