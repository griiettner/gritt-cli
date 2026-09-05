//! OpenAI Responses adapter. Continuation is `previous_response_id`: the
//! adapter stores the top-level response `id` and sends it verbatim on the
//! next request. Endpoint is `{base_url}/responses`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gritt_core::event::{Event, StopReason, Usage};
use gritt_core::provider::{
    EventStream, ModelCapabilities, PromptRequest, Protocol, ProviderAdapter, ReasoningIntent,
    RequestOptions, Role,
};
use gritt_core::session::{BoxFuture, ContinuationState};
use gritt_core::tool::{ToolCall, ToolCallId, ToolDefinition, ToolResult};
use gritt_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::adapter::{
    cancelled_stream, is_cancelled, normalized_stream, stream_error, AdapterContext, EventEmitter,
    Normalizer, PartialToolCall,
};
use crate::sse::SseEvent;
use crate::transport::HttpRequest;

pub const OWNER: &str = "responses";

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct State {
    previous_response_id: Option<String>,
    instructions: Option<String>,
    model: Option<String>,
    tools: Vec<serde_json::Value>,
    options: RequestOptions,
    sequence: u64,
}

pub struct ResponsesAdapter {
    context: AdapterContext,
    emitter: Arc<EventEmitter>,
    state: Arc<Mutex<State>>,
}

impl ResponsesAdapter {
    pub fn new(context: AdapterContext) -> Self {
        let emitter = Arc::new(EventEmitter::new(
            context.session_id.clone(),
            Protocol::Responses,
        ));
        Self {
            context,
            emitter,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn tool_schema(tool: &ToolDefinition) -> serde_json::Value {
        serde_json::json!({
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/responses",
            self.context.profile.base_url.trim_end_matches('/')
        )
    }

    async fn run(&self, input: serde_json::Value) -> Result<EventStream<'_>> {
        let body = {
            let state = self.state.lock().expect("responses state");
            let mut body = serde_json::json!({
                "model": state.model,
                "input": input,
                "stream": true,
            });
            if let Some(instructions) = &state.instructions {
                body["instructions"] = serde_json::json!(instructions);
            }
            if let Some(previous) = &state.previous_response_id {
                body["previous_response_id"] = serde_json::json!(previous);
            }
            if !state.tools.is_empty() {
                body["tools"] = serde_json::Value::Array(state.tools.clone());
            }
            if let Some(max_tokens) = state.options.max_tokens {
                body["max_output_tokens"] = serde_json::json!(max_tokens);
            }
            // Effort maps to the documented `reasoning.effort` field. The
            // legacy switch enables reasoning at the provider's default
            // level; `Auto` sends nothing. The intent was validated before
            // the first request, so a contradictory stored state falls
            // back to sending nothing rather than failing a continuation.
            match state.options.reasoning_intent() {
                Ok(ReasoningIntent::Explicit(effort)) => {
                    body["reasoning"] =
                        serde_json::json!({ "effort": effort.as_str(), "summary": "auto" });
                }
                Ok(ReasoningIntent::Enabled) => {
                    body["reasoning"] = serde_json::json!({ "summary": "auto" });
                }
                Ok(ReasoningIntent::Default) | Err(_) => {}
            }
            if let Some(schema) = &state.options.structured_output {
                body["text"] = serde_json::json!({
                    "format": { "type": "json_schema", "name": "response", "schema": schema }
                });
            }
            body
        };
        let key = self.context.key_for(&self.emitter)?;
        let request = HttpRequest::post_json(self.endpoint(), &body)
            .secret_header(
                "authorization",
                gritt_core::secret::Secret::new(format!("Bearer {}", key.expose())),
            )
            .header("accept", "text/event-stream");
        let response = match self.context.send_checked(request, &self.emitter).await {
            Ok(response) => response,
            Err(error) if is_cancelled(&error) => return Ok(cancelled_stream(&self.emitter)),
            Err(error) => return Err(error),
        };
        let normalizer = ResponsesNormalizer {
            state: Arc::clone(&self.state),
            ..ResponsesNormalizer::default()
        };
        Ok(normalized_stream(
            Arc::clone(&self.emitter),
            self.context.cancel.clone(),
            response,
            normalizer,
        ))
    }
}

impl ProviderAdapter for ResponsesAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::Responses
    }

