//! The stdio transport: newline-delimited JSON-RPC over a child process's
//! stdin and stdout.
//!
//! The child is launched with the workspace as its working directory, its
//! argument array verbatim, and a minimal environment plus the variables the
//! entry declares. Nothing goes through a shell, so an argument or a command
//! cannot expand into something else.
//!
//! Shutdown follows the transport specification: close stdin, wait,
//! terminate, then kill. Gritt owns the whole process group it started, so
//! cleanup continues after the direct child is reaped: a server may exit
//! while a descendant it spawned is still running.
//!
//! Input is bounded while it is read, not after. A server that never sends a
//! newline must not be able to exhaust Gritt and take the healthy servers
//! down with it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gritt_core::mcp::McpTransportKind;
use gritt_core::{Error, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::connection::{closed, Command, Connection, ConnectionFlags};
use super::jsonrpc::{self, Incoming};

/// Longest single line accepted from a server. Enforced while the line is
/// being read, so an unterminated stream is stopped at the limit instead of
/// after it has already been buffered.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// How much of a server's stderr is kept for the failure summary, and the
/// most that is read before a line is cut.
const STDERR_TAIL_BYTES: usize = 4 * 1024;
const MAX_STDERR_LINE_BYTES: usize = 64 * 1024;

/// Most requests that may be outstanding at once. A server that never
/// answers cannot make the pending map grow without limit.
const MAX_PENDING_REQUESTS: usize = 256;

/// How long one write may take before the connection is treated as wedged.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Environment names a child may inherit. Everything else is dropped, so a
/// provider key configured for Gritt never reaches an MCP server; the entry's
/// own `env` block is the only way to pass one deliberately (ADR-008).
const INHERITED_ENV: [&str; 24] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "TERM",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMFILES",
    "PROGRAMDATA",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "NUMBER_OF_PROCESSORS",
];

/// The launch environment a server child gets, before its declared
/// variables are added.
fn base_env() -> BTreeMap<String, String> {
    std::env::vars()
        .filter(|(name, _)| {
            let upper = name.to_ascii_uppercase();
            INHERITED_ENV.contains(&upper.as_str()) || upper.starts_with("LC_")
        })
        .collect()
}

