//! Provider setup contracts (feature plan, step 3). The harness exposes
//! non-secret profile availability and typed outcomes for saving a
//! profile or a credential; the binary implements the writes because
//! config files and the keychain are its responsibility (ADR-006,
//! ADR-008). No type here can carry a key value.

use std::path::PathBuf;

use gritt_core::config::Config;
use gritt_core::provider::{Protocol, ProviderProfile};
use gritt_core::secret::Secret;
use serde::{Deserialize, Serialize};

/// Whether a profile's key can be resolved right now. The value itself
/// never leaves the resolver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "credential", rename_all = "snake_case")]
pub enum CredentialState {
    Available,
    /// Names the variable Gritt looked for after the keychain entry.
    Missing {
        env_var_name: String,
    },
}

/// One configured profile as the connection picker sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub name: String,
    pub protocol: Protocol,
    pub base_url: String,
    pub credential: CredentialState,
    pub is_default: bool,
}

/// Which config file a profile is written to. User is the default; the
/// project file needs an explicit choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigDestination {
    User,
    Project,
}

/// Why a profile cannot be saved as given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "problem", rename_all = "snake_case")]
pub enum ProfileSpecError {
    EmptyName,
    /// Names are limited to letters, digits, `-`, and `_` so they are
    /// safe as TOML keys, cache file names, and qualified model prefixes.
    InvalidName {
        name: String,
    },
    EmptyBaseUrl,
    /// The endpoint must be an `http://` or `https://` URL.
    InvalidBaseUrl {
        base_url: String,
    },
    EmptyEnvVarName,
    EmptyKeychainEntry,
}

/// Checks the non-secret fields of a profile before it is written.
pub fn validate_profile_spec(profile: &ProviderProfile) -> Result<(), ProfileSpecError> {
    if profile.name.is_empty() {
        return Err(ProfileSpecError::EmptyName);
    }
    if !profile
        .name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ProfileSpecError::InvalidName {
            name: profile.name.clone(),
        });
    }
    if profile.base_url.trim().is_empty() {
        return Err(ProfileSpecError::EmptyBaseUrl);
    }
    if !(profile.base_url.starts_with("http://") || profile.base_url.starts_with("https://")) {
        return Err(ProfileSpecError::InvalidBaseUrl {
            base_url: profile.base_url.clone(),
        });
    }
    if profile.key.env_var_name.trim().is_empty()
        || profile.key.env_var_name.chars().any(char::is_whitespace)
    {
        return Err(ProfileSpecError::EmptyEnvVarName);
    }
    if profile.key.keychain_service_entry.trim().is_empty() {
        return Err(ProfileSpecError::EmptyKeychainEntry);
    }
    Ok(())
}

/// What saving a profile did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ProfileSaveOutcome {
    Saved {
        destination: ConfigDestination,
        path: PathBuf,
        /// Set when a higher-precedence layer already defines the same
        /// profile, so the saved one will not be the effective one.
        shadowed_by: Option<ConfigDestination>,
    },
    Invalid {
        problem: ProfileSpecError,
    },
    /// This setup service cannot write config (a read-only harness, or
    /// no user config directory on this system).
    Unavailable {
        reason: String,
    },
    /// The write failed. The message never repeats file content.
    Failed {
        message: String,
    },
}

/// What storing a credential did. Keys go to the keychain only; when it
/// is unavailable the interface explains the environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CredentialStoreOutcome {
    Stored {
        profile: String,
        keychain_service_entry: String,
    },
    KeychainUnavailable {
        profile: String,
        env_var_name: String,
        message: String,
    },
    Unavailable {
        reason: String,
    },
}

/// The writes the binary performs for the setup flow. Injected into the
/// control plane; the harness never touches a config file or keychain.
pub trait ProviderSetup: Send + Sync {
    /// Writes the non-secret fields of a profile, preserving unrelated
    /// configuration in the destination file.
    fn save_profile(
        &self,
        profile: &ProviderProfile,
        destination: ConfigDestination,
    ) -> ProfileSaveOutcome;
    /// Writes a key to the keychain entry the profile names. Never to a
    /// file.
    fn store_credential(&self, profile: &ProviderProfile, value: Secret) -> CredentialStoreOutcome;

    /// Re-reads the configuration layers this service writes to.
    ///
    /// [`ProfileSaveOutcome::Saved`] says a file changed, not that the
    /// running configuration did. An interface that just created a profile
    /// needs it to be usable without a restart, and only the binary knows
    /// which layers to merge (ADR-006), so the reload lives here too.
    /// `None` means this service cannot reload, which is the default and
    /// what a read-only harness answers.
    fn reload_config(&self) -> Option<Config> {
        None
    }
}

/// The profile spec and the key a setup interface collected, handed to
/// [`apply_setup`] once. The key travels here and nowhere else: it is not
/// in the interface's action type, its transcript, or its state after the
/// submission is taken.
pub struct SetupSubmission {
    pub profile: ProviderProfile,
    /// `None` when no key was typed, which is allowed: the variable may
    /// already be set in the environment.
    pub secret: Option<Secret>,
    pub destination: ConfigDestination,
}

