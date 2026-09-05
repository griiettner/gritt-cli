//! A scriptable MCP server over stdio, for the runtime's tests.
//!
//! It exists only so the tests can drive real child processes through the
//! real transport: handshake, pagination, tool errors, malformed output,
//! timeouts, and shutdown. It is not part of the product, and nothing in
//! `gritt-harness` links against it.
//!
//! Usage: `gritt-mcp-fixture <behavior>`. Each behavior is one scenario the
//! runtime has to survive.

use std::io::{BufRead, Write};

fn send(value: &serde_json::Value) {
    let mut stdout = std::io::stdout().lock();
    // One line per message, as the stdio transport requires.
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
}

fn tool(name: &str, description: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
        },
    })
}

fn main() {
    let behavior = std::env::args().nth(1).unwrap_or_else(|| "basic".into());
    match behavior.as_str() {
        // Exits before it can answer anything.
        "crash" => {
            eprintln!("fixture: refusing to start");
            std::process::exit(3);
        }
        // Valid line framing, invalid protocol content.
        "garbage" => {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "this is not JSON-RPC");
            let _ = stdout.flush();
            std::thread::sleep(std::time::Duration::from_secs(30));
            return;
        }
        // Never answers. The client's deadline has to end the wait.
        "silent" => {
            std::thread::sleep(std::time::Duration::from_secs(300));
            return;
        }
        _ => {}
    }

    let stdin = std::io::stdin();
    let mut cancelled: Vec<i64> = Vec::new();
    let mut called = false;
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let id = message.get("id").cloned();
        match method.as_str() {
            "notifications/cancelled" => {
                if let Some(request) = message
                    .get("params")
                    .and_then(|params| params.get("requestId"))
                    .and_then(serde_json::Value::as_i64)
                {
                    cancelled.push(request);
                    // Report the cancellation the client asked for, so the
                    // test can prove the notification really arrived.
                    eprintln!("fixture: cancelled {request}");
                }
            }
            "initialize" => {
                let version = match behavior.as_str() {
                    // A revision from a future Gritt does not know.
                    "future" => "2099-01-01",
                    // An older revision Gritt still supports.
                    "old" => "2024-11-05",
                    _ => "2025-06-18",
                };
                let capabilities = if behavior == "notools" {
                    serde_json::json!({})
                } else {
                    serde_json::json!({"tools": {"listChanged": true}})
                };
                send(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": version,
                        "capabilities": capabilities,
                        "serverInfo": {"name": "fixture", "version": "0.1.0"},
                    },
                }));
            }
            "notifications/initialized" => {}
            "ping" => send(&serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "tools/list" => {
                let cursor = message
                    .get("params")
                    .and_then(|params| params.get("cursor"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let result = match behavior.as_str() {
                    "paged" => match cursor.as_str() {
                        "" => serde_json::json!({
                            "tools": [tool("first", "page one")],
                            "nextCursor": "page-2"
                        }),
                        "page-2" => serde_json::json!({
                            "tools": [tool("second", "page two")],
                            "nextCursor": "page-3"
                        }),
                        _ => serde_json::json!({"tools": [tool("third", "page three")]}),
                    },
                    // Pages forever with the same cursor.
                    "loop" => serde_json::json!({
                        "tools": [tool("again", "same page")],
                        "nextCursor": "stuck"
                    }),
                    "listchanged" if called => {
                        serde_json::json!({"tools": [tool("search", "after the change"),
                                                     tool("extra", "appeared later")]})
                    }
                    _ => serde_json::json!({
                        "tools": [tool("search", "search this server"),
                                  tool("echo", "return the text given")]
                    }),
                };
                send(&serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}));
            }
            "tools/call" => {
                called = true;
                let params = message.get("params").cloned().unwrap_or_default();
                let name = params
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let text = params
                    .get("arguments")
                    .and_then(|arguments| arguments.get("text"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                match behavior.as_str() {
                    // Leaves proof on disk that the call arrived, so a test
                    // can show a denied call never got here.
                    "marker" => {
                        if let Ok(path) = std::env::var("FIXTURE_MARKER") {
                            let _ = std::fs::write(&path, &name);
                        }
                        send(&serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"content": [{"type": "text",
                                "text": format!("{name} ran")}]}
                        }));
                    }
                    // Records the call, then never answers it.
                    "slowcall" => {
                        eprintln!("fixture: received {name}");
                        continue;
                    }
                    "toolerror" => send(&serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {"content": [{"type": "text",
                            "text": "the upstream API rejected the query"}], "isError": true}
                    })),
                    "unknowntool" => send(&serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32602, "message": format!("Unknown tool: {name}")}
                    })),
                    "structured" => send(&serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "content": [
                                {"type": "text", "text": "two rows"},
                                {"type": "image", "data": "AAAA", "mimeType": "image/png"}
                            ],
                            "structuredContent": {"rows": 2},
                        }
                    })),
                    // Proves the child's environment, not the parent's.
                    "env" => {
                        let declared =
                            std::env::var("FIXTURE_DECLARED").unwrap_or_else(|_| "unset".into());
                        let leaked = std::env::var("GRITT_TEST_LEAK_API_KEY")
                            .unwrap_or_else(|_| "unset".into());
                        let cwd = std::env::current_dir().unwrap_or_default();
                        send(&serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"content": [{"type": "text", "text": format!(
                                "declared={declared} leaked={leaked} cwd={} args={}",
                                cwd.display(),
                                std::env::args().skip(1).collect::<Vec<_>>().join(" ")
                            )}]}
                        }));
                    }
                    _ => {
                        if behavior == "listchanged" {
                            send(&serde_json::json!({"jsonrpc": "2.0",
                                "method": "notifications/tools/list_changed"}));
                        }
                        send(&serde_json::json!({
                            "jsonrpc": "2.0", "id": id,
                            "result": {"content": [{"type": "text",
                                "text": format!("{name}: {text}")}]}
                        }));
                    }
                }
            }
            _ => {
                if let Some(id) = id {
                    send(&serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "method not found"}
                    }));
                }
            }
        }
    }
    // Standard input closed. A well-behaved server exits here; `lingering`
    // does not, so the runtime has to escalate.
    if behavior == "lingering" {
        std::thread::sleep(std::time::Duration::from_secs(120));
    }
}
