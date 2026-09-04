//! Pieces shared by every adapter: the context they are built from, key
//! and capability lookups, the request-time capability gate, event
//! emission with a monotonic sequence, and the stream wrapper that turns
//! cancellation into a terminal event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

    /// Resolves the key and registers it with the emitter so every error
    /// message and diagnostic produced for this request is redacted.
    pub fn key_for(&self, emitter: &EventEmitter) -> Result<Secret> {
        let key = self
            .key()
            .inspect_err(|_| emitter.clear_pending_diagnostic())?;
        emitter.protect(key.clone());
        Ok(key)
    }

    /// Refuses a request that asks for a feature the model list reports as
    /// unsupported. Unknown (`None`) capabilities do not block the request,
    /// because OpenAI and Anthropic lists report no capability flags at all
    /// (recorded exception in TKT-0010). The gap is made visible instead: a
    /// `capability_warning` diagnostic naming the unreported features is
    /// attached to the first event of the stream. The warning is scoped to
    /// this request: any warning left by an earlier request is dropped
    /// here, and every pre-stream failure clears it again.
    pub fn check_capabilities(
        &self,
        request: &PromptRequest,
        emitter: &EventEmitter,
    ) -> Result<Option<ModelCapabilities>> {
        emitter.clear_pending_diagnostic();
        let capabilities = self
            .capabilities
            .capabilities(&self.profile.name, &request.model);
        let mut requested: Vec<(&str, Option<bool>)> = Vec::new();
        if !request.tools.is_empty() {
            requested.push(("tools", capabilities.as_ref().and_then(|c| c.tools)));
        }
        if request.options.structured_output.is_some() {
            requested.push((
                "structured output",
                capabilities.as_ref().and_then(|c| c.structured_output),
            ));
        }
        if request.options.reasoning == Some(true) {
            requested.push(("reasoning", capabilities.as_ref().and_then(|c| c.reasoning)));
        }
        let mut unreported = Vec::new();
        for (feature, reported) in requested {
            match reported {
                Some(false) => {
                    return Err(Error::unsupported_capability(&request.model, feature));
                }
                Some(true) => {}
                None => unreported.push(feature),
            }
        }
        if !unreported.is_empty() {
            emitter.set_pending_diagnostic(serde_json::json!({
                "warning": "provider did not report support for requested features",
                "model": request.model,
                "features": unreported,
                "model_list_entry": capabilities.is_some(),
            }));
        }
        Ok(capabilities)
    }

    /// Sends the request and fails on a non-success status with the
    /// redacted provider body kept in the diagnostic. Cancellation is
    /// observed while the transport waits for the connection and headers;
    /// the pending send is dropped and `Cancelled` is returned. Any failure
    /// here ends the request, so a queued capability warning is cleared and
    /// cannot attach to a later request.
    pub async fn send_checked(
        &self,
        request: HttpRequest,
        emitter: &EventEmitter,
    ) -> Result<HttpResponse> {
        let result = self.send_unchecked(request, emitter).await;
        if result.is_err() {
            emitter.clear_pending_diagnostic();
        }
        result
    }

    async fn send_unchecked(
        &self,
        request: HttpRequest,
        emitter: &EventEmitter,
    ) -> Result<HttpResponse> {
        if self.cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let response = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Err(Error::cancelled()),
            response = self.transport.send(request) => response?,
        };
        if response.is_success() {
            return Ok(response);
        }
        let status = response.status;
        let body = response.bytes().await.unwrap_or_default();
        Err(provider_error(status, &body, &emitter.secrets()))
    }
}

/// Longest provider body kept in a diagnostic, in characters.
pub const MAX_DIAGNOSTIC_BODY_CHARS: usize = 4096;
const REDACTED: &str = "[redacted]";

/// Replaces every occurrence of each non-empty secret with `[redacted]`.
/// Length is no exemption: a short credential is still a credential.
pub fn redact_text(text: &str, secrets: &[Secret]) -> String {
    let mut out = text.to_owned();
    for secret in secrets {
        let value = secret.expose();
        if !value.is_empty() && out.contains(value) {
            out = out.replace(value, REDACTED);
        }
    }
    out
}

/// Redacts every string inside a JSON value, including object keys.
pub fn redact_value(value: serde_json::Value, secrets: &[Secret]) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(redact_text(&text, secrets)),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(|item| redact_value(item, secrets))
                .collect(),
        ),
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, item)| (redact_text(&key, secrets), redact_value(item, secrets)))
                .collect(),
        ),
        other => other,
    }
}

/// Builds the provider error for a failed response. The body is redacted
/// against the request's secrets and capped before anything is retained;
/// the one-line message comes from the body's `error.message` when present.
pub fn provider_error(status: u16, body: &[u8], secrets: &[Secret]) -> Error {
    let text = redact_text(&String::from_utf8_lossy(body), secrets);
    let truncated = text.chars().count() > MAX_DIAGNOSTIC_BODY_CHARS;
    let diagnostic: serde_json::Value = if truncated {
        serde_json::json!({
            "raw": text.chars().take(MAX_DIAGNOSTIC_BODY_CHARS).collect::<String>(),
            "truncated": true,
        })
    } else {
        serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "raw": text }))
    };
    let message = diagnostic
        .pointer("/error/message")
        .and_then(|value| value.as_str())
        .or_else(|| diagnostic.get("message").and_then(|value| value.as_str()))
        .unwrap_or("request rejected")
        .chars()
        .take(500)
        .collect::<String>();
    Error::provider(Some(status), message).with_diagnostic(serde_json::json!({
        "status": status,
        "body": diagnostic,
    }))
}

