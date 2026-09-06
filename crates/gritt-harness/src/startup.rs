//! Startup resolution for a new native session (TKT-0022). One resolver
//! serves print, REPL, and the full-screen mode: it picks the profile,
//! model, and effort a new session starts on, tries the candidate profiles
//! in the configured fallback order, and reports every skipped profile
//! without a key value. Resumed sessions never come here: they keep the
//! profile and model their transcript was produced under.
//!
//! Precedence for each choice, highest first: an explicit flag or picker
//! selection, the last successful new session's choice, then the
//! configured default. A profile the user chose is pinned, so startup does
//! not fail over away from it; only a remembered or configured profile
//! starts a chain. On the command line a model name that spells out a
//! profile (a qualified name or a global alias) wins over the `--profile`
//! hint, as it always did; a picker's choice is not overridden.
//!
//! With more than one candidate the checks are strict: credentials, a live
//! model-list fetch, and the requested model's presence in that list all
//! have to pass, and a failure moves to the next profile. With a single
//! candidate the checks are exactly the ones Gritt applied before failover
//! existed, so a configuration without `fallback_profiles` behaves as it
//! did.

use chrono::Utc;
use gritt_core::provider::ReasoningEffort;
use gritt_core::secret::Secret;
use gritt_core::session::LastUsedNative;
use gritt_core::{Error, ErrorKind, Result};
use gritt_provider::adapter::{redact_text, CapabilitySource, StaticKey};
use gritt_provider::alias;
use gritt_provider::effort::{effort_support, EffortSupport};
use gritt_provider::models::probe_models;
use serde::{Deserialize, Serialize};

use crate::agent::AgentBuilder;
use crate::draft::{CatalogState, DraftError, DraftWarning, SessionDraft};

/// What the caller asked for. Every field is optional; see the module
/// documentation for how gaps are filled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupRequest {
    pub profile: Option<String>,
    /// `profile` is the user's own choice, which pins it.
    pub pinned: bool,
    /// `profile` is a hint a qualified model name may override, as
    /// `--profile` always was on the command line. A picker's choice is
    /// not: the full-screen mode rejects a model from another profile so
    /// it can clear the selection instead.
    pub profile_is_hint: bool,
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
}

impl StartupRequest {
    /// Command-line flags. A `--profile` is always the user's choice.
    pub fn from_flags(
        profile: Option<&str>,
        model: Option<&str>,
        effort: Option<ReasoningEffort>,
    ) -> Self {
        Self {
            profile: profile.map(str::to_owned),
            pinned: profile.is_some(),
            profile_is_hint: true,
            model: model.map(str::to_owned),
            effort,
        }
    }

    /// An interface draft. The draft records whether its profile was
    /// chosen or seeded.
    pub fn from_draft(draft: &SessionDraft) -> Self {
        Self {
            profile: draft.profile.clone(),
            pinned: draft.explicit_profile,
            profile_is_hint: false,
            model: draft.model.clone(),
            effort: draft.effort,
        }
    }
}

/// Why a candidate profile was skipped. Typed so an interface can group
/// or colour the report; the label is the wording every mode shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    MissingCredentials,
    Authentication,
    Connection,
    Provider,
    ModelUnavailable,
    EffortUnsupported,
}

impl FailureClass {
    pub fn label(self) -> &'static str {
        match self {
            FailureClass::MissingCredentials => "missing credentials",
            FailureClass::Authentication => "authentication failed",
            FailureClass::Connection => "connection failed",
            FailureClass::Provider => "provider error",
            FailureClass::ModelUnavailable => "model unavailable",
            FailureClass::EffortUnsupported => "effort unsupported",
        }
    }
}

