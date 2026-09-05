//! Unified stdio MCP server for Gritt's harness-integral tools.
//!
//! One server per harness session exposes every Gritt tool family (local
//! memory, agent delegation) so each harness needs a single MCP entry.
//! The transport is newline-delimited JSON-RPC 2.0 on stdin and stdout.
//! Logging goes to stderr only so the protocol stream stays clean.

use std::io::{self, BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};
use turso::Connection;

use crate::delegate;
use crate::memory;
use crate::Result;

pub const SERVER_NAME: &str = "gritt-agent";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_PROTOCOL: &str = "2025-06-18";
const SUPPORTED_PROTOCOLS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

const MEMORY_TOOLS: [&str; 2] = ["search_local_memory", "read_local_memory"];
const DELEGATE_TOOLS: [&str; 1] = ["delegate_run"];

pub fn tool_definitions() -> Value {
    let mut tools = memory::mcp::tool_definitions().as_array().cloned().unwrap_or_default();
    if let Some(delegate) = delegate::mcp::tool_definitions().as_array().cloned() {
        tools.extend(delegate);
    }
    Value::Array(tools)
}

/// Indexes the workspace, then serves all Gritt tools until stdin closes.
pub async fn serve(repo: &Path) -> Result<()> {
    let summary = memory::index::index_workspace(repo).await?;
    memory::index::report(&summary);
    let connection = memory::db::open(repo).await?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&connection, &line).await {
            serde_json::to_writer(&mut out, &response)?;
            out.write_all(b"\n")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Handles one JSON-RPC line. Notifications return `None`.
pub async fn handle_line(connection: &Connection, line: &str) -> Option<Value> {
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
    Some(match dispatch(connection, method, &params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error_response(id, code, &message),
    })
}

async fn dispatch(
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
        "tools/call" => call_tool(connection, params).await,
        _ => Err((-32601, format!("method not found: {method}"))),
    }
}

async fn call_tool(
    connection: &Connection,
    params: &Value,
) -> std::result::Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "missing tool name".to_owned()))?;
    if MEMORY_TOOLS.contains(&name) {
        memory::mcp::call_tool(connection, params).await
    } else if DELEGATE_TOOLS.contains(&name) {
        delegate::mcp::call_tool(params).await
    } else {
        Err((-32602, format!("unknown tool: {name}")))
    }
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::db;

    #[tokio::test]
    async fn initialize_reports_unified_server() {
        let connection = db::open_in_memory().await.unwrap();
        let response = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#,
        )
        .await
        .expect("initialize must respond");
        assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
    }

    #[tokio::test]
    async fn tools_list_merges_memory_and_delegate_tools() {
        let connection = db::open_in_memory().await.unwrap();
        let response = handle_line(&connection, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .expect("tools/list must respond");
        let tools = response["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(
            names,
            ["search_local_memory", "read_local_memory", "delegate_run"]
        );
    }

    #[tokio::test]
    async fn memory_tool_routes_to_memory_search() {
        let connection = db::open_in_memory().await.unwrap();
        let response = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_local_memory","arguments":{"query":"anything"}}}"#,
        )
        .await
        .expect("tools/call must respond");
        assert_eq!(response["result"]["isError"], false);
    }

    #[tokio::test]
    async fn delegate_tool_routes_to_delegation() {
        let connection = db::open_in_memory().await.unwrap();
        let response = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"delegate_run","arguments":{"cli":"sh","prompt":"echo hi"}}}"#,
        )
        .await
        .expect("tools/call must respond");
        assert_eq!(response["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn unknown_tool_is_rejected() {
        let connection = db::open_in_memory().await.unwrap();
        let response = handle_line(
            &connection,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"bogus","arguments":{}}}"#,
        )
        .await
        .expect("tools/call must respond");
        assert_eq!(response["error"]["code"], -32602);
    }
}
