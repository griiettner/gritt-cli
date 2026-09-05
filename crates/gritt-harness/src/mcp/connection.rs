//! One live connection to one MCP server.
//!
//! Both transports present the same handle: a command channel served by a
//! background task that owns the process or the HTTP endpoint. The handle
//! assigns request ids, so a caller that stops waiting can name the request
//! in `notifications/cancelled`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gritt_core::mcp::McpTransportKind;
use gritt_core::{Error, ErrorKind, Result};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::jsonrpc;
use crate::CancellationToken;

/// What the handle asks the transport task to do.
pub(super) enum Command {
    Request {
        id: u64,
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value>>,
    },
    Notify {
        method: String,
        params: Value,
    },
    /// Ends the connection: closes stdin and waits out the grace period, or
    /// drops the HTTP session.
    Shutdown {
        reply: oneshot::Sender<()>,
    },
}

/// Flags a transport task sets and the runtime reads between turns.
#[derive(Debug, Default)]
pub struct ConnectionFlags {
    /// The server sent `notifications/tools/list_changed`.
    pub tools_changed: AtomicBool,
    /// The transport ended: the process exited or the session was lost.
    pub closed: AtomicBool,
    /// The negotiated revision, once the handshake settled. The Streamable
    /// HTTP transport must send it on every later request.
    pub protocol: std::sync::Mutex<Option<String>>,
}

pub struct Connection {
    commands: mpsc::Sender<Command>,
    next_id: AtomicU64,
    flags: Arc<ConnectionFlags>,
    kind: McpTransportKind,
}

impl Connection {
    pub(super) fn new(
        commands: mpsc::Sender<Command>,
        flags: Arc<ConnectionFlags>,
        kind: McpTransportKind,
    ) -> Self {
        Self {
            commands,
            // The handshake uses id 1, so operation starts above it.
            next_id: AtomicU64::new(1),
            flags,
            kind,
        }
    }

    pub fn kind(&self) -> McpTransportKind {
        self.kind
    }

    /// Records the revision the handshake settled on.
    pub fn set_protocol_version(&self, version: &str) {
        *self.flags.protocol.lock().expect("mcp protocol") = Some(version.to_owned());
    }

    /// True once and then false: the runtime rediscovers tools on the first
    /// read after the notification and does not repeat it.
    pub fn take_tools_changed(&self) -> bool {
        self.flags.tools_changed.swap(false, Ordering::SeqCst)
    }

    pub fn is_closed(&self) -> bool {
        self.flags.closed.load(Ordering::SeqCst) || self.commands.is_closed()
    }

    /// Sends a request and waits for its response.
    ///
    /// Waiting stops immediately on cancellation or at the deadline, and the
    /// server is told with `notifications/cancelled` so it can stop work.
    /// A late response is discarded by the transport task, as the
    /// specification requires. `initialize` is never cancelled this way; the
    /// runtime uses [`Connection::request_uncancellable`] for it.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let (id, rx) = self.dispatch(method, params).await?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.notify_cancelled(id, "the user cancelled the turn").await;
                Err(Error::cancelled())
            }
            _ = tokio::time::sleep(deadline) => {
                self.notify_cancelled(id, "the client deadline expired").await;
                Err(Error::new(
                    ErrorKind::Config,
                    format!("`{method}` did not answer within {}s", deadline.as_secs()),
                ))
            }
            reply = rx => reply.unwrap_or_else(|_| Err(closed())),
        }
    }

    /// A request that may only time out. The specification forbids
    /// cancelling `initialize`, so its deadline ends the whole connection
    /// instead of sending a cancellation the server must ignore.
    pub async fn request_uncancellable(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value> {
        let (_, rx) = self.dispatch(method, params).await?;
        match tokio::time::timeout(deadline, rx).await {
            Ok(reply) => reply.unwrap_or_else(|_| Err(closed())),
            Err(_) => Err(Error::new(
                ErrorKind::Config,
                format!("`{method}` did not answer within {}s", deadline.as_secs()),
            )),
        }
    }

    async fn dispatch(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(u64, oneshot::Receiver<Result<Value>>)> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (reply, rx) = oneshot::channel();
        self.commands
            .send(Command::Request {
                id,
                method: method.to_owned(),
                params,
                reply,
            })
            .await
            .map_err(|_| closed())?;
        Ok((id, rx))
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.commands
            .send(Command::Notify {
                method: method.to_owned(),
                params,
            })
            .await
            .map_err(|_| closed())
    }

    /// Best effort: a server that has already answered ignores it, and a
    /// connection that has already gone needs nothing.
    async fn notify_cancelled(&self, id: u64, reason: &str) {
        let params = jsonrpc::cancellation(id, reason);
        let params = params.get("params").cloned().unwrap_or(Value::Null);
        let _ = self.notify(jsonrpc::method::CANCELLED, params).await;
    }

    /// Ends the connection and waits for the transport task to finish
    /// releasing its resources.
    pub async fn shutdown(&self) {
        let (reply, rx) = oneshot::channel();
        if self
            .commands
            .send(Command::Shutdown { reply })
            .await
            .is_ok()
        {
            let _ = rx.await;
        }
    }
}

pub(super) fn closed() -> Error {
    Error::new(ErrorKind::Config, "the MCP connection is closed")
}