/// Sorts an error into the class the fallback chain acts on. A provider
/// status of 401 or 403 is an authentication failure; any other status is
/// a provider error; no status at all means the request never got an
/// answer.
pub fn classify(error: &Error) -> FailureClass {
    match error.kind {
        ErrorKind::MissingKey => FailureClass::MissingCredentials,
        ErrorKind::Provider => match error
            .diagnostic
            .as_ref()
            .and_then(|diagnostic| diagnostic.get("status"))
            .and_then(|status| status.as_u64())
        {
            Some(401 | 403) => FailureClass::Authentication,
            Some(_) => FailureClass::Provider,
            None => FailureClass::Connection,
        },
        ErrorKind::StaleModelList | ErrorKind::MissingModelList => FailureClass::Connection,
        ErrorKind::UnsupportedCapability => FailureClass::EffortUnsupported,
        _ => FailureClass::Provider,
    }
}

/// One profile the chain moved past.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedProfile {
    pub profile: String,
    pub class: FailureClass,
    /// One line, key-redacted, never a prompt or a response body.
    pub message: String,
}

impl std::fmt::Display for SkippedProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}: {})",
            self.profile,
            self.class.label(),
            self.message
        )
    }
}

/// The choices a new session starts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupSelection {
    pub profile: String,
    pub model: String,
    pub effort: ReasoningEffort,
    pub catalog: CatalogState,
    /// Skipped profiles first, in the order tried, then the notes about
    /// the selected profile.
    pub warnings: Vec<DraftWarning>,
}

/// The result of resolving a startup request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupOutcome {
    Ready(StartupSelection),
    Rejected {
        errors: Vec<DraftError>,
        catalog: Option<CatalogState>,
    },
}

/// Where the primary profile came from, which decides whether it is
/// pinned and whether it counts as a remembered choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimarySource {
    /// The request named it. Pinned when the request says so.
    Requested { pinned: bool },
    /// A qualified model name or global alias spelled it out. Pinned.
    ModelNamed,
    /// The configured default profile.
    Configured,
    /// The last successful session's profile.
    LastUsed,
}

impl PrimarySource {
    pub fn is_pinned(self) -> bool {
        matches!(
            self,
            PrimarySource::Requested { pinned: true } | PrimarySource::ModelNamed
        )
    }
}

/// Where a candidate model name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelSource {
    Requested,
    ProfileFallback,
    Configured,
    LastUsed,
}

struct Choice {
    model: String,
    effort: ReasoningEffort,
    catalog: CatalogState,
    warnings: Vec<DraftWarning>,
}

struct Skip {
    class: FailureClass,
    message: String,
    /// The typed rejection for a single-candidate chain. Strict-only
    /// failures carry none, because they are reported in aggregate.
    error: Option<DraftError>,
    catalog: Option<CatalogState>,
}

impl Skip {
    fn typed(error: DraftError, class: FailureClass, catalog: Option<CatalogState>) -> Self {
        Self {
            class,
            message: error.to_string(),
            error: Some(error),
            catalog,
        }
    }
}

impl AgentBuilder {
    /// The choices of the last successful new session in this workspace.
    pub async fn last_used(&self) -> Result<Option<LastUsedNative>> {
        self.store.last_used(self.workspace.root()).await
    }

    /// The profile a new session starts on before any failover, with
    /// where it came from: the profile a requested model name spells out,
    /// an explicit profile, the last successful session's profile while it
    /// is still configured, the profile a qualified `default_model` spells
    /// out, or the configured default. `None` when nothing names one.
    pub fn primary_profile(
        &self,
        request: &StartupRequest,
        last_used: Option<&LastUsedNative>,
    ) -> Option<(String, PrimarySource)> {
        // A qualified model name or global alias names its own profile and
        // outranks a `--profile` hint, as alias resolution always did. It
        // does not outrank a picker's choice, and an id the hinted
        // profile's list already contains (OpenRouter's `openai/gpt-5-nano`
        // next to an `openai` profile) is that profile's model, not a
        // qualified name.
        let overridable = request.profile.is_none() || request.profile_is_hint;
        let named = request.model.as_deref().and_then(|name| {
            let listed_under_hint = request
                .profile
                .as_deref()
                .is_some_and(|hint| self.catalog.model(hint, name).is_some());
            if !overridable || listed_under_hint {
                None
            } else {
                alias::named_profile(&self.config, name)
            }
        });
        if let Some(profile) = named {
            return Some((profile, PrimarySource::ModelNamed));
        }
        if let Some(profile) = &request.profile {
            return Some((
                profile.clone(),
                PrimarySource::Requested {
                    pinned: request.pinned,
                },
            ));
        }
        // A remembered profile that was removed from the configuration is
        // simply forgotten; it never blocks the chain.
        if let Some(last) =
            last_used.filter(|last| self.config.profiles.contains_key(&last.provider_profile))
        {
            return Some((last.provider_profile.clone(), PrimarySource::LastUsed));
        }
        // A qualified `default_model` names its profile, as it did before
        // failover existed, and outranks `default_profile`.
        if let Some(profile) = self
            .config
            .default_model
            .as_deref()
            .and_then(|name| alias::named_profile(&self.config, name))
        {
            return Some((profile, PrimarySource::Configured));
        }
        self.config
            .default_profile
            .clone()
            .map(|profile| (profile, PrimarySource::Configured))
    }

