//! External agent connector contract (ADR-010). The native path implements
//! the same contract so the control plane never special-cases it.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::event::{ApprovalDecision, ApprovalId};
use crate::provider::EventStream;
use crate::session::{BoxFuture, ContinuationState, SessionId};
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
pub struct ConnectorModel {
    pub id: String,
    /// Human label from the CLI when it reports one. Absent means the
    /// interface should show `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorModelFreshness {
    Current,
    Stale,
}

/// A connector's model catalog after discovery. `source` names the
/// documented command or interface that produced it, never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorModelCatalog {
    pub connector: ConnectorId,
    pub models: Vec<ConnectorModel>,
    pub source: String,
    pub fetched_at: DateTime<Utc>,
    pub freshness: ConnectorModelFreshness,
}

/// Typed result of asking a connector for its current models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ConnectorModelDiscovery {
    Current {
        catalog: ConnectorModelCatalog,
    },
    CachedStale {
        catalog: ConnectorModelCatalog,
        reason: String,
    },
    Unavailable {
        connector: ConnectorId,
        reason: String,
    },
    Unsupported {
        connector: ConnectorId,
        reason: String,
    },
    CommandFailure {
        connector: ConnectorId,
        reason: String,
    },
    MalformedOutput {
        connector: ConnectorId,
        reason: String,
    },
}

impl ConnectorModelDiscovery {
    pub fn catalog(&self) -> Option<&ConnectorModelCatalog> {
        match self {
            Self::Current { catalog } | Self::CachedStale { catalog, .. } => Some(catalog),
            _ => None,
        }
    }

    pub fn connector(&self) -> ConnectorId {
        match self {
            Self::Current { catalog } | Self::CachedStale { catalog, .. } => catalog.connector,
            Self::Unavailable { connector, .. }
            | Self::Unsupported { connector, .. }
            | Self::CommandFailure { connector, .. }
            | Self::MalformedOutput { connector, .. } => *connector,
        }
    }

    /// One line for print, REPL, and TUI diagnostics. Names the CLI and
    /// the catalog source. Never includes a key, a prompt, or tool output.
    pub fn describe(&self) -> String {
        match self {
            Self::Current { catalog } => format!(
                "{} models from {} (fetched {})",
                catalog.connector.as_str(),
                catalog.source,
                catalog.fetched_at.to_rfc3339()
            ),
            Self::CachedStale { catalog, reason } => format!(
                "{} models from {} are stale (cached {}); {reason}",
                catalog.connector.as_str(),
                catalog.source,
                catalog.fetched_at.to_rfc3339()
            ),
            Self::Unavailable { connector, reason } => {
                format!("{} is unavailable: {reason}", connector.as_str())
            }
            Self::Unsupported { connector, reason } => {
                format!("{} does not list models: {reason}", connector.as_str())
            }
            Self::CommandFailure { connector, reason } => {
                format!("{} model listing failed: {reason}", connector.as_str())
            }
            Self::MalformedOutput { connector, reason } => {
                format!(
                    "{} model listing was unreadable: {reason}",
                    connector.as_str()
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub session_id: SessionId,
    pub prompt: String,
    pub workspace: PathBuf,
    /// State a previous turn left behind the session interface, so a
    /// connector can pick its external thread back up after a restart.
    /// Opaque to everything above the connector that wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ContinuationState>,
    /// Explicit model for a new connector session. Absent means the
    /// external CLI's own default. Never guessed from a display label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub trait Connector: Send + Sync {
    fn id(&self) -> ConnectorId;
    fn info(&self) -> BoxFuture<'_, Result<ConnectorInfo>>;
    /// Discovers the models this connector currently exposes. Default is
    /// [`ConnectorModelDiscovery::Unsupported`]. Cache, freshness, and
    /// stale fallback belong to the implementation that talks to the CLI.
    fn discover_models(&self, _refresh: bool) -> BoxFuture<'_, ConnectorModelDiscovery> {
        let connector = self.id();
        Box::pin(async move {
            ConnectorModelDiscovery::Unsupported {
                connector,
                reason: format!(
                    "{} does not document a model listing command",
                    connector.as_str()
                ),
            }
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_outcomes_round_trip_and_never_claim_stale_is_current() {
        let catalog = ConnectorModelCatalog {
            connector: ConnectorId::Codex,
            models: vec![ConnectorModel {
                id: "gpt-5.4".into(),
                display_label: Some("GPT-5.4".into()),
            }],
            source: "codex debug models".into(),
            fetched_at: DateTime::parse_from_rfc3339("2026-09-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            freshness: ConnectorModelFreshness::Stale,
        };
        let stale = ConnectorModelDiscovery::CachedStale {
            catalog: catalog.clone(),
            reason: "codex debug models exited 1".into(),
        };
        let text = serde_json::to_string(&stale).unwrap();
        assert!(text.contains("cached_stale"));
        assert!(!text.contains("\"current\""));
        assert_eq!(
            stale.catalog().unwrap().freshness,
            ConnectorModelFreshness::Stale
        );
        assert!(stale.describe().contains("codex"));
        assert!(stale.describe().contains("stale"));
        let back: ConnectorModelDiscovery = serde_json::from_str(&text).unwrap();
        assert_eq!(back, stale);
    }

    #[test]
    fn task_request_without_model_still_loads() {
        let raw = serde_json::json!({
            "session_id": "s1",
            "prompt": "hi",
            "workspace": "/tmp"
        });
        let request: TaskRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(request.model, None);
    }
}
