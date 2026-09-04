//! Process supervision shared by every external connector: launch through
//! pipes (the machine-readable default) or a PTY (the fallback), read
//! output line by line, write follow-up input, and kill the whole process
//! tree on cancellation or timeout.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use gritt_core::connector::Transport;
use gritt_core::session::BoxFuture;
use gritt_core::{Error, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// What to run. `env_remove` names variables the child must not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env_remove: Vec<String>,
    pub transport: Transport,
}

/// One line of child output. A PTY has no separate stderr, so every PTY
/// line is `Out`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Out(String),
    Err(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitOutcome {
    pub code: Option<i32>,
    pub success: bool,
}

/// Control over a running child, independent of transport.
pub trait ChildControl: Send {
    fn pid(&self) -> Option<u32>;
    /// Writes one line to the child's input.
    fn write_line<'a>(&'a mut self, line: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Kills the child and its process tree. Idempotent.
    fn kill(&mut self) -> BoxFuture<'_, ()>;
    /// Waits for exit, at most `limit`. `None` when the child is still
    /// running after the limit.
    fn wait(&mut self, limit: Duration) -> BoxFuture<'_, Option<ExitOutcome>>;
}

/// A launched child: its output lines and its control handle.
pub struct Supervised {
    pub lines: mpsc::Receiver<Line>,
    pub control: Box<dyn ChildControl>,
    pub transport: Transport,
}

/// Launches `launch` through pipes or a PTY.
pub async fn spawn(launch: &Launch) -> Result<Supervised> {
    match launch.transport {
        Transport::Pty | Transport::TerminalScrape => crate::pty::spawn(launch).await,
        Transport::MachineReadable | Transport::InProcess => spawn_piped(launch).await,
    }
}

struct PipeChild {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    killed: bool,
}

impl ChildControl for PipeChild {
    fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    fn write_line<'a>(&'a mut self, line: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let stdin = self
                .stdin
                .as_mut()
                .ok_or_else(|| Error::connector("the agent's input is closed"))?;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|error| Error::connector(format!("cannot write to the agent: {error}")))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|error| Error::connector(format!("cannot write to the agent: {error}")))?;
            stdin
                .flush()
                .await
                .map_err(|error| Error::connector(format!("cannot write to the agent: {error}")))
        })
    }

    fn kill(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.killed {
                return;
            }
            self.killed = true;
            if let Some(pid) = self.child.id() {
                kill_tree(pid).await;
            }
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
        })
    }

    fn wait(&mut self, limit: Duration) -> BoxFuture<'_, Option<ExitOutcome>> {
        Box::pin(async move {
            match tokio::time::timeout(limit, self.child.wait()).await {
                Ok(Ok(status)) => Some(ExitOutcome {
                    code: status.code(),
                    success: status.success(),
                }),
                Ok(Err(_)) => Some(ExitOutcome {
                    code: None,
                    success: false,
                }),
                Err(_) => None,
            }
        })
    }
}

async fn spawn_piped(launch: &Launch) -> Result<Supervised> {
    let mut command = tokio::process::Command::new(&launch.program);
    // Stdin is closed: every supported agent takes its prompt as an
    // argument, and Codex waits for end-of-input on an open pipe before it
    // starts. Follow-up input goes through a new turn, not stdin.
    command
        .args(&launch.args)
        .current_dir(&launch.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in &launch.env_remove {
        command.env_remove(name);
    }
    #[cfg(unix)]
    {
        // Its own group, so cancellation reaches everything it started.
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| {
        Error::connector(format!(
            "cannot start {}: {error}",
            launch.program.display()
        ))
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdin = child.stdin.take();
    let (tx, rx) = mpsc::channel(256);
    if let Some(stdout) = stdout {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(Line::Out(line)).await.is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stderr) = stderr {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx.send(Line::Err(line)).await.is_err() {
                    break;
                }
            }
        });
    }
    Ok(Supervised {
        lines: rx,
        control: Box::new(PipeChild {
            child,
            stdin,
            killed: false,
        }),
        transport: Transport::MachineReadable,
    })
}

/// Kills the process group (Unix) or the process tree (Windows) rooted at
/// `pid` with the platform's own tool, so no extra dependency is needed.
pub async fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = tokio::process::Command::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        let _ = tokio::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

/// True when a process with `pid` still exists.
pub async fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        tokio::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let output = tokio::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .await;
        output
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}