    fn send(&self, request: PromptRequest) -> BoxFuture<'_, Result<EventStream<'_>>> {
        Box::pin(async move {
            self.context.check_capabilities(&request, &self.emitter)?;
            let mut input = Vec::new();
            {
                let mut state = self.state.lock().expect("responses state");
                state.model = Some(request.model.clone());
                state.tools = request.tools.iter().map(Self::tool_schema).collect();
                state.options = request.options.clone();
                for message in &request.messages {
                    match message.role {
                        Role::System => state.instructions = Some(message.content.clone()),
                        Role::User => input.push(serde_json::json!({
                            "role": "user", "content": message.content
                        })),
                        Role::Assistant => input.push(serde_json::json!({
                            "role": "assistant", "content": message.content
                        })),
                    }
                }
            }
            self.run(serde_json::Value::Array(input)).await
        })
    }

    fn submit_tool_results(
        &self,
        results: Vec<ToolResult>,
    ) -> BoxFuture<'_, Result<EventStream<'_>>> {
        Box::pin(async move {
            // A continuation never queues its own warning, so anything still
            // pending belongs to an earlier stream that was dropped unpolled.
            self.emitter.clear_pending_diagnostic();
            {
                let state = self.state.lock().expect("responses state");
                if state.previous_response_id.is_none() {
                    return Err(Error::config(
                        "no response to continue; send a prompt first",
                    ));
                }
            }
            let input: Vec<serde_json::Value> = results
                .into_iter()
                .map(|result| {
                    serde_json::json!({
                        "type": "function_call_output",
                        "call_id": result.call_id.0,
                        "output": result.output,
                    })
                })
                .collect();
            self.run(serde_json::Value::Array(input)).await
        })
    }

    fn restore(&self, state: ContinuationState) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if state.owner != OWNER {
                return Err(Error::config(format!(
                    "continuation state belongs to `{}`, not `{OWNER}`",
                    state.owner
                )));
            }
            let restored: State = serde_json::from_value(state.state)
                .map_err(|error| Error::config(format!("invalid continuation state: {error}")))?;
            self.emitter.set_sequence(restored.sequence);
            *self.state.lock().expect("responses state") = restored;
            Ok(())
        })
    }

    fn continuation(&self) -> BoxFuture<'_, Result<Option<ContinuationState>>> {
        Box::pin(async move {
            let mut snapshot = self.state.lock().expect("responses state").clone();
            if snapshot.model.is_none() {
                return Ok(None);
            }
            snapshot.sequence = self.emitter.next_sequence();
            Ok(Some(ContinuationState {
                owner: OWNER.into(),
                state: serde_json::to_value(snapshot)
                    .map_err(|error| Error::config(error.to_string()))?,
            }))
        })
    }

    fn capabilities(&self, model: &str) -> BoxFuture<'_, Result<ModelCapabilities>> {
        let found = self
            .context
            .capabilities
            .capabilities(&self.context.profile.name, model);
        Box::pin(async move { Ok(found.unwrap_or_default()) })
    }
}

#[derive(Default)]
struct ResponsesNormalizer {
    state: Arc<Mutex<State>>,
    calls: BTreeMap<u64, PartialToolCall>,
    emitted_tool_call: bool,
    terminal: bool,
    skipped: Vec<String>,
    /// The last `sequence_number` seen on the wire.
    last_wire_sequence: Option<u64>,
    /// Every gap or regression observed, for the completion diagnostic.
    sequence_warnings: Vec<serde_json::Value>,
}

