//! Internal error kinds. Adapters and connectors translate their failures
//! into these before anything above them sees the error.

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

/// One error with a kind, a one-line human message, and optional
/// diagnostic detail. The message never contains a key value, a prompt, or
/// tool content.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    /// Provider error bodies and similar detail for troubleshooting. Never
    /// shown by default.
    pub diagnostic: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// No key was found in the keychain or the environment for a profile.
    MissingKey,
    /// The selected model does not report the requested capability.
    UnsupportedCapability,
    /// The model list refresh failed and the cached copy is being used.
    StaleModelList,
    /// No model list is cached and the refresh failed.
    MissingModelList,
    /// The provider returned an error response.
    Provider,
    /// No configured profile could start a new native session: every
    /// candidate was skipped for a missing key, an authentication or
    /// connection failure, or an unavailable model.
    NoUsableProfile,
    /// Configuration is invalid.
    Config,
    /// A config layer contained a literal secret value.
    SecretInConfig,
    /// Database or filesystem persistence failed.
    Storage,
    /// An external agent connector failed.
    Connector,
    /// The operation was cancelled.
    Cancelled,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(mut self, diagnostic: serde_json::Value) -> Self {
        self.diagnostic = Some(diagnostic);
        self
    }

    /// Names the profile and the variable Gritt looked for. Never the value.
    pub fn missing_key(profile: &str, env_var_name: &str) -> Self {
        Self::new(
            ErrorKind::MissingKey,
            format!(
                "no key for profile `{profile}`: nothing in the keychain and `{env_var_name}` is not set; set `{env_var_name}` for this process ({}) or run `gritt key-set {profile}` to save it in the OS keychain",
                env_var_setup_hint(env_var_name)
            ),
        )
    }

    pub fn unsupported_capability(model: &str, capability: &str) -> Self {
        Self::new(
            ErrorKind::UnsupportedCapability,
            format!("model `{model}` does not report support for {capability}"),
        )
    }

    pub fn secret_in_config(path_hint: &str, field: &str) -> Self {
        Self::new(
            ErrorKind::SecretInConfig,
            format!(
                "config {path_hint} contains a literal secret in field `{field}`; name the environment variable instead"
            ),
        )
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, message)
    }

    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Storage, message)
    }

    pub fn provider(status: Option<u16>, message: impl Into<String>) -> Self {
        let message = message.into();
        let message = match status {
            Some(status) => format!("provider returned {status}: {message}"),
            None => format!("provider request failed: {message}"),
        };
        Self::new(ErrorKind::Provider, message)
    }

    pub fn connector(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Connector, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "operation cancelled")
    }
}

#[cfg(windows)]
fn env_var_setup_hint(name: &str) -> String {
    format!("PowerShell: $env:{name} = '<key>'; Command Prompt: set {name}=<key>")
}

#[cfg(not(windows))]
fn env_var_setup_hint(name: &str) -> String {
    format!("macOS/Linux shell: export {name}='<key>'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_names_profile_and_variable_only() {
        let error = Error::missing_key("openrouter", "OPENROUTER_API_KEY");
        assert_eq!(error.kind, ErrorKind::MissingKey);
        assert!(error.message.contains("openrouter"));
        assert!(error.message.contains("OPENROUTER_API_KEY"));
        assert!(error.message.contains("gritt key-set openrouter"));
        assert!(error.message.contains("<key>"));
        assert!(error.diagnostic.is_none());
    }

    #[test]
    fn provider_error_keeps_body_in_diagnostic() {
        let error = Error::provider(Some(429), "rate limited")
            .with_diagnostic(serde_json::json!({"error": {"type": "rate_limit"}}));
        assert_eq!(error.to_string(), "provider returned 429: rate limited");
        assert!(error.diagnostic.is_some());
    }
}
