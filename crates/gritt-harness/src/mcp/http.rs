//! The Streamable HTTP transport: one MCP endpoint that answers a POST with
//! either a JSON object or an SSE stream.
//!
//! Gritt reuses the provider crate's HTTP client and incremental SSE parser
//! rather than adding an MCP client dependency. Each message is its own POST,
//! so requests run concurrently and a cancellation can leave while another
//! request is still open. The optional server-initiated `GET` stream is not
//! opened: Gritt advertises no client capability that needs one, and
//! `tools/list_changed` also arrives on the POST streams.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use gritt_core::mcp::McpTransportKind;
use gritt_core::secret::{is_secret_env_name, Secret};
use gritt_core::{Error, Result};
use gritt_provider::sse::SseParser;
use gritt_provider::transport::{HttpRequest, HttpTransport};
use serde_json::Value;
use tokio::sync::mpsc;

use super::connection::{Command, Connection, ConnectionFlags};
use super::jsonrpc::{self, Incoming};

/// Largest non-streamed body read from an endpoint.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Shared endpoint state. The session id is assigned by the server on the
/// response that carries `InitializeResult` and must travel on every later
/// request.
struct Endpoint {
    transport: Arc<dyn HttpTransport>,
    url: String,
    headers: BTreeMap<String, String>,
    session: Mutex<Option<String>>,
    flags: Arc<ConnectionFlags>,
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
    });
    let (tx, mut commands) = mpsc::channel::<Command>(64);
    let task = Arc::clone(&endpoint);
    tokio::spawn(async move {
        while let Some(command) = commands.recv().await {
            match command {
                Command::Request {
                    id,
                    method,
                    params,
                    reply,
                } => {
                    // Each request is its own POST, so a cancellation sent
                    // while this one is open still goes out immediately.
                    let endpoint = Arc::clone(&task);
                    tokio::spawn(async move {
                        let frame = jsonrpc::request(id, &method, params);
                        let outcome = post(&endpoint, frame, Some(id)).await;
                        let _ = reply.send(outcome);
                    });
                }
                Command::Notify { method, params } => {
                    let endpoint = Arc::clone(&task);
                    tokio::spawn(async move {
                        let frame = jsonrpc::notification(&method, params);
                        let _ = post(&endpoint, frame, None).await;
                    });
                }
                Command::Shutdown { reply } => {
                    end_session(&task).await;
                    task.flags.closed.store(true, Ordering::SeqCst);
                    let _ = reply.send(());
                    break;
                }
            }
        }
    });
    Connection::new(tx, flags, McpTransportKind::Http)
}

/// Asks the server to release the session. A server that does not allow
/// client termination answers 405, which is not a failure.
async fn end_session(endpoint: &Endpoint) {
    let has_session = endpoint.session.lock().expect("mcp session").is_some();
    if !has_session {
        return;
    }
    let request = endpoint.request(HttpRequest::delete(&endpoint.url));
    let _ = endpoint.transport.send(request).await;
}

/// Sends one message and, when `expect` names a request id, waits for that
/// response on whichever body shape the server chose.
async fn post(endpoint: &Endpoint, frame: Value, expect: Option<u64>) -> Result<Value> {
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
    endpoint: &Endpoint,
    mut body: gritt_provider::transport::ByteStream,
    expect: u64,
) -> Result<Value> {
    let mut parser = SseParser::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        for event in parser.feed(&chunk) {
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
fn route(endpoint: &Endpoint, value: Value, expect: u64) -> Option<Result<Value>> {
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
        // Server-initiated requests need a POST back, which Gritt does not
        // make: it advertises no capability that asks for one.
        _ => None,
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
    fn credential_headers_are_carried_as_secrets() {
        assert!(is_auth_header("Authorization"));
        assert!(!is_auth_header("x-region"));
        assert!(is_secret_env_name("X-Api-Key", &[]));
    }
}
