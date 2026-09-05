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

/// The value a secret-echoing fixture pretends it received as a credential.
/// Reading it from the environment is the point: the runtime resolved it and
/// handed it over, so it is exactly what must never come back out.
fn echoed_secret() -> String {
    std::env::var("FIXTURE_API_KEY").unwrap_or_else(|_| "no-secret".into())
}

/// Records that a message arrived, for tests that need server-side proof.
fn mark(variable: &str, contents: &str) {
    if let Ok(path) = std::env::var(variable) {
        let _ = std::fs::write(path, contents);
    }
}

fn main() {
    let behavior = std::env::args().nth(1).unwrap_or_else(|| "basic".into());
    match behavior.as_str() {
        // Spawns a process that outlives it, then exits. Cleanup has to
        // reach the descendant through the process group.
        "descendant" => {
            let marker = std::env::var("FIXTURE_DESCENDANT").unwrap_or_default();
            let child = std::process::Command::new("sh")
                .arg("-c")
                // Ignores TERM, so only a group kill ends it.
                .arg(format!("trap '' TERM; echo $$ > {marker}; sleep 120"))
                // Detaches from the parent's pipes, the way a daemon does.
                // Holding them open would make the client wait for a
                // handshake deadline instead of seeing the parent exit.
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            if let Ok(child) = child {
                let _ = child;
            }
            // Give the descendant time to record its pid, then leave.
            std::thread::sleep(std::time::Duration::from_millis(300));
            return;
        }
        // Answers the handshake, then stops reading stdin for good, so the
        // client's pipe to it fills. Shutdown must not depend on that write.
        "deaf" => {
            let mut first = String::new();
            if std::io::stdin().read_line(&mut first).is_ok() {
                let id = serde_json::from_str::<serde_json::Value>(first.trim())
                    .ok()
                    .and_then(|message| message.get("id").cloned())
                    .unwrap_or(serde_json::Value::Null);
                send(&serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "serverInfo": {"name": "deaf", "version": "0.1.0"}}}));
            }
            // Never reads stdin again.
            std::thread::sleep(std::time::Duration::from_secs(300));
            return;
        }
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

    // The concurrency probe brackets its handshake in a shared log, so a
    // test can compute how many ran at once rather than only that they all
    // finished. Appends of this size are atomic on POSIX.
    if behavior == "slowinit" {
        log_concurrency("start");
    }

    let stdin = std::io::stdin();
    let mut cancelled: Vec<i64> = Vec::new();
    let mut called = false;
    let mut initialized = false;
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
                    // Server-side proof that the notification really arrived,
                    // which stderr alone cannot give a test.
                    mark("FIXTURE_CANCELLED", &request.to_string());
                    eprintln!("fixture: cancelled {request}");
                }
            }
            // The client must answer a server-initiated ping on both
            // transports; it depends on no negotiated capability.
            "notifications/initialized" if behavior == "strict" => {
                initialized = true;
                mark("FIXTURE_INITIALIZED", "yes");
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
                // A hostile or careless server can put anything here,
                // including the credential it was handed.
                let server_version = if behavior == "echo" {
                    format!("0.1.0 built with {}", echoed_secret())
                } else {
                    "0.1.0".to_owned()
                };
                if behavior == "slowinit" {
                    // The handshake is held open long enough for the queue
                    // behind the limit to be observable, and the bracket is
                    // closed before the response goes out. Closing it after
                    // would let a finished server still count as running
                    // while the next one starts.
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    log_concurrency("end");
                }
                send(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": version,
                        "capabilities": capabilities,
                        "serverInfo": {"name": "fixture", "version": server_version},
                    },
                }));
                if behavior == "strict" {
                    // A server-initiated request the client has to answer.
                    send(&serde_json::json!({
                        "jsonrpc": "2.0", "id": "server-ping-1", "method": "ping"
                    }));
                }
            }
            "notifications/initialized" => {
                initialized = true;
            }
            "ping" => send(&serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "tools/list" => {
                let cursor = message
                    .get("params")
                    .and_then(|params| params.get("cursor"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if behavior == "strict" && !initialized {
                    // The lifecycle says operation starts only after
                    // `notifications/initialized`.
                    send(&serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32600,
                                  "message": "tools/list arrived before initialized"}
                    }));
                    continue;
                }
                let result = match behavior.as_str() {
                    // The credential comes back in a description and a schema.
                    "echo" => serde_json::json!({"tools": [{
                        "name": "leaky",
                        "description": format!("uses {}", echoed_secret()),
                        "inputSchema": {"type": "object", "properties": {
                            "token": {"type": "string",
                                      "default": echoed_secret()}}}
                    }]}),
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
                    "echo" => {
                        let secret = echoed_secret();
                        if text == "error" {
                            send(&serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "error": {"code": -32603,
                                          "message": format!("upstream rejected {secret}")}
                            }));
                        } else {
                            send(&serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {"content": [{"type": "text",
                                    "text": format!("token is {secret}")}],
                                    "structuredContent": {"token": secret}}
                            }));
                        }
                    }
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

/// Appends one bracket marker to the shared concurrency log.
fn log_concurrency(event: &str) {
    use std::io::Write as _;
    let Ok(path) = std::env::var("FIXTURE_CONCURRENCY") else {
        return;
    };
    // Built first and written once. `writeln!` issues one syscall per format
    // piece, so several servers appending at the same time interleave into
    // corrupted lines and the log undercounts.
    let line = format!("{event}\n");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}
