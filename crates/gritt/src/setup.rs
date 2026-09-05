//! The binary's side of the setup contract: writes a profile's non-secret
//! fields to the user or project config file and a key to the keychain
//! (ADR-006, ADR-008). The harness only sees the typed outcomes.

use std::path::{Path, PathBuf};

use gritt_core::provider::ProviderProfile;
use gritt_core::secret::Secret;
use gritt_harness::setup::{
    validate_profile_spec, ConfigDestination, CredentialStoreOutcome, ProfileSaveOutcome,
    ProviderSetup,
};

use crate::config::{user_config_path, PROJECT_CONFIG};
use crate::keys::{EnvSource, KeyResolver, Keychain};

pub struct FileSetup<K, E> {
    pub workspace: PathBuf,
    /// `None` when the system has no user config directory.
    pub user_path: Option<PathBuf>,
    pub resolver: KeyResolver<K, E>,
}

impl<K: Keychain, E: EnvSource> FileSetup<K, E> {
    pub fn new(workspace: &Path, resolver: KeyResolver<K, E>) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            user_path: user_config_path(),
            resolver,
        }
    }

    fn path_for(&self, destination: ConfigDestination) -> Option<PathBuf> {
        match destination {
            ConfigDestination::User => self.user_path.clone(),
            ConfigDestination::Project => Some(self.workspace.join(PROJECT_CONFIG)),
        }
    }

    /// Whether the given file already defines `profiles.<name>`.
    fn defines_profile(path: &Path, name: &str) -> bool {
        read_table(path).is_ok_and(|table| {
            table
                .get("profiles")
                .and_then(|profiles| profiles.as_table())
                .is_some_and(|profiles| profiles.contains_key(name))
        })
    }
}

