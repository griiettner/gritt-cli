//! The Streamable HTTP transport: one MCP endpoint that answers a POST with
//! either a JSON object or an SSE stream.
//!
//! Gritt reuses the provider crate's HTTP client and incremental SSE parser
//! rather than adding an MCP client dependency. Each message is its own POST,
//! so requests run concurrently and a cancellation can leave while another
//! request is still open. The optional server-initiated `GET` stream is not
//! opened: Gritt advertises no client capability that needs one, and
//! `tools/list_changed` also arrives on the POST streams.
//!
//! Every request task is owned rather than detached. A caller that stops
//! waiting, a reload, or a shutdown cancels the work it started, so no HTTP
//! body outlives the connection that asked for it. That cancels Gritt's local
//! wait only: a remote server may still complete a side effect, which is why
//! a call is never replayed.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use gritt_core::mcp::McpTransportKind;
use gritt_core::secret::{is_secret_env_name, Secret};
use gritt_core::{Error, Result};
use gritt_provider::sse::SseParser;
use gritt_provider::transport::{HttpRequest, HttpTransport};
use serde_json::Value;
use tokio::sync::{mpsc, Semaphore};

use super::connection::{Command, Connection, ConnectionFlags};
use super::jsonrpc::{self, Incoming};

/// Largest non-streamed body read from an endpoint.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Largest amount of SSE input accepted between two complete events, which
/// bounds what the parser can accumulate from a delimiter-free stream.
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

/// Largest total body accepted on one stream.
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// How many requests may be in flight against one endpoint at once.
const MAX_INFLIGHT_REQUESTS: usize = 32;

/// A separate, smaller budget for the POSTs Gritt makes on its own account:
/// notifications and answers to server requests. Separate because an answer
/// is produced while a request permit is held, so sharing one budget could
/// deadlock a busy endpoint.
const MAX_AUXILIARY_POSTS: usize = 8;

/// Bound on ending the session, so an unresponsive endpoint cannot hold up
/// application exit.
const DELETE_TIMEOUT: Duration = Duration::from_secs(5);

/// Bound on delivering a notification the caller is waiting for.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(30);

/// How many abandoned request ids are remembered, so a request whose abort
/// overtook it is dropped rather than sent.
const MAX_ABANDONED_IDS: usize = 1024;

