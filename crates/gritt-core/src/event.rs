//! The provider-neutral event model shared by native and connector sessions
//! (ADR-007).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::connector::ConnectorId;
use crate::error::ErrorKind;
use crate::session::SessionId;
use crate::tool::{ToolCall, ToolCallId, ToolResult};

/// Which path produced an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "path", rename_all = "snake_case")]
pub enum EventSource {
    Native,
    Connector { id: ConnectorId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub session_id: SessionId,
    /// Monotonic within a session. A gap or reorder is a diagnostic
    /// warning, never a silent fix.
    pub sequence: u64,
    pub source: EventSource,
    pub timestamp: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: EventKind,
    /// Provider- or connector-specific metadata. Optional and opaque.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    TextDelta {
        text: String,
    },
    ReasoningSummary {
        text: String,
    },
    ToolCall {
        call: ToolCall,
    },
    ToolResult {
        result: ToolResult,
    },
    ApprovalRequested {
        request: ApprovalRequest,
    },
    ApprovalDecided {
        request_id: ApprovalId,
        decision: ApprovalDecision,
    },
    Usage {
        usage: Usage,
    },
    StatusChanged {
        status: SessionStatus,
    },
    Error {
        error_kind: ErrorKind,
        message: String,
    },
    Completed {
        stop_reason: StopReason,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalId(pub String);

/// What the user is asked to approve. The reason is one line; sensitive
/// argument values are not recorded unless content logging is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub id: ApprovalId,
    pub tool: String,
    pub resource: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<ToolCallId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Connecting,
    Streaming,
    WaitingForApproval,
    RunningTool,
    Cancelling,
    Finished,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    ContentFilter,
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolCall;

    #[test]
    fn event_round_trips_through_json() {
        let event = Event {
            session_id: SessionId("s1".into()),
            sequence: 7,
            source: EventSource::Connector {
                id: ConnectorId::Codex,
            },
            timestamp: DateTime::parse_from_rfc3339("2026-09-04T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            kind: EventKind::ToolCall {
                call: ToolCall {
                    id: ToolCallId("c1".into()),
                    name: "shell".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                },
            },
            diagnostic: Some(serde_json::json!({"raw_type": "function_call"})),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"kind\":\"tool_call\""));
        assert!(json.contains("\"path\":\"connector\""));
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn every_kind_serializes_with_a_kind_tag() {
        let kinds = [
            EventKind::TextDelta { text: "a".into() },
            EventKind::ReasoningSummary { text: "b".into() },
            EventKind::Usage {
                usage: Usage::default(),
            },
            EventKind::StatusChanged {
                status: SessionStatus::Streaming,
            },
            EventKind::Error {
                error_kind: ErrorKind::Provider,
                message: "x".into(),
            },
            EventKind::Completed {
                stop_reason: StopReason::EndTurn,
            },
            EventKind::Cancelled,
        ];
        for kind in kinds {
            let value = serde_json::to_value(&kind).unwrap();
            assert!(value.get("kind").is_some(), "{value}");
        }
    }
}
