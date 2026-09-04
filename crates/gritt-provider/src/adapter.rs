//! Pieces shared by every adapter: the context they are built from, key
//! and capability lookups, the request-time capability gate, event
//! emission with a monotonic sequence, and the stream wrapper that turns
//! cancellation into a terminal event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use futures::stream::{Stream, StreamExt};
use gritt_core::event::{Event, EventKind, EventSource, SessionStatus, StopReason, Usage};
use gritt_core::provider::{
    EventStream, ModelCapabilities, PromptRequest, Protocol, ProviderProfile,
};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::SessionId;
use gritt_core::tool::{ToolCall, ToolCallId};
use gritt_core::{Error, ErrorKind, Result};

use crate::cancel::CancellationToken;
use crate::sse::{sse_stream, SseEvent};
use crate::transport::{HttpRequest, HttpResponse, HttpTransport};

/// Resolves the key for a profile at request time. The binary's keychain
/// plus environment resolver implements it; tests use a fake.
pub trait KeyProvider: Send + Sync {
    fn key(&self, profile: &str, reference: &SecretRef) -> Result<Secret>;
}

/// A fixed key for tests. The value never reaches a formatter.
pub struct StaticKey(pub Secret);

impl KeyProvider for StaticKey {
    fn key(&self, _profile: &str, _reference: &SecretRef) -> Result<Secret> {
        Ok(self.0.clone())
    }
}

/// What the provider's model list reports for a model. `None` when the
/// model is unknown or the list is unavailable.
pub trait CapabilitySource: Send + Sync {
    fn capabilities(&self, profile: &str, model: &str) -> Option<ModelCapabilities>;
}

/// No capability data at all. Requests are sent as asked.
pub struct NoCapabilities;

impl CapabilitySource for NoCapabilities {
    fn capabilities(&self, _profile: &str, _model: &str) -> Option<ModelCapabilities> {
        None
    }
}

/// Everything an adapter needs besides its own wire logic.
#[derive(Clone)]
pub struct AdapterContext {
    pub profile: ProviderProfile,
    pub session_id: SessionId,
    pub transport: Arc<dyn HttpTransport>,
    pub keys: Arc<dyn KeyProvider>,
    pub capabilities: Arc<dyn CapabilitySource>,
    pub cancel: CancellationToken,
}

impl AdapterContext {
    pub fn key(&self) -> Result<Secret> {
        self.keys.key(&self.profile.name, &self.profile.key)
    }

    /// Refuses a request that asks for a feature the model list reports as
    /// unsupported. Unknown (`None`) capabilities do not block the request;
    /// the gap is recorded in the adapter's diagnostics instead.
    pub fn check_capabilities(&self, request: &PromptRequest) -> Result<Option<ModelCapabilities>> {
        let capabilities = self
            .capabilities
            .capabilities(&self.profile.name, &request.model);
        if let Some(capabilities) = &capabilities {
            if !request.tools.is_empty() && capabilities.tools == Some(false) {
                return Err(Error::unsupported_capability(&request.model, "tools"));
            }
            if request.options.structured_output.is_some()
                && capabilities.structured_output == Some(false)
            {
                return Err(Error::unsupported_capability(
                    &request.model,
                    "structured output",
                ));
            }
            if request.options.reasoning == Some(true) && capabilities.reasoning == Some(false) {
                return Err(Error::unsupported_capability(&request.model, "reasoning"));
            }
        }
        Ok(capabilities)
    }

    /// Sends the request and fails on a non-success status with the
    /// provider body kept in the diagnostic.
    pub async fn send_checked(&self, request: HttpRequest) -> Result<HttpResponse> {
        if self.cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let response = self.transport.send(request).await?;
        if response.is_success() {
            return Ok(response);
        }
        let status = response.status;
        let body = response.bytes().await.unwrap_or_default();
        Err(provider_error(status, &body))
    }
}

/// Builds the provider error for a failed response. The one-line message
/// comes from the body's `error.message` when present.
pub fn provider_error(status: u16, body: &[u8]) -> Error {
    let text = String::from_utf8_lossy(body);
    let diagnostic: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(
        |_| serde_json::json!({ "raw": text.chars().take(2000).collect::<String>() }),
    );
    let message = diagnostic
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .or_else(|| diagnostic.get("message").and_then(|value| value.as_str()))
        .unwrap_or("request rejected")
        .to_owned();
    Error::provider(Some(status), message).with_diagnostic(serde_json::json!({
        "status": status,
        "body": diagnostic,
    }))
}

/// Stamps events with the session, source, sequence, and timestamp.
pub struct EventEmitter {
    session_id: SessionId,
    protocol: Protocol,
    sequence: AtomicU64,
}