/// Shared endpoint state. The session id is assigned by the server on the
/// response that carries `InitializeResult` and must travel on every later
/// request.
struct Endpoint {
    transport: Arc<dyn HttpTransport>,
    url: String,
    headers: BTreeMap<String, String>,
    session: Mutex<Option<String>>,
    flags: Arc<ConnectionFlags>,
    /// Bounds concurrent outstanding work against one server.
    permits: Semaphore,
    /// Bounds Gritt's own POSTs: notifications and server-request answers.
    auxiliary: Semaphore,
    /// Those POSTs as owned tasks, so shutdown can cancel and await them
    /// rather than leaving them holding credentials and permits.
    aux_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Endpoint {
    /// Spawns one of Gritt's own POSTs and keeps its handle.
    fn spawn_auxiliary(self: &Arc<Self>, frame: Value) {
        let endpoint = Arc::clone(self);
        let handle = tokio::spawn(async move {
            let _ = post_with(&endpoint, frame, None, &endpoint.auxiliary).await;
        });
        let mut tasks = self.aux_tasks.lock().expect("mcp aux tasks");
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    /// Cancels and reaps every auxiliary POST.
    async fn drain_auxiliary(&self) {
        let tasks: Vec<tokio::task::JoinHandle<()>> = {
            let mut held = self.aux_tasks.lock().expect("mcp aux tasks");
            std::mem::take(&mut held)
        };
        for task in tasks {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Endpoint {
    fn request(&self, mut request: HttpRequest) -> HttpRequest {
        for (name, value) in &self.headers {
            // A configured credential keeps its redaction all the way to the
            // wire, so a debug print of the request cannot leak it.
            if is_secret_env_name(name, &[]) || is_auth_header(name) {
                request = request.secret_header(name, Secret::new(value.clone()));
            } else {
                request = request.header(name, value.clone());
            }
        }
        if let Some(session) = self.session.lock().expect("mcp session").as_ref() {
            request = request.header("mcp-session-id", session.clone());
        }
        if let Some(version) = self.flags.protocol.lock().expect("mcp protocol").as_ref() {
            request = request.header("mcp-protocol-version", version.clone());
        }
        request
    }
}

fn is_auth_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "x-auth-token"
    )
}

/// Opens the endpoint and starts its transport task. Nothing is sent until
/// the runtime issues `initialize`.
pub fn connect(
    transport: Arc<dyn HttpTransport>,
    url: &str,
    headers: &BTreeMap<String, String>,
) -> Connection {
    let flags = Arc::new(ConnectionFlags::default());
    let endpoint = Arc::new(Endpoint {
        transport,
        url: url.to_owned(),
        headers: headers.clone(),
        session: Mutex::new(None),
        flags: Arc::clone(&flags),
        permits: Semaphore::new(MAX_INFLIGHT_REQUESTS),
        auxiliary: Semaphore::new(MAX_AUXILIARY_POSTS),
        aux_tasks: Mutex::new(Vec::new()),
    });
    let (tx, mut commands) = mpsc::channel::<Command>(64);
    let task = Arc::clone(&endpoint);
    tokio::spawn(async move {
        // Every spawned request is tracked so it can be cancelled by the
        // caller, by a reload, or by shutdown, and awaited before shutdown
        // reports that it is done.
        let mut inflight: HashMap<u64, tokio::task::JoinHandle<()>> = HashMap::new();
        // Ids whose caller gave up before the request was admitted here.
        let mut abandoned: VecDeque<u64> = VecDeque::new();
        loop {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    match command {
                        Command::Request { id, method, params, reply } => {
                            // The caller stopped waiting before this was
                            // admitted, so it must never be sent.
                            if let Some(index) =
                                abandoned.iter().position(|known| *known == id)
                            {
                                abandoned.remove(index);
                                let _ = reply.send(Err(Error::cancelled()));
                                continue;
                            }
                            let endpoint = Arc::clone(&task);
                            let handle = tokio::spawn(async move {
                                let frame = jsonrpc::request(id, &method, params);
                                let outcome = post(&endpoint, frame, Some(id)).await;
                                let _ = reply.send(outcome);
                            });
                            inflight.insert(id, handle);
                        }
                        Command::Notify { method, params, ack } => {
                            let frame = jsonrpc::notification(&method, params);
                            match ack {
                                // A delivery barrier: `notifications/
                                // initialized` must reach the server before
                                // the next POST is even started, so this one
                                // is awaited in the loop rather than spawned.
                                Some(ack) => {
                                    let outcome = match tokio::time::timeout(
                                        NOTIFY_TIMEOUT,
                                        post(&task, frame, None),
                                    )
                                    .await
                                    {
                                        Ok(outcome) => outcome.map(|_| ()),
                                        Err(_) => Err(Error::config(
                                            "the endpoint did not accept the notification in time",
                                        )),
                                    };
                                    let _ = ack.send(outcome);
                                }
                                // Fire and forget for the caller, but still
                                // owned here: a hung notification would
                                // otherwise keep credentials and a permit
                                // past shutdown.
                                None => task.spawn_auxiliary(frame),
                            }
                        }
                        Command::Abort { id } => match inflight.remove(&id) {
                            Some(handle) => {
                                // Stops Gritt reading the body. The server may
                                // still finish; nothing is retried.
                                handle.abort();
                            }
                            None => {
                                abandoned.push_back(id);
                                while abandoned.len() > MAX_ABANDONED_IDS {
                                    abandoned.pop_front();
                                }
                            }
                        },
                        Command::Shutdown { .. } => break,
                    }
                }
                _ = task.flags.stopped() => break,
            }
            inflight.retain(|_, handle| !handle.is_finished());
        }
        // Cancel and reap everything this connection started before saying it
        // is done: an aborted task still has to unwind.
        for (_, handle) in inflight.drain() {
            handle.abort();
            let _ = handle.await;
        }
        task.drain_auxiliary().await;
        end_session(&task).await;
        task.flags.closed.store(true, Ordering::SeqCst);
        task.flags.finish();
    });
    Connection::new(tx, flags, McpTransportKind::Http)
}

/// Asks the server to release the session. A server that does not allow
/// client termination answers 405, which is not a failure, and an endpoint
/// that does not answer at all must not delay exit.
async fn end_session(endpoint: &Endpoint) {
    let has_session = endpoint.session.lock().expect("mcp session").is_some();
    if !has_session {
        return;
    }
    let request = endpoint.request(HttpRequest::delete(&endpoint.url));
    let _ = tokio::time::timeout(DELETE_TIMEOUT, endpoint.transport.send(request)).await;
}

/// Sends one message and, when `expect` names a request id, waits for that
/// response on whichever body shape the server chose.
async fn post(endpoint: &Arc<Endpoint>, frame: Value, expect: Option<u64>) -> Result<Value> {
    post_with(endpoint, frame, expect, &endpoint.permits).await
}

/// The POST itself, against a named permit pool.
///
/// Every client message goes through here, which is what guarantees each one
/// carries both accepted content types, the session, and the negotiated
/// revision. Answers to server requests are client messages too.
async fn post_with(
    endpoint: &Arc<Endpoint>,
    frame: Value,
    expect: Option<u64>,
    permits: &Semaphore,
) -> Result<Value> {
    let _permit = permits
        .acquire()
        .await
        .map_err(|_| Error::config("the MCP connection is closed"))?;
    let request = endpoint
        .request(HttpRequest::post_json(&endpoint.url, &frame))
        .header("accept", "application/json, text/event-stream");
    let response = endpoint.transport.send(request).await?;
    let status = response.status;
    if let Some(session) = response.header("mcp-session-id") {
        *endpoint.session.lock().expect("mcp session") = Some(session.to_owned());
    }
    match status {
        // A notification or response was accepted; there is no body.
        202 => return Ok(Value::Null),
        404 => {
            *endpoint.session.lock().expect("mcp session") = None;
            endpoint.flags.closed.store(true, Ordering::SeqCst);
            return Err(Error::config(
                "the server ended the MCP session; reconnect to continue",
            ));
        }
        405 => {
            return Err(Error::config(
                "the endpoint does not accept MCP messages on this method",
            ))
        }
        _ => {}
    }
    if !(200..300).contains(&status) {
        // The body may hold anything, including an echoed header, so only
        // the status is reported.
        return Err(Error::config(format!(
            "the MCP endpoint returned HTTP {status}"
        )));
    }
    let streaming = response
        .content_type
        .as_deref()
        .is_some_and(|value| value.starts_with("text/event-stream"));
    let Some(expect) = expect else {
        return Ok(Value::Null);
    };
    if streaming {
        read_stream(endpoint, response.body, expect).await
    } else {
        let body = read_bounded(response.body).await?;
        let value: Value = serde_json::from_slice(&body)
            .map_err(|_| Error::config("the MCP endpoint returned a body that is not JSON"))?;
        match route(endpoint, value, expect) {
            Some(outcome) => outcome,
            None => Err(Error::config(
                "the MCP endpoint answered without the requested response",
            )),
        }
    }
}

async fn read_stream(
    endpoint: &Arc<Endpoint>,
    mut body: gritt_provider::transport::ByteStream,
    expect: u64,
) -> Result<Value> {
    let mut parser = SseParser::new();
    let mut budget = SseBudget::new(MAX_EVENT_BYTES, MAX_STREAM_BYTES);
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        // Admitted before the parser is handed anything, so it is never asked
        // to buffer more than the bound allows.
        if let Err(error) = budget.admit(&chunk) {
            endpoint.flags.closed.store(true, Ordering::SeqCst);
            return Err(error);
        }
        let events = parser.feed(&chunk);
        for event in events {
            let Some(value) = event.json() else { continue };
            if let Some(outcome) = route(endpoint, value, expect) {
                return outcome;
            }
        }
    }
    Err(Error::config(
        "the MCP stream closed before the response arrived",
    ))
}

