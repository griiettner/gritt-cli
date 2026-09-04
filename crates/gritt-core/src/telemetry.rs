//! Content-free local telemetry and analytics records (ADR-008). These
//! types carry names, ids, counts, durations, and statuses. They have no
//! field for a prompt, file content, transcript, or secret.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::session::SessionId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub name: String,
    pub session_id: Option<SessionId>,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub status: Option<String>,
    /// Numeric counters such as token counts or tool call totals.
    #[serde(default)]
    pub counters: std::collections::BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsRecord {
    pub metric: String,
    pub session_id: Option<SessionId>,
    pub timestamp: DateTime<Utc>,
    pub value: u64,
    /// Low-cardinality labels such as provider profile, model, or connector.
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}
