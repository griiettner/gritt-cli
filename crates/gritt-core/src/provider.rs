//! Provider profile, model list, and adapter contracts (ADR-007, ADR-008).

use std::collections::BTreeMap;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::event::Event;
use crate::secret::SecretRef;
use crate::session::{BoxFuture, ContinuationState};
use crate::tool::{ToolDefinition, ToolResult};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// OpenAI-compatible Chat Completions. Serves OpenRouter, OpenAI in Chat
    /// Completions mode, and any generic endpoint.
    ChatCompletions,
    /// OpenAI Responses with `previous_response_id` continuation.
    Responses,
    /// Anthropic Messages.
    Messages,
}

/// A configured endpoint. Routing is by profile, never by model name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub key: SecretRef,
    /// Alias to model id, scoped to this profile.
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub context_length: Option<u64>,
    pub tools: Option<bool>,
    pub vision: Option<bool>,
    pub structured_output: Option<bool>,
    pub reasoning: Option<bool>,
    /// Price per million input tokens, when reported.
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
    /// Provider-declared replacement for a deprecated model, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
    #[serde(default)]
    pub deprecated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelListStatus {
    Fresh {
        fetched_at: DateTime<Utc>,
    },
    /// The refresh failed and the last cached list is in use.
    Stale {
        fetched_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelList {
    pub profile: String,
    pub status: ModelListStatus,
    pub models: Vec<ModelInfo>,
}

/// One turn of input to a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub options: RequestOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOptions {
    pub max_tokens: Option<u32>,
    pub reasoning: Option<bool>,
    pub structured_output: Option<serde_json::Value>,
}

pub type EventStream<'a> = Pin<Box<dyn Stream<Item = Result<Event>> + Send + 'a>>;

/// The one trait every wire protocol implements. Nothing above it learns
/// which provider served a request.
pub trait ProviderAdapter: Send + Sync {
    fn protocol(&self) -> Protocol;
    /// Sends a prompt and streams provider-neutral events.
    fn send(&self, request: PromptRequest) -> BoxFuture<'_, Result<EventStream<'_>>>;
    /// Submits tool results for outstanding tool calls and streams the
    /// continuation.
    fn submit_tool_results(
        &self,
        results: Vec<ToolResult>,
    ) -> BoxFuture<'_, Result<EventStream<'_>>>;
    /// Restores adapter state from stored continuation data.
    fn restore(&self, state: ContinuationState) -> BoxFuture<'_, Result<()>>;
    /// Exports the state needed to continue later.
    fn continuation(&self) -> BoxFuture<'_, Result<Option<ContinuationState>>>;
    fn capabilities(&self, model: &str) -> BoxFuture<'_, Result<ModelCapabilities>>;
}
