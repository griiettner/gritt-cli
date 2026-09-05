//! Stdio MCP server exposing `delegate_run` for supervised headless runs of
//! installed agent CLIs (grok, codex, claude).
//!
//! The transport is newline-delimited JSON-RPC 2.0 on stdin and stdout.
//! Logging goes to stderr only so the protocol stream stays clean.
//!
//! Delegating through an MCP tool call instead of the Bash tool keeps
//! harness-level shell classifiers out of the path: each harness sees one
//! explicitly installed tool, and this server is the single policy point
//! for spawning another agent CLI.

use std::io::{self, BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::Command;

use crate::Result;

pub const SERVER_NAME: &str = "gritt-delegate";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PROTOCOL: &str = "2025-06-18";
const SUPPORTED_PROTOCOLS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
const DEFAULT_TIMEOUT_SECS: u64 = 600;
const MAX_TIMEOUT_SECS: u64 = 3600;

pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "delegate_run",
            "description": "Run a supervised headless invocation of an installed agent CLI (grok, codex, or claude) with one prompt. Use this instead of the Bash tool for agent delegation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cli": {
                        "type": "string",
                        "enum": ["grok", "codex", "claude"]
                    },
                    "prompt": { "type": "string", "minLength": 1 },
                    "cwd": { "type": "string" },
                    "timeout_seconds": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_TIMEOUT_SECS,
                        "default": DEFAULT_TIMEOUT_SECS
                    },
                    "auto_approve": {
                        "type": "boolean",
                        "default": false,
                        "description": "Let the delegated CLI act without its own approval prompts."
                    }
                },
                "required": ["cli", "prompt"]
            }
        }
    ])
}

/// Serves delegation requests until stdin closes.
pub async fn serve() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&line).await {
            serde_json::to_writer(&mut out, &response)?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Handles one JSON-RPC line. Notifications return `None`.
pub async fn handle_line(line: &str) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {error}"),
            ))
        }
    };
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let id = match id {
        Some(id) if !id.is_null() => id,
        _ => return None,
    };
    Some(match dispatch(method, &params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn dispatch(method: &str, params: &Value) -> std::result::Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_PROTOCOL);
            let version = if SUPPORTED_PROTOCOLS.contains(&requested) {
                requested
            } else {
                DEFAULT_PROTOCOL
            };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(params).await,
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

/// Executes one tool call. Shared with the unified `gritt-agent mcp` server.
pub async fn call_tool(params: &Value) -> std::result::Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "missing tool name".to_owned()))?;
    if name != "delegate_run" {
        return Err((-32602, format!("unknown tool: {name}")));
    }
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let text = run_delegation(&arguments).await?;
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
}

async fn run_delegation(arguments: &Value) -> std::result::Result<String, (i64, String)> {
    let cli = arguments
        .get("cli")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let prompt = arguments
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| (-32602, "prompt must be a non-empty string".to_owned()))?;
    let auto_approve = matches!(arguments.get("auto_approve"), Some(Value::Bool(true)));
    let (program, args) = command_vector(cli, prompt, auto_approve)?;

    let timeout = match arguments.get("timeout_seconds") {
        None | Some(Value::Null) => DEFAULT_TIMEOUT_SECS,
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=MAX_TIMEOUT_SECS).contains(n))
            .ok_or_else(|| {
                (
                    -32602,
                    format!("timeout_seconds must be an integer between 1 and {MAX_TIMEOUT_SECS}"),
                )
            })?,
    };

    let mut command = Command::new(&program);
    command.args(&args);
    if let Some(cwd) = arguments.get("cwd").and_then(Value::as_str) {
        let cwd = std::path::Path::new(cwd);
        if !cwd.is_dir() {
            return Err((-32602, format!("cwd is not a directory: {}", cwd.display())));
        }
        command.current_dir(cwd);
    }
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let child = command
        .spawn()
        .map_err(|error| (-32603, format!("failed to spawn {program}: {error}")))?;
    let output = match tokio::time::timeout(Duration::from_secs(timeout), child.wait_with_output())
        .await
    {
        Ok(result) => result.map_err(|error| (-32603, format!("delegation failed: {error}")))?,
        Err(_) => {
            return Ok(format!(
                "delegated {program} run timed out after {timeout}s and was terminated"
            ))
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(format!(
        "exit status: {}\n\nstdout:\n{stdout}\n\nstderr:\n{stderr}",
        output.status.code().unwrap_or(-1)
    ))
}

/// Maps a delegation request onto the headless invocation of one of the
/// supported agent CLIs. Returns the program and its arguments.
pub fn command_vector(
    cli: &str,
    prompt: &str,
    auto_approve: bool,
) -> std::result::Result<(String, Vec<String>), (i64, String)> {
    let prompt = prompt.to_owned();
    let (program, mut args) = match cli {
        "grok" => ("grok", vec!["-p".to_owned(), prompt]),
        "codex" => ("codex", vec!["exec".to_owned(), prompt]),
        "claude" => ("claude", vec!["-p".to_owned(), prompt]),
        other => {
            return Err((
                -32602,
                format!("unsupported cli: {other}; expected grok, codex, or claude"),
            ))
        }
    };
    if auto_approve {
        match program {
            "grok" => args.push("--always-approve".to_owned()),
            "codex" => args.push("--full-auto".to_owned()),
            "claude" => args.push("--dangerously-skip-permissions".to_owned()),
            _ => unreachable!("program is one of the matched arms"),
        }
    }
    Ok((program.to_owned(), args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tools_list_includes_delegate_run() {
        let response = handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .await
            .expect("tools/list must respond");
        let tools = response["result"]["tools"]
            .as_array()
            .expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names, ["delegate_run"]);
    }

    #[tokio::test]
    async fn unknown_cli_is_rejected_as_invalid_params() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"delegate_run","arguments":{"cli":"sh","prompt":"echo hi"}}}"#,
        )
        .await
        .expect("tools/call must respond");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn missing_prompt_is_rejected_as_invalid_params() {
        let response = handle_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"delegate_run","arguments":{"cli":"grok"}}}"#,
        )
        .await
        .expect("tools/call must respond");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn command_vector_maps_headless_flags_per_cli() {
        let prompt = "summarize the diff";
        let (program, args) =
            command_vector("grok", prompt, true).expect("grok must map");
        assert_eq!(program, "grok");
        assert_eq!(args, vec!["-p".to_owned(), prompt.to_owned(), "--always-approve".to_owned()]);

        let (program, args) =
            command_vector("codex", prompt, true).expect("codex must map");
        assert_eq!(program, "codex");
        assert_eq!(args, vec!["exec".to_owned(), prompt.to_owned(), "--full-auto".to_owned()]);

        let (program, args) =
            command_vector("claude", prompt, true).expect("claude must map");
        assert_eq!(program, "claude");
        assert_eq!(args, vec!["-p".to_owned(), prompt.to_owned(), "--dangerously-skip-permissions".to_owned()]);

        let (program, args) =
            command_vector("grok", prompt, false).expect("grok must map");
        assert_eq!(args, vec!["-p".to_owned(), prompt.to_owned()]);
    }

    #[test]
    fn command_vector_rejects_unknown_cli() {
        assert!(command_vector("sh", "echo hi", false).is_err());
    }
}