    /// Resolves a startup request against the configuration, the key
    /// sources, the provider endpoints, and the remembered choices.
    /// Storage failures are `Err`; everything a user can fix is a typed
    /// [`StartupOutcome::Rejected`].
    pub async fn resolve_startup(&self, request: &StartupRequest) -> Result<StartupOutcome> {
        let last_used = self.last_used().await?;
        let primary = self.primary_profile(request, last_used.as_ref());
        let primary_from_last = matches!(primary, Some((_, PrimarySource::LastUsed)));
        let mut candidates: Vec<String> = Vec::new();
        let mut pinned = false;
        if let Some((profile, source)) = primary {
            if !self.config.profiles.contains_key(&profile) {
                return Ok(StartupOutcome::Rejected {
                    errors: vec![DraftError::UnknownProfile { profile }],
                    catalog: None,
                });
            }
            candidates.push(profile);
            pinned = source.is_pinned();
        }
        if !pinned {
            for profile in self.config.fallback_order()? {
                if !candidates.contains(&profile) {
                    candidates.push(profile);
                }
            }
        }
        if candidates.is_empty() {
            return Ok(StartupOutcome::Rejected {
                errors: vec![DraftError::MissingProfile],
                catalog: None,
            });
        }
        let strict = candidates.len() > 1;
        let mut skipped: Vec<SkippedProfile> = Vec::new();
        for (index, profile) in candidates.iter().enumerate() {
            let is_primary = index == 0;
            let attempt = self
                .try_profile(
                    profile,
                    is_primary,
                    strict,
                    request,
                    last_used.as_ref(),
                    is_primary && primary_from_last,
                )
                .await?;
            match attempt {
                Ok(choice) => {
                    let mut warnings: Vec<DraftWarning> = skipped
                        .into_iter()
                        .map(DraftWarning::ProfileSkipped)
                        .collect();
                    warnings.extend(choice.warnings);
                    return Ok(StartupOutcome::Ready(StartupSelection {
                        profile: profile.clone(),
                        model: choice.model,
                        effort: choice.effort,
                        catalog: choice.catalog,
                        warnings,
                    }));
                }
                Err(skip) => {
                    let entry = SkippedProfile {
                        profile: profile.clone(),
                        class: skip.class,
                        message: skip.message,
                    };
                    if !strict {
                        let error = skip.error.unwrap_or_else(|| DraftError::NoUsableProfile {
                            skipped: vec![entry],
                        });
                        return Ok(StartupOutcome::Rejected {
                            errors: vec![error],
                            catalog: skip.catalog,
                        });
                    }
                    skipped.push(entry);
                }
            }
        }
        Ok(StartupOutcome::Rejected {
            errors: vec![DraftError::NoUsableProfile { skipped }],
            catalog: None,
        })
    }

