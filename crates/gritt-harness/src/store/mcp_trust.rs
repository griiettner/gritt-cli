//! Persistence for MCP first-use decisions.
//!
//! The runtime owns the trust rule; this is only the store behind it, using
//! the same embedded database as sessions and events. No definition detail
//! is written: the row holds the workspace, the name, the fingerprint, and
//! the decision, so an environment or header value can never land here.

use std::sync::Arc;

use chrono::Utc;
use gritt_core::mcp::{TrustDecision, TrustRecord};
use gritt_core::session::BoxFuture;
use gritt_core::Result;

use super::{storage_error, Store};
use crate::mcp::trust::TrustStore;

impl Store {
    /// The recorded decision for one exact workspace, server, and
    /// fingerprint.
    pub async fn mcp_trust(
        &self,
        workspace: &str,
        server: &str,
        fingerprint: &str,
    ) -> Result<Option<TrustDecision>> {
        let mut rows = self
            .connection()
            .query(
                "SELECT decision FROM gritt_mcp_trust \
                 WHERE workspace = ?1 AND server = ?2 AND fingerprint = ?3",
                turso::params![
                    workspace.to_owned(),
                    server.to_owned(),
                    fingerprint.to_owned()
                ],
            )
            .await
            .map_err(storage_error)?;
        let Some(row) = rows.next().await.map_err(storage_error)? else {
            return Ok(None);
        };
        let decision: String = row.get(0).map_err(storage_error)?;
        Ok(match decision.as_str() {
            "approved" => Some(TrustDecision::Approved),
            "denied" => Some(TrustDecision::Denied),
            // An unknown value is treated as never asked, which is the safe
            // direction: it asks again rather than assuming approval.
            _ => None,
        })
    }

    /// Writes or replaces one decision.
    pub async fn set_mcp_trust(&self, record: &TrustRecord) -> Result<()> {
        let decision = match record.decision {
            TrustDecision::Approved => "approved",
            TrustDecision::Denied => "denied",
        };
        self.connection()
            .execute(
                "INSERT INTO gritt_mcp_trust \
                   (workspace, server, fingerprint, decision, decided_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(workspace, server, fingerprint) DO UPDATE SET \
                   decision = excluded.decision, decided_at = excluded.decided_at",
                turso::params![
                    record.workspace.clone(),
                    record.server.clone(),
                    record.fingerprint.clone(),
                    decision,
                    Utc::now().to_rfc3339()
                ],
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Forgets every decision for a workspace, for a "review trust again"
    /// action.
    pub async fn clear_mcp_trust(&self, workspace: &str) -> Result<()> {
        self.connection()
            .execute(
                "DELETE FROM gritt_mcp_trust WHERE workspace = ?1",
                turso::params![workspace.to_owned()],
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }
}

/// Adapts the store to the runtime's trust seam.
pub struct StoreTrustStore {
    store: Arc<Store>,
}

impl StoreTrustStore {
    pub fn new(store: Arc<Store>) -> Arc<Self> {
        Arc::new(Self { store })
    }
}

impl TrustStore for StoreTrustStore {
    fn decision<'a>(
        &'a self,
        workspace: &'a str,
        server: &'a str,
        fingerprint: &'a str,
    ) -> BoxFuture<'a, Result<Option<TrustDecision>>> {
        Box::pin(async move { self.store.mcp_trust(workspace, server, fingerprint).await })
    }

    fn record<'a>(&'a self, record: TrustRecord) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { self.store.set_mcp_trust(&record).await })
    }
}