impl EventEmitter {
    pub fn new(session_id: SessionId, protocol: Protocol) -> Self {
        Self {
            session_id,
            protocol,
            sequence: AtomicU64::new(0),
        }
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Restores the sequence counter from stored continuation state.
    pub fn set_sequence(&self, next: u64) {
        self.sequence.store(next, Ordering::SeqCst);
    }

    pub fn next_sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    pub fn emit(&self, kind: EventKind, diagnostic: Option<serde_json::Value>) -> Event {
        Event {
            session_id: self.session_id.clone(),
            sequence: self.sequence.fetch_add(1, Ordering::SeqCst),
            source: EventSource::Native,
            timestamp: Utc::now(),
            kind,
            diagnostic,
        }
    }

    pub fn text(&self, text: String, raw: Option<serde_json::Value>) -> Event {
        self.emit(EventKind::TextDelta { text }, raw)
    }

    pub fn reasoning(&self, text: String, raw: Option<serde_json::Value>) -> Event {
        self.emit(EventKind::ReasoningSummary { text }, raw)
    }

    pub fn tool_call(&self, call: ToolCall, raw: Option<serde_json::Value>) -> Event {
        self.emit(EventKind::ToolCall { call }, raw)
    }

    pub fn usage(&self, usage: Usage, raw: Option<serde_json::Value>) -> Event {
        self.emit(EventKind::Usage { usage }, raw)
    }

    pub fn status(&self, status: SessionStatus) -> Event {
        self.emit(EventKind::StatusChanged { status }, None)
    }

    pub fn completed(&self, stop_reason: StopReason, raw: Option<serde_json::Value>) -> Event {
        self.emit(EventKind::Completed { stop_reason }, raw)
    }

    pub fn error(&self, error: &Error) -> Event {
        self.emit(
            EventKind::Error {
                error_kind: error.kind,
                message: error.message.clone(),
            },
            error.diagnostic.clone(),
        )
    }

    pub fn cancelled(&self) -> Event {
        self.emit(EventKind::Cancelled, None)
    }

    /// Diagnostic for a stream element the normalizer does not understand.
    /// The element is skipped, never fatal.
    pub fn unknown(&self, protocol: &str, raw: &SseEvent) -> serde_json::Value {
        serde_json::json!({
            "protocol": protocol,
            "warning": "unknown stream element skipped",
            "event": raw.event,
            "data": raw.data.chars().take(500).collect::<String>(),
        })
    }
}

/// A normalizer consumes one SSE event and returns the events it produced.
/// `finish` runs when the stream ends without an explicit completion.
pub trait Normalizer: Send {
    fn handle(&mut self, emitter: &EventEmitter, event: &SseEvent) -> Vec<Event>;
    fn finish(&mut self, emitter: &EventEmitter) -> Vec<Event>;
    /// Whether a terminal event (completed or error) has been emitted.
    fn is_terminal(&self) -> bool;
}

/// Runs the normalizer over a response body, honoring cancellation, and
/// boxes the result as the adapter's event stream.
pub fn normalized_stream<'a, N: Normalizer + 'a>(
    emitter: Arc<EventEmitter>,
    cancel: CancellationToken,
    response: HttpResponse,
    normalizer: N,
) -> EventStream<'a> {
    struct State<N> {
        events: std::pin::Pin<Box<dyn Stream<Item = Result<SseEvent>> + Send>>,
        emitter: Arc<EventEmitter>,
        cancel: CancellationToken,
        normalizer: N,
        pending: std::collections::VecDeque<Event>,
        done: bool,
    }
    let state = State {
        events: Box::pin(sse_stream(response.body)),
        emitter,
        cancel,
        normalizer,
        pending: Default::default(),
        done: false,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.pending.pop_front() {
                return Some((Ok(event), state));
            }
            if state.done {
                return None;
            }
            let next = tokio::select! {
                biased;
                _ = state.cancel.cancelled() => {
                    state.done = true;
                    // Dropping the body closes the connection.
                    state.events = Box::pin(futures::stream::empty());
                    return Some((Ok(state.emitter.cancelled()), state));
                }
                next = state.events.next() => next,
            };
            match next {
                Some(Ok(sse)) => {
                    let produced = state.normalizer.handle(&state.emitter, &sse);
                    state.pending.extend(produced);
                    if state.normalizer.is_terminal() {
                        state.done = true;
                    }
                }
                Some(Err(error)) => {
                    state.done = true;
                    state.pending.push_back(state.emitter.error(&error));
                }
                None => {
                    state.done = true;
                    let produced = state.normalizer.finish(&state.emitter);
                    state.pending.extend(produced);
                }
            }
        }
    }))
}

/// Accumulates a streamed tool call from indexed fragments.
#[derive(Debug, Default, Clone)]
pub struct PartialToolCall {
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: String,
}

impl PartialToolCall {
    pub fn finish(self, fallback_index: usize) -> ToolCall {
        let arguments = if self.arguments.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&self.arguments)
                .unwrap_or_else(|_| serde_json::json!({ "_raw": self.arguments }))
        };
        ToolCall {
            id: ToolCallId(self.id.unwrap_or_else(|| format!("call_{fallback_index}"))),
            name: self.name.unwrap_or_default(),
            arguments,
        }
    }
}

/// Reads an error element embedded in a stream body.
pub fn stream_error(value: &serde_json::Value) -> Option<Error> {
    let error = value.get("error")?;
    let message = error
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("provider stream error")
        .to_owned();
    let status = error
        .get("code")
        .and_then(|value| value.as_u64())
        .and_then(|code| u16::try_from(code).ok());
    Some(Error::provider(status, message).with_diagnostic(serde_json::json!({ "body": value })))
}

pub fn is_cancelled(error: &Error) -> bool {
    error.kind == ErrorKind::Cancelled
}

/// Reads the key from the environment variable a [`SecretRef`] names.
/// Used by live tests and as the fallback half of the binary's resolver.
pub struct EnvKeys;

impl KeyProvider for EnvKeys {
    fn key(&self, profile: &str, reference: &SecretRef) -> Result<Secret> {
        std::env::var(&reference.env_var_name)
            .ok()
            .filter(|value| !value.is_empty())
            .map(Secret::new)
            .ok_or_else(|| Error::missing_key(profile, &reference.env_var_name))
    }
}