/// Stamps events with the session, source, sequence, and timestamp, and
/// redacts every registered secret out of messages and diagnostics.
pub struct EventEmitter {
    session_id: SessionId,
    protocol: Protocol,
    sequence: AtomicU64,
    secrets: Mutex<Vec<Secret>>,
    pending_diagnostic: Mutex<Option<serde_json::Value>>,
}

impl EventEmitter {
    pub fn new(session_id: SessionId, protocol: Protocol) -> Self {
        Self {
            session_id,
            protocol,
            sequence: AtomicU64::new(0),
            secrets: Mutex::new(Vec::new()),
            pending_diagnostic: Mutex::new(None),
        }
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }

    /// Registers a secret to redact from every later event.
    pub fn protect(&self, secret: Secret) {
        let mut secrets = self.secrets.lock().expect("emitter secrets");
        if !secrets
            .iter()
            .any(|known| known.expose() == secret.expose())
        {
            secrets.push(secret);
        }
    }

    /// The secrets registered so far, for redacting errors built outside
    /// the emitter.
    pub fn secrets(&self) -> Vec<Secret> {
        self.secrets.lock().expect("emitter secrets").clone()
    }

    /// Queues a diagnostic that is merged into the next emitted event under
    /// `capability_warning`.
    pub fn set_pending_diagnostic(&self, diagnostic: serde_json::Value) {
        *self.pending_diagnostic.lock().expect("pending diagnostic") = Some(diagnostic);
    }

    /// Drops a queued diagnostic so it cannot attach to another request.
    pub fn clear_pending_diagnostic(&self) {
        *self.pending_diagnostic.lock().expect("pending diagnostic") = None;
    }

    fn merge_pending(&self, diagnostic: Option<serde_json::Value>) -> Option<serde_json::Value> {
        let pending = self
            .pending_diagnostic
            .lock()
            .expect("pending diagnostic")
            .take();
        match (diagnostic, pending) {
            (diagnostic, None) => diagnostic,
            (None, Some(pending)) => Some(serde_json::json!({ "capability_warning": pending })),
            (Some(serde_json::Value::Object(mut map)), Some(pending)) => {
                map.insert("capability_warning".into(), pending);
                Some(serde_json::Value::Object(map))
            }
            (Some(other), Some(pending)) => Some(serde_json::json!({
                "raw": other,
                "capability_warning": pending,
            })),
        }
    }

    /// Restores the sequence counter from stored continuation state.
    pub fn set_sequence(&self, next: u64) {
        self.sequence.store(next, Ordering::SeqCst);
    }

    pub fn next_sequence(&self) -> u64 {
        self.sequence.load(Ordering::SeqCst)
    }

    pub fn emit(&self, kind: EventKind, diagnostic: Option<serde_json::Value>) -> Event {
        let secrets = self.secrets();
        let diagnostic = self
            .merge_pending(diagnostic)
            .map(|diagnostic| redact_value(diagnostic, &secrets));
        let kind = match kind {
            EventKind::Error {
                error_kind,
                message,
            } => EventKind::Error {
                error_kind,
                message: redact_text(&message, &secrets),
            },
            other => other,
        };
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

/// A stream holding only the terminal `Cancelled` event, for a request
/// cancelled before the provider answered.
pub fn cancelled_stream<'a>(emitter: &EventEmitter) -> EventStream<'a> {
    Box::pin(futures::stream::iter([Ok(emitter.cancelled())]))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_replaces_every_occurrence_including_short_values() {
        let secrets = vec![
            Secret::new("sk-live-1234"),
            Secret::new("ab"),
            Secret::new(""),
        ];
        assert_eq!(
            redact_text("key sk-live-1234 twice sk-live-1234 ab", &secrets),
            "key [redacted] twice [redacted] [redacted]"
        );
        let value = serde_json::json!({
            "sk-live-1234": ["Bearer sk-live-1234", 7, { "nested": "sk-live-1234" }]
        });
        let redacted = serde_json::to_string(&redact_value(value, &secrets)).unwrap();
        assert!(!redacted.contains("sk-live-1234"));
        assert_eq!(redacted.matches("[redacted]").count(), 3);
    }

    #[test]
    fn emitter_redacts_error_messages_and_merges_the_pending_warning_once() {
        let emitter = EventEmitter::new(SessionId("s".into()), Protocol::ChatCompletions);
        emitter.protect(Secret::new("sk-live-1234"));
        emitter.set_pending_diagnostic(serde_json::json!({ "features": ["tools"] }));
        let error = Error::provider(Some(401), "bad key sk-live-1234")
            .with_diagnostic(serde_json::json!({ "body": "sk-live-1234" }));
        let first = emitter.error(&error);
        match &first.kind {
            EventKind::Error { message, .. } => assert!(message.ends_with("bad key [redacted]")),
            other => panic!("unexpected {other:?}"),
        }
        let diagnostic = first.diagnostic.unwrap();
        assert_eq!(diagnostic["body"], "[redacted]");
        assert_eq!(diagnostic["capability_warning"]["features"][0], "tools");
        let second = emitter.text("hi".into(), None);
        assert!(second.diagnostic.is_none());
    }

    #[test]
    fn clearing_the_pending_warning_keeps_it_off_later_events() {
        let emitter = EventEmitter::new(SessionId("s".into()), Protocol::Responses);
        emitter.set_pending_diagnostic(serde_json::json!({ "features": ["tools"] }));
        emitter.clear_pending_diagnostic();
        assert!(emitter.text("hi".into(), None).diagnostic.is_none());
    }
}