/// Handles one decoded message, returning the caller's answer when this is
/// it. A disconnection is not a cancellation, so nothing is retried here.
fn route(endpoint: &Arc<Endpoint>, value: Value, expect: u64) -> Option<Result<Value>> {
    match jsonrpc::classify(&value) {
        Incoming::Response { id, result } if id == expect => {
            Some(result.map_err(|error| Error::config(error.summary())))
        }
        Incoming::Notification { method, .. } => {
            if method == jsonrpc::method::TOOLS_LIST_CHANGED {
                endpoint.flags.tools_changed.store(true, Ordering::SeqCst);
            }
            None
        }
        // A server request needs an answer even on this transport: `ping`
        // depends on no capability, and anything else must be refused rather
        // than left waiting.
        Incoming::Request { id, method } => {
            let frame = if method == jsonrpc::method::PING {
                jsonrpc::response(id, serde_json::json!({}))
            } else {
                jsonrpc::error_response(
                    id,
                    jsonrpc::METHOD_NOT_FOUND,
                    &format!("gritt does not implement `{method}`"),
                )
            };
            // Sent through the same helper as every other client message, so
            // it carries both accepted content types; a strict endpoint
            // rejects a POST without them and the originating call would then
            // wait forever. It is spawned because this stream is still
            // carrying the caller's own response.
            endpoint.spawn_auxiliary(frame);
            None
        }
        _ => None,
    }
}

/// Tracks how much of an SSE stream has arrived and how much of it the
/// parser can still be holding for an event that has not ended.
///
/// Kept apart from the read loop so the limits can be exercised directly at
/// sizes a test can produce.
struct SseBudget {
    pending: usize,
    total: usize,
    max_event: usize,
    max_stream: usize,
}