/// Where a declared command actually lives. A path with a separator is
/// resolved against the workspace when it is relative; a bare name is left
/// for the operating system to find on `PATH`.
pub fn resolve_program(workspace: &Path, command: &str) -> PathBuf {
    let path = Path::new(command);
    if !command.contains('/') && !command.contains('\\') {
        return path.to_path_buf();
    }
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

pub struct StdioLaunch {
    pub connection: Connection,
    pub pid: Option<u32>,
    /// Tail of the server's stderr. Servers log there by design, so it is
    /// the only useful diagnostic when a handshake never happens.
    pub stderr: Arc<Mutex<String>>,
}

/// Launches the server and starts its transport task.
pub fn launch(
    workspace: &Path,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    grace: Duration,
) -> Result<StdioLaunch> {
    let program = resolve_program(workspace, command);
    let mut process = tokio::process::Command::new(&program);
    process
        .args(args)
        .current_dir(workspace)
        .env_clear()
        .envs(base_env())
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    process.process_group(0);
    let mut child = process.spawn().map_err(|error| {
        Error::config(format!(
            "cannot start `{}`: {error}",
            program.to_string_lossy()
        ))
    })?;
    let pid = child.id();
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::config("the MCP server did not provide a standard input stream"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::config("the MCP server did not provide a standard output stream"))?;
    let stderr_pipe = child.stderr.take();

    let flags = Arc::new(ConnectionFlags::default());
    let stderr = Arc::new(Mutex::new(String::new()));
    let (tx, mut commands) = mpsc::channel::<Command>(64);
    let mut readers: Vec<JoinHandle<()>> = Vec::new();

    // Stderr drains on its own task: a server that logs heavily must not
    // block on a full pipe while the handshake is waiting. Each line is
    // capped as it is read, and the tail is capped after.
    if let Some(pipe) = stderr_pipe {
        let sink = Arc::clone(&stderr);
        readers.push(tokio::spawn(async move {
            let mut reader = BufReader::new(pipe);
            loop {
                let mut line = String::new();
                match read_line_bounded(&mut reader, &mut line, MAX_STDERR_LINE_BYTES).await {
                    ReadOutcome::Eof | ReadOutcome::Failed(_) => break,
                    // An over-long log line is truncated; logging is not a
                    // protocol stream, so this is not fatal.
                    ReadOutcome::Line | ReadOutcome::TooLong => {
                        let mut tail = sink.lock().expect("mcp stderr");
                        tail.push_str(&line);
                        if tail.len() > STDERR_TAIL_BYTES {
                            let cut = tail.len() - STDERR_TAIL_BYTES;
                            let cut = (cut..tail.len())
                                .find(|index| tail.is_char_boundary(*index))
                                .unwrap_or(tail.len());
                            *tail = tail[cut..].to_owned();
                        }
                    }
                }
            }
        }));
    }

    // Reader task: one line in, one decoded value out. Keeping it separate
    // means a server that never answers still cannot block the writer.
    let (line_tx, mut lines) = mpsc::channel::<std::result::Result<Value, String>>(64);
    readers.push(tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            let message = match read_line_bounded(&mut reader, &mut line, MAX_LINE_BYTES).await {
                ReadOutcome::Eof => break,
                ReadOutcome::TooLong => {
                    let _ = line_tx
                        .send(Err("the server sent an oversized message".into()))
                        .await;
                    break;
                }
                ReadOutcome::Failed(error) => {
                    let _ = line_tx
                        .send(Err(format!("cannot read from the server: {error}")))
                        .await;
                    break;
                }
                ReadOutcome::Line => match serde_json::from_str::<Value>(line.trim()) {
                    Ok(value) => Ok(value),
                    // A blank keep-alive line is not an error.
                    Err(_) if line.trim().is_empty() => continue,
                    Err(_) => Err("the server sent output that is not JSON-RPC".to_owned()),
                },
            };
            if line_tx.send(message).await.is_err() {
                break;
            }
        }
    }));

    let task_flags = Arc::clone(&flags);
    let task_stderr = Arc::clone(&stderr);
    tokio::spawn(async move {
        let mut pending: HashMap<u64, oneshot::Sender<Result<Value>>> = HashMap::new();
        let mut stop: Option<oneshot::Sender<()>> = None;
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    match command {
                        Command::Request { id, method, params, reply, cancelled } => {
                            // Checked here, immediately before the write, so
                            // a caller that gave up while this sat in the
                            // queue is honoured no matter what order the
                            // commands arrived in.
                            if cancelled.load(Ordering::SeqCst) {
                                let _ = reply.send(Err(Error::cancelled()));
                                continue;
                            }
                            if pending.len() >= MAX_PENDING_REQUESTS {
                                let _ = reply.send(Err(Error::config(
                                    "too many MCP requests are already waiting for this server",
                                )));
                                continue;
                            }
                            let frame = jsonrpc::request(id, &method, params);
                            match write_frame(&mut stdin, &frame, &task_flags).await {
                                Ok(()) => { pending.insert(id, reply); }
                                Err(error) => {
                                    let _ = reply.send(Err(error));
                                    task_flags.closed.store(true, Ordering::SeqCst);
                                    break;
                                }
                            }
                        }
                        Command::Notify { method, params, ack } => {
                            let frame = jsonrpc::notification(&method, params);
                            let outcome = write_frame(&mut stdin, &frame, &task_flags).await;
                            let failed = outcome.is_err();
                            if let Some(ack) = ack {
                                let _ = ack.send(outcome);
                            }
                            if failed {
                                task_flags.closed.store(true, Ordering::SeqCst);
                                break;
                            }
                        }
                        // The caller stopped waiting. Dropping the entry is
                        // what keeps the map bounded; a late response then
                        // has nowhere to go and is discarded. A request that
                        // has not been written yet is stopped by the flag the
                        // caller set, checked above.
                        Command::Abort { id } => {
                            pending.remove(&id);
                        }
                        Command::Shutdown { reply } => { stop = Some(reply); break; }
                    }
                }
                message = lines.recv() => {
                    let Some(message) = message else { break };
                    match message {
                        Ok(value) => {
                            if let Some(frame) = route(value, &mut pending, &task_flags) {
                                if write_frame(&mut stdin, &frame, &task_flags).await.is_err() {
                                    task_flags.closed.store(true, Ordering::SeqCst);
                                    break;
                                }
                            }
                        }
                        Err(reason) => {
                            fail_pending(&mut pending, &reason);
                            task_flags.closed.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
                // Shutdown can also arrive out of band, so a task that was
                // parked on a write does not need the command queue at all.
                _ = task_flags.stopped() => break,
            }
        }
        task_flags.closed.store(true, Ordering::SeqCst);
        let tail = task_stderr.lock().expect("mcp stderr").clone();
        fail_pending(&mut pending, &connection_lost(&tail));
        // Specified shutdown: close stdin first and give the server a chance
        // to exit on its own.
        drop(stdin);
        terminate(child, pid, grace, readers).await;
        drop(stop);
        // Everything this connection owned is released; `shutdown` returns
        // only after this point.
        task_flags.finish();
    });

    Ok(StdioLaunch {
        connection: Connection::new(tx, flags, McpTransportKind::Stdio),
        pid,
        stderr,
    })
}

