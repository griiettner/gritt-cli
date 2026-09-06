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

#[tokio::test]
async fn default_database_opens_while_memory_is_locked_by_another_process() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join(".agents/brain/data");
    std::fs::create_dir_all(&data).unwrap();
    let memory_path = data.join("agent-memory.db");
    let database = turso::Builder::new_local(memory_path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let connection = database.connect().unwrap();
    connection
        .execute("CREATE TABLE memory_marker (value TEXT)", ())
        .await
        .unwrap();

    // The child really encounters the parent's lock with the legacy path.
    let locked = gritt()
        .arg("--workspace")
        .arg(dir.path())
        .arg("--database")
        .arg(&memory_path)
        .args(["session", "list"])
        .output()
        .unwrap();
    assert!(!locked.status.success());
    assert!(String::from_utf8_lossy(&locked.stderr).contains("lock"));

    let output = gritt()
        .arg("--workspace")
        .arg(dir.path())
        .args(["session", "list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(data.join("gritt.db").is_file());
    connection
        .execute("INSERT INTO memory_marker VALUES ('still open')", ())
        .await
        .unwrap();
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

/// The model list the stand-in answers `GET /models` with: one model
/// that reports reasoning, so an explicit effort is accepted on Chat
/// Completions.
const MODELS_JSON: &str =
    r#"{"data":[{"id":"openai/gpt-5-nano","supported_parameters":["reasoning","tools"]}]}"#;

/// A provider stand-in: answers each POST in order with the next canned
/// body and records every request body; answers every GET with the model
/// list. `stall` keeps a connection open without answering, for the
/// cancellation test.
struct Provider {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
}

/// A port nothing listens on, for an unreachable profile.
fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
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
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                return;
            }
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
            if request_line.starts_with("GET ") {
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{MODELS_JSON}",
                    MODELS_JSON.len()
                );
                let _ = stream.flush();
                continue;
            }
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

/// The `[profiles.<name>]` table for a local Chat Completions endpoint.
fn local_profile(name: &str, port: u16) -> String {
    format!(
        "[profiles.{name}]\nname = \"{name}\"\nprotocol = \"chat_completions\"\n\
         base_url = \"http://127.0.0.1:{port}/v1\"\n\
         [profiles.{name}.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/{name}\"\n\
         env_var_name = \"{KEY_VAR}\"\n"
    )
}

impl Space {
    fn new(port: u16) -> Self {
        Self::with_config(format!(
            "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n{}",
            local_profile("local", port)
        ))
    }

    fn with_config(config: String) -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        std::fs::write(dir.path().join("README.md"), "# Readme\nhello\n").unwrap();
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn database(&self) -> PathBuf {
        self.dir.path().join("gritt.db")
    }

    /// The child's home is the workspace, so its model cache and user
    /// config live in the temporary directory rather than the developer's.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = gritt();
        command
            .arg("--workspace")
            .arg(self.path())
            .arg("--database")
            .arg(self.database())
            .args(args)
            .env(KEY_VAR, KEY)
            .env("HOME", self.path())
            .env("XDG_CACHE_HOME", self.path().join("cache"))
            .env("XDG_CONFIG_HOME", self.path().join("config"))
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
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["function"]["name"], "file_read");
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
fn startup_falls_over_to_the_next_profile_when_the_default_is_unreachable() {
    let provider = serve(
        vec![text_sse("From the fallback."), text_sse("Still here.")],
        false,
    );
    let space = Space::with_config(format!(
        "default_profile = \"dead\"\ndefault_model = \"openai/gpt-5-nano\"\n\
         fallback_profiles = [\"local\"]\n{}{}",
        local_profile("dead", closed_port()),
        local_profile("local", provider.port)
    ));
    // No `--no-models`: the chain probes each endpoint live.
    let output = space.run(&["run", "--plan", "--session", "moved", "say hello"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output).trim_end(), "From the fallback.");
    let err = stderr(&output);
    assert!(
        err.contains("skipped profile dead (connection failed"),
        "{err}"
    );
    assert!(
        err.contains("runs on profile `local` with model `openai/gpt-5-nano`"),
        "{err}"
    );
    assert!(!err.contains(KEY) && !stdout(&output).contains(KEY));
    let list = space.run(&["session", "list"]);
    assert!(
        stdout(&list).contains("local/openai/gpt-5-nano"),
        "{}",
        stdout(&list)
    );

    // Resuming keeps the session on the fallback profile without probing
    // the dead default again, and says nothing about a chain.
    let second = space.run(&["run", "--session", "moved", "again"]);
    assert!(second.status.success(), "stderr: {}", stderr(&second));
    assert_eq!(stdout(&second).trim_end(), "Still here.");
    assert!(
        !stderr(&second).contains("skipped profile"),
        "{}",
        stderr(&second)
    );

    // With nothing usable the aggregate error names every profile and
    // its failure class.
    let none = Space::with_config(format!(
        "default_profile = \"dead\"\ndefault_model = \"openai/gpt-5-nano\"\n\
         fallback_profiles = [\"also-dead\"]\n{}{}",
        local_profile("dead", closed_port()),
        local_profile("also-dead", closed_port())
    ));
    let output = none.run(&["run", "--plan", "say hello"]);
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("no usable provider profile"), "{err}");
    assert!(err.contains("dead (connection failed"), "{err}");
    assert!(err.contains("also-dead (connection failed"), "{err}");
    assert!(!err.contains(KEY));

    // A fallback list naming an unknown profile fails at load, loudly.
    let typo = Space::with_config(format!(
        "default_profile = \"local\"\nfallback_profiles = [\"nope\"]\n{}",
        local_profile("local", provider.port)
    ));
    let output = typo.run(&["config"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("`nope`"), "{}", stderr(&output));
}

#[test]
fn a_new_session_reuses_the_last_successful_choices_and_flags_win() {
    let provider = serve(
        vec![
            text_sse("One."),
            text_sse("Two."),
            text_sse("Three."),
            text_sse("Four."),
        ],
        false,
    );
    // No defaults configured: the first run has to say what it wants.
    let space = Space::with_config(local_profile("local", provider.port));
    let output = space.run(&["run", "--plan", "say one"]);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no profile given"),
        "{}",
        stderr(&output)
    );

    let first = space.run(&[
        "run",
        "--plan",
        "--profile",
        "local",
        "--model",
        "openai/gpt-5-nano",
        "--effort",
        "high",
        "say one",
    ]);
    assert!(first.status.success(), "stderr: {}", stderr(&first));
    assert_eq!(stdout(&first).trim_end(), "One.");

    // Nothing asked for: the remembered profile, model, and effort apply
    // and the notes say so.
    let second = space.run(&["run", "--plan", "say two"]);
    assert!(second.status.success(), "stderr: {}", stderr(&second));
    assert_eq!(stdout(&second).trim_end(), "Two.");
    let err = stderr(&second);
    assert!(
        err.contains(
            "using the last session's profile local, model openai/gpt-5-nano, effort high"
        ),
        "{err}"
    );

    // A flag wins for its own field; the rest stays remembered.
    let third = space.run(&["run", "--plan", "--effort", "low", "say three"]);
    assert!(third.status.success(), "stderr: {}", stderr(&third));
    assert!(
        stderr(&third).contains("using the last session's profile local, model openai/gpt-5-nano"),
        "{}",
        stderr(&third)
    );
    assert!(
        !stderr(&third).contains("effort high"),
        "{}",
        stderr(&third)
    );

    // The remembered choices beat a configured default that arrives later.
    std::fs::write(
        space.path().join("config.toml"),
        format!(
            "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n{}",
            local_profile("local", provider.port)
        ),
    )
    .unwrap();
    let fourth = space.run(&["run", "--plan", "say four"]);
    assert!(fourth.status.success(), "stderr: {}", stderr(&fourth));
    assert!(
        stderr(&fourth).contains(
            "using the last session's profile local, model openai/gpt-5-nano, effort low"
        ),
        "{}",
        stderr(&fourth)
    );

    let bodies: Vec<serde_json::Value> = provider
        .bodies
        .lock()
        .unwrap()
        .iter()
        .map(|body| serde_json::from_str(body).unwrap())
        .collect();
    assert_eq!(bodies.len(), 4);
    assert_eq!(bodies[0]["reasoning"]["effort"], "high");
    assert_eq!(bodies[1]["reasoning"]["effort"], "high");
    assert_eq!(bodies[2]["reasoning"]["effort"], "low");
    assert_eq!(bodies[3]["reasoning"]["effort"], "low");
    for output in [&first, &second, &third, &fourth] {
        assert!(!stderr(output).contains(KEY) && !stdout(output).contains(KEY));
    }
    let doctor = space.run(&["doctor"]);
    assert!(
        stdout(&doctor).contains("0005_last_used: applied"),
        "{}",
        stdout(&doctor)
    );
}

