//! Local, content-free telemetry and analytics (ADR-008). Records carry
//! names, ids, durations, counts, and statuses. Prompt text, file content,
//! shell output, and key material have no field to land in. Content
//! logging is a separate opt-in table with a retention purge.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use gritt_core::config::LoggingConfig;
use gritt_core::session::SessionId;
use gritt_core::telemetry::{AnalyticsRecord, TelemetryEvent};
use gritt_core::{Error, Result};

use crate::store::{storage_error, Store};

pub struct Telemetry {
    store: Arc<Store>,
    logging: LoggingConfig,
}

impl Telemetry {
    pub fn new(store: Arc<Store>, logging: LoggingConfig) -> Self {
        Self { store, logging }
    }

    pub fn content_logging(&self) -> bool {
        self.logging.content_logging
    }

    pub async fn record(&self, event: TelemetryEvent) -> Result<()> {
        let counters = serde_json::to_string(&event.counters)
            .map_err(|error| Error::storage(error.to_string()))?;
        self.store
            .connection()
            .execute(
                "INSERT INTO gritt_telemetry_events (name, session_id, timestamp, duration_ms, status, counters)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                turso::params![
                    event.name,
                    event.session_id.map(|id| id.0),
                    event.timestamp.to_rfc3339(),
                    event.duration_ms.map(|value| i64::try_from(value).unwrap_or(i64::MAX)),
                    event.status,
                    counters
                ],
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    pub async fn record_metric(&self, record: AnalyticsRecord) -> Result<()> {
        let labels = serde_json::to_string(&record.labels)
            .map_err(|error| Error::storage(error.to_string()))?;
        self.store
            .connection()
            .execute(
                "INSERT INTO gritt_analytics_records (metric, session_id, timestamp, value, labels)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                turso::params![
                    record.metric,
                    record.session_id.map(|id| id.0),
                    record.timestamp.to_rfc3339(),
                    i64::try_from(record.value).unwrap_or(i64::MAX),
                    labels
                ],
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Convenience for the agent loop: one turn's outcome.
    pub async fn turn(
        &self,
        session: &SessionId,
        started: DateTime<Utc>,
        status: &str,
        counters: BTreeMap<String, u64>,
        labels: BTreeMap<String, String>,
    ) -> Result<()> {
        let now = Utc::now();
        let duration = now.signed_duration_since(started).num_milliseconds();
        let tokens = counters.get("input_tokens").copied().unwrap_or(0)
            + counters.get("output_tokens").copied().unwrap_or(0);
        self.record(TelemetryEvent {
            name: "turn".into(),
            session_id: Some(session.clone()),
            timestamp: now,
            duration_ms: u64::try_from(duration).ok(),
            status: Some(status.to_owned()),
            counters,
        })
        .await?;
        self.record_metric(AnalyticsRecord {
            metric: "tokens_total".into(),
            session_id: Some(session.clone()),
            timestamp: now,
            value: tokens,
            labels,
        })
        .await
    }

    /// Writes a content row only when content logging is on.
    pub async fn content(&self, session: &SessionId, role: &str, content: &str) -> Result<()> {
        if !self.logging.content_logging {
            return Ok(());
        }
        self.store
            .connection()
            .execute(
                "INSERT INTO gritt_content_log (session_id, timestamp, role, content) VALUES (?1, ?2, ?3, ?4)",
                turso::params![session.0.clone(), Utc::now().to_rfc3339(), role, content],
            )
            .await
            .map_err(storage_error)?;
        Ok(())
    }

    /// Deletes content rows older than the retention window.
    pub async fn purge_content(&self, now: DateTime<Utc>) -> Result<u64> {
        let cutoff = now - Duration::days(i64::from(self.logging.content_retention_days));
        self.store
            .connection()
            .execute(
                "DELETE FROM gritt_content_log WHERE timestamp < ?1",
                turso::params![cutoff.to_rfc3339()],
            )
            .await
            .map_err(storage_error)
    }

    /// Every string stored in the telemetry and analytics tables, for the
    /// content-safety test.
    pub async fn dump_text(&self) -> Result<String> {
        let mut out = String::new();
        for sql in [
            "SELECT name || ' ' || COALESCE(session_id, '') || ' ' || COALESCE(status, '') || ' ' || counters FROM gritt_telemetry_events",
            "SELECT metric || ' ' || COALESCE(session_id, '') || ' ' || labels FROM gritt_analytics_records",
        ] {
            let mut rows = self
                .store
                .connection()
                .query(sql, ())
                .await
                .map_err(storage_error)?;
            while let Some(row) = rows.next().await.map_err(storage_error)? {
                let text: String = row.get(0).map_err(storage_error)?;
                out.push_str(&text);
                out.push('\n');
            }
        }
        Ok(out)
    }

    pub async fn content_rows(&self) -> Result<u64> {
        let mut rows = self
            .store
            .connection()
            .query("SELECT COUNT(*) FROM gritt_content_log", ())
            .await
            .map_err(storage_error)?;
        let row = rows
            .next()
            .await
            .map_err(storage_error)?
            .ok_or_else(|| Error::storage("count returned no row"))?;
        let count: i64 = row.get(0).map_err(storage_error)?;
        Ok(u64::try_from(count).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::DatabaseLocation;

    async fn telemetry(content_logging: bool) -> (tempfile::TempDir, Telemetry) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(DatabaseLocation::Explicit(dir.path().join("t.db")))
            .await
            .unwrap();
        let telemetry = Telemetry::new(
            Arc::new(store),
            LoggingConfig {
                content_logging,
                content_retention_days: 7,
            },
        );
        (dir, telemetry)
    }

    #[tokio::test]
    async fn content_is_not_logged_by_default_and_purges_when_on() {
        let (_dir, off) = telemetry(false).await;
        let session = SessionId("s".into());
        off.content(&session, "user", "secret prompt")
            .await
            .unwrap();
        assert_eq!(off.content_rows().await.unwrap(), 0);

        let (_dir, on) = telemetry(true).await;
        on.content(&session, "user", "secret prompt").await.unwrap();
        assert_eq!(on.content_rows().await.unwrap(), 1);
        let purged = on
            .purge_content(Utc::now() + Duration::days(8))
            .await
            .unwrap();
        assert_eq!(purged, 1);
        assert_eq!(on.content_rows().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn turn_records_are_content_free() {
        let (_dir, telemetry) = telemetry(false).await;
        let session = SessionId("s".into());
        let counters = BTreeMap::from([
            ("input_tokens".to_string(), 3),
            ("output_tokens".to_string(), 4),
        ]);
        let labels = BTreeMap::from([("profile".to_string(), "openrouter".to_string())]);
        telemetry
            .turn(&session, Utc::now(), "completed", counters, labels)
            .await
            .unwrap();
        let text = telemetry.dump_text().await.unwrap();
        assert!(text.contains("turn"));
        assert!(text.contains("tokens_total"));
        assert!(text.contains("openrouter"));
    }
}