/// What one bounded read produced.
enum ReadOutcome {
    Line,
    Eof,
    /// The limit was reached before a newline arrived.
    TooLong,
    Failed(std::io::Error),
}

/// Reads one line, giving up at `limit` bytes rather than buffering an
/// unbounded amount from a stream that may never send a delimiter.
async fn read_line_bounded<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    out: &mut String,
    limit: usize,
) -> ReadOutcome {
    let mut raw: Vec<u8> = Vec::new();
    loop {
        let available = match reader.fill_buf().await {
            Ok(buffer) => buffer,
            Err(error) => return ReadOutcome::Failed(error),
        };
        if available.is_empty() {
            if raw.is_empty() {
                return ReadOutcome::Eof;
            }
            *out = String::from_utf8_lossy(&raw).into_owned();
            return ReadOutcome::Line;
        }
        match available.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                raw.extend_from_slice(&available[..=end]);
                reader.consume(end + 1);
                // The terminating chunk counts too. Without this check a
                // line could exceed the limit as long as its last chunk
                // happened to carry the newline.
                if raw.len() > limit {
                    raw.truncate(limit);
                    *out = String::from_utf8_lossy(&raw).into_owned();
                    return ReadOutcome::TooLong;
                }
                *out = String::from_utf8_lossy(&raw).into_owned();
                return ReadOutcome::Line;
            }
            None => {
                let taken = available.len();
                raw.extend_from_slice(available);
                reader.consume(taken);
                if raw.len() > limit {
                    // Keep what fits so a truncated log line is still useful.
                    raw.truncate(limit);
                    *out = String::from_utf8_lossy(&raw).into_owned();
                    return ReadOutcome::TooLong;
                }
            }
        }
    }
}

fn connection_lost(stderr_tail: &str) -> String {
    let tail = stderr_tail.trim();
    if tail.is_empty() {
        "the server closed the connection".to_owned()
    } else {
        let last = tail.lines().next_back().unwrap_or_default();
        format!("the server closed the connection: {last}")
    }
}

fn fail_pending(pending: &mut HashMap<u64, oneshot::Sender<Result<Value>>>, reason: &str) {
    for (_, reply) in pending.drain() {
        let _ = reply.send(Err(Error::config(reason.to_owned())));
    }
}

/// Delivers one decoded message. Returns a frame to write back when the
/// server asked something.
fn route(
    value: Value,
    pending: &mut HashMap<u64, oneshot::Sender<Result<Value>>>,
    flags: &ConnectionFlags,
) -> Option<Value> {
    match jsonrpc::classify(&value) {
        Incoming::Response { id, result } => {
            // A response for an id that is gone belongs to a request the
            // caller stopped waiting for. Discarding it is required.
            if let Some(reply) = pending.remove(&id) {
                let _ = reply.send(result.map_err(|error| Error::config(error.summary())));
            }
            None
        }
        Incoming::Notification { method, .. } => {
            if method == jsonrpc::method::TOOLS_LIST_CHANGED {
                flags.tools_changed.store(true, Ordering::SeqCst);
            }
            None
        }
        Incoming::Request { id, method } if method == jsonrpc::method::PING => {
            Some(jsonrpc::response(id, serde_json::json!({})))
        }
        Incoming::Request { id, method } => Some(jsonrpc::error_response(
            id,
            jsonrpc::METHOD_NOT_FOUND,
            &format!("gritt does not implement `{method}`"),
        )),
        Incoming::Unroutable => None,
    }
}

/// Writes one frame, giving up if the server stopped reading or shutdown was
/// requested. Without this a full pipe would park the only task that can
/// service the shutdown command.
async fn write_frame(
    stdin: &mut tokio::process::ChildStdin,
    frame: &Value,
    flags: &ConnectionFlags,
) -> Result<()> {
    let line = jsonrpc::encode_line(frame)?;
    let write = async {
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|_| closed())?;
        stdin.flush().await.map_err(|_| closed())
    };
    tokio::select! {
        biased;
        _ = flags.stopped() => Err(closed()),
        outcome = write => outcome,
        _ = tokio::time::sleep(WRITE_TIMEOUT) => {
            Err(Error::config("the server stopped reading its standard input"))
        }
    }
}