impl ResponsesNormalizer {
    /// Tracks the wire `sequence_number` without reordering anything.
    /// Returns a warning when the number gaps or regresses.
    fn note_sequence(&mut self, value: &serde_json::Value) -> Option<serde_json::Value> {
        let wire = value.get("sequence_number").and_then(|v| v.as_u64())?;
        let previous = self.last_wire_sequence.replace(wire);
        let expected = previous.map(|p| p + 1)?;
        if wire == expected {
            return None;
        }
        let warning = serde_json::json!({
            "protocol": OWNER,
            "warning": if wire > expected { "wire sequence gap" } else { "wire sequence regressed" },
            "wire_sequence": wire,
            "expected_wire_sequence": expected,
        });
        self.sequence_warnings.push(warning.clone());
        Some(warning)
    }

    /// Attaches a sequence warning to every event one wire element produced.
    fn attach_warning(events: &mut [Event], warning: &serde_json::Value) {
        for event in events {
            event.diagnostic = Some(match event.diagnostic.take() {
                Some(serde_json::Value::Object(mut map)) => {
                    map.insert("sequence_warning".into(), warning.clone());
                    serde_json::Value::Object(map)
                }
                Some(other) => serde_json::json!({ "raw": other, "sequence_warning": warning }),
                None => serde_json::json!({ "sequence_warning": warning }),
            });
        }
    }
    fn remember_response_id(&self, value: &serde_json::Value) {
        if let Some(id) = value.pointer("/response/id").and_then(|v| v.as_str()) {
            self.state
                .lock()
                .expect("responses state")
                .previous_response_id = Some(id.to_owned());
        }
    }

    fn complete(&mut self, emitter: &EventEmitter, value: &serde_json::Value) -> Vec<Event> {
        self.terminal = true;
        self.remember_response_id(value);
        let mut events = Vec::new();
        if let Some(usage) = value.pointer("/response/usage") {
            events.push(
                emitter.usage(
                    Usage {
                        input_tokens: usage.get("input_tokens").and_then(|v| v.as_u64()),
                        output_tokens: usage.get("output_tokens").and_then(|v| v.as_u64()),
                        reasoning_tokens: usage
                            .pointer("/output_tokens_details/reasoning_tokens")
                            .and_then(|v| v.as_u64()),
                        cached_input_tokens: usage
                            .pointer("/input_tokens_details/cached_tokens")
                            .and_then(|v| v.as_u64()),
                    },
                    Some(serde_json::json!({ "protocol": OWNER, "usage": usage })),
                ),
            );
        }
        let status = value.pointer("/response/status").and_then(|v| v.as_str());
        let incomplete = value
            .pointer("/response/incomplete_details/reason")
            .and_then(|v| v.as_str());
        let stop_reason = match (status, incomplete) {
            (_, Some("max_output_tokens")) => StopReason::MaxTokens,
            (_, Some("content_filter")) => StopReason::ContentFilter,
            (Some("completed"), _) if self.emitted_tool_call => StopReason::ToolUse,
            (Some("completed"), _) => StopReason::EndTurn,
            _ => StopReason::Other,
        };
        let mut diagnostic = serde_json::json!({
            "protocol": OWNER,
            "response_id": value.pointer("/response/id"),
            "status": status,
            "incomplete_reason": incomplete,
            "last_wire_sequence": self.last_wire_sequence,
        });
        if !self.skipped.is_empty() {
            diagnostic["skipped"] = serde_json::json!(self.skipped);
        }
        if !self.sequence_warnings.is_empty() {
            diagnostic["sequence_warnings"] = serde_json::json!(self.sequence_warnings);
        }
        events.push(emitter.completed(stop_reason, Some(diagnostic)));
        events
    }
}

impl Normalizer for ResponsesNormalizer {
    fn handle(&mut self, emitter: &EventEmitter, sse: &SseEvent) -> Vec<Event> {
        if self.terminal || sse.is_done() {
            return Vec::new();
        }
        let Some(value) = sse.json() else {
            self.skipped.push(sse.data.chars().take(120).collect());
            return Vec::new();
        };
        let warning = self.note_sequence(&value);
        let mut events = self.handle_element(emitter, sse, &value);
        if let Some(warning) = warning {
            Self::attach_warning(&mut events, &warning);
        }
        events
    }

