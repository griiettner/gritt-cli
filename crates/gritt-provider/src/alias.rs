//! Alias and deprecated-model resolution. Deterministic: a name resolves to
//! exactly one profile and model or fails with an actionable error. A
//! deprecated model remaps to the provider-declared replacement, else to an
//! explicit configured alias, else Gritt refuses (TKT-0008 plan).

use gritt_core::config::Config;
use gritt_core::{Error, Result};

use crate::models::ModelCatalog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRef {
    pub profile: String,
    pub model: String,
    /// Set when a deprecated id was remapped, naming the original.
    pub remapped_from: Option<String>,
}

fn split_qualified<'a>(config: &Config, name: &'a str) -> Option<(&'a str, &'a str)> {
    let (profile, model) = name.split_once('/')?;
    config
        .profiles
        .contains_key(profile)
        .then_some((profile, model))
}

/// Step one: turn a user-facing name into a profile and model id without
/// consulting the catalog.
fn resolve_name(
    config: &Config,
    name: &str,
    profile_hint: Option<&str>,
) -> Result<(String, String)> {
    if let Some((profile, model)) = split_qualified(config, name) {
        return Ok((profile.to_owned(), model.to_owned()));
    }
    if let Some(target) = config.aliases.get(name) {
        return match split_qualified(config, target) {
            Some((profile, model)) => Ok((profile.to_owned(), model.to_owned())),
            None => Err(Error::config(format!(
                "alias `{name}` maps to `{target}`, which is not `<profile>/<model>` for a configured profile"
            ))),
        };
    }
    let hits: Vec<(&String, &String)> = config
        .profiles
        .iter()
        .filter(|(profile, _)| profile_hint.is_none_or(|hint| hint == profile.as_str()))
        .filter_map(|(profile, definition)| definition.aliases.get(name).map(|m| (profile, m)))
        .collect();
    match hits.as_slice() {
        [(profile, model)] => return Ok(((*profile).clone(), (*model).clone())),
        [] => {}
        many => {
            let profiles: Vec<&str> = many.iter().map(|(p, _)| p.as_str()).collect();
            return Err(Error::config(format!(
                "alias `{name}` is defined in more than one profile ({}); use `<profile>/{name}`",
                profiles.join(", ")
            )));
        }
    }
    let profile = profile_hint
        .map(str::to_owned)
        .or_else(|| config.default_profile.clone())
        .ok_or_else(|| {
            Error::config(format!(
                "cannot resolve `{name}`: no profile given and no default_profile configured"
            ))
        })?;
    if !config.profiles.contains_key(&profile) {
        return Err(Error::config(format!("unknown profile `{profile}`")));
    }
    Ok((profile, name.to_owned()))
}

/// Resolves `name` and applies the deprecation policy against the catalog.
pub fn resolve(
    config: &Config,
    catalog: &ModelCatalog,
    name: &str,
    profile_hint: Option<&str>,
) -> Result<ModelRef> {
    let (profile, model) = resolve_name(config, name, profile_hint)?;
    apply_deprecation(config, catalog, profile, model)
}

