//! OpenAI-compatible Chat Completions adapter. One implementation serves
//! OpenRouter, OpenAI in Chat Completions mode, and any generic endpoint;
//! the profile supplies the base URL and key. Endpoints are
//! `{base_url}/chat/completions` and `{base_url}/models`, so the base URL
//! includes the `/v1` segment (for example `https://openrouter.ai/api/v1`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gritt_core::event::{Event, StopReason, Usage};
use gritt_core::provider::{
    EventStream, ModelCapabilities, PromptRequest, Protocol, ProviderAdapter, RequestOptions, Role,
};
use gritt_core::session::{BoxFuture, ContinuationState};
use gritt_core::tool::{ToolDefinition, ToolResult};
use gritt_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::adapter::{
    cancelled_stream, is_cancelled, normalized_stream, stream_error, AdapterContext, EventEmitter,
    Normalizer, PartialToolCall,
};
use crate::sse::SseEvent;
use crate::transport::HttpRequest;

pub const OWNER: &str = "chat_completions";
const ATTRIBUTION_REFERER: &str = "https://github.com/griiettner/gritt-cli";
const ATTRIBUTION_TITLE: &str = "Gritt";

/// Wire-form conversation kept for continuation.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct State {
    messages: Vec<serde_json::Value>,
    model: Option<String>,
    tools: Vec<serde_json::Value>,
    options: RequestOptions,
    sequence: u64,
}

pub struct ChatCompletionsAdapter {
    context: AdapterContext,
    emitter: Arc<EventEmitter>,
    state: Arc<Mutex<State>>,
}

impl ChatCompletionsAdapter {
    pub fn new(context: AdapterContext) -> Self {
        let emitter = Arc::new(EventEmitter::new(
            context.session_id.clone(),
            Protocol::ChatCompletions,
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
            "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            }
        })
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.context.profile.base_url.trim_end_matches('/')
        )
    }

    fn is_openrouter(&self) -> bool {
        self.context.profile.base_url.contains("openrouter.ai")
    }

    fn build_body(
        &self,
        state: &State,
        capabilities: Option<&ModelCapabilities>,
    ) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": state.model,
            "messages": state.messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if !state.tools.is_empty() {
            body["tools"] = serde_json::Value::Array(state.tools.clone());
        }
        if let Some(max_tokens) = state.options.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }
        if let Some(schema) = &state.options.structured_output {
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "json_schema": { "name": "response", "schema": schema },
            });
        }
        // Reasoning is only requested where the list says it exists; the
        // OpenRouter form is the one Chat Completions endpoints document.
        if state.options.reasoning == Some(true)
            && capabilities.and_then(|c| c.reasoning) == Some(true)
        {
            body["reasoning"] = serde_json::json!({ "effort": "medium" });
        }
        body
    }

    async fn run(&self) -> Result<EventStream<'_>> {
        let (body, model) = {
            let state = self.state.lock().expect("chat state");
            let model = state.model.clone().unwrap_or_default();
            let probe = PromptRequest {
                model: model.clone(),
                messages: Vec::new(),
                tools: Vec::new(),
                options: state.options.clone(),
            };
            let capabilities = self
                .context
                .capabilities
                .capabilities(&self.context.profile.name, &probe.model);
            (self.build_body(&state, capabilities.as_ref()), model)
        };
        let key = self.context.key_for(&self.emitter)?;
        let mut request = HttpRequest::post_json(self.endpoint(), &body)
            .secret_header("authorization", key_bearer(&key))
            .header("accept", "text/event-stream");
        if self.is_openrouter() {
            request = request
                .header("http-referer", ATTRIBUTION_REFERER)
                .header("x-title", ATTRIBUTION_TITLE);
        }
        let response = match self.context.send_checked(request, &self.emitter).await {
            Ok(response) => response,
            Err(error) if is_cancelled(&error) => return Ok(cancelled_stream(&self.emitter)),
            Err(error) => return Err(error),
        };
        let normalizer = ChatNormalizer {
            state: Arc::clone(&self.state),
            model,
            ..ChatNormalizer::default()
        };
        Ok(normalized_stream(
            Arc::clone(&self.emitter),
            self.context.cancel.clone(),
            response,
            normalizer,
        ))
    }
}

fn key_bearer(key: &gritt_core::secret::Secret) -> gritt_core::secret::Secret {
    gritt_core::secret::Secret::new(format!("Bearer {}", key.expose()))
}

