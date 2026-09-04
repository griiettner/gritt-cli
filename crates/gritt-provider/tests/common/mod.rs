//! Shared helpers for the provider contract tests. Each test binary uses a
//! subset, so unused-item lints are silenced here.
#![allow(dead_code)]

use std::sync::Arc;

use futures::StreamExt;
use gritt_core::event::{Event, EventKind};
use gritt_core::provider::{
    EventStream, Message, ModelCapabilities, PromptRequest, Protocol, ProviderProfile, Role,
};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::SessionId;
use gritt_core::tool::ToolDefinition;
use gritt_provider::adapter::{CapabilitySource, StaticKey};
use gritt_provider::{
    AdapterContext, CancellationToken, FixtureResponse, FixtureTransport, NoCapabilities,
};

pub const TEST_KEY: &str = "test-key-value-never-printed";

pub fn fixture(protocol: &str, name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/{protocol}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

pub fn profile(protocol: Protocol) -> ProviderProfile {
    let (name, base_url, var) = match protocol {
        Protocol::ChatCompletions => (
            "openrouter",
            "https://openrouter.ai/api/v1",
            "OPENROUTER_API_KEY",
        ),
        Protocol::Responses => ("openai", "https://api.openai.com/v1", "OPENAI_API_KEY"),
        Protocol::Messages => (
            "anthropic",
            "https://api.anthropic.com",
            "ANTHROPIC_API_KEY",
        ),
    };
    ProviderProfile {
        name: name.into(),
        protocol,
        base_url: base_url.into(),
        key: SecretRef::for_profile(name, var),
        aliases: Default::default(),
    }
}

pub fn protocol_dir(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::ChatCompletions => "chat-completions",
        Protocol::Responses => "responses",
        Protocol::Messages => "messages",
    }
}

pub fn model_for(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::ChatCompletions => "openai/gpt-5-nano",
        Protocol::Responses => "gpt-5-nano",
        Protocol::Messages => "claude-sonnet-5",
    }
}

pub struct FixedCapabilities(pub ModelCapabilities);

impl CapabilitySource for FixedCapabilities {
    fn capabilities(&self, _profile: &str, _model: &str) -> Option<ModelCapabilities> {
        Some(self.0.clone())
    }
}

pub fn make_context_with(
    protocol: Protocol,
    responses: Vec<FixtureResponse>,
    chunk_size: usize,
    capabilities: Arc<dyn CapabilitySource>,
) -> (AdapterContext, Arc<FixtureTransport>, CancellationToken) {
    let transport = Arc::new(FixtureTransport::new(responses, chunk_size));
    let cancel = CancellationToken::new();
    let context = AdapterContext {
        profile: profile(protocol),
        session_id: SessionId("session-test".into()),
        transport: transport.clone(),
        keys: Arc::new(StaticKey(Secret::new(TEST_KEY))),
        capabilities,
        cancel: cancel.clone(),
    };
    (context, transport, cancel)
}

pub fn make_context(
    protocol: Protocol,
    responses: Vec<FixtureResponse>,
    chunk_size: usize,
) -> (AdapterContext, Arc<FixtureTransport>, CancellationToken) {
    make_context_with(protocol, responses, chunk_size, Arc::new(NoCapabilities))
}

pub fn prompt(protocol: Protocol, tools: bool) -> PromptRequest {
    PromptRequest {
        model: model_for(protocol).into(),
        messages: vec![
            Message {
                role: Role::System,
                content: "You are terse.".into(),
            },
            Message {
                role: Role::User,
                content: "Say hello".into(),
            },
        ],
        tools: if tools {
            vec![ToolDefinition {
                name: "file_read".into(),
                description: "Read a file in the workspace".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }),
            }]
        } else {
            Vec::new()
        },
        options: Default::default(),
    }
}

pub async fn collect(stream: EventStream<'_>) -> Vec<Event> {
    stream.map(|event| event.expect("event")).collect().await
}

pub fn kinds(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .map(|event| match &event.kind {
            EventKind::TextDelta { .. } => "text".to_string(),
            EventKind::ReasoningSummary { .. } => "reasoning".to_string(),
            EventKind::ToolCall { .. } => "tool_call".to_string(),
            EventKind::ToolResult { .. } => "tool_result".to_string(),
            EventKind::ApprovalRequested { .. } => "approval_requested".to_string(),
            EventKind::ApprovalDecided { .. } => "approval_decided".to_string(),
            EventKind::Usage { .. } => "usage".to_string(),
            EventKind::StatusChanged { .. } => "status".to_string(),
            EventKind::Error { .. } => "error".to_string(),
            EventKind::Completed { stop_reason } => format!("completed:{stop_reason:?}"),
            EventKind::Cancelled => "cancelled".to_string(),
        })
        .collect()
}

pub fn text_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub fn reasoning_of(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ReasoningSummary { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub fn assert_monotonic(events: &[Event]) {
    for pair in events.windows(2) {
        assert!(
            pair[1].sequence > pair[0].sequence,
            "sequence must increase"
        );
    }
}