/// Reads a config file as a TOML table, or an empty table when absent. A
/// parse failure names the file only; the content is never repeated
/// because a malformed file may hold a key value.
fn read_table(path: &Path) -> Result<toml::Table, String> {
    if !path.is_file() {
        return Ok(toml::Table::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    toml::from_str(&text).map_err(|_| format!("cannot parse {} as TOML", path.display()))
}

impl<K: Keychain + Send + Sync, E: EnvSource + Send + Sync> ProviderSetup for FileSetup<K, E> {
    fn save_profile(
        &self,
        profile: &ProviderProfile,
        destination: ConfigDestination,
    ) -> ProfileSaveOutcome {
        if let Err(problem) = validate_profile_spec(profile) {
            return ProfileSaveOutcome::Invalid { problem };
        }
        let Some(path) = self.path_for(destination) else {
            return ProfileSaveOutcome::Unavailable {
                reason: "this system has no user configuration directory".into(),
            };
        };
        let mut table = match read_table(&path) {
            Ok(table) => table,
            Err(message) => return ProfileSaveOutcome::Failed { message },
        };
        let value = match toml::Value::try_from(profile) {
            Ok(value) => value,
            Err(error) => {
                return ProfileSaveOutcome::Failed {
                    message: format!("cannot encode profile: {error}"),
                }
            }
        };
        let profiles = table
            .entry("profiles")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        let Some(profiles) = profiles.as_table_mut() else {
            return ProfileSaveOutcome::Failed {
                message: format!("`profiles` in {} is not a table", path.display()),
            };
        };
        profiles.insert(profile.name.clone(), value);
        let text = match toml::to_string_pretty(&table) {
            Ok(text) => text,
            Err(error) => {
                return ProfileSaveOutcome::Failed {
                    message: format!("cannot encode {}: {error}", path.display()),
                }
            }
        };
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                return ProfileSaveOutcome::Failed {
                    message: format!("cannot create {}: {error}", parent.display()),
                };
            }
        }
        if let Err(error) = std::fs::write(&path, text) {
            return ProfileSaveOutcome::Failed {
                message: format!("cannot write {}: {error}", path.display()),
            };
        }
        let shadowed_by = match destination {
            ConfigDestination::User => {
                let project = self.workspace.join(PROJECT_CONFIG);
                Self::defines_profile(&project, &profile.name).then_some(ConfigDestination::Project)
            }
            ConfigDestination::Project => None,
        };
        ProfileSaveOutcome::Saved {
            destination,
            path,
            shadowed_by,
        }
    }

    fn store_credential(&self, profile: &ProviderProfile, value: Secret) -> CredentialStoreOutcome {
        match self.resolver.store(&profile.key, &value) {
            Ok(()) => CredentialStoreOutcome::Stored {
                profile: profile.name.clone(),
                keychain_service_entry: profile.key.keychain_service_entry.clone(),
            },
            Err(error) => CredentialStoreOutcome::KeychainUnavailable {
                profile: profile.name.clone(),
                env_var_name: profile.key.env_var_name.clone(),
                message: error.message,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gritt_core::provider::Protocol;
    use gritt_core::secret::SecretRef;
    use gritt_core::{Error, Result};

    struct FailingKeychain;

    impl Keychain for FailingKeychain {
        fn get(&self, _service: &str, _user: &str) -> Result<Option<Secret>> {
            Ok(None)
        }
        fn set(&self, service: &str, user: &str, _value: &Secret) -> Result<()> {
            Err(Error::storage(format!(
                "cannot write keychain entry {service}/{user}"
            )))
        }
    }

    struct NoEnv;

    impl EnvSource for NoEnv {
        fn var(&self, _name: &str) -> Option<String> {
            None
        }
    }

    fn setup(dir: &Path) -> FileSetup<FailingKeychain, NoEnv> {
        FileSetup {
            workspace: dir.to_path_buf(),
            user_path: Some(dir.join("user").join("config.toml")),
            resolver: KeyResolver {
                keychain: FailingKeychain,
                env: NoEnv,
            },
        }
    }

    fn profile(name: &str) -> ProviderProfile {
        ProviderProfile {
            name: name.into(),
            protocol: Protocol::Responses,
            base_url: "https://api.openai.com/v1".into(),
            key: SecretRef::for_profile(name, "OPENAI_API_KEY"),
            aliases: Default::default(),
        }
    }

    #[test]
    fn saving_a_profile_preserves_the_rest_of_the_file_and_reports_shadowing() {
        let dir = tempfile::tempdir().unwrap();
        let setup = setup(dir.path());
        let user = setup.user_path.clone().unwrap();
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(
            &user,
            "default_model = \"keep-me\"\n\n[profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\nbase_url = \"http://127.0.0.1:8080/v1\"\n[profiles.local.key]\nkeychain_service_entry = \"gritt/local\"\nenv_var_name = \"LOCAL_KEY\"\n",
        )
        .unwrap();
        let outcome = setup.save_profile(&profile("openai"), ConfigDestination::User);
        assert_eq!(
            outcome,
            ProfileSaveOutcome::Saved {
                destination: ConfigDestination::User,
                path: user.clone(),
                shadowed_by: None,
            }
        );
        let config =
            crate::config::load_with(dir.path(), Some(&user), [], Default::default()).unwrap();
        assert_eq!(config.default_model.as_deref(), Some("keep-me"));
        assert_eq!(config.profiles.len(), 2);
        assert_eq!(config.profiles["openai"].protocol, Protocol::Responses);
        assert_eq!(config.profiles["local"].key.env_var_name, "LOCAL_KEY");
        assert!(!std::fs::read_to_string(&user).unwrap().contains("sk-"));

        // A project file defining the same profile wins, and the outcome
        // says so.
        let project = setup.save_profile(&profile("openai"), ConfigDestination::Project);
        assert!(matches!(
            project,
            ProfileSaveOutcome::Saved {
                destination: ConfigDestination::Project,
                shadowed_by: None,
                ..
            }
        ));
        let again = setup.save_profile(&profile("openai"), ConfigDestination::User);
        assert!(matches!(
            again,
            ProfileSaveOutcome::Saved {
                shadowed_by: Some(ConfigDestination::Project),
                ..
            }
        ));
    }

    #[test]
    fn invalid_specs_and_missing_user_directory_are_typed_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let mut setup = setup(dir.path());
        let mut bad = profile("bad name");
        bad.name = "bad name".into();
        assert!(matches!(
            setup.save_profile(&bad, ConfigDestination::User),
            ProfileSaveOutcome::Invalid { .. }
        ));
        setup.user_path = None;
        assert!(matches!(
            setup.save_profile(&profile("openai"), ConfigDestination::User),
            ProfileSaveOutcome::Unavailable { .. }
        ));
        assert!(!dir.path().join(PROJECT_CONFIG).exists());
    }

    #[test]
    fn a_failing_keychain_names_the_variable_and_never_the_value() {
        let dir = tempfile::tempdir().unwrap();
        let setup = setup(dir.path());
        let outcome = setup.store_credential(&profile("openai"), Secret::new("sk-secret-value"));
        let text = serde_json::to_string(&outcome).unwrap();
        assert!(!text.contains("sk-secret-value"));
        assert_eq!(
            outcome,
            CredentialStoreOutcome::KeychainUnavailable {
                profile: "openai".into(),
                env_var_name: "OPENAI_API_KEY".into(),
                message: "cannot write keychain entry gritt/openai/api-key".into(),
            }
        );
        // Nothing was written to any file.
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }
}
