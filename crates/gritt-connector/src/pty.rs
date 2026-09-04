//! PTY transport: the fallback for an agent whose machine-readable
//! interface is unavailable. Output is read on a blocking thread and
//! delivered as lines with the carriage returns a terminal adds removed.
//! There is no separate stderr on a PTY, so every line is `Line::Out`.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gritt_core::connector::Transport;
use gritt_core::session::BoxFuture;
use gritt_core::{Error, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::mpsc;

use crate::process::{ChildControl, ExitOutcome, Launch, Line, Supervised};

struct PtyChild {
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    writer: Option<Box<dyn Write + Send>>,
    pid: Option<u32>,
    killed: bool,
}

impl ChildControl for PtyChild {
    fn pid(&self) -> Option<u32> {
        self.pid
    }

    fn write_line<'a>(&'a mut self, line: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let writer = self
                .writer
                .as_mut()
                .ok_or_else(|| Error::connector("the agent's terminal input is closed"))?;
            writer
                .write_all(line.as_bytes())
                .and_then(|()| writer.write_all(b"\n"))
                .and_then(|()| writer.flush())
                .map_err(|error| Error::connector(format!("cannot write to the agent: {error}")))
        })
    }

    fn kill(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.killed {
                return;
            }
            self.killed = true;
            if let Some(pid) = self.pid {
                crate::process::kill_tree(pid).await;
            }
            let child = Arc::clone(&self.child);
            let _ = tokio::task::spawn_blocking(move || {
                let mut child = child.lock().expect("pty child");
                let _ = child.kill();
                let _ = child.wait();
            })
            .await;
        })
    }

    fn wait(&mut self, limit: Duration) -> BoxFuture<'_, Option<ExitOutcome>> {
        Box::pin(async move {
            let child = Arc::clone(&self.child);
            let waiter = tokio::task::spawn_blocking(move || {
                let mut child = child.lock().expect("pty child");
                child.wait().ok()
            });
            match tokio::time::timeout(limit, waiter).await {
                Ok(Ok(Some(status))) => Some(ExitOutcome {
                    code: Some(status.exit_code() as i32),
                    success: status.success(),
                }),
                Ok(_) => Some(ExitOutcome {
                    code: None,
                    success: false,
                }),
                Err(_) => None,
            }
        })
    }
}

pub async fn spawn(launch: &Launch) -> Result<Supervised> {
    let launch = launch.clone();
    let (tx, rx) = mpsc::channel(256);
    let spawned = tokio::task::spawn_blocking(move || -> Result<PtyChild> {
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows: 40,
                cols: 200,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::connector(format!("cannot open a pty: {error}")))?;
        let mut command = CommandBuilder::new(&launch.program);
        command.args(&launch.args);
        command.cwd(&launch.cwd);
        for name in &launch.env_remove {
            command.env_remove(name);
        }
        let child = pair.slave.spawn_command(command).map_err(|error| {
            Error::connector(format!(
                "cannot start {} on a pty: {error}",
                launch.program.display()
            ))
        })?;
        drop(pair.slave);
        let pid = child.process_id();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| Error::connector(format!("cannot read the pty: {error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| Error::connector(format!("cannot write the pty: {error}")))?;
        // The master must outlive the reader thread or the slave sees EOF.
        let master = pair.master;
        std::thread::spawn(move || {
            let _master = master;
            let mut pending = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                pending.extend_from_slice(&buffer[..read]);
                while let Some(index) = pending.iter().position(|byte| *byte == b'\n') {
                    let mut line = pending.drain(..=index).collect::<Vec<u8>>();
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    let text = String::from_utf8_lossy(&line).into_owned();
                    if tx.blocking_send(Line::Out(text)).is_err() {
                        return;
                    }
                }
            }
            if !pending.is_empty() {
                let text = String::from_utf8_lossy(&pending).into_owned();
                let _ = tx.blocking_send(Line::Out(text));
            }
        });
        Ok(PtyChild {
            child: Arc::new(Mutex::new(child)),
            writer: Some(writer),
            pid,
            killed: false,
        })
    })
    .await
    .map_err(|error| Error::connector(format!("pty task failed: {error}")))??;
    Ok(Supervised {
        lines: rx,
        control: Box::new(spawned),
        transport: Transport::Pty,
    })
}
