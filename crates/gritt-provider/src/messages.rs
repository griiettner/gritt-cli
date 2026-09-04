//! Anthropic Messages adapter. The base URL is the API root without a
//! version segment (for example `https://api.anthropic.com`); endpoints are
//! `{base_url}/v1/messages` and `{base_url}/v1/models`. `max_tokens` and the
//! versioned `anthropic-version` header stay inside this adapter.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gritt_core::event::{Event, StopReason, Usage};
use gritt_core::provider::{
    EventStream, ModelCapabilities, PromptRequest, Protocol, ProviderAdapter, RequestOptions, Role,
};
use gritt_core::session::{BoxFuture, ContinuationState};
use gritt_core::tool::{ToolCall, ToolCallId, ToolDefinition, ToolResult};
use gritt_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::adapter::{
    cancelled_stream, is_cancelled, normalized_stream, stream_error, AdapterContext, EventEmitter,
    Normalizer,
};
use crate::sse::SseEvent;
use crate::transport::HttpRequest;

pub const OWNER: &str = "messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const THINKING_BUDGET: u32 = 1024;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct State {
    system: Option<String>,
    messages: Vec<serde_json::Value>,
    model: Option<String>,
    tools: Vec<serde_json::Value>,
    options: RequestOptions,
    sequence: u64,
}

pub struct MessagesAdapter {
    context: AdapterContext,
    emitter: Arc<EventEmitter>,
    state: Arc<Mutex<State>>,
}

impl MessagesAdapter {
    pub fn new(context: AdapterContext) -> Self {
        let emitter = Arc::new(EventEmitter::new(
            context.session_id.clone(),
            Protocol::Messages,
        ));
        Self {
            context,
            emitter,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn tool_schema(tool: &ToolDefinition) -> serde_json::Value {
        serde_json::json!({
            "name": tool.name,
            "description": tool.description,
            "input_schema": tool.parameters,
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/v1/messages",
            self.context.profile.base_url.trim_end_matches('/')
        )
    }

    async fn run(&self) -> Result<EventStream<'_>> {
        let body = {
            let state = self.state.lock().expect("messages state");
            let mut body = serde_json::json!({
                "model": state.model,
                "max_tokens": state.options.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
                "messages": state.messages,
                "stream": true,
            });
            if let Some(system) = &state.system {
                body["system"] = serde_json::json!(system);
            }
            if !state.tools.is_empty() {
                body["tools"] = serde_json::Value::Array(state.tools.clone());
            }
            if state.options.reasoning == Some(true) {
                body["thinking"] = serde_json::json!({
                    "type": "enabled",
                    "budget_tokens": THINKING_BUDGET,
                });
            }
            body
        };
        let key = self.context.key_for(&self.emitter)?;
        let request = HttpRequest::post_json(self.endpoint(), &body)
            .secret_header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("accept", "text/event-stream");
        let response = match self.context.send_checked(request, &self.emitter).await {
            Ok(response) => response,
            Err(error) if is_cancelled(&error) => return Ok(cancelled_stream(&self.emitter)),
            Err(error) => return Err(error),
        };
        let normalizer = MessagesNormalizer {
            state: Arc::clone(&self.state),
            ..MessagesNormalizer::default()
        };
        Ok(normalized_stream(
            Arc::clone(&self.emitter),
            self.context.cancel.clone(),
            response,
            normalizer,
        ))
    }
}

impl ProviderAdapter for MessagesAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::Messages
    }

