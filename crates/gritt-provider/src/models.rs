//! Model lists: per-profile fetch, on-disk cache with a fetch timestamp,
//! daily refresh, and stale fallback (ADR-008). Capabilities are recorded
//! exactly as the provider reports them; gaps stay `None`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Duration, Utc};
use gritt_core::config::ModelListPolicy;
use gritt_core::provider::{
    ModelCapabilities, ModelInfo, ModelList, ModelListStatus, Protocol, ProviderProfile,
};
use gritt_core::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};

use crate::adapter::{provider_error, CapabilitySource, KeyProvider};
use crate::transport::{HttpRequest, HttpTransport};

/// The on-disk shape: one file per profile. `last_attempt_at` records the
/// most recent refresh attempt, successful or not, so a failing provider is
/// retried at most once per interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModelList {
    pub fetched_at: DateTime<Utc>,
    #[serde(default)]
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone)]
pub struct ModelCache {
    dir: PathBuf,
}

impl ModelCache {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `<user cache dir>/gritt/models`.
    pub fn default_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|dir| dir.join("gritt").join("models"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `<dir>/<sanitized>-<fnv1a64 of the original name>.json`. The hash
    /// keeps distinct profiles such as `a/b` and `a_b` in distinct files.
    pub fn path(&self, profile: &str) -> PathBuf {
        let safe: String = profile
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.dir
            .join(format!("{safe}-{:016x}.json", fnv1a64(profile.as_bytes())))
    }

    pub fn read(&self, profile: &str) -> Result<Option<CachedModelList>> {
        let path = self.path(profile);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| Error::storage(format!("cannot read {}: {error}", path.display())))?;
        match serde_json::from_str(&text) {
            Ok(cached) => Ok(Some(cached)),
            // A damaged cache is treated as absent so a refresh repairs it.
            Err(_) => Ok(None),
        }
    }

    pub fn write(&self, profile: &str, cached: &CachedModelList) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(|error| {
            Error::storage(format!("cannot create {}: {error}", self.dir.display()))
        })?;
        let path = self.path(profile);
        let text = serde_json::to_string_pretty(cached)
            .map_err(|error| Error::storage(error.to_string()))?;
        std::fs::write(&path, text)
            .map_err(|error| Error::storage(format!("cannot write {}: {error}", path.display())))
    }
}

/// FNV-1a, 64 bit. Stable across platforms and builds; not a security hash.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Where a profile's list lives.
pub fn models_url(profile: &ProviderProfile) -> String {
    let base = profile.base_url.trim_end_matches('/');
    match profile.protocol {
        Protocol::ChatCompletions | Protocol::Responses => format!("{base}/models"),
        Protocol::Messages => format!("{base}/v1/models"),
    }
}

fn header_secret(
    profile: &ProviderProfile,
    key: gritt_core::secret::Secret,
) -> (&'static str, gritt_core::secret::Secret) {
    match profile.protocol {
        Protocol::Messages => ("x-api-key", key),
        _ => (
            "authorization",
            gritt_core::secret::Secret::new(format!("Bearer {}", key.expose())),
        ),
    }
}

/// Fetches the list from the provider without touching the cache.
pub async fn fetch_models(
    transport: &dyn HttpTransport,
    keys: &dyn KeyProvider,
    profile: &ProviderProfile,
) -> Result<Vec<ModelInfo>> {
    let key = keys.key(&profile.name, &profile.key)?;
    let (name, value) = header_secret(profile, key.clone());
    let mut request = HttpRequest::get(models_url(profile)).secret_header(name, value);
    if profile.protocol == Protocol::Messages {
        request = request.header("anthropic-version", crate::messages::ANTHROPIC_VERSION);
    }
    let response = transport.send(request).await?;
    let status = response.status;
    let body = response.bytes().await?;
    if !(200..300).contains(&status) {
        return Err(provider_error(status, &body, &[key]));
    }
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(|error| {
        Error::provider(Some(status), format!("invalid model list JSON: {error}"))
    })?;
    parse_model_list(profile.protocol, &value)
}