/// Ends the child and everything it started.
///
/// The direct child exiting is not the end of the job: it may have spawned a
/// descendant that outlives it, and Gritt owns the process group it created.
async fn terminate(
    mut child: tokio::process::Child,
    pid: Option<u32>,
    grace: Duration,
    readers: Vec<JoinHandle<()>>,
) {
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        if let Some(pid) = pid {
            signal_tree(pid, "TERM").await;
        }
        if tokio::time::timeout(grace, child.wait()).await.is_err() {
            if let Some(pid) = pid {
                crate::tools::kill_tree(pid).await;
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }
    // The child is reaped. Anything still in its group is a descendant Gritt
    // launched indirectly and still owns.
    if let Some(pid) = pid {
        if group_alive(pid) {
            signal_tree(pid, "TERM").await;
            let deadline = std::time::Instant::now() + grace;
            while group_alive(pid) && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            if group_alive(pid) {
                crate::tools::kill_tree(pid).await;
            }
        }
    }
    // The readers end when their pipes close, which the exit above
    // guarantees. The bound is there so a wedged descendant holding a pipe
    // open cannot keep a task alive forever.
    for reader in readers {
        let abort = reader.abort_handle();
        if tokio::time::timeout(grace, reader).await.is_err() {
            abort.abort();
        }
    }
}

/// True when any process remains in the group rooted at `pid`.
#[cfg(unix)]
fn group_alive(pid: u32) -> bool {
    // `kill -0` on the negative pid asks about the whole group without
    // signalling it.
    std::process::Command::new("kill")
        .args(["-0", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn group_alive(_pid: u32) -> bool {
    false
}

/// Sends a signal to the child's process group. Uses the platform tool so no
/// extra dependency is needed, matching the native shell tool.
async fn signal_tree(pid: u32, signal: &str) {
    #[cfg(unix)]
    {
        for target in [format!("-{pid}"), pid.to_string()] {
            let _ = tokio::process::Command::new("kill")
                .args([&format!("-{signal}"), "--", &target])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
        }
    }
    #[cfg(windows)]
    {
        let _ = signal;
        crate::tools::kill_tree(pid).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_resolve_against_the_workspace_or_the_path() {
        let workspace = Path::new("/tmp/ws");
        assert_eq!(resolve_program(workspace, "npx"), PathBuf::from("npx"));
        assert_eq!(
            resolve_program(workspace, ".agents/gritt-agent"),
            PathBuf::from("/tmp/ws/.agents/gritt-agent")
        );
        assert_eq!(
            resolve_program(workspace, "/usr/bin/srv"),
            PathBuf::from("/usr/bin/srv")
        );
    }

    #[test]
    fn the_inherited_environment_carries_no_credential() {
        std::env::set_var("GRITT_TEST_MCP_API_KEY", "mcp-env-secret-9001");
        let env = base_env();
        assert!(!env.contains_key("GRITT_TEST_MCP_API_KEY"));
        assert!(env.contains_key("PATH"));
        for name in env.keys() {
            assert!(
                !gritt_core::secret::is_secret_env_name(name, &[]),
                "inherited {name}"
            );
        }
    }

    #[tokio::test]
    async fn a_line_without_a_delimiter_stops_at_the_limit() {
        // 4 KiB of payload against a 1 KiB limit, and never a newline.
        let flood = "x".repeat(4096);
        let mut reader = BufReader::new(flood.as_bytes());
        let mut line = String::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 1024).await,
            ReadOutcome::TooLong
        ));
        // Only the bounded prefix was ever held.
        assert!(line.len() <= 1024, "{}", line.len());
    }

    #[tokio::test]
    async fn a_line_that_crosses_the_limit_in_its_final_chunk_is_refused() {
        // The newline arrives in the same chunk that pushes the line past the
        // limit, which is the case a check placed only on the no-newline
        // branch would wave through.
        let mut flood = "y".repeat(2048);
        flood.push('\n');
        let mut reader = BufReader::new(flood.as_bytes());
        let mut line = String::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 1024).await,
            ReadOutcome::TooLong
        ));
        assert!(line.len() <= 1024, "{}", line.len());
    }

    #[tokio::test]
    async fn a_line_exactly_at_the_limit_is_still_a_line() {
        let mut exact = "z".repeat(1023);
        exact.push('\n');
        let mut reader = BufReader::new(exact.as_bytes());
        let mut line = String::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 1024).await,
            ReadOutcome::Line
        ));
        assert_eq!(line.len(), 1024);
    }

    #[tokio::test]
    async fn bounded_reads_still_deliver_ordinary_lines() {
        let mut reader = BufReader::new("first\nsecond\n".as_bytes());
        let mut line = String::new();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 1024).await,
            ReadOutcome::Line
        ));
        assert_eq!(line, "first\n");
        line.clear();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 1024).await,
            ReadOutcome::Line
        ));
        assert_eq!(line, "second\n");
        line.clear();
        assert!(matches!(
            read_line_bounded(&mut reader, &mut line, 1024).await,
            ReadOutcome::Eof
        ));
    }
}