    /// One candidate: credentials, endpoint, model, then effort.
    async fn try_profile(
        &self,
        profile: &str,
        is_primary: bool,
        strict: bool,
        request: &StartupRequest,
        last_used: Option<&LastUsedNative>,
        profile_from_last: bool,
    ) -> Result<std::result::Result<Choice, Skip>> {
        let Some(definition) = self.config.profiles.get(profile) else {
            return Ok(Err(Skip::typed(
                DraftError::UnknownProfile {
                    profile: profile.to_owned(),
                },
                FailureClass::Provider,
                None,
            )));
        };
        // Credentials. In a chain a missing key skips the profile. A single
        // candidate opens as it always did and the adapter reports the
        // missing key on the first request, so nothing changes for a
        // configuration without failover. The value itself is used only to
        // redact the messages produced below.
        let key = match self.keys.key(profile, &definition.key) {
            Ok(key) => Some(key),
            Err(error) if strict => {
                return Ok(Err(Skip {
                    class: FailureClass::MissingCredentials,
                    message: error.message,
                    error: None,
                    catalog: None,
                }));
            }
            Err(_) => None,
        };
        let secrets: Vec<Secret> = key.iter().cloned().collect();
        // Endpoint. A chain probes live, with the key it already resolved
        // rather than a second keychain lookup; a single candidate loads
        // the list the way it always did, because with nowhere to fall
        // over to the probe would only move the same failure earlier.
        let catalog = match (&self.cache, &key) {
            (Some(cache), Some(key)) if strict => {
                match probe_models(
                    cache,
                    self.transport.as_ref(),
                    &StaticKey(key.clone()),
                    definition,
                    Utc::now(),
                )
                .await
                {
                    Ok(list) => {
                        let fetched_at = match list.status {
                            gritt_core::provider::ModelListStatus::Fresh { fetched_at }
                            | gritt_core::provider::ModelListStatus::Stale { fetched_at } => {
                                fetched_at
                            }
                        };
                        self.catalog.insert(list);
                        CatalogState::Fresh { fetched_at }
                    }
                    Err(error) => {
                        return Ok(Err(Skip {
                            class: classify(&error),
                            message: redact_text(&error.message, &secrets),
                            error: None,
                            catalog: None,
                        }));
                    }
                }
            }
            _ => self.warm_catalog(profile).await?,
        };
        // Model. The primary takes the requested name, else the remembered
        // model when it belongs here, else the configured default. A
        // fallback profile tries the requested name, then its own fallback
        // model, then the remembered model, then the configured default,
        // and must find whatever it settles on in its list.
        let remembered = last_used
            .filter(|last| last.provider_profile == profile)
            .map(|last| last.model.clone());
        let mut names: Vec<(String, ModelSource)> = Vec::new();
        let mut push = |name: Option<&String>, source: ModelSource| {
            if let Some(name) = name {
                if !names.iter().any(|(seen, _)| seen == name) {
                    names.push((name.clone(), source));
                }
            }
        };
        push(request.model.as_ref(), ModelSource::Requested);
        if !is_primary {
            push(
                definition.fallback_model.as_ref(),
                ModelSource::ProfileFallback,
            );
        }
        push(remembered.as_ref(), ModelSource::LastUsed);
        push(self.config.default_model.as_ref(), ModelSource::Configured);
        if names.is_empty() {
            return Ok(Err(Skip::typed(
                DraftError::MissingModel,
                FailureClass::ModelUnavailable,
                Some(catalog),
            )));
        }
        let mut warnings = Vec::new();
        let mut rejected: Vec<String> = Vec::new();
        let mut chosen: Option<(alias::ModelRef, ModelSource)> = None;
        for (name, source) in names {
            let resolved = match self.resolve_under_profile(profile, &name) {
                Ok(resolved) => resolved,
                Err(error) => {
                    if is_primary {
                        return Ok(Err(Skip::typed(
                            DraftError::ModelResolution {
                                model: name,
                                message: error.message,
                            },
                            FailureClass::ModelUnavailable,
                            Some(catalog),
                        )));
                    }
                    rejected.push(format!("`{name}`: {}", error.message));
                    continue;
                }
            };
            if resolved.profile != profile {
                if is_primary {
                    return Ok(Err(Skip::typed(
                        DraftError::ModelOutsideProfile {
                            model: name,
                            model_profile: resolved.profile,
                            profile: profile.to_owned(),
                        },
                        FailureClass::ModelUnavailable,
                        Some(catalog),
                    )));
                }
                rejected.push(format!(
                    "`{name}` belongs to profile `{}`",
                    resolved.profile
                ));
                continue;
            }
            let listed = self.catalog.model(profile, &resolved.model).is_some();
            if catalog.has_list() && !listed {
                if is_primary && !strict {
                    // The rule from before failover: a typed model the
                    // list lacks is allowed with its capabilities
                    // unreported.
                    warnings.push(DraftWarning::ModelNotInCatalog {
                        profile: profile.to_owned(),
                        model: resolved.model.clone(),
                    });
                    chosen = Some((resolved, source));
                    break;
                }
                if is_primary {
                    return Ok(Err(Skip {
                        class: FailureClass::ModelUnavailable,
                        message: format!(
                            "`{}` is not in the model list for `{profile}`",
                            resolved.model
                        ),
                        error: None,
                        catalog: Some(catalog),
                    }));
                }
                rejected.push(format!("`{}` is not in the model list", resolved.model));
                continue;
            }
            if !catalog.has_list() && !is_primary && source != ModelSource::ProfileFallback {
                rejected.push(format!(
                    "`{}` cannot be checked without a model list; set fallback_model on `{profile}`",
                    resolved.model
                ));
                continue;
            }
            chosen = Some((resolved, source));
            break;
        }
        let Some((resolved, source)) = chosen else {
            return Ok(Err(Skip {
                class: FailureClass::ModelUnavailable,
                message: format!("no compatible model: {}", rejected.join(", ")),
                error: None,
                catalog: Some(catalog),
            }));
        };
        if let Some(from) = &resolved.remapped_from {
            warnings.push(DraftWarning::DeprecatedModelRemapped {
                from: from.clone(),
                to: resolved.model.clone(),
            });
        }
        // Effort. A remembered level the model cannot take falls back to
        // the provider default; a requested one is refused as before.
        let (mut effort, effort_from_last) = match request.effort {
            Some(effort) => (effort, false),
            None => match last_used {
                Some(last) if last.effort.is_explicit() => (last.effort, true),
                _ => (ReasoningEffort::Auto, false),
            },
        };
        if let Some(error) = self.effort_error(profile, &resolved.model, effort) {
            if effort_from_last {
                if let DraftError::EffortUnsupported { reason, .. } = &error {
                    warnings.push(DraftWarning::EffortReset {
                        effort,
                        profile: profile.to_owned(),
                        model: resolved.model.clone(),
                        reason: reason.clone(),
                    });
                }
                effort = ReasoningEffort::Auto;
            } else {
                return Ok(Err(Skip::typed(
                    error,
                    FailureClass::EffortUnsupported,
                    Some(catalog),
                )));
            }
        }
        let effort_applied = effort_from_last && effort.is_explicit();
        let model_from_last = source == ModelSource::LastUsed;
        if profile_from_last || model_from_last || effort_applied {
            warnings.push(DraftWarning::LastUsedApplied {
                profile: profile_from_last.then(|| profile.to_owned()),
                model: model_from_last.then(|| resolved.model.clone()),
                effort: effort_applied.then_some(effort),
            });
        }
        Ok(Ok(Choice {
            model: resolved.model,
            effort,
            catalog,
            warnings,
        }))
    }

