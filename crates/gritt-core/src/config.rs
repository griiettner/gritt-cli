//! Layered configuration (ADR-008). Precedence is command-line flags, then
//! project config, then user config, then environment variables, then
//! built-in defaults. Config never holds a key value; a layer that does is
//! rejected before it is merged.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::embeddings::{EmbeddingConfig, RerankConfig};
use crate::policy::PolicyConfig;
use crate::provider::ProviderProfile;
use crate::{Error, Result};

/// Fully resolved configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub profiles: BTreeMap<String, ProviderProfile>,
    /// Global alias to `profile/model`. Per-profile aliases live on the
    /// profile.
    pub aliases: BTreeMap<String, String>,
    pub default_profile: Option<String>,
    pub default_model: Option<String>,
    /// Profiles tried, in order, when the default profile is unusable for
    /// a new native session. The default stays first; it must not be
    /// repeated here. See [`Config::fallback_order`].
    #[serde(default)]
    pub fallback_profiles: Vec<String>,
    pub model_list: ModelListPolicy,
    pub policy: PolicyConfig,
    pub connectors: ConnectorSettings,
    pub interface: InterfacePreferences,
    pub logging: LoggingConfig,
    pub embeddings: Option<EmbeddingConfig>,
    pub rerank: Option<RerankConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelListPolicy {
    /// Minimum seconds between refreshes. Default is one day.
    pub refresh_interval_secs: u64,
    /// Use the last cached list, marked stale, when a refresh fails.
    pub stale_fallback: bool,
}