/// Applies the deprecation policy to an already resolved profile and
/// model id: a model the catalog does not list or does not deprecate is
/// returned as is; a deprecated one remaps to the provider-declared
/// replacement, then to a configured alias on the profile or a global
/// alias into the same profile, else it is refused. Callers that take a
/// catalog id ahead of alias resolution use this so a deprecated id can
/// never be stored or sent unchanged.
pub fn apply_deprecation(
    config: &Config,
    catalog: &ModelCatalog,
    profile: String,
    model: String,
) -> Result<ModelRef> {
    let Some(info) = catalog.model(&profile, &model) else {
        return Ok(ModelRef {
            profile,
            model,
            remapped_from: None,
        });
    };
    if !info.deprecated {
        return Ok(ModelRef {
            profile,
            model,
            remapped_from: None,
        });
    }
    if let Some(replacement) = info.replaced_by {
        return Ok(ModelRef {
            profile,
            model: replacement,
            remapped_from: Some(model),
        });
    }
    let configured = config
        .profiles
        .get(&profile)
        .and_then(|p| p.aliases.get(&model).cloned())
        .or_else(|| {
            config
                .aliases
                .get(&model)
                .and_then(|target| split_qualified(config, target))
                .filter(|(target_profile, _)| *target_profile == profile)
                .map(|(_, target_model)| target_model.to_owned())
        });
    if let Some(target) = configured {
        return Ok(ModelRef {
            profile,
            model: target,
            remapped_from: Some(model),
        });
    }
    Err(Error::config(format!(
        "model `{model}` on profile `{profile}` is deprecated and the provider declares no replacement; \
         add `aliases.{model} = \"<new model id>\"` to the `{profile}` profile or choose another model"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gritt_core::provider::{ModelInfo, ModelList, ModelListStatus, Protocol, ProviderProfile};
    use gritt_core::secret::SecretRef;

    fn profile(name: &str, aliases: &[(&str, &str)]) -> ProviderProfile {
        ProviderProfile {
            name: name.into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://example.test/v1".into(),
            key: SecretRef::for_profile(name, "KEY"),
            aliases: aliases
                .iter()
                .map(|(a, m)| (a.to_string(), m.to_string()))
                .collect(),
        }
    }

    fn config() -> Config {
        let mut config = Config::default();
        config.profiles.insert(
            "openrouter".into(),
            profile("openrouter", &[("fast", "or/fast"), ("old", "or/new")]),
        );
        config
            .profiles
            .insert("openai".into(), profile("openai", &[("fast", "gpt-fast")]));
        config
            .aliases
            .insert("smart".into(), "openai/gpt-smart".into());
        config.default_profile = Some("openrouter".into());
        config
    }

    fn catalog() -> ModelCatalog {
        let catalog = ModelCatalog::default();
        let model = |id: &str, deprecated: bool, replaced_by: Option<&str>| ModelInfo {
            id: id.into(),
            display_name: None,
            capabilities: Default::default(),
            replaced_by: replaced_by.map(str::to_owned),
            deprecated,
        };
        catalog.insert(ModelList {
            profile: "openrouter".into(),
            status: ModelListStatus::Fresh {
                fetched_at: chrono::Utc::now(),
            },
            models: vec![
                model("or/fast", false, None),
                model("or/legacy", true, Some("or/modern")),
                model("old", true, None),
                model("gone", true, None),
            ],
        });
        catalog
    }

    #[test]
    fn qualified_global_and_profile_aliases_resolve() {
        let config = config();
        let catalog = catalog();
        assert_eq!(
            resolve(&config, &catalog, "openai/gpt-x", None)
                .unwrap()
                .model,
            "gpt-x"
        );
        let smart = resolve(&config, &catalog, "smart", None).unwrap();
        assert_eq!(
            (smart.profile.as_str(), smart.model.as_str()),
            ("openai", "gpt-smart")
        );
        let fast = resolve(&config, &catalog, "fast", Some("openai")).unwrap();
        assert_eq!(fast.model, "gpt-fast");
        let bare = resolve(&config, &catalog, "or/fast", None).unwrap();
        assert_eq!(bare.profile, "openrouter");
    }

    #[test]
    fn ambiguous_alias_is_refused() {
        let error = resolve(&config(), &catalog(), "fast", None).unwrap_err();
        assert!(error.message.contains("more than one profile"));
    }

    #[test]
    fn deprecated_models_remap_provider_first_then_config_then_refuse() {
        let config = config();
        let catalog = catalog();
        let provider = resolve(&config, &catalog, "or/legacy", None).unwrap();
        assert_eq!(provider.model, "or/modern");
        assert_eq!(provider.remapped_from.as_deref(), Some("or/legacy"));
        let configured = resolve(&config, &catalog, "old", None).unwrap();
        assert_eq!(configured.model, "or/new");
        let error = resolve(&config, &catalog, "gone", None).unwrap_err();
        assert!(error.message.contains("deprecated"));
        assert!(error.message.contains("aliases.gone"));
    }
}
