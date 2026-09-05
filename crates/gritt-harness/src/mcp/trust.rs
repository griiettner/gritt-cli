//! First-use trust for MCP servers.
//!
//! Reading `.mcp.json` never authorizes running what it names. A server is
//! only launched once a [`TrustRecord`] approves the exact workspace and the
//! exact definition; editing the entry changes its fingerprint, so the old
//! approval no longer matches and the server returns to `awaiting approval`.
//!
//! The interactive prompt is interface work. This is the typed decision API
//! and the persistence seam behind it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gritt_core::mcp::{TrustDecision, TrustRecord};
use gritt_core::session::BoxFuture;
use gritt_core::Result;

/// Where first-use decisions live between runs.
pub trait TrustStore: Send + Sync {
    /// The recorded decision for this exact workspace, server, and
    /// fingerprint, or `None` when the user has never been asked.
    fn decision<'a>(
        &'a self,
        workspace: &'a str,
        server: &'a str,
        fingerprint: &'a str,
    ) -> BoxFuture<'a, Result<Option<TrustDecision>>>;

    fn record<'a>(&'a self, record: TrustRecord) -> BoxFuture<'a, Result<()>>;
}

/// The default store: decisions last for the run and nothing is written.
/// Used by tests and by any caller that has not wired persistence.
#[derive(Debug, Default)]
pub struct MemoryTrustStore {
    records: Mutex<HashMap<(String, String, String), TrustDecision>>,
}

impl MemoryTrustStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Approves everything, for tests that are not exercising trust.
    pub fn trust_all() -> Arc<AlwaysTrust> {
        Arc::new(AlwaysTrust)
    }
}

impl TrustStore for MemoryTrustStore {
    fn decision<'a>(
        &'a self,
        workspace: &'a str,
        server: &'a str,
        fingerprint: &'a str,
    ) -> BoxFuture<'a, Result<Option<TrustDecision>>> {
        let key = (
            workspace.to_owned(),
            server.to_owned(),
            fingerprint.to_owned(),
        );
        Box::pin(async move { Ok(self.records.lock().expect("mcp trust").get(&key).copied()) })
    }

    fn record<'a>(&'a self, record: TrustRecord) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.records.lock().expect("mcp trust").insert(
                (record.workspace, record.server, record.fingerprint),
                record.decision,
            );
            Ok(())
        })
    }
}

/// Approves every definition without asking. Only for tests and for a
/// caller that has already taken responsibility for the file.
#[derive(Debug, Default)]
pub struct AlwaysTrust;

impl TrustStore for AlwaysTrust {
    fn decision<'a>(
        &'a self,
        _workspace: &'a str,
        _server: &'a str,
        _fingerprint: &'a str,
    ) -> BoxFuture<'a, Result<Option<TrustDecision>>> {
        Box::pin(async { Ok(Some(TrustDecision::Approved)) })
    }

    fn record<'a>(&'a self, _record: TrustRecord) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_changed_definition_invalidates_the_approval() {
        let store = MemoryTrustStore::new();
        store
            .record(TrustRecord {
                workspace: "/ws".into(),
                server: "any".into(),
                fingerprint: "aaa".into(),
                decision: TrustDecision::Approved,
            })
            .await
            .unwrap();
        assert_eq!(
            store.decision("/ws", "any", "aaa").await.unwrap(),
            Some(TrustDecision::Approved)
        );
        // A different fingerprint, workspace, or name is a different key.
        assert_eq!(store.decision("/ws", "any", "bbb").await.unwrap(), None);
        assert_eq!(store.decision("/other", "any", "aaa").await.unwrap(), None);
        assert_eq!(store.decision("/ws", "renamed", "aaa").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_denial_is_remembered_as_a_denial() {
        let store = MemoryTrustStore::new();
        store
            .record(TrustRecord {
                workspace: "/ws".into(),
                server: "any".into(),
                fingerprint: "aaa".into(),
                decision: TrustDecision::Denied,
            })
            .await
            .unwrap();
        assert_eq!(
            store.decision("/ws", "any", "aaa").await.unwrap(),
            Some(TrustDecision::Denied)
        );
    }
}
