//! Opt-in embedding and reranking configuration from the environment,
//! matching `.agents/brain/providers.md`. Pure parsing; no network.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::secret::SecretRef;

pub const EMBEDDING_PROVIDER_VAR: &str = "AGENT_EMBEDDING_PROVIDER";
pub const RERANK_PROVIDER_VAR: &str = "AGENT_RERANK_PROVIDER";
pub const API_KEY_VAR: &str = "AGENT_MEMORY_API_KEY";
pub const BASE_URL_VAR: &str = "AGENT_MEMORY_BASE_URL";

/// Shared gateway settings. The key is a reference; the value is never read
/// here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryGateway {
    pub base_url: Option<String>,
    pub key: SecretRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// `None` means disabled. `Some` is the model identifier.
    pub model: Option<String>,
    pub gateway: MemoryGateway,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RerankConfig {
    pub model: Option<String>,
    pub gateway: MemoryGateway,
}

impl EmbeddingConfig {
    pub fn is_enabled(&self) -> bool {
        self.model.is_some()
    }
}

impl RerankConfig {
    pub fn is_enabled(&self) -> bool {
        self.model.is_some()
    }
}

/// Missing, empty, and `none` (any case) all mean off.
fn capability(env: &HashMap<String, String>, key: &str) -> Option<String> {
    env.get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("none"))
        .map(str::to_owned)
}

fn gateway(env: &HashMap<String, String>) -> MemoryGateway {
    MemoryGateway {
        base_url: env
            .get(BASE_URL_VAR)
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty()),
        key: SecretRef {
            keychain_service_entry: "gritt/memory-gateway".into(),
            env_var_name: API_KEY_VAR.into(),
        },
    }
}

pub fn embedding_config(env: &HashMap<String, String>) -> EmbeddingConfig {
    EmbeddingConfig {
        model: capability(env, EMBEDDING_PROVIDER_VAR),
        gateway: gateway(env),
    }
}

pub fn rerank_config(env: &HashMap<String, String>) -> RerankConfig {
    RerankConfig {
        model: capability(env, RERANK_PROVIDER_VAR),
        gateway: gateway(env),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn disabled_by_default() {
        let empty = HashMap::new();
        assert!(!embedding_config(&empty).is_enabled());
        assert!(!rerank_config(&empty).is_enabled());
    }

    #[test]
    fn none_and_empty_disable() {
        let env = env(&[
            (EMBEDDING_PROVIDER_VAR, "None"),
            (RERANK_PROVIDER_VAR, "  "),
        ]);
        assert!(!embedding_config(&env).is_enabled());
        assert!(!rerank_config(&env).is_enabled());
    }

    #[test]
    fn enabled_uses_configured_endpoint_and_key_reference() {
        let env = env(&[
            (EMBEDDING_PROVIDER_VAR, "text-embedding-3-small"),
            (BASE_URL_VAR, "https://openrouter.ai/api/"),
            (API_KEY_VAR, "literal-value-that-must-not-be-copied"),
        ]);
        let config = embedding_config(&env);
        assert_eq!(config.model.as_deref(), Some("text-embedding-3-small"));
        assert_eq!(
            config.gateway.base_url.as_deref(),
            Some("https://openrouter.ai/api")
        );
        assert_eq!(config.gateway.key.env_var_name, API_KEY_VAR);
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("literal-value"));
    }
}