impl SseBudget {
    fn new(max_event: usize, max_stream: usize) -> Self {
        Self {
            pending: 0,
            total: 0,
            max_event,
            max_stream,
        }
    }

    /// Accounts for one chunk before it is parsed, refusing it when either
    /// bound would be crossed.
    fn admit(&mut self, chunk: &[u8]) -> Result<()> {
        self.total += chunk.len();
        if self.total > self.max_stream {
            return Err(Error::config("the MCP stream exceeded its size limit"));
        }
        if self.pending + chunk.len() > self.max_event {
            return Err(Error::config("the MCP stream sent an oversized event"));
        }
        // What the parser still holds is whatever followed the last
        // terminator in this chunk. Resetting to zero because some event
        // completed would forget a trailing fragment and let it grow
        // unbounded.
        self.pending = trailing_bytes(chunk, self.pending);
        Ok(())
    }
}

/// Bytes after the last SSE event terminator in `chunk`, carrying `pending`
/// forward when the chunk contains no terminator at all.
///
/// An event ends at a blank line, so the terminator is `\n\n` or `\r\n\r\n`.
/// Everything after the last one is still being accumulated by the parser.
fn trailing_bytes(chunk: &[u8], pending: usize) -> usize {
    let mut boundary: Option<usize> = None;
    for index in 0..chunk.len() {
        let after_lf = index + 2 <= chunk.len() && &chunk[index..index + 2] == b"\n\n";
        let after_crlf = index + 4 <= chunk.len() && &chunk[index..index + 4] == b"\r\n\r\n";
        if after_crlf {
            boundary = Some(index + 4);
        } else if after_lf {
            boundary = Some(index + 2);
        }
    }
    match boundary {
        Some(end) => chunk.len() - end,
        None => pending + chunk.len(),
    }
}

async fn read_bounded(mut body: gritt_provider::transport::ByteStream) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = body.next().await {
        out.extend_from_slice(&chunk?);
        if out.len() > MAX_BODY_BYTES {
            return Err(Error::config("the MCP endpoint returned an oversized body"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_bytes_tracks_what_the_parser_still_holds() {
        // No terminator: everything accumulates on top of what came before.
        assert_eq!(trailing_bytes(b"data: abc", 10), 19);
        // A terminator resets to whatever followed it, not to zero.
        assert_eq!(trailing_bytes(b"data: a\n\ndata: b", 100), 7);
        // CRLF framing is the same rule.
        assert_eq!(trailing_bytes(b"data: a\r\n\r\nxy", 100), 2);
        // A chunk that ends exactly on a boundary leaves nothing pending.
        assert_eq!(trailing_bytes(b"data: a\n\n", 100), 0);
        // The last terminator wins, not the first.
        assert_eq!(trailing_bytes(b"a\n\nbb\n\nccc", 0), 3);
    }

    #[test]
    fn an_oversized_event_is_refused_even_after_earlier_events_completed() {
        let mut budget = SseBudget::new(32, 1024);
        // Two complete events, well inside the bound.
        assert!(budget.admit(b"data: a\n\ndata: b\n\n").is_ok());
        // A fragment that follows them is still accounted for: the earlier
        // completions do not reset it.
        assert!(budget.admit(b"data: ").is_ok());
        assert!(budget.admit(&[b'x'; 20]).is_ok());
        let error = budget.admit(&[b'x'; 20]).unwrap_err();
        assert!(error.message.contains("oversized event"), "{error}");
    }

    #[test]
    fn a_single_event_larger_than_the_bound_is_refused_before_parsing() {
        let mut budget = SseBudget::new(16, 1024);
        let error = budget.admit(&[b'y'; 64]).unwrap_err();
        assert!(error.message.contains("oversized event"), "{error}");
    }

    #[test]
    fn a_stream_that_stays_within_the_event_bound_can_still_end() {
        // Many small complete events: each one ends on a boundary, so the
        // per-event count resets and only the stream bound can stop it.
        let event = b"data: a\n\n";
        let mut budget = SseBudget::new(16, event.len() * 4);
        for _ in 0..4 {
            budget.admit(event).unwrap();
        }
        let error = budget.admit(event).unwrap_err();
        assert!(error.message.contains("size limit"), "{error}");
    }

    #[test]
    fn credential_headers_are_carried_as_secrets() {
        assert!(is_auth_header("Authorization"));
        assert!(!is_auth_header("x-region"));
        assert!(is_secret_env_name("X-Api-Key", &[]));
    }
}
