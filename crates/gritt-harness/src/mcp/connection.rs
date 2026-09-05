//! One live connection to one MCP server.
//!
//! Both transports present the same handle: a command channel served by a
//! background task that owns the process or the HTTP endpoint. The handle
//! assigns request ids, so a caller that stops waiting can name the request
//! in `notifications/cancelled` and tell its own transport to drop the work.
//!
//! Nothing here waits without a bound. A server that stops reading, or an
//! endpoint that never answers, must not be able to hold a caller or block
//! shutdown, so every queue operation has a deadline and shutdown is
//! signalled out of band rather than through the queue alone.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gritt_core::mcp::McpTransportKind;
use gritt_core::{Error, ErrorKind, Result};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Notify};

use super::jsonrpc;
use crate::CancellationToken;

/// How long a caller waits to hand work to the transport task before giving
/// up. Reaching it means the task is wedged, which is a connection failure.
pub const QUEUE_TIMEOUT: Duration = Duration::from_secs(5);

/// Last-resort bound on waiting for a transport task to release its
/// resources. Only a panicked task should ever reach it.
pub const HARD_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(60);

/// Giving up on a request must itself be prompt. Telling the server and
/// telling the transport are both best effort, so they get a short deadline
/// rather than the full queue one: cancellation should not be delayed by the
/// bookkeeping that follows it.
const GIVE_UP_TIMEOUT: Duration = Duration::from_millis(250);

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
        /// Present when the caller needs to know the message actually went
        /// out before it sends the next one.
        ack: Option<oneshot::Sender<Result<()>>>,
    },
    /// The caller stopped waiting for `id`. The transport drops its pending
    /// entry and cancels any local work still running for it.
    Abort { id: u64 },
    /// Ends the connection: closes stdin and waits out the grace period, or
    /// drops the HTTP session.
    Shutdown { reply: oneshot::Sender<()> },
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
    /// Raised before a shutdown command is queued, so a transport task
    /// blocked on a write can abandon it instead of waiting the command out.
    pub stopping: AtomicBool,
    /// Wakes a transport task parked on a write or a stream.
    pub stop: Notify,
    /// Set by the transport task once it has released everything it owns:
    /// the child process and its group, or the HTTP session.
    pub done: AtomicBool,
    /// Wakes whoever is waiting for that.
    pub finished: Notify,
}

impl ConnectionFlags {
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Waits until shutdown is requested. Returns at once when it already
    /// was, so a task that checks late does not miss the signal.
    pub async fn stopped(&self) {
        if self.is_stopping() {
            return;
        }
        self.stop.notified().await;
    }

    /// Marks the transport's resources released and wakes the waiters.
    pub fn finish(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.finished.notify_waiters();
    }