/// Loads the list, refreshing at most once per `policy.refresh_interval`.
/// A failed refresh returns the cached list marked stale when the policy
/// allows it, records the attempt so the provider is not retried again
/// until the interval passes, and no cache plus a failed refresh is
/// `MissingModelList`. `force_refresh` bypasses both timers.
pub async fn load_models(
    cache: &ModelCache,
    transport: &dyn HttpTransport,
    keys: &dyn KeyProvider,
    profile: &ProviderProfile,
    policy: &ModelListPolicy,
    now: DateTime<Utc>,
    force_refresh: bool,
) -> Result<ModelList> {
    let cached = cache.read(&profile.name)?;
    let interval =
        Duration::seconds(i64::try_from(policy.refresh_interval_secs).unwrap_or(i64::MAX));
    if let Some(cached) = &cached {
        if !force_refresh && now.signed_duration_since(cached.fetched_at) < interval {
            return Ok(ModelList {
                profile: profile.name.clone(),
                status: ModelListStatus::Fresh {
                    fetched_at: cached.fetched_at,
                },
                models: cached.models.clone(),
            });
        }
        let attempted_recently = cached
            .last_attempt_at
            .is_some_and(|attempt| now.signed_duration_since(attempt) < interval);
        if !force_refresh && attempted_recently && policy.stale_fallback {
            return Ok(ModelList {
                profile: profile.name.clone(),
                status: ModelListStatus::Stale {
                    fetched_at: cached.fetched_at,
                },
                models: cached.models.clone(),
            });
        }
    }
    match fetch_models(transport, keys, profile).await {
        Ok(models) => {
            let fresh = CachedModelList {
                fetched_at: now,
                last_attempt_at: Some(now),
                models,
            };
            cache.write(&profile.name, &fresh)?;
            Ok(ModelList {
                profile: profile.name.clone(),
                status: ModelListStatus::Fresh { fetched_at: now },
                models: fresh.models,
            })
        }
        Err(error) => match cached {
            Some(mut cached) if policy.stale_fallback => {
                cached.last_attempt_at = Some(now);
                cache.write(&profile.name, &cached)?;
                Ok(ModelList {
                    profile: profile.name.clone(),
                    status: ModelListStatus::Stale {
                        fetched_at: cached.fetched_at,
                    },
                    models: cached.models,
                })
            }
            Some(cached) => Err(Error::new(
                ErrorKind::StaleModelList,
                format!(
                    "model list refresh for `{}` failed and stale fallback is disabled (cached {})",
                    profile.name, cached.fetched_at
                ),
            )
            .with_diagnostic(
                serde_json::json!({ "cause": error.message, "detail": error.diagnostic }),
            )),
            None => Err(Error::new(
                ErrorKind::MissingModelList,
                format!(
                    "no cached model list for `{}` and the refresh failed: {}",
                    profile.name, error.message
                ),
            )
            .with_diagnostic(
                serde_json::json!({ "cause": error.message, "detail": error.diagnostic }),
            )),
        },
    }
}

/// Parses a provider model list. OpenAI-compatible lists are `{data: [...]}`
/// with optional OpenRouter extras; Anthropic lists are `{data: [...]}`
/// with `display_name`.
pub fn parse_model_list(protocol: Protocol, value: &serde_json::Value) -> Result<Vec<ModelInfo>> {
    let items = value
        .get("data")
        .and_then(|data| data.as_array())
        .ok_or_else(|| Error::provider(None, "model list has no `data` array"))?;
    Ok(items
        .iter()
        .filter_map(|item| parse_model(protocol, item))
        .collect())
}

