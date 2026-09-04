//! Key resolution (ADR-008): OS keychain entry first, then the named
//! environment variable. Nothing else. No value ever reaches a formatter.

use gritt_core::secret::{Secret, SecretRef};
use gritt_core::{Error, Result};

/// A keychain backend. The real one wraps the `keyring` crate; tests use a
/// fake.
pub trait Keychain {
    /// `Ok(None)` when no entry exists or no keychain is available.
    fn get(&self, service: &str, user: &str) -> Result<Option<Secret>>;
    fn set(&self, service: &str, user: &str, value: &Secret) -> Result<()>;
}

/// Reads environment variables. Abstracted so tests never touch the
/// process environment.
pub trait EnvSource {
    fn var(&self, name: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|value| !value.is_empty())
    }
}

pub struct SystemKeychain;

impl Keychain for SystemKeychain {
    fn get(&self, service: &str, user: &str) -> Result<Option<Secret>> {
        match keyring::Entry::new(service, user) {
            Ok(entry) => match entry.get_password() {
                Ok(value) => Ok(Some(Secret::new(value))),
                Err(keyring::Error::NoEntry) => Ok(None),
                // Every other failure means "no keychain", so the caller
                // falls back to the environment (ADR-008).
                Err(_) => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }

    fn set(&self, service: &str, user: &str, value: &Secret) -> Result<()> {
        let entry = keyring::Entry::new(service, user)
            .map_err(|_| Error::storage(format!("cannot open keychain entry {service}/{user}")))?;
        entry
            .set_password(value.expose())
            .map_err(|_| Error::storage(format!("cannot write keychain entry {service}/{user}")))
    }
}

pub const KEYCHAIN_USER: &str = "api-key";

pub struct KeyResolver<K, E> {
    pub keychain: K,
    pub env: E,
}

impl<K: Keychain, E: EnvSource> KeyResolver<K, E> {
    /// Resolves the key for `profile`. The error names the profile and the
    /// variable, never a value.
    pub fn resolve(&self, profile: &str, reference: &SecretRef) -> Result<Secret> {
        if let Some(secret) = self
            .keychain
            .get(&reference.keychain_service_entry, KEYCHAIN_USER)?
        {
            return Ok(secret);
        }
        if let Some(value) = self.env.var(&reference.env_var_name) {
            return Ok(Secret::new(value));
        }
        Err(Error::missing_key(profile, &reference.env_var_name))
    }

    /// Stores a key entered through the interface in the keychain only.
    pub fn store(&self, reference: &SecretRef, value: &Secret) -> Result<()> {
        self.keychain
            .set(&reference.keychain_service_entry, KEYCHAIN_USER, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeKeychain(RefCell<HashMap<String, Secret>>);

    impl Keychain for FakeKeychain {
        fn get(&self, service: &str, user: &str) -> Result<Option<Secret>> {
            Ok(self.0.borrow().get(&format!("{service}/{user}")).cloned())
        }
        fn set(&self, service: &str, user: &str, value: &Secret) -> Result<()> {
            self.0
                .borrow_mut()
                .insert(format!("{service}/{user}"), value.clone());
            Ok(())
        }
    }

    struct FakeEnv(HashMap<String, String>);

    impl EnvSource for FakeEnv {
        fn var(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn reference() -> SecretRef {
        SecretRef::for_profile("openrouter", "OPENROUTER_API_KEY")
    }

    #[test]
    fn keychain_wins_over_environment() {
        let resolver = KeyResolver {
            keychain: FakeKeychain::default(),
            env: FakeEnv(HashMap::from([(
                "OPENROUTER_API_KEY".to_string(),
                "env-value".to_string(),
            )])),
        };
        resolver
            .store(&reference(), &Secret::new("keychain-value"))
            .unwrap();
        let secret = resolver.resolve("openrouter", &reference()).unwrap();
        assert_eq!(secret.expose(), "keychain-value");
    }

    #[test]
    fn environment_is_the_fallback() {
        let resolver = KeyResolver {
            keychain: FakeKeychain::default(),
            env: FakeEnv(HashMap::from([(
                "OPENROUTER_API_KEY".to_string(),
                "env-value".to_string(),
            )])),
        };
        let secret = resolver.resolve("openrouter", &reference()).unwrap();
        assert_eq!(secret.expose(), "env-value");
    }

    #[test]
    fn missing_key_error_never_contains_a_value() {
        let resolver = KeyResolver {
            keychain: FakeKeychain::default(),
            env: FakeEnv(HashMap::new()),
        };
        let error = resolver.resolve("openrouter", &reference()).unwrap_err();
        assert_eq!(error.kind, gritt_core::ErrorKind::MissingKey);
        assert!(error.message.contains("openrouter"));
        assert!(error.message.contains("OPENROUTER_API_KEY"));
        assert_eq!(format!("{:?}", Secret::new("x")), "[redacted]");
    }
}