    /// Waits for the transport task to finish releasing its resources.
    pub async fn wait_finished(&self) {
        loop {
            if self.done.load(Ordering::SeqCst) {
                return;
            }
            // Registering before the re-check closes the race with a task
            // that finishes between the two.
            let waiting = self.finished.notified();
            if self.done.load(Ordering::SeqCst) {
                return;
            }
            waiting.await;
        }
    }
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
    /// Waiting stops immediately on cancellation or at the deadline. The
    /// server is told with `notifications/cancelled` so it can stop work, and
    /// the local transport is told to drop the request so a detached HTTP
    /// body or a pending stdio entry does not outlive the caller. A late
    /// response is discarded, as the specification requires.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.request_inner(method, params, deadline, cancel, true)
            .await
    }

    /// A request whose caller may stop waiting but whose cancellation must
    /// not be announced. The specification forbids cancelling `initialize`,
    /// so the handshake stops locally and drops the connection instead of
    /// sending a notification the server is required to ignore.
    pub async fn request_uncancellable(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        self.request_inner(method, params, deadline, cancel, false)
            .await
    }

    async fn request_inner(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
        cancel: &CancellationToken,
        announce: bool,
    ) -> Result<Value> {
        // Already cancelled: nothing is sent at all.
        if cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (reply, rx) = oneshot::channel();
        // Waiting for queue capacity is itself cancellable. Without this a
        // cancelled request could sit in the admission wait and then still be
        // written to the server.
        let admitted = self.enqueue(Command::Request {
            id,
            method: method.to_owned(),
            params,
            reply,
        });
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // The command may or may not have reached the queue. The
                // abort covers both: an id that never arrived is recorded as
                // abandoned, and one that did is dropped before it is sent.
                self.give_up(id, "the user cancelled the turn", announce).await;
                return Err(Error::cancelled());
            }
            outcome = admitted => outcome?,
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.give_up(id, "the user cancelled the turn", announce).await;
                Err(Error::cancelled())
            }
            _ = tokio::time::sleep(deadline) => {
                self.give_up(id, "the client deadline expired", announce).await;
                Err(Error::new(
                    ErrorKind::Config,
                    format!("`{method}` did not answer within {}s", deadline.as_secs()),
                ))
            }
            reply = rx => reply.unwrap_or_else(|_| Err(closed())),
        }
    }

    /// Hands one command to the transport task without waiting forever. A
    /// full queue past the deadline means the task cannot make progress.
    async fn enqueue(&self, command: Command) -> Result<()> {
        self.enqueue_within(command, QUEUE_TIMEOUT).await
    }

    async fn enqueue_within(&self, command: Command, deadline: Duration) -> Result<()> {
        match self.commands.send_timeout(command, deadline).await {
            Ok(()) => Ok(()),
            Err(mpsc::error::SendTimeoutError::Timeout(_)) => Err(Error::new(
                ErrorKind::Config,
                "the MCP connection stopped accepting messages",
            )),
            Err(mpsc::error::SendTimeoutError::Closed(_)) => Err(closed()),
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.enqueue(Command::Notify {
            method: method.to_owned(),
            params,
            ack: None,
        })
        .await
    }

    /// Sends a notification and waits for the transport to confirm it left.
    ///
    /// `notifications/initialized` must arrive before the first operation
    /// request, which on Streamable HTTP means before the next POST is even
    /// started. Its failure is a handshake failure, not something to discard.
    pub async fn notify_delivered(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<()> {
        let (ack, rx) = oneshot::channel();
        self.enqueue(Command::Notify {
            method: method.to_owned(),
            params,
            ack: Some(ack),
        })
        .await?;
        match tokio::time::timeout(deadline, rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(closed()),
            Err(_) => Err(Error::new(
                ErrorKind::Config,
                format!(
                    "`{method}` was not delivered within {}s",
                    deadline.as_secs()
                ),
            )),
        }
    }

    /// Stops waiting for `id`: optionally tells the server, and always tells
    /// the local transport to release the work.
    async fn give_up(&self, id: u64, reason: &str, announce: bool) {
        // The abort goes first: stopping the local work matters more than
        // telling the server, and it is what keeps an abandoned request from
        // being written after the caller has gone.
        let _ = self
            .enqueue_within(Command::Abort { id }, GIVE_UP_TIMEOUT)
            .await;
        if announce {
            let params = jsonrpc::cancellation_params(id, reason);
            // Best effort: a server that already answered ignores it, and a
            // connection that has gone needs nothing.
            let _ = self
                .enqueue_within(
                    Command::Notify {
                        method: jsonrpc::method::CANCELLED.to_owned(),
                        params,
                        ack: None,
                    },
                    GIVE_UP_TIMEOUT,
                )
                .await;
        }
    }

    /// Ends the connection and waits for the transport task to finish
    /// releasing its resources.
    ///
    /// The stop flag is raised first, so a task parked on a write to a server
    /// that stopped reading abandons it rather than waiting for a command it
    /// cannot reach. Completion is reported through the shared flag rather
    /// than a reply to the command, so this still waits for the real cleanup
    /// on the paths where the queue is already gone.
    ///
    /// `HARD_SHUTDOWN_TIMEOUT` only guards against a panicked transport task;
    /// the transports bound their own cleanup well below it.
    pub async fn shutdown(&self) {
        self.flags.stopping.store(true, Ordering::SeqCst);
        self.flags.stop.notify_waiters();
        // A nudge for a task waiting on the queue rather than on the flag.
        let (reply, _rx) = oneshot::channel();
        let _ = self.commands.try_send(Command::Shutdown { reply });
        let _ = tokio::time::timeout(HARD_SHUTDOWN_TIMEOUT, self.flags.wait_finished()).await;
    }
}

pub(super) fn closed() -> Error {
    Error::new(ErrorKind::Config, "the MCP connection is closed")
}