fn parse_model(protocol: Protocol, item: &serde_json::Value) -> Option<ModelInfo> {
    let id = item.get("id")?.as_str()?.to_owned();
    let display_name = item
        .get("display_name")
        .or_else(|| item.get("name"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let mut capabilities = ModelCapabilities::default();
    if protocol != Protocol::Messages {
        capabilities.context_length = item.get("context_length").and_then(|v| v.as_u64());
        if let Some(params) = item.get("supported_parameters").and_then(|v| v.as_array()) {
            let has = |name: &str| params.iter().any(|p| p.as_str() == Some(name));
            capabilities.tools = Some(has("tools"));
            capabilities.structured_output =
                Some(has("response_format") || has("structured_outputs"));
            capabilities.reasoning = Some(has("reasoning") || has("include_reasoning"));
        }
        if let Some(modalities) = item
            .pointer("/architecture/input_modalities")
            .and_then(|v| v.as_array())
        {
            capabilities.vision = Some(modalities.iter().any(|m| m.as_str() == Some("image")));
        }
        let per_token = |field: &str| {
            item.pointer(&format!("/pricing/{field}"))
                .and_then(|v| {
                    v.as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .or_else(|| v.as_f64())
                })
                .map(per_million)
        };
        capabilities.input_price_per_million = per_token("prompt");
        capabilities.output_price_per_million = per_token("completion");
    }
    let deprecated = item
        .get("deprecated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || item.get("deprecation").is_some();
    let replaced_by = item
        .get("replaced_by")
        .or_else(|| item.get("replacement"))
        .or_else(|| item.pointer("/deprecation/replacement"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    Some(ModelInfo {
        id,
        display_name,
        capabilities,
        replaced_by,
        deprecated,
    })
}

/// Converts a per-token price to per-million tokens, rounded to a
/// nanodollar so the value survives a JSON round trip unchanged.
fn per_million(per_token: f64) -> f64 {
    (per_token * 1_000_000.0 * 1e9).round() / 1e9
}

/// In-memory lists per profile, shared with adapters as their capability
/// source.
#[derive(Default)]
pub struct ModelCatalog {
    lists: RwLock<BTreeMap<String, ModelList>>,
}

impl ModelCatalog {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn insert(&self, list: ModelList) {
        self.lists
            .write()
            .expect("catalog")
            .insert(list.profile.clone(), list);
    }

    pub fn list(&self, profile: &str) -> Option<ModelList> {
        self.lists.read().expect("catalog").get(profile).cloned()
    }

    pub fn model(&self, profile: &str, model: &str) -> Option<ModelInfo> {
        self.lists
            .read()
            .expect("catalog")
            .get(profile)
            .and_then(|list| list.models.iter().find(|m| m.id == model).cloned())
    }

    pub fn status(&self, profile: &str) -> Option<ModelListStatus> {
        self.list(profile).map(|list| list.status)
    }
}

impl CapabilitySource for ModelCatalog {
    fn capabilities(&self, profile: &str, model: &str) -> Option<ModelCapabilities> {
        self.model(profile, model).map(|m| m.capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_extras_become_capabilities_without_guessing() {
        let value = serde_json::json!({ "data": [
            { "id": "a/b", "name": "A B", "context_length": 128000,
              "architecture": { "input_modalities": ["text", "image"] },
              "supported_parameters": ["tools", "response_format"],
              "pricing": { "prompt": "0.000001", "completion": "0.000002" } },
            { "id": "plain" }
        ]});
        let models = parse_model_list(Protocol::ChatCompletions, &value).unwrap();
        let rich = &models[0].capabilities;
        assert_eq!(rich.context_length, Some(128000));
        assert_eq!(rich.tools, Some(true));
        assert_eq!(rich.vision, Some(true));
        assert_eq!(rich.reasoning, Some(false));
        assert!((rich.input_price_per_million.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(models[1].capabilities, ModelCapabilities::default());
    }

    #[test]
    fn anthropic_list_records_display_name_only() {
        let value = serde_json::json!({ "data": [
            { "id": "claude-x", "display_name": "Claude X", "type": "model" }
        ]});
        let models = parse_model_list(Protocol::Messages, &value).unwrap();
        assert_eq!(models[0].display_name.as_deref(), Some("Claude X"));
        assert_eq!(models[0].capabilities, ModelCapabilities::default());
    }

    #[test]
    fn cache_path_is_sanitized_and_injective() {
        let cache = ModelCache::new("/tmp/x");
        let slashed = cache.path("open/router");
        let name = slashed.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("open_router-"));
        assert!(name.ends_with(".json"));
        assert_ne!(slashed, cache.path("open_router"));
        assert_eq!(slashed, cache.path("open/router"));
    }
}
