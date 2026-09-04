//! Secret references and redacted secret values.
//!
//! Config files name the variable that carries a key and never hold the
//! value (ADR-008). [`SecretRef`] is what persists; [`Secret`] is the
//! in-memory value that must never reach a formatter, log, or fixture.

use serde::{Deserialize, Serialize};

/// Where a secret can be looked up. Resolution order is the OS keychain
/// entry first, then the named environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    /// Keychain service entry name, for example `gritt/openrouter`.
    pub keychain_service_entry: String,
    /// Environment variable name, for example `OPENROUTER_API_KEY`.
    pub env_var_name: String,
}

impl SecretRef {
    /// Builds the conventional reference for a provider profile.
    pub fn for_profile(profile: &str, env_var_name: impl Into<String>) -> Self {
        Self {
            keychain_service_entry: format!("gritt/{profile}"),
            env_var_name: env_var_name.into(),
        }
    }
}

/// A secret value. Debug and Display print `[redacted]`. The type does not
/// implement Serialize, so it cannot be written to a file or fixture by
/// accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Exposes the value. Call sites are the only place a key is read, so
    /// keep them few and never pass the result to a formatter.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_is_redacted_in_debug_and_display() {
        let secret = Secret::new("sk-live-value");
        assert_eq!(format!("{secret:?}"), "[redacted]");
        assert_eq!(format!("{secret}"), "[redacted]");
        assert_eq!(secret.expose(), "sk-live-value");
    }

    #[test]
    fn profile_reference_uses_gritt_namespace() {
        let reference = SecretRef::for_profile("openrouter", "OPENROUTER_API_KEY");
        assert_eq!(reference.keychain_service_entry, "gritt/openrouter");
        assert_eq!(reference.env_var_name, "OPENROUTER_API_KEY");
    }
}
