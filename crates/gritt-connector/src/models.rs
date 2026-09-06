//! Connector model catalog cache. Discovery commands and parsers live on
//! each protocol; this module stores the last good list and applies the
//! same freshness and stale-fallback rules as native model lists (ADR-008).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use gritt_core::config::ModelListPolicy;
use gritt_core::connector::{
    ConnectorId, ConnectorModel, ConnectorModelCatalog, ConnectorModelFreshness,
};
use gritt_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedConnectorModels {
    #[serde(default)]
    pub fetched_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_attempt_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub models: Vec<ConnectorModel>,
}

#[derive(Debug, Clone)]
pub struct ConnectorModelCache {
    dir: PathBuf,
}

impl ConnectorModelCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|dir| dir.join("gritt").join("connector-models"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, id: ConnectorId) -> PathBuf {
        self.dir.join(format!("{}.json", id.as_str()))
    }

    pub fn read(&self, id: ConnectorId) -> Result<Option<CachedConnectorModels>> {
        let path = self.path(id);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| Error::storage(format!("cannot read {}: {error}", path.display())))?;
        match serde_json::from_str(&text) {
            Ok(cached) => Ok(Some(cached)),
            Err(_) => Ok(None),
        }
    }

    pub fn write(&self, id: ConnectorId, cached: &CachedConnectorModels) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|error| {
            Error::storage(format!("cannot create {}: {error}", self.dir.display()))
        })?;
        let path = self.path(id);
        let text = serde_json::to_string_pretty(cached)
            .map_err(|error| Error::storage(error.to_string()))?;
        std::fs::write(&path, text)
            .map_err(|error| Error::storage(format!("cannot write {}: {error}", path.display())))
    }
}

pub fn interval(policy: &ModelListPolicy) -> Duration {
    Duration::seconds(i64::try_from(policy.refresh_interval_secs).unwrap_or(i64::MAX))
}

pub fn catalog_from_cache(
    id: ConnectorId,
    cached: &CachedConnectorModels,
    freshness: ConnectorModelFreshness,
) -> Option<ConnectorModelCatalog> {
    let fetched_at = cached.fetched_at?;
    Some(ConnectorModelCatalog {
        connector: id,
        models: cached.models.clone(),
        source: cached.source.clone(),
        fetched_at,
        freshness,
    })
}

pub fn cache_is_fresh(
    cached: &CachedConnectorModels,
    policy: &ModelListPolicy,
    now: DateTime<Utc>,
) -> bool {
    cached
        .fetched_at
        .is_some_and(|fetched_at| now.signed_duration_since(fetched_at) < interval(policy))
}

pub fn attempted_recently(
    cached: &CachedConnectorModels,
    policy: &ModelListPolicy,
    now: DateTime<Utc>,
) -> bool {
    cached
        .last_attempt_at
        .is_some_and(|attempt| now.signed_duration_since(attempt) < interval(policy))
}

/// CSI and similar escape sequences never belong in a model id.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\u{7}' {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_but_keeps_ids() {
        assert_eq!(
            strip_ansi("\u{1b}[32mgpt-5.4\u{1b}[0m (default)"),
            "gpt-5.4 (default)"
        );
        assert_eq!(strip_ansi("plain"), "plain");
    }
}
