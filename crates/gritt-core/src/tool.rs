//! Tool definitions, calls, and results shared by every adapter and the
//! harness. Per-provider schema quirks stay inside adapters.

use serde::{Deserialize, Serialize};

/// Names of the first-version native tools.
pub mod native {
    pub const FILE_READ: &str = "file_read";
    pub const FILE_WRITE: &str = "file_write";
    pub const SHELL: &str = "shell";
    pub const ALL: [&str; 3] = [FILE_READ, FILE_WRITE, SHELL];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON schema for the arguments object.
    pub parameters: serde_json::Value,
}

/// Identifies one tool invocation across a call and its result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: ToolCallId,
    pub name: String,
    pub is_error: bool,
    /// Text returned to the model. Content logging decides whether it is
    /// persisted; the store never records it in telemetry.
    pub output: String,
}