    fn send(&self, request: PromptRequest) -> BoxFuture<'_, Result<EventStream<'_>>> {
        Box::pin(async move {
            if request.options.structured_output.is_some() {
                // Messages has no response-format field; the harness must
                // use a tool for structured output instead. This runs before
                // the capability check so no request-scoped warning is
                // queued for a request that never reaches the wire, and it
                // drops any warning an earlier unpolled stream left behind.
                self.emitter.clear_pending_diagnostic();
                return Err(Error::unsupported_capability(
                    &request.model,
                    "structured output on the Messages protocol",
                ));
            }
            self.context.check_capabilities(&request, &self.emitter)?;
            {
                let mut state = self.state.lock().expect("messages state");
                state.model = Some(request.model.clone());
                state.tools = request.tools.iter().map(Self::tool_schema).collect();
                state.options = request.options.clone();
                for message in &request.messages {
                    match message.role {
                        Role::System => state.system = Some(message.content.clone()),
                        Role::User => state.messages.push(serde_json::json!({
                            "role": "user", "content": message.content
                        })),
                        Role::Assistant => state.messages.push(serde_json::json!({
                            "role": "assistant", "content": message.content
                        })),
                    }
                }
            }
            self.run().await
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
                let mut state = self.state.lock().expect("messages state");
                if state.model.is_none() {
                    return Err(Error::config(
                        "no conversation to continue; send a prompt first",
                    ));
                }
                let blocks: Vec<serde_json::Value> = results
                    .into_iter()
                    .map(|result| {
                        serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": result.call_id.0,
                            "content": result.output,
                            "is_error": result.is_error,
                        })
                    })
                    .collect();
                state
                    .messages
                    .push(serde_json::json!({ "role": "user", "content": blocks }));
            }
            self.run().await
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
            *self.state.lock().expect("messages state") = restored;
            Ok(())
        })
    }

    fn continuation(&self) -> BoxFuture<'_, Result<Option<ContinuationState>>> {
        Box::pin(async move {
            let mut snapshot = self.state.lock().expect("messages state").clone();
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

#[derive(Debug, Clone)]
enum Block {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        partial_json: String,
    },
}

#[derive(Default)]
struct MessagesNormalizer {
    state: Arc<Mutex<State>>,
    open: BTreeMap<u64, Block>,
    finished: Vec<serde_json::Value>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    stop_reason: Option<String>,
    used_tool: bool,
    terminal: bool,
    skipped: Vec<String>,
}

impl Normalizer for MessagesNormalizer {
    fn handle(&mut self, emitter: &EventEmitter, sse: &SseEvent) -> Vec<Event> {
        if self.terminal {
            return Vec::new();
        }
        let Some(value) = sse.json() else {
            self.skipped.push(sse.data.chars().take(120).collect());
            return Vec::new();
        };
        let kind = value
            .get("type")
            .and_then(|v| v.as_str())
            .or(sse.event.as_deref())
            .unwrap_or_default()
            .to_owned();
        let index = value.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        match kind.as_str() {
            "message_start" => {
                self.input_tokens = value
                    .pointer("/message/usage/input_tokens")
                    .and_then(|v| v.as_u64());
                self.cached_input_tokens = value
                    .pointer("/message/usage/cache_read_input_tokens")
                    .and_then(|v| v.as_u64());
                Vec::new()
            }
            "ping" => Vec::new(),
            "content_block_start" => {
                let block = value.get("content_block").cloned().unwrap_or_default();
                let entry = match block.get("type").and_then(|v| v.as_str()) {
                    Some("tool_use") => Block::ToolUse {
                        id: block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        name: block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        partial_json: String::new(),
                    },
                    Some("thinking") => Block::Thinking(String::new()),
                    _ => Block::Text(
                        block
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                    ),
                };
                self.open.insert(index, entry);
                Vec::new()
            }
            "content_block_delta" => {
                let delta = value.get("delta").cloned().unwrap_or_default();
                let Some(block) = self.open.get_mut(&index) else {
                    self.skipped
                        .push(format!("delta for unknown block {index}"));
                    return Vec::new();
                };
                match (delta.get("type").and_then(|v| v.as_str()), block) {
                    (Some("text_delta"), Block::Text(text)) => {
                        let piece = delta
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        text.push_str(piece);
                        vec![emitter.text(piece.to_owned(), None)]
                    }
                    (Some("thinking_delta"), Block::Thinking(text)) => {
                        let piece = delta
                            .get("thinking")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        text.push_str(piece);
                        vec![emitter.reasoning(piece.to_owned(), None)]
                    }
                    (Some("input_json_delta"), Block::ToolUse { partial_json, .. }) => {
                        partial_json.push_str(
                            delta
                                .get("partial_json")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default(),
                        );
                        Vec::new()
                    }
                    // Signature deltas and other block-level metadata carry
                    // nothing the event model needs.
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => {
                let Some(block) = self.open.remove(&index) else {
                    return Vec::new();
                };
                match block {
                    Block::Text(text) => {
                        self.finished
                            .push(serde_json::json!({ "type": "text", "text": text }));
                        Vec::new()
                    }
                    Block::Thinking(text) => {
                        self.finished
                            .push(serde_json::json!({ "type": "thinking", "thinking": text }));
                        Vec::new()
                    }
                    Block::ToolUse {
                        id,
                        name,
                        partial_json,
                    } => {
                        let input: serde_json::Value = if partial_json.trim().is_empty() {
                            serde_json::json!({})
                        } else {
                            serde_json::from_str(&partial_json)
                                .unwrap_or_else(|_| serde_json::json!({ "_raw": partial_json }))
                        };
                        self.finished.push(serde_json::json!({
                            "type": "tool_use", "id": id, "name": name, "input": input
                        }));
                        self.used_tool = true;
                        vec![emitter.tool_call(
                            ToolCall {
                                id: ToolCallId(id),
                                name,
                                arguments: input,
                            },
                            Some(serde_json::json!({ "protocol": OWNER, "raw_type": "tool_use" })),
                        )]
                    }
                }
            }
            "message_delta" => {
                if let Some(reason) = value.pointer("/delta/stop_reason").and_then(|v| v.as_str()) {
                    self.stop_reason = Some(reason.to_owned());
                }
                let output = value
                    .pointer("/usage/output_tokens")
                    .and_then(|v| v.as_u64());
                vec![emitter.usage(
                    Usage {
                        input_tokens: self.input_tokens,
                        output_tokens: output,
                        reasoning_tokens: None,
                        cached_input_tokens: self.cached_input_tokens,
                    },
                    Some(serde_json::json!({ "protocol": OWNER, "usage": value.get("usage") })),
                )]
            }
            "message_stop" => {
                self.terminal = true;
                let content = std::mem::take(&mut self.finished);
                self.state
                    .lock()
                    .expect("messages state")
                    .messages
                    .push(serde_json::json!({ "role": "assistant", "content": content }));
                let stop_reason = match self.stop_reason.as_deref() {
                    Some("end_turn") | Some("stop_sequence") => StopReason::EndTurn,
                    Some("tool_use") => StopReason::ToolUse,
                    Some("max_tokens") => StopReason::MaxTokens,
                    Some("refusal") => StopReason::ContentFilter,
                    None if self.used_tool => StopReason::ToolUse,
                    None => StopReason::EndTurn,
                    Some(_) => StopReason::Other,
                };
                let mut diagnostic = serde_json::json!({
                    "protocol": OWNER,
                    "stop_reason": self.stop_reason,
                });
                if !self.skipped.is_empty() {
                    diagnostic["skipped"] = serde_json::json!(self.skipped);
                }
                vec![emitter.completed(stop_reason, Some(diagnostic))]
            }
            "error" => {
                self.terminal = true;
                let error =
                    stream_error(&value).unwrap_or_else(|| Error::provider(None, "stream error"));
                vec![emitter.error(&error)]
            }
            _ => {
                self.skipped.push(kind);
                Vec::new()
            }
        }
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
                "warning": "stream ended without message_stop",
                "skipped": self.skipped,
            })),
        )]
    }

    fn is_terminal(&self) -> bool {
        self.terminal
    }
}
