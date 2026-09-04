//! One module per external agent: how it is launched and how its output
//! is normalized. Each records the CLI version and flags it targets.

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;

use gritt_core::event::EventKind;
use gritt_core::tool::{ToolCall, ToolCallId, ToolResult};

pub(crate) fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| v.as_str()).map(str::to_owned)
}

pub(crate) fn number(value: &serde_json::Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|v| v.as_u64())
}

pub(crate) fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> EventKind {
    EventKind::ToolCall {
        call: ToolCall {
            id: ToolCallId(id.to_owned()),
            name: name.to_owned(),
            arguments,
        },
    }
}

pub(crate) fn tool_result(id: &str, name: &str, output: String, is_error: bool) -> EventKind {
    EventKind::ToolResult {
        result: ToolResult {
            call_id: ToolCallId(id.to_owned()),
            name: name.to_owned(),
            is_error,
            output,
        },
    }
}

/// A value rendered as tool output: strings as they are, anything else as
/// compact JSON.
pub(crate) fn render(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