    fn finish(&mut self, emitter: &EventEmitter) -> Vec<Event> {
        if self.terminal {
            return Vec::new();
        }
        self.terminal = true;
        vec![emitter.completed(
            StopReason::Other,
            Some(serde_json::json!({
                "protocol": OWNER,
                "warning": "stream ended without response.completed",
                "skipped": self.skipped,
                "last_wire_sequence": self.last_wire_sequence,
                "sequence_warnings": self.sequence_warnings,
            })),
        )]
    }

    fn is_terminal(&self) -> bool {
        self.terminal
    }
}

impl ResponsesNormalizer {
    fn handle_element(
        &mut self,
        emitter: &EventEmitter,
        sse: &SseEvent,
        value: &serde_json::Value,
    ) -> Vec<Event> {
        let kind = value
            .get("type")
            .and_then(|v| v.as_str())
            .or(sse.event.as_deref())
            .unwrap_or_default()
            .to_owned();
        match kind.as_str() {
            "response.created" | "response.in_progress" => {
                self.remember_response_id(value);
                Vec::new()
            }
            "response.output_text.delta" => value
                .get("delta")
                .and_then(|v| v.as_str())
                .filter(|text| !text.is_empty())
                .map(|text| vec![emitter.text(text.to_owned(), None)])
                .unwrap_or_default(),
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => value
                .get("delta")
                .and_then(|v| v.as_str())
                .filter(|text| !text.is_empty())
                .map(|text| vec![emitter.reasoning(text.to_owned(), None)])
                .unwrap_or_default(),
            "response.output_item.added" => {
                let item = value.get("item").cloned().unwrap_or_default();
                if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                    let index = value
                        .get("output_index")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let partial = self.calls.entry(index).or_default();
                    partial.id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned);
                    partial.name = item.get("name").and_then(|v| v.as_str()).map(str::to_owned);
                }
                Vec::new()
            }
            "response.function_call_arguments.delta" => {
                let index = value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if let Some(delta) = value.get("delta").and_then(|v| v.as_str()) {
                    self.calls
                        .entry(index)
                        .or_default()
                        .arguments
                        .push_str(delta);
                }
                Vec::new()
            }
            "response.output_item.done" => {
                let item = value.get("item").cloned().unwrap_or_default();
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    return Vec::new();
                }
                let index = value
                    .get("output_index")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let partial = self.calls.remove(&index).unwrap_or_default();
                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or(partial.arguments);
                let call = ToolCall {
                    id: ToolCallId(
                        item.get("call_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                            .or(partial.id)
                            .unwrap_or_else(|| format!("call_{index}")),
                    ),
                    name: item
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                        .or(partial.name)
                        .unwrap_or_default(),
                    arguments: serde_json::from_str(&arguments)
                        .unwrap_or_else(|_| serde_json::json!({ "_raw": arguments })),
                };
                self.emitted_tool_call = true;
                vec![emitter.tool_call(
                    call,
                    Some(serde_json::json!({ "protocol": OWNER, "item_id": item.get("id") })),
                )]
            }
            "response.completed" | "response.incomplete" => self.complete(emitter, value),
            "response.failed" => {
                self.terminal = true;
                let error =
                    stream_error(value.get("response").unwrap_or(value)).unwrap_or_else(|| {
                        Error::provider(None, "response failed")
                            .with_diagnostic(serde_json::json!({ "body": value }))
                    });
                vec![emitter.error(&error)]
            }
            "error" => {
                self.terminal = true;
                let error = stream_error(&serde_json::json!({ "error": value }))
                    .unwrap_or_else(|| Error::provider(None, "stream error"));
                vec![emitter.error(&error)]
            }
            _ => {
                self.skipped.push(kind);
                Vec::new()
            }
        }
    }
}
