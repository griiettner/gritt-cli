//! End-to-end tests through the real `gritt` binary. A local HTTP server
//! plays the provider from canned Chat Completions fixtures, so every run
//! exercises configuration loading, key resolution, the adapter, the
//! session store, the policy engine, native tools, print mode, and exit
//! codes exactly as a user would.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const KEY: &str = "e2e-key-never-printed";
const KEY_VAR: &str = "GRITT_E2E_KEY";

fn gritt() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gritt"))
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../gritt-provider/tests/fixtures/chat-completions/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

fn text_sse(text: &str) -> Vec<u8> {
    let delta = serde_json::json!({
        "id": "chatcmpl-e2e", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let stop = serde_json::json!({
        "id": "chatcmpl-e2e", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    format!("data: {delta}\n\ndata: {stop}\n\ndata: [DONE]\n\n").into_bytes()
}

fn tool_call_sse(tool: &str, arguments: serde_json::Value) -> Vec<u8> {
    let first = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": null,
            "tool_calls": [{"index": 0, "id": format!("call_{tool}"), "type": "function",
                "function": {"name": tool, "arguments": arguments.to_string()}}]},
            "finish_reason": null}]
    });
    let second = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {first}\n\ndata: {second}\n\ndata: [DONE]\n\n").into_bytes()
}

/// A provider stand-in: answers each POST in order with the next canned
/// body and records every request body. `stall` keeps a connection open
/// without answering, for the cancellation test.
struct Provider {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
}

fn serve(responses: Vec<Vec<u8>>, stall: bool) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    thread::spawn(move || {
        let mut responses = responses.into_iter();
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).unwrap();
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&body).into_owned());
            if stall {
                // Hold the connection until the client goes away.
                let mut sink = [0u8; 1];
                let _ = reader.read(&mut sink);
                return;
            }
            let Some(response) = responses.next() else {
                return;
            };
            let mut stream = stream;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            stream.write_all(&response).unwrap();
            stream.flush().unwrap();
        }
    });
    Provider { port, bodies }
}

/// A workspace with a project config pointing at the local provider and an
/// explicit database beside it.
struct Space {
    dir: tempfile::TempDir,
}