#[test]
fn the_repl_starts_on_the_fallback_profile_and_says_so() {
    let provider = serve(vec![text_sse("Hi from the fallback.")], false);
    let space = Space::with_config(format!(
        "default_profile = \"dead\"\ndefault_model = \"openai/gpt-5-nano\"\n\
         fallback_profiles = [\"local\"]\n{}{}",
        local_profile("dead", closed_port()),
        local_profile("local", provider.port)
    ));
    let mut child = space
        .command(&["repl", "--plan"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"say hi\n/quit\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("Hi from the fallback."),
        "{}",
        stdout(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("skipped profile dead (connection failed"),
        "{err}"
    );
    assert!(err.contains("runs on profile `local`"), "{err}");
    assert!(!err.contains(KEY));
}

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

/// Renders a `[connectors.executables]` section with the path encoded as a
/// TOML string, so Windows backslashes survive parsing.
fn executables_section(connector: &str, path: &Path) -> String {
    let value = toml::Value::from(path.to_string_lossy().into_owned());
    format!("[connectors.executables]\n{connector} = {value}\n")
}

#[test]
fn executable_paths_with_backslashes_round_trip_through_the_config() {
    let raw = r"C:\tools\no-such-agent.exe";
    let section = executables_section("cursor", Path::new(raw));
    let parsed: toml::Value = toml::from_str(&section).unwrap();
    assert_eq!(
        parsed["connectors"]["executables"]["cursor"].as_str(),
        Some(raw)
    );
}

#[test]
fn a_missing_connector_fails_alone_and_native_keeps_working() {
    let provider = serve(vec![text_sse("Native is fine.")], false);
    let space = Space::new(provider.port);
    // Point Cursor at an executable that cannot exist, so the test proves
    // the missing-executable path on every machine, installed CLI or not.
    let missing = space.path().join("no-such-cursor-agent");
    let config = space.path().join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(&executables_section("cursor", &missing));
    std::fs::write(&config, text).unwrap();
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
    assert!(text.contains("product migrations: 5/5 applied"), "{text}");
    assert!(text.contains("0003_session_told_phase: applied"), "{text}");
    assert!(text.contains("0004_mcp_trust: applied"), "{text}");
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
fn doctor_never_echoes_a_malformed_config_line() {
    let provider = serve(Vec::new(), false);
    let space = Space::new(provider.port);
    std::fs::write(
        space.path().join("config.toml"),
        "[profiles.broken]\nname = \"broken\"\napi_key = \"sk-leak\n",
    )
    .unwrap();
    let doctor = space.run(&["doctor"]);
    let text = format!("{}{}", stdout(&doctor), stderr(&doctor));
    assert!(text.contains("config error"), "{text}");
    assert!(text.contains("invalid TOML"), "{text}");
    assert!(!text.contains("sk-leak"), "{text}");
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

/// An interrupt during MCP startup must take the servers with it.
///
/// `gritt mcp trust` launches approved servers before anything else happens,
/// and those children sit in their own process groups, so a signal delivered
/// to Gritt alone does not reach them. The server here never speaks MCP, so
/// the interrupt lands squarely inside initialization.
#[cfg(unix)]
#[test]
fn interrupting_mcp_startup_leaves_no_server_running() {
    let space = Space::new(1);
    let pidfile = space.path().join("server.pid");
    std::fs::write(
        space.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {"never-answers": {
            "command": "sh",
            "args": ["-c", format!("echo $$ > {}; sleep 120", pidfile.display())],
        }}})
        .to_string(),
    )
    .unwrap();
    let child = space
        .command(&["mcp", "trust", "never-answers"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Wait until the server is actually running, then interrupt.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !pidfile.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the MCP server never started"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let server: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
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
        .recv_timeout(Duration::from_secs(60))
        .expect("gritt did not stop after Ctrl-C during MCP startup");
    assert_eq!(
        output.status.code(),
        Some(130),
        "stderr: {}",
        stderr(&output)
    );

    // The server it launched is gone too.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let alive = Command::new("kill")
            .args(["-0", &server.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !alive {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "an MCP server outlived the interrupted command"
        );
        thread::sleep(Duration::from_millis(100));
    }
}
