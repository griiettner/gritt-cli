mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout, Stdio};

use serde_json::Value;

use common::{command, fixture};

#[test]
fn serve_answers_initialize_list_and_both_tools() {
    let repo = fixture();
    let mut child = command(&repo.root)
        .args(["memory", "serve"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    let mut exchange = |request: &str| exchange(&mut stdin, &mut stdout, request);

    let init = exchange(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "gritt-local-memory");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    assert!(exchange(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}{"jsonrpc":"2.0","id":7,"method":"ping"}"#)["result"].is_object());

    let list = exchange(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["search_local_memory", "read_local_memory"]);

    let search = exchange(
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_local_memory","arguments":{"query":"catalog cache","limit":2}}}"#,
    );
    let text = search["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(search["result"]["isError"], false);
    assert!(text.starts_with("[1] "));
    assert!(text.contains("Source: .agents/tasks/alice/TKT-0001-0025/TKT-0001/concept.md:12-13"));

    let read = exchange(
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"read_local_memory","arguments":{"path":"docs/config.json"}}}"#,
    );
    let text = read["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("## config\nPath: docs/config.json\n\n{"));

    let missing = exchange(
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"read_local_memory","arguments":{"path":"nope.md"}}}"#,
    );
    assert_eq!(
        missing["result"]["content"][0]["text"],
        "No local document exists at nope.md."
    );

    let ping = exchange(r#"{"jsonrpc":"2.0","id":6,"method":"ping"}"#);
    assert!(ping["result"].is_object());

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success());
}

/// Writes one request line and reads one response line.
fn exchange(stdin: &mut ChildStdin, stdout: &mut BufReader<ChildStdout>, request: &str) -> Value {
    for part in request.split("}{") {
        let line = if part.starts_with('{') && part.ends_with('}') {
            part.to_owned()
        } else if part.starts_with('{') {
            format!("{part}}}")
        } else if part.ends_with('}') {
            format!("{{{part}")
        } else {
            format!("{{{part}}}")
        };
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|error| panic!("bad response {line:?}: {error}"))
}