    /// Loads a known profile's model list the way it always was loaded,
    /// refreshing at most once per interval, and reports its state
    /// without exposing the provider body. An unknown profile is a config
    /// error.
    pub async fn warm_catalog(&self, profile: &str) -> Result<CatalogState> {
        if !self.config.profiles.contains_key(profile) {
            return Err(DraftError::UnknownProfile {
                profile: profile.to_owned(),
            }
            .into_error());
        }
        if self.cache.is_none() {
            return Ok(self.catalog_state(profile));
        }
        match self.load_catalog(profile).await? {
            None => Ok(self.catalog_state(profile)),
            Some(error) if error.kind == ErrorKind::MissingModelList => Ok(CatalogState::Missing {
                reason: error.message,
            }),
            Some(error) => Ok(CatalogState::RefreshFailed {
                reason: error.message,
            }),
        }
    }

    /// What the in-memory catalog holds for a profile.
    pub fn catalog_state(&self, profile: &str) -> CatalogState {
        match self.catalog.status(profile) {
            Some(gritt_core::provider::ModelListStatus::Fresh { fetched_at }) => {
                CatalogState::Fresh { fetched_at }
            }
            Some(gritt_core::provider::ModelListStatus::Stale { fetched_at }) => {
                CatalogState::Stale { fetched_at }
            }
            None => CatalogState::Skipped,
        }
    }

