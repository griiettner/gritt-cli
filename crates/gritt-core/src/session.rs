//! Sessions are named, listable, resumable, and removable, and belong to
//! Gritt whichever path produced them (ADR-007).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::connector::ConnectorId;
use crate::event::Event;
use crate::provider::ReasoningEffort;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionKind {
    /// Pinned to a profile and model for its whole life. Effort is the one
    /// native setting that changes between turns; it defaults to `Auto` for
    /// sessions stored before the field existed.
    Native {
        provider_profile: String,
        model: String,
        #[serde(default)]
        effort: ReasoningEffort,
    },
    Connector {
        id: ConnectorId,
        /// Explicit model chosen before this session started. Absent on
        /// older rows and when the user left the CLI default in place.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Planning,
    Coding,
}

/// Native tool authority for the current run. Elevated modes are never
/// restored from session history; resuming requires a fresh selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    #[default]
    Planning,
    Supervised,
    AutoApprove,
    FullAccess,
}

impl ExecutionMode {
    pub const ALL: [Self; 4] = [
        Self::Planning,
        Self::Supervised,
        Self::AutoApprove,
        Self::FullAccess,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Supervised => "supervised",
            Self::AutoApprove => "auto-approve",
            Self::FullAccess => "full-access",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Planning => "Planning",
            Self::Supervised => "Supervised",
            Self::AutoApprove => "Auto Approve",
            Self::FullAccess => "Full Access",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Planning => "Read workspace files and plan. Writes, shell, and MCP calls are disabled.",
            Self::Supervised => "Follow tool policy and ask before actions that require approval.",
            Self::AutoApprove => "Approve permission prompts automatically. Policy denials and file boundaries still apply.",
            Self::FullAccess => "Run tools without policy prompts or denials, including files outside the workspace. OS permissions still apply.",
        }
    }

    pub fn phase(self) -> Phase {
        if self == Self::Planning {
            Phase::Planning
        } else {
            Phase::Coding
        }
    }
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ExecutionMode {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .ok_or_else(|| "choose planning, supervised, auto-approve, or full-access".to_owned())
    }
}

impl SessionKind {
    /// The native effort, `None` for a connector session (managed by the
    /// external agent).
    pub fn effort(&self) -> Option<ReasoningEffort> {
        match self {
            SessionKind::Native { effort, .. } => Some(*effort),
            SessionKind::Connector { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub kind: SessionKind,
    pub phase: Phase,
    pub workspace: PathBuf,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Reserved for child sessions. Not populated before Phase 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<SessionId>,
}

/// The native choices of the last successful new session, remembered for
/// later new sessions. Never a credential: a profile name, a model id, and
/// an effort level, nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastUsedNative {
    pub provider_profile: String,
    pub model: String,
    #[serde(default)]
    pub effort: ReasoningEffort,
}

/// Whatever an adapter or connector needs to continue a session. Opaque to
/// everything above the session interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationState {
    /// Identifies which adapter or connector wrote the state.
    pub owner: String,
    pub state: serde_json::Value,
}

/// A boxed future that does not require a specific runtime.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Persistence contract for sessions and their events.
pub trait SessionStore: Send + Sync {
    fn create(&self, session: Session) -> BoxFuture<'_, Result<()>>;
    fn get(&self, id: &SessionId) -> BoxFuture<'_, Result<Option<Session>>>;
    fn list(&self) -> BoxFuture<'_, Result<Vec<Session>>>;
    fn rename(&self, id: &SessionId, name: String) -> BoxFuture<'_, Result<()>>;
    fn remove(&self, id: &SessionId) -> BoxFuture<'_, Result<()>>;
    fn append_events(&self, events: Vec<Event>) -> BoxFuture<'_, Result<()>>;
    fn read_events(&self, id: &SessionId) -> BoxFuture<'_, Result<Vec<Event>>>;
    fn save_continuation(
        &self,
        id: &SessionId,
        state: ContinuationState,
    ) -> BoxFuture<'_, Result<()>>;
    fn load_continuation(&self, id: &SessionId)
        -> BoxFuture<'_, Result<Option<ContinuationState>>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connector::ConnectorId;

    #[test]
    fn native_sessions_stored_without_effort_load_as_auto() {
        let old = serde_json::json!({
            "kind": "native", "provider_profile": "openrouter", "model": "openai/gpt-5-nano"
        });
        let kind: SessionKind = serde_json::from_value(old).unwrap();
        assert_eq!(
            kind,
            SessionKind::Native {
                provider_profile: "openrouter".into(),
                model: "openai/gpt-5-nano".into(),
                effort: ReasoningEffort::Auto,
            }
        );
        assert_eq!(kind.effort(), Some(ReasoningEffort::Auto));
        let json = serde_json::to_value(&kind).unwrap();
        assert_eq!(json["effort"], "auto");
        assert_eq!(serde_json::from_value::<SessionKind>(json).unwrap(), kind);
    }

    #[test]
    fn explicit_effort_round_trips_and_connectors_have_none() {
        let native = SessionKind::Native {
            provider_profile: "openai".into(),
            model: "gpt-5-nano".into(),
            effort: ReasoningEffort::High,
        };
        let text = serde_json::to_string(&native).unwrap();
        assert!(text.contains("\"effort\":\"high\""));
        assert_eq!(serde_json::from_str::<SessionKind>(&text).unwrap(), native);
        let connector = SessionKind::Connector {
            id: ConnectorId::Codex,
            model: None,
        };
        assert_eq!(connector.effort(), None);
        let text = serde_json::to_string(&connector).unwrap();
        assert!(!text.contains("effort"));
        assert!(!text.contains("model"));
        let old = serde_json::json!({ "kind": "connector", "id": "codex" });
        let loaded: SessionKind = serde_json::from_value(old).unwrap();
        assert_eq!(
            loaded,
            SessionKind::Connector {
                id: ConnectorId::Codex,
                model: None,
            }
        );
        let chosen = SessionKind::Connector {
            id: ConnectorId::ClaudeCode,
            model: Some("sonnet".into()),
        };
        let text = serde_json::to_string(&chosen).unwrap();
        assert!(text.contains("\"model\":\"sonnet\""));
        assert_eq!(serde_json::from_str::<SessionKind>(&text).unwrap(), chosen);
    }
}
