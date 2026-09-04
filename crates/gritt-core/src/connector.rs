//! External agent connector contract (ADR-010). The native path implements
//! the same contract so the control plane never special-cases it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::event::{ApprovalDecision, ApprovalId};
use crate::provider::EventStream;
use crate::session::{BoxFuture, SessionId};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorId {
    Native,
    Codex,
    ClaudeCode,
    Cursor,
    OpenCode,
}

impl ConnectorId {
    /// The evaluation and launch order recorded in ADR-010.
    pub const ORDER: [ConnectorId; 5] = [
        ConnectorId::Native,
        ConnectorId::Codex,
        ConnectorId::ClaudeCode,
        ConnectorId::Cursor,
        ConnectorId::OpenCode,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ConnectorId::Native => "native",
            ConnectorId::Codex => "codex",
            ConnectorId::ClaudeCode => "claude_code",
            ConnectorId::Cursor => "cursor",
            ConnectorId::OpenCode => "opencode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    InProcess,
    MachineReadable,
    Pty,
    TerminalScrape,
}

/// Shown, never faked. A missing capability is displayed as such.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCapabilities {
    pub follow_up_input: bool,
    pub approvals: bool,
    pub cancel: bool,
    pub resume: bool,
    pub inspect: bool,
    pub structured_events: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AuthState {
    Authenticated,
    Unauthenticated,
    Unknown,
    NotInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorInfo {
    pub id: ConnectorId,
    pub version: Option<String>,
    pub transport: Transport,
    pub capabilities: ConnectorCapabilities,
    pub auth: AuthState,
}

/// Coarse task state a connector reports through [`Connector::inspect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Idle,
    Running,
    AwaitingApproval,
    AwaitingInput,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

/// Provider-neutral snapshot of a connector session. Raw connector detail
/// travels only in `diagnostic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorInspection {
    pub session_id: SessionId,
    /// The external agent's own task or thread identifier, when it has one.
    pub external_id: Option<String>,
    pub state: TaskState,
    pub version: Option<String>,
    pub auth: AuthState,
    pub capabilities: ConnectorCapabilities,
    pub diagnostic: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub session_id: SessionId,
    pub prompt: String,
    pub workspace: PathBuf,
}

pub trait Connector: Send + Sync {
    fn id(&self) -> ConnectorId;
    fn info(&self) -> BoxFuture<'_, Result<ConnectorInfo>>;
    fn start(&self, request: TaskRequest) -> BoxFuture<'_, Result<EventStream<'_>>>;
    fn send_input(&self, session_id: &SessionId, input: String) -> BoxFuture<'_, Result<()>>;
    fn answer_approval(
        &self,
        session_id: &SessionId,
        request_id: ApprovalId,
        decision: ApprovalDecision,
    ) -> BoxFuture<'_, Result<()>>;
    fn cancel(&self, session_id: &SessionId) -> BoxFuture<'_, Result<()>>;
    /// Resumes a session when the connector supports it; otherwise returns a
    /// connector error naming the missing capability.
    fn resume(&self, session_id: &SessionId) -> BoxFuture<'_, Result<EventStream<'_>>>;
    /// Reports the current state of a session when
    /// [`ConnectorCapabilities::inspect`] is set; otherwise returns a
    /// connector error naming the missing capability.
    fn inspect(&self, session_id: &SessionId) -> BoxFuture<'_, Result<ConnectorInspection>>;
}
