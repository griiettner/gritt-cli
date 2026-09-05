//! The stdio transport: newline-delimited JSON-RPC over a child process's
//! stdin and stdout.
//!
//! The child is launched with the workspace as its working directory, its
//! argument array verbatim, and a minimal environment plus the variables the
//! entry declares. Nothing goes through a shell, so an argument or a command
//! cannot expand into something else. Shutdown follows the transport
//! specification: close stdin, wait, terminate, then kill.

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

use super::connection::{closed, Command, Connection, ConnectionFlags};
use super::jsonrpc::{self, Incoming};

/// Longest single line accepted from a server. A line past it is a protocol
/// violation, not a message worth buffering.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// How much of a server's stderr is kept for the failure summary.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

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

    // Stderr drains on its own task: a server that logs heavily must not
    // block on a full pipe while the handshake is waiting.
    if let Some(pipe) = stderr_pipe {
        let sink = Arc::clone(&stderr);
        tokio::spawn(async move {
            let mut reader = BufReader::new(pipe);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
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
        });
    }

    // Reader task: one line in, one decoded value out. Keeping it separate
    // means a server that never answers still cannot block the writer.
    let (line_tx, mut lines) = mpsc::channel::<std::result::Result<Value, String>>(64);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let read = reader.read_line(&mut line).await;
            let message = match read {
                Ok(0) => break,
                Ok(_) if line.len() > MAX_LINE_BYTES => {
                    let _ = line_tx
                        .send(Err("the server sent an oversized message".into()))
                        .await;
                    break;
                }
                Ok(_) => match serde_json::from_str::<Value>(line.trim()) {
                    Ok(value) => Ok(value),
                    // A blank keep-alive line is not an error.
                    Err(_) if line.trim().is_empty() => continue,
                    Err(_) => Err("the server sent output that is not JSON-RPC".to_owned()),
                },
                Err(error) => Err(format!("cannot read from the server: {error}")),
            };
            if line_tx.send(message).await.is_err() {
                break;
            }
        }
    });

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
                        Command::Request { id, method, params, reply } => {
                            let frame = jsonrpc::request(id, &method, params);
                            match write_frame(&mut stdin, &frame).await {
                                Ok(()) => { pending.insert(id, reply); }
                                Err(error) => { let _ = reply.send(Err(error)); }
                            }
                        }
                        Command::Notify { method, params } => {
                            let frame = jsonrpc::notification(&method, params);
                            let _ = write_frame(&mut stdin, &frame).await;
                        }
                        Command::Shutdown { reply } => { stop = Some(reply); break; }
                    }
                }
                message = lines.recv() => {
                    let Some(message) = message else { break };
                    match message {
                        Ok(value) => {
                            if let Some(frame) = route(value, &mut pending, &task_flags) {
                                let _ = write_frame(&mut stdin, &frame).await;
                            }
                        }
                        Err(reason) => {
                            fail_pending(&mut pending, &reason);
                            task_flags.closed.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
        }
        task_flags.closed.store(true, Ordering::SeqCst);
        let tail = task_stderr.lock().expect("mcp stderr").clone();
        fail_pending(&mut pending, &connection_lost(&tail));
        // Specified shutdown: close stdin first and give the server a chance
        // to exit on its own.
        drop(stdin);
        terminate(child, pid, grace).await;
        if let Some(reply) = stop {
            let _ = reply.send(());
        }
    });

    Ok(StdioLaunch {
        connection: Connection::new(tx, flags, McpTransportKind::Stdio),
        pid,
        stderr,
    })
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

async fn write_frame(stdin: &mut tokio::process::ChildStdin, frame: &Value) -> Result<()> {
    let line = jsonrpc::encode_line(frame)?;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|_| closed())?;
    stdin.flush().await.map_err(|_| closed())
}

/// Waits out the grace period after stdin closed, then escalates. Gritt
/// owns every stdio child it launched, so none may outlive the runtime.
async fn terminate(mut child: tokio::process::Child, pid: Option<u32>, grace: Duration) {
    if tokio::time::timeout(grace, child.wait()).await.is_ok() {
        return;
    }
    if let Some(pid) = pid {
        signal_tree(pid, "TERM").await;
    }
    if tokio::time::timeout(grace, child.wait()).await.is_ok() {
        return;
    }
    if let Some(pid) = pid {
        crate::tools::kill_tree(pid).await;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
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
}