fn wire_role(role: &Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

impl ProviderAdapter for ChatCompletionsAdapter {
    fn protocol(&self) -> Protocol {
        Protocol::ChatCompletions
    }

    fn send(&self, request: PromptRequest) -> BoxFuture<'_, Result<EventStream<'_>>> {
        Box::pin(async move {
            self.context.check_capabilities(&request, &self.emitter)?;
            {
                let mut state = self.state.lock().expect("chat state");
                state.model = Some(request.model.clone());
                state.tools = request.tools.iter().map(Self::tool_schema).collect();
                state.options = request.options.clone();
                for message in &request.messages {
                    state.messages.push(serde_json::json!({
                        "role": wire_role(&message.role),
                        "content": message.content,
                    }));
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
                let mut state = self.state.lock().expect("chat state");
                if state.model.is_none() {
                    return Err(Error::config(
                        "no conversation to continue; send a prompt first",
                    ));
                }
                for result in results {
                    state.messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": result.call_id.0,
                        "content": result.output,
                    }));
                }
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
            *self.state.lock().expect("chat state") = restored;
            Ok(())
        })
    }

    fn continuation(&self) -> BoxFuture<'_, Result<Option<ContinuationState>>> {
        Box::pin(async move {
            let mut snapshot = self.state.lock().expect("chat state").clone();
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

/// Normalizes `choices[].delta` fragments into events and records the
/// assembled assistant turn for continuation.
#[derive(Default)]
struct ChatNormalizer {
    state: Arc<Mutex<State>>,
    model: String,
    text: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    finish_reason: Option<String>,
    terminal: bool,
    skipped: Vec<String>,
}

impl ChatNormalizer {
    fn finalize(&mut self, emitter: &EventEmitter) -> Vec<Event> {
        if self.terminal {
            return Vec::new();
        }
        self.terminal = true;
        let mut events = Vec::new();
        let calls: Vec<_> = std::mem::take(&mut self.tool_calls)
            .into_iter()
            .map(|(index, partial)| partial.finish(index))
            .collect();
        let mut assistant = serde_json::json!({ "role": "assistant" });
        assistant["content"] = if self.text.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(self.text.clone())
        };
        if !calls.is_empty() {
            assistant["tool_calls"] = serde_json::Value::Array(
                calls
                    .iter()
                    .map(|call| {
                        serde_json::json!({
                            "id": call.id.0,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            }
                        })
                    })
                    .collect(),
            );
        }
        self.state
            .lock()
            .expect("chat state")
            .messages
            .push(assistant);
        for call in calls.iter() {
            events.push(emitter.tool_call(
                call.clone(),
                Some(serde_json::json!({ "protocol": OWNER, "raw_type": "tool_calls" })),
            ));
        }
        let stop_reason = match self.finish_reason.as_deref() {
            Some("tool_calls") | Some("function_call") => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::ContentFilter,
            Some("stop") => {
                if calls.is_empty() {
                    StopReason::EndTurn
                } else {
                    StopReason::ToolUse
                }
            }
            None if !calls.is_empty() => StopReason::ToolUse,
            None => StopReason::EndTurn,
            Some(_) => StopReason::Other,
        };
        let mut diagnostic = serde_json::json!({
            "protocol": OWNER,
            "model": self.model,
            "finish_reason": self.finish_reason,
        });
        if !self.skipped.is_empty() {
            diagnostic["skipped"] = serde_json::json!(self.skipped);
        }
        events.push(emitter.completed(stop_reason, Some(diagnostic)));
        events
    }
}

impl Normalizer for ChatNormalizer {
    fn handle(&mut self, emitter: &EventEmitter, sse: &SseEvent) -> Vec<Event> {
        if self.terminal {
            return Vec::new();
        }
        if sse.is_done() {
            return self.finalize(emitter);
        }
        let Some(value) = sse.json() else {
            self.skipped.push(sse.data.chars().take(120).collect());
            return Vec::new();
        };
        if let Some(error) = stream_error(&value) {
            self.terminal = true;
            return vec![emitter.error(&error)];
        }
        let mut events = Vec::new();
        if let Some(usage) = value.get("usage").filter(|usage| usage.is_object()) {
            events.push(
                emitter.usage(
                    Usage {
                        input_tokens: usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                        output_tokens: usage.get("completion_tokens").and_then(|v| v.as_u64()),
                        reasoning_tokens: usage
                            .pointer("/completion_tokens_details/reasoning_tokens")
                            .and_then(|v| v.as_u64()),
                        cached_input_tokens: usage
                            .pointer("/prompt_tokens_details/cached_tokens")
                            .and_then(|v| v.as_u64()),
                    },
                    Some(serde_json::json!({ "protocol": OWNER, "usage": usage })),
                ),
            );
        }
        let Some(choice) = value
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first())
        else {
            if value.get("usage").is_none() {
                self.skipped
                    .push(emitter.unknown(OWNER, sse)["data"].to_string());
            }
            return events;
        };
        if let Some(delta) = choice.get("delta") {
            if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    self.text.push_str(text);
                    events.push(emitter.text(text.to_owned(), None));
                }
            }
            let reasoning = delta
                .get("reasoning")
                .or_else(|| delta.get("reasoning_content"))
                .and_then(|v| v.as_str());
            if let Some(reasoning) = reasoning {
                if !reasoning.is_empty() {
                    events.push(emitter.reasoning(reasoning.to_owned(), None));
                }
            }
            if let Some(calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                for (position, fragment) in calls.iter().enumerate() {
                    let index = fragment
                        .get("index")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize)
                        .unwrap_or(position);
                    let partial = self.tool_calls.entry(index).or_default();
                    if let Some(id) = fragment.get("id").and_then(|v| v.as_str()) {
                        partial.id = Some(id.to_owned());
                    }
                    if let Some(function) = fragment.get("function") {
                        if let Some(name) = function.get("name").and_then(|v| v.as_str()) {
                            partial.name = Some(name.to_owned());
                        }
                        if let Some(args) = function.get("arguments").and_then(|v| v.as_str()) {
                            partial.arguments.push_str(args);
                        }
                    }
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            self.finish_reason = Some(reason.to_owned());
        }
        events
    }

    fn finish(&mut self, emitter: &EventEmitter) -> Vec<Event> {
        self.finalize(emitter)
    }

    fn is_terminal(&self) -> bool {
        self.terminal
    }
}
