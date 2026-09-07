//! One module per external agent: how it is launched and how its output
//! is normalized. Each records the CLI version and flags it targets.

pub mod claude;
pub mod codex;
pub mod cursor;
pub mod opencode;

use gritt_core::event::EventKind;
use gritt_core::tool::{ToolCall, ToolCallId, ToolResult};

/// Why a connector model listing could not be turned into a catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelParseError {
    /// The CLI has no documented listing command.
    Unsupported,
    /// The output was not the documented catalog shape.
    Malformed,
}

/// Why a connector MCP listing could not be turned into an inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpParseError {
    /// The CLI has no documented machine-readable listing.
    Unsupported,
    /// The output was not the documented inventory shape.
    Malformed,
}

/// The transport word for an MCP server whose listing only shows its
/// launch command or URL: `http` for a URL, `stdio` for a command. A CLI
/// that names the transport itself (Codex) keeps its own word.
pub(crate) fn transport_from_target(target: &str) -> &'static str {
    if target.contains("://") {
        "http"
    } else {
        "stdio"
    }
}

/// The documented `--model` flag pair, or nothing when the user left the
/// CLI default in place. The identifier is passed as a separate argument,
/// never interpolated into a shell string.
pub(crate) fn model_flag(model: Option<&str>) -> Vec<String> {
    match model {
        Some(id) if !id.is_empty() => vec!["--model".to_owned(), id.to_owned()],
        _ => Vec::new(),
    }
}

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