/// Writes a profile and, when one was typed, its key. Returns the line to
/// show and whether the setup flow may close.
///
/// The order matters: the profile is written first, because a key with no
/// profile to belong to is unusable, and a keychain that refuses still
/// leaves a usable profile as long as the variable is exported.
pub fn apply_setup(setup: &dyn ProviderSetup, submission: SetupSubmission) -> (String, bool) {
    let saved = setup.save_profile(&submission.profile, submission.destination);
    let (message, ok) = match saved {
        ProfileSaveOutcome::Saved {
            path, shadowed_by, ..
        } => (
            match shadowed_by {
                Some(layer) => format!(
                    "saved to {}, but a {} configuration already defines this profile and wins",
                    path.display(),
                    match layer {
                        ConfigDestination::User => "user",
                        ConfigDestination::Project => "project",
                    }
                ),
                None => format!("saved to {}", path.display()),
            },
            true,
        ),
        ProfileSaveOutcome::Invalid { problem } => {
            (format!("the profile is not valid: {problem:?}"), false)
        }
        ProfileSaveOutcome::Unavailable { reason } => (reason, false),
        ProfileSaveOutcome::Failed { message } => (message, false),
    };
    if !ok {
        return (message, false);
    }
    let Some(secret) = submission.secret else {
        return (
            format!(
                "{message}; no key was typed, so {} must be set in the environment",
                submission.profile.key.env_var_name
            ),
            true,
        );
    };
    match setup.store_credential(&submission.profile, secret) {
        CredentialStoreOutcome::Stored { .. } => {
            (format!("{message}; the key went to the keychain"), true)
        }
        CredentialStoreOutcome::KeychainUnavailable {
            env_var_name,
            message: reason,
            ..
        } => (
            format!(
                "{message}, but the keychain is unavailable ({reason}). Export {env_var_name} instead."
            ),
            // The profile exists; only the key did not land. The flow
            // closes so the profile is usable once the variable is set.
            true,
        ),
        CredentialStoreOutcome::Unavailable { reason } => (
            format!("{message}, but no keychain writer is available: {reason}"),
            true,
        ),
    }
}

/// The default when nothing was injected: every write is reported as
/// unavailable, so an interface can still show state without a setup
/// backend (tests, embedded use).
pub struct ReadOnlySetup;

impl ProviderSetup for ReadOnlySetup {
    fn save_profile(
        &self,
        _profile: &ProviderProfile,
        _destination: ConfigDestination,
    ) -> ProfileSaveOutcome {
        ProfileSaveOutcome::Unavailable {
            reason: "no configuration writer is available in this harness".into(),
        }
    }

    fn store_credential(
        &self,
        _profile: &ProviderProfile,
        _value: Secret,
    ) -> CredentialStoreOutcome {
        CredentialStoreOutcome::Unavailable {
            reason: "no keychain writer is available in this harness".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gritt_core::secret::SecretRef;

    fn profile(name: &str, base_url: &str, var: &str) -> ProviderProfile {
        ProviderProfile {
            name: name.into(),
            protocol: Protocol::ChatCompletions,
            base_url: base_url.into(),
            key: SecretRef::for_profile(name, var),
            aliases: Default::default(),
        }
    }

    #[test]
    fn profile_specs_are_checked_field_by_field() {
        assert_eq!(
            validate_profile_spec(&profile("local-1", "http://127.0.0.1:8080/v1", "LOCAL_KEY")),
            Ok(())
        );
        assert_eq!(
            validate_profile_spec(&profile("", "https://x", "K")),
            Err(ProfileSpecError::EmptyName)
        );
        assert_eq!(
            validate_profile_spec(&profile("a/b", "https://x", "K")),
            Err(ProfileSpecError::InvalidName { name: "a/b".into() })
        );
        assert_eq!(
            validate_profile_spec(&profile("a", "api.example", "K")),
            Err(ProfileSpecError::InvalidBaseUrl {
                base_url: "api.example".into()
            })
        );
        assert_eq!(
            validate_profile_spec(&profile("a", "https://x", "HAS SPACE")),
            Err(ProfileSpecError::EmptyEnvVarName)
        );
        let mut blank = profile("a", "https://x", "K");
        blank.key.keychain_service_entry = String::new();
        assert_eq!(
            validate_profile_spec(&blank),
            Err(ProfileSpecError::EmptyKeychainEntry)
        );
    }

    #[test]
    fn the_read_only_setup_reports_unavailable_and_never_holds_the_value() {
        let setup = ReadOnlySetup;
        let target = profile("a", "https://x", "K");
        let outcome = setup.store_credential(&target, Secret::new("sk-never"));
        let text = serde_json::to_string(&outcome).unwrap();
        assert!(text.contains("unavailable"));
        assert!(!text.contains("sk-never"));
        assert!(matches!(
            setup.save_profile(&target, ConfigDestination::User),
            ProfileSaveOutcome::Unavailable { .. }
        ));
    }
}
