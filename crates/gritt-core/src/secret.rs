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

/// Name fragments that mark a credential. Any variable whose name
/// contains one of these (case-insensitive), every configured profile key
/// variable, and `AGENT_MEMORY_API_KEY` count as credentials: the native
/// shell tool removes them from its child's environment, and the connector
/// layer redacts their values out of an external agent's output. The
/// conventional cloud and VCS names (`AWS_SECRET_ACCESS_KEY`,
/// `GITHUB_TOKEN`, `NPM_TOKEN`) all fall under this rule.
pub const SECRET_ENV_MARKERS: [&str; 6] =
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL"];

/// Variables that a shell needs and never carry a credential.
const PLAIN_ENV_NAMES: [&str; 12] = [
    "PATH", "HOME", "TMPDIR", "TEMP", "TMP", "TERM", "LANG", "USER", "SHELL", "PWD", "LOGNAME",
    "SHLVL",
];

/// True when an environment variable name looks like it carries a
/// credential. `blocked` lists the configured profile key variables.
pub fn is_secret_env_name(name: &str, blocked: &[String]) -> bool {
    let upper = name.to_ascii_uppercase();
    if PLAIN_ENV_NAMES.contains(&upper.as_str()) || upper.starts_with("LC_") {
        return false;
    }
    upper == "AGENT_MEMORY_API_KEY"
        || blocked.iter().any(|known| known.eq_ignore_ascii_case(name))
        || SECRET_ENV_MARKERS
            .iter()
            .any(|marker| upper.contains(marker))
}

/// Picks the credential values out of an environment snapshot so they can
/// be redacted wherever that environment's output is shown or stored.
pub fn secret_env_values<'a>(
    vars: impl IntoIterator<Item = (&'a str, &'a str)>,
    blocked: &[String],
) -> Vec<Secret> {
    vars.into_iter()
        .filter(|(name, value)| !value.is_empty() && is_secret_env_name(name, blocked))
        .map(|(_, value)| Secret::new(value))
        .collect()
}

#[cfg(test)]
mod env_tests {
    use super::*;

    #[test]
    fn secret_env_values_follow_the_name_rule() {
        let vars = [
            ("AWS_SECRET_ACCESS_KEY", "aws-value"),
            ("GITHUB_TOKEN", "gh-value"),
            ("PATH", "/usr/bin"),
            ("MY_PROFILE_VAR", "profile-value"),
            ("EMPTY_TOKEN", ""),
            ("LC_ALL", "C"),
        ];
        let secrets = secret_env_values(vars, &["MY_PROFILE_VAR".to_owned()]);
        let exposed: Vec<&str> = secrets.iter().map(Secret::expose).collect();
        assert_eq!(exposed, vec!["aws-value", "gh-value", "profile-value"]);
        assert!(is_secret_env_name("KEYCHAIN_PATH", &[]));
        assert!(!is_secret_env_name("HOME", &[]));
    }
}