impl Default for ModelListPolicy {
    fn default() -> Self {
        Self {
            refresh_interval_secs: 24 * 60 * 60,
            stale_fallback: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectorSettings {
    /// Connector name to executable path override.
    pub executables: BTreeMap<String, String>,
    pub health_check_timeout_secs: Option<u64>,
    pub task_timeout_secs: Option<u64>,
    /// Connector names launched through a PTY instead of pipes. The
    /// machine-readable interface stays the default (ADR-010).
    #[serde(default)]
    pub pty: Vec<String>,
    /// Extra command-line arguments per connector, passed through verbatim
    /// so the user can select the external agent's own permission mode.
    #[serde(default)]
    pub extra_args: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Print,
    Repl,
    FullScreen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterfacePreferences {
    pub default_mode: Mode,
    pub color: bool,
    pub reduced_motion: bool,
}

impl Default for InterfacePreferences {
    fn default() -> Self {
        Self {
            default_mode: Mode::Print,
            color: true,
            reduced_motion: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Off by default. Structured logs stay content-free.
    pub content_logging: bool,
    /// Retention for content logs when enabled.
    pub content_retention_days: u32,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            content_logging: false,
            content_retention_days: 7,
        }
    }
}

/// One source of configuration. Every field is optional so layers can be
/// merged with the higher-precedence layer winning per field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigLayer {
    #[serde(default)]
    pub profiles: BTreeMap<String, ProviderProfile>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    pub default_profile: Option<String>,
    pub default_model: Option<String>,
    /// Replaces, never extends, a lower layer's list.
    pub fallback_profiles: Option<Vec<String>>,
    pub model_list: Option<ModelListPolicy>,
    pub policy: Option<PolicyConfig>,
    pub connectors: Option<ConnectorSettings>,
    pub interface: Option<InterfacePreferences>,
    pub logging: Option<LoggingConfig>,
    pub embeddings: Option<EmbeddingConfig>,
    pub rerank: Option<RerankConfig>,
}

/// Field names that may only ever hold a reference, never a value.
pub const SECRET_FIELD_NAMES: [&str; 5] = ["api_key", "apikey", "key", "token", "secret"];

fn looks_like_secret_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_FIELD_NAMES
        .iter()
        .any(|forbidden| lower == *forbidden || lower.ends_with(&format!("_{forbidden}")))
}

/// Walks a raw layer and fails on any string-valued field whose name looks
/// like a secret. `key` objects that are a [`crate::secret::SecretRef`] are
/// fine because their value is a table, not a string.
pub fn reject_literal_secrets(raw: &serde_json::Value, path_hint: &str) -> Result<()> {
    fn walk(value: &serde_json::Value, trail: &str, path_hint: &str) -> Result<()> {
        match value {
            serde_json::Value::Object(map) => {
                for (name, child) in map {
                    let here = if trail.is_empty() {
                        name.clone()
                    } else {
                        format!("{trail}.{name}")
                    };
                    if child.is_string() && looks_like_secret_field(name) {
                        return Err(Error::secret_in_config(path_hint, &here));
                    }
                    walk(child, &here, path_hint)?;
                }
                Ok(())
            }
            serde_json::Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(item, &format!("{trail}[{index}]"), path_hint)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    walk(raw, "", path_hint)
}

/// Sections that only the environment layer may populate. Embedding and
/// reranking are opt-in through `AGENT_*` variables (TKT-0008 plan), so a
/// config file must not be able to switch them on.
pub const ENV_ONLY_SECTIONS: [&str; 2] = ["embeddings", "rerank"];

/// Fails when a file layer tries to set an environment-only section.
pub fn reject_env_only_sections(raw: &serde_json::Value, path_hint: &str) -> Result<()> {
    if let serde_json::Value::Object(map) = raw {
        if let Some(section) = ENV_ONLY_SECTIONS
            .iter()
            .find(|name| map.contains_key(**name))
        {
            return Err(Error::config(format!(
                "config {path_hint} sets `{section}`, which is enabled only through the \
                 AGENT_EMBEDDING_PROVIDER and AGENT_RERANK_PROVIDER environment variables"
            )));
        }
    }
    Ok(())
}

/// Parses a file layer from a JSON-shaped value after rejecting literal
/// secrets and environment-only sections.
pub fn layer_from_value(raw: serde_json::Value, path_hint: &str) -> Result<ConfigLayer> {
    reject_literal_secrets(&raw, path_hint)?;
    reject_env_only_sections(&raw, path_hint)?;
    serde_json::from_value(raw)
        .map_err(|error| Error::config(format!("invalid config {path_hint}: {error}")))
}

/// Merges layers ordered from lowest to highest precedence into the
/// defaults. Later layers win per field; profile and alias maps merge by
/// key.
pub fn merge(layers: impl IntoIterator<Item = ConfigLayer>) -> Config {
    let mut config = Config::default();
    for layer in layers {
        config.profiles.extend(layer.profiles);
        config.aliases.extend(layer.aliases);
        if layer.default_profile.is_some() {
            config.default_profile = layer.default_profile;
        }
        if layer.default_model.is_some() {
            config.default_model = layer.default_model;
        }
        if let Some(value) = layer.fallback_profiles {
            config.fallback_profiles = value;
        }
        if let Some(value) = layer.model_list {
            config.model_list = value;
        }
        if let Some(value) = layer.policy {
            config.policy = value;
        }
        if let Some(value) = layer.connectors {
            config.connectors = value;
        }
        if let Some(value) = layer.interface {
            config.interface = value;
        }
        if let Some(value) = layer.logging {
            config.logging = value;
        }
        if layer.embeddings.is_some() {
            config.embeddings = layer.embeddings;
        }
        if layer.rerank.is_some() {
            config.rerank = layer.rerank;
        }
    }
    config
}

impl Config {
    /// The profiles a new native session tries, in order: the default
    /// profile, then `fallback_profiles`. Fails on a name that is not a
    /// configured profile and on a duplicate, including the default
    /// profile repeated in the fallback list, so a typo cannot silently
    /// shorten the chain.
    pub fn fallback_order(&self) -> Result<Vec<String>> {
        let mut order: Vec<String> = Vec::new();
        if let Some(default) = &self.default_profile {
            order.push(default.clone());
        }
        for name in &self.fallback_profiles {
            if !self.profiles.contains_key(name) {
                return Err(Error::config(format!(
                    "fallback_profiles names `{name}`, which is not a configured profile"
                )));
            }
            if order.iter().any(|seen| seen == name) {
                let role = if self.default_profile.as_deref() == Some(name.as_str()) {
                    "the default profile and is repeated"
                } else {
                    "listed more than once"
                };
                return Err(Error::config(format!(
                    "fallback_profiles: `{name}` is {role}; each profile may appear once"
                )));
            }
            order.push(name.clone());
        }
        Ok(order)
    }

    /// Checks the cross-field rules a layer cannot check on its own.
    pub fn validate(&self) -> Result<()> {
        self.fallback_order().map(|_| ())
    }
}

/// The precedence order, lowest first, for callers that assemble layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayerSource {
    Defaults,
    Environment,
    User,
    Project,
    Flags,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;

    fn layer(default_model: &str) -> ConfigLayer {
        ConfigLayer {
            default_model: Some(default_model.into()),
            ..ConfigLayer::default()
        }
    }

    #[test]
    fn later_layers_win_and_maps_merge() {
        let mut user = layer("user-model");
        user.aliases
            .insert("fast".into(), "openai/gpt-5-nano".into());
        let mut project = layer("project-model");
        project
            .aliases
            .insert("smart".into(), "anthropic/claude".into());
        let flags = ConfigLayer::default();
        let config = merge([layer("env-model"), user, project, flags]);
        assert_eq!(config.default_model.as_deref(), Some("project-model"));
        assert_eq!(config.aliases.len(), 2);
        assert_eq!(config.model_list.refresh_interval_secs, 86_400);
        assert!(!config.logging.content_logging);
        assert_eq!(config.logging.content_retention_days, 7);
    }

    fn profile(name: &str) -> ProviderProfile {
        ProviderProfile {
            name: name.into(),
            protocol: crate::provider::Protocol::ChatCompletions,
            base_url: "https://example.test/v1".into(),
            key: crate::secret::SecretRef::for_profile(name, "KEY"),
            aliases: Default::default(),
            fallback_model: None,
        }
    }

    #[test]
    fn fallback_order_is_default_first_then_the_list_and_rejects_bad_entries() {
        let mut config = Config::default();
        for name in ["openrouter", "anthropic", "openai"] {
            config.profiles.insert(name.into(), profile(name));
        }
        assert!(config.fallback_order().unwrap().is_empty());
        config.default_profile = Some("openrouter".into());
        config.fallback_profiles = vec!["anthropic".into(), "openai".into()];
        assert_eq!(
            config.fallback_order().unwrap(),
            vec!["openrouter", "anthropic", "openai"]
        );
        config.validate().unwrap();

        config.fallback_profiles = vec!["anthropic".into(), "nope".into()];
        let error = config.fallback_order().unwrap_err();
        assert_eq!(error.kind, ErrorKind::Config);
        assert!(error.message.contains("`nope`"), "{}", error.message);

        config.fallback_profiles = vec!["anthropic".into(), "anthropic".into()];
        let error = config.validate().unwrap_err();
        assert!(
            error.message.contains("more than once"),
            "{}",
            error.message
        );

        config.fallback_profiles = vec!["openrouter".into()];
        let error = config.validate().unwrap_err();
        assert!(
            error.message.contains("default profile"),
            "{}",
            error.message
        );

        // Without a default the list alone is the chain.
        config.default_profile = None;
        config.fallback_profiles = vec!["openai".into()];
        assert_eq!(config.fallback_order().unwrap(), vec!["openai"]);
    }

    #[test]
    fn a_higher_layer_replaces_the_fallback_list_and_an_absent_one_keeps_it() {
        let user = ConfigLayer {
            fallback_profiles: Some(vec!["anthropic".into()]),
            ..ConfigLayer::default()
        };
        let config = merge([user.clone(), ConfigLayer::default()]);
        assert_eq!(config.fallback_profiles, vec!["anthropic"]);
        let project = ConfigLayer {
            fallback_profiles: Some(vec![]),
            ..ConfigLayer::default()
        };
        let config = merge([user, project]);
        assert!(config.fallback_profiles.is_empty());
        let layer = layer_from_value(
            serde_json::json!({ "fallback_profiles": ["a", "b"] }),
            "test",
        )
        .unwrap();
        assert_eq!(layer.fallback_profiles.unwrap(), vec!["a", "b"]);
        let legacy: Config = serde_json::from_value(serde_json::json!({
            "profiles": {}, "aliases": {}, "default_profile": null, "default_model": null,
            "model_list": {"refresh_interval_secs": 1, "stale_fallback": true},
            "policy": PolicyConfig::default(), "connectors": {}, "interface": InterfacePreferences::default(),
            "logging": LoggingConfig::default(), "embeddings": null, "rerank": null
        }))
        .unwrap();
        assert!(legacy.fallback_profiles.is_empty());
    }

    #[test]
    fn literal_secret_is_rejected() {
        let raw = serde_json::json!({
            "profiles": {"openai": {"name": "openai", "protocol": "responses",
                "base_url": "https://api.openai.com/v1", "api_key": "sk-literal"}}
        });
        let error = layer_from_value(raw, "project config").unwrap_err();
        assert_eq!(error.kind, ErrorKind::SecretInConfig);
        assert!(error.message.contains("profiles.openai.api_key"));
        assert!(!error.message.contains("sk-literal"));
    }

    #[test]
    fn file_layer_cannot_enable_embeddings_or_reranking() {
        for section in ENV_ONLY_SECTIONS {
            let raw = serde_json::json!({
                section: {"model": "text-embedding-3-small",
                    "gateway": {"base_url": null,
                        "key": {"keychain_service_entry": "gritt/memory-gateway",
                            "env_var_name": "AGENT_MEMORY_API_KEY"}}}
            });
            let error = layer_from_value(raw, "project config").unwrap_err();
            assert_eq!(error.kind, ErrorKind::Config);
            assert!(error.message.contains(section));
            assert!(error.message.contains("AGENT_EMBEDDING_PROVIDER"));
        }
    }

    #[test]
    fn environment_layer_still_enables_embeddings() {
        let env: std::collections::HashMap<String, String> = [(
            "AGENT_EMBEDDING_PROVIDER".to_string(),
            "text-embedding-3-small".to_string(),
        )]
        .into_iter()
        .collect();
        let embedding = crate::embeddings::embedding_config(&env);
        let layer = ConfigLayer {
            embeddings: Some(embedding),
            ..ConfigLayer::default()
        };
        let config = merge([layer]);
        assert!(config.embeddings.as_ref().is_some_and(|e| e.is_enabled()));
        assert!(config.rerank.is_none());
    }

    #[test]
    fn secret_reference_table_is_accepted() {
        let raw = serde_json::json!({
            "profiles": {"openai": {"name": "openai", "protocol": "responses",
                "base_url": "https://api.openai.com/v1",
                "key": {"keychain_service_entry": "gritt/openai", "env_var_name": "OPENAI_API_KEY"}}}
        });
        let layer = layer_from_value(raw, "user config").unwrap();
        assert_eq!(layer.profiles["openai"].key.env_var_name, "OPENAI_API_KEY");
    }

    #[test]
    fn a_partial_connectors_section_parses_with_defaults() {
        let layer = layer_from_value(
            serde_json::json!({ "connectors": { "pty": ["codex"] } }),
            "test",
        )
        .unwrap();
        let connectors = layer.connectors.unwrap();
        assert_eq!(connectors.pty, vec!["codex".to_string()]);
        assert!(connectors.executables.is_empty());
        assert!(connectors.extra_args.is_empty());
        let layer = layer_from_value(
            serde_json::json!({ "connectors": { "extra_args": { "codex": ["--full-auto"] } } }),
            "test",
        )
        .unwrap();
        let connectors = layer.connectors.unwrap();
        assert_eq!(
            connectors.extra_args["codex"],
            vec!["--full-auto".to_string()]
        );
        assert!(connectors.pty.is_empty());
    }
}