impl Space {
    fn new(port: u16) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".gritt")).unwrap();
        std::fs::write(
            dir.path().join(".gritt/config.toml"),
            format!(
                "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
                 [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
                 base_url = \"http://127.0.0.1:{port}/v1\"\n\
                 [profiles.local.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/local\"\n\
                 env_var_name = \"{KEY_VAR}\"\n"
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("README.md"), "# Readme\nhello\n").unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn database(&self) -> PathBuf {
        self.dir.path().join("gritt.db")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = gritt();
        command
            .arg("--workspace")
            .arg(self.path())
            .arg("--database")
            .arg(self.database())
            .args(args)
            .env(KEY_VAR, KEY)
            .env_remove("NO_COLOR")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn planning_turn_streams_text_and_exits_zero() {
    let provider = serve(vec![fixture("stream-text.sse")], false);
    let space = Space::new(provider.port);
    let output = space.run(&["run", "--plan", "--no-models", "say hello"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output).trim_end(), "Hello, world");
    let body = provider.bodies.lock().unwrap()[0].clone();
    assert!(body.contains("say hello"));
    assert!(!body.contains("\"tools\""), "planning must not offer tools");
    assert!(!stdout(&output).contains(KEY) && !stderr(&output).contains(KEY));
}

#[test]
fn coding_turn_with_an_approved_write_applies_the_diff() {
    let provider = serve(
        vec![
            tool_call_sse(
                "file_write",
                serde_json::json!({"path": "notes.txt", "content": "written by the agent\n"}),
            ),
            text_sse("Done."),
        ],
        false,
    );
    let space = Space::new(provider.port);
    let output = space.run(&[
        "run",
        "--code",
        "--approve-all",
        "--no-models",
        "--session",
        "coding",
        "write notes",
    ]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output).trim_end(), "Done.");
    assert_eq!(
        std::fs::read_to_string(space.path().join("notes.txt")).unwrap(),
        "written by the agent\n"
    );
    let bodies = provider.bodies.lock().unwrap();
    assert!(bodies[0].contains("\"tools\""), "coding offers tools");
    assert!(
        bodies[1].contains("\"tool\""),
        "the result goes back to the model"
    );
    let shown = space.run(&["session", "show", "coding"]);
    let text = stdout(&shown);
    assert!(text.contains("tool_call file_write"), "{text}");
    assert!(text.contains("approval_requested file_write"), "{text}");
    assert!(text.contains("approval Approved"), "{text}");
}

#[test]
fn a_denied_write_leaves_the_file_alone_and_reports_the_denial() {
    let provider = serve(
        vec![
            tool_call_sse(
                "file_write",
                serde_json::json!({"path": "secret.txt", "content": "nope\n"}),
            ),
            text_sse("Understood."),
        ],
        false,
    );
    let space = Space::new(provider.port);
    // No terminal on stdin: the print mode denies every `ask`.
    let output = space.run(&["run", "--code", "--no-models", "--session", "deny", "write"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(!space.path().join("secret.txt").exists());
    let bodies = provider.bodies.lock().unwrap();
    assert!(bodies[1].contains("not permitted"), "{}", bodies[1]);
    let shown = space.run(&["session", "show", "deny"]);
    assert!(
        stdout(&shown).contains("approval Denied"),
        "{}",
        stdout(&shown)
    );
}

#[test]
fn a_session_resumes_after_the_process_exits() {
    let provider = serve(
        vec![text_sse("First answer."), text_sse("Second answer.")],
        false,
    );
    let space = Space::new(provider.port);
    let first = space.run(&["run", "--plan", "--no-models", "--session", "keep", "one"]);
    assert!(first.status.success(), "stderr: {}", stderr(&first));
    let second = space.run(&["run", "--no-models", "--session", "keep", "two"]);
    assert!(second.status.success(), "stderr: {}", stderr(&second));
    assert_eq!(stdout(&second).trim_end(), "Second answer.");
    let bodies = provider.bodies.lock().unwrap();
    assert!(
        bodies[1].contains("First answer."),
        "the resumed turn carries the earlier transcript: {}",
        bodies[1]
    );
    let list = space.run(&["session", "list"]);
    assert!(stdout(&list).contains("keep"), "{}", stdout(&list));
    let shown = space.run(&["session", "show", "keep"]);
    let completed = stdout(&shown).matches("Completed").count();
    assert!(completed >= 2, "{}", stdout(&shown));
}

#[cfg(unix)]
#[test]
fn ctrl_c_cancels_a_running_turn_with_exit_130() {
    let provider = serve(Vec::new(), true);
    let space = Space::new(provider.port);
    let child = space
        .command(&[
            "run",
            "--plan",
            "--no-models",
            "--session",
            "cancel",
            "wait",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Wait for the request to reach the provider, then interrupt.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while provider.bodies.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the request never arrived"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(child.wait_with_output().unwrap());
    });
    let output = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the process did not stop after Ctrl-C");
    assert_eq!(
        output.status.code(),
        Some(130),
        "stderr: {}",
        stderr(&output)
    );
    let shown = space.run(&["session", "show", "cancel"]);
    assert!(stdout(&shown).contains("Cancelled"), "{}", stdout(&shown));
}

#[test]
fn a_missing_connector_fails_alone_and_native_keeps_working() {
    let provider = serve(vec![text_sse("Native is fine.")], false);
    let space = Space::new(provider.port);
    // Cursor's CLI is not installed on the machines this test runs on; the
    // control plane refuses before creating a session.
    let failed = space.run(&[
        "run",
        "--no-models",
        "--connector",
        "cursor",
        "--session",
        "cur",
        "hi",
    ]);
    assert!(!failed.status.success());
    assert!(
        stderr(&failed).contains("not installed"),
        "{}",
        stderr(&failed)
    );
    let list = space.run(&["session", "list"]);
    assert!(!stdout(&list).contains("cur"), "{}", stdout(&list));
    let native = space.run(&["run", "--plan", "--no-models", "hello"]);
    assert!(native.status.success(), "stderr: {}", stderr(&native));
    assert_eq!(stdout(&native).trim_end(), "Native is fine.");
    let connectors = space.run(&["connectors"]);
    assert!(stdout(&connectors).contains("cursor"));
    assert!(stdout(&connectors).contains("not installed"));
}

/// The product schema as TKT-0009 first shipped it: migration 0001 only.
const FIRST_SCHEMA: &str = include_str!("../../gritt-harness/src/store/product_schema.sql");

#[test]
fn an_old_database_upgrades_in_place_and_keeps_its_rows() {
    let provider = serve(Vec::new(), false);
    let space = Space::new(provider.port);
    let database = space.database();
    // Seed a database that only knows the first migration.
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let db = turso::Builder::new_local(&database.to_string_lossy())
            .experimental_index_method(true)
            .build()
            .await
            .unwrap();
        let connection = db.connect().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS gritt_schema_migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP)",
            )
            .await
            .unwrap();
        connection.execute_batch(FIRST_SCHEMA).await.unwrap();
        connection
            .execute(
                "INSERT INTO gritt_schema_migrations (name) VALUES ('0001_product_tables')",
                (),
            )
            .await
            .unwrap();
        connection
            .execute(
                "INSERT INTO gritt_sessions (id, name, kind, phase, workspace, created_at, updated_at)
                 VALUES ('old-1', 'legacy', '{\"kind\":\"native\",\"provider_profile\":\"local\",\"model\":\"openai/gpt-5-nano\"}', 'planning', ?1, '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')",
                turso::params![space.path().to_string_lossy().into_owned()],
            )
            .await
            .unwrap();
    });
    let doctor = space.run(&["doctor"]);
    assert!(doctor.status.success(), "stderr: {}", stderr(&doctor));
    let text = stdout(&doctor);
    assert!(text.contains("product migrations: 3/3 applied"), "{text}");
    assert!(text.contains("0003_session_told_phase: applied"), "{text}");
    assert!(text.contains("sessions: 1"), "{text}");
    assert!(!text.contains(KEY));
    let list = space.run(&["session", "list"]);
    assert!(
        stdout(&list).contains("legacy"),
        "stdout: {} stderr: {}",
        stdout(&list),
        stderr(&list)
    );
}

#[test]
fn doctor_and_telemetry_stay_content_free() {
    let provider = serve(vec![text_sse("Telemetry check.")], false);
    let space = Space::new(provider.port);
    let marker = "zebra-quartz-prompt-marker";
    let output = space.run(&["run", "--plan", "--no-models", marker]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let telemetry = space.run(&["telemetry"]);
    assert!(telemetry.status.success());
    let text = stdout(&telemetry);
    assert!(text.contains("turn"), "{text}");
    assert!(!text.contains(marker) && !text.contains(KEY), "{text}");
    let doctor = space.run(&["doctor"]);
    let text = stdout(&doctor);
    assert!(text.contains("local: ChatCompletions"), "{text}");
    assert!(text.contains("key available"), "{text}");
    assert!(!text.contains(marker) && !text.contains(KEY), "{text}");
}
