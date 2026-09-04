//! Stdio MCP server exposing `search_local_memory` and `read_local_memory`.
//!
//! The transport is newline-delimited JSON-RPC 2.0 on stdin and stdout.
//! Logging goes to stderr only so the protocol stream stays clean.

use std::io::{self, BufRead, Write};
use std::path::Path;

use rusqlite::Connection;
use serde_json::{json, Value};

use super::{db, index, search};
use crate::Result;

pub const SERVER_NAME: &str = "gritt-local-memory";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PROTOCOL: &str = "2025-06-18";
const SUPPORTED_PROTOCOLS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];
const MAX_LIMIT: u64 = 50;

pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "search_local_memory",
            "description": "Search the local gritt-cli workspace knowledge index with SQLite FTS5. Returns chunk citations as path:start-end line ranges.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "minLength": 1 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "default": 10 }
                },
                "required": ["query"]
            }
        },
        {
            "name": "read_local_memory",
            "description": "Read one indexed local knowledge document by its workspace-relative path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "minLength": 1 }
                },
                "required": ["path"]
            }
        }
    ])
}

/// Indexes the workspace, then serves requests until stdin closes.
pub fn serve(repo: &Path) -> Result<()> {
    let summary = index::index_workspace(repo)?;
    index::report(&summary);
    let connection = db::open(repo)?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&connection, &line) {
            serde_json::to_writer(&mut out, &response)?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Handles one JSON-RPC line. Notifications return `None`.
pub fn handle_line(connection: &Connection, line: &str) -> Option<Value> {
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
    Some(match dispatch(connection, method, &params) {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn dispatch(
    connection: &Connection,
    method: &str,
    params: &Value,
) -> std::result::Result<Value, (i64, String)> {
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
        "tools/call" => call_tool(connection, params),
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

fn call_tool(connection: &Connection, params: &Value) -> std::result::Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "missing tool name".to_owned()))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let text = match name {
        "search_local_memory" => {
            let query = arguments
                .get("query")
                .and_then(Value::as_str)
                .filter(|q| !q.is_empty())
                .ok_or_else(|| (-32602, "query must be a non-empty string".to_owned()))?;
            let limit = match arguments.get("limit") {
                None | Some(Value::Null) => 10,
                Some(value) => integral_u64(value)
                    .filter(|n| (1..=MAX_LIMIT).contains(n))
                    .ok_or_else(|| {
                        (
                            -32602,
                            format!("limit must be an integer between 1 and {MAX_LIMIT}"),
                        )
                    })?,
            };
            match search::search(connection, query, limit as usize) {
                Ok(hits) => search::format_hits(&hits),
                Err(error) => return Ok(tool_error(&error.message)),
            }
        }
        "read_local_memory" => {
            let path = arguments
                .get("path")
                .and_then(Value::as_str)
                .filter(|p| !p.is_empty())
                .ok_or_else(|| (-32602, "path must be a non-empty string".to_owned()))?;
            match search::read_document(connection, path) {
                Ok(document) => search::format_document(path, document.as_ref()),
                Err(error) => return Ok(tool_error(&error.message)),
            }
        }
        other => return Err((-32602, format!("unknown tool: {other}"))),
    };
    Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": false }))
}

/// Reads a JSON number as an integer, accepting integral floats such as `5.0`.
fn integral_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|f| f.fract() == 0.0 && *f >= 0.0)
            .map(|f| f as u64)
    })
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_and_list_tools() {
        let connection = db::open_in_memory().unwrap();
        let init = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], SERVER_NAME);
        assert!(handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
        )
        .is_none());
        let list = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        )
        .unwrap();
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 2);
        let unknown =
            handle_line(&connection, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn rejects_bad_arguments() {
        let connection = db::open_in_memory().unwrap();
        let response = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_local_memory","arguments":{"query":"x","limit":0}}}"#,
        )
        .unwrap();
        assert_eq!(response["error"]["code"], -32602);
        let float = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"search_local_memory","arguments":{"query":"x","limit":5.0}}}"#,
        )
        .unwrap();
        assert_eq!(float["result"]["isError"], false);
        let missing = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_local_memory","arguments":{"path":"nope.md"}}}"#,
        )
        .unwrap();
        assert_eq!(
            missing["result"]["content"][0]["text"],
            "No local document exists at nope.md."
        );
    }
}
