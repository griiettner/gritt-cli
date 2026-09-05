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
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Planning,
    Coding,
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
        };
        assert_eq!(connector.effort(), None);
        let text = serde_json::to_string(&connector).unwrap();
        assert!(!text.contains("effort"));
    }
}