    /// Resolves a model name under the selected profile. An id the
    /// profile's catalog lists is taken as that model before any alias or
    /// `profile/model` reading, because catalog ids such as OpenRouter's
    /// `openai/gpt-5-nano` share the qualified-name shape whenever a
    /// profile of the same name is configured. The deprecation policy
    /// still applies to it: a declared or configured replacement is used
    /// and a deprecated id with neither is refused. Anything else goes
    /// through alias resolution with the profile as the hint.
    pub fn resolve_under_profile(&self, profile: &str, name: &str) -> Result<alias::ModelRef> {
        let config = &self.config;
        let catalog = &self.catalog;
        if catalog.model(profile, name).is_some() {
            return alias::apply_deprecation(config, catalog, profile.to_owned(), name.to_owned());
        }
        alias::resolve(config, catalog, name, Some(profile))
    }

    /// The same rule the adapter applies before a request.
    pub fn effort_error(
        &self,
        profile: &str,
        model: &str,
        effort: ReasoningEffort,
    ) -> Option<DraftError> {
        let protocol = self.config.profiles.get(profile)?.protocol;
        let capabilities = self.catalog.capabilities(profile, model);
        match effort_support(protocol, capabilities.as_ref(), effort) {
            EffortSupport::Supported => None,
            EffortSupport::Unsupported(reason) => Some(DraftError::EffortUnsupported {
                profile: profile.to_owned(),
                model: model.to_owned(),
                effort,
                reason,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_classify_by_kind_and_status() {
        assert_eq!(
            classify(&Error::missing_key("p", "P_KEY")),
            FailureClass::MissingCredentials
        );
        let status = |code: u16| {
            Error::provider(Some(code), "x")
                .with_diagnostic(serde_json::json!({ "status": code, "body": {} }))
        };
        assert_eq!(classify(&status(401)), FailureClass::Authentication);
        assert_eq!(classify(&status(403)), FailureClass::Authentication);
        assert_eq!(classify(&status(500)), FailureClass::Provider);
        assert_eq!(classify(&status(404)), FailureClass::Provider);
        assert_eq!(
            classify(&Error::provider(None, "connection refused")),
            FailureClass::Connection
        );
        assert_eq!(
            classify(&Error::new(ErrorKind::MissingModelList, "x")),
            FailureClass::Connection
        );
        assert_eq!(classify(&Error::config("x")), FailureClass::Provider);
        assert_eq!(
            SkippedProfile {
                profile: "openrouter".into(),
                class: FailureClass::Connection,
                message: "connection refused".into(),
            }
            .to_string(),
            "openrouter (connection failed: connection refused)"
        );
    }

    #[test]
    fn requests_from_flags_pin_an_explicit_profile_and_drafts_carry_their_flag() {
        let flags = StartupRequest::from_flags(Some("openrouter"), None, None);
        assert!(flags.pinned);
        assert!(!StartupRequest::from_flags(None, Some("m"), None).pinned);
        let seeded = SessionDraft {
            profile: Some("openrouter".into()),
            ..SessionDraft::default()
        };
        assert!(!StartupRequest::from_draft(&seeded).pinned);
        let chosen = SessionDraft::default().with_profile("openrouter");
        assert!(StartupRequest::from_draft(&chosen).pinned);
    }
}
