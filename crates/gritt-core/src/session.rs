//! Sessions are named, listable, resumable, and removable, and belong to
//! Gritt whichever path produced them (ADR-007).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::connector::ConnectorId;
use crate::event::Event;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionKind {
    Native {
        provider_profile: String,
        model: String,
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
