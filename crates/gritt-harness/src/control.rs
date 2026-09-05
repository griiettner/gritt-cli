//! The control plane: picks the backend for a session and hands the modes
//! one [`Driver`] whichever path produced it. The native connector is the
//! first connector; external ones are optional and a missing one only
//! fails its own sessions. It also validates and opens session drafts and
//! reports profile and catalog state for the setup flow.

use std::sync::Arc;

use chrono::Utc;
use gritt_core::connector::{AuthState, Connector, ConnectorId, ConnectorInfo, Transport};
use gritt_core::provider::{ModelInfo, ModelListStatus, Protocol, ReasoningEffort};
use gritt_core::secret::Secret;
use gritt_core::session::{Phase, Session, SessionId, SessionKind, SessionStore};
use gritt_core::{Error, ErrorKind, Result};
use gritt_provider::adapter::CapabilitySource;
use gritt_provider::alias;
use gritt_provider::effort::{effort_support, EffortSupport};

use crate::agent::{AgentBuilder, SessionSelector};
use crate::connector_session::ConnectorSession;
use crate::draft::{
    CatalogState, DraftError, DraftOutcome, DraftWarning, ResolvedDraft, SessionDraft,
};
use crate::driver::Driver;
use crate::native_connector::NativeConnector;
use crate::setup::{CredentialState, ProfileSummary, ProviderSetup, ReadOnlySetup};

/// A profile's model list as the model picker sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileCatalog {
    pub profile: String,
    pub protocol: Protocol,
    pub state: CatalogState,
    pub models: Vec<ModelInfo>,
}

/// What opening a draft did.
pub enum DraftOpen {
    Opened {
        driver: Box<dyn Driver>,
        catalog: CatalogState,
        warnings: Vec<DraftWarning>,
    },
    Rejected {
        errors: Vec<DraftError>,
        catalog: Option<CatalogState>,
    },
}

pub struct ControlPlane {
    pub builder: Arc<AgentBuilder>,
    native: Arc<NativeConnector>,
    external: Vec<Arc<dyn Connector>>,
    setup: Arc<dyn ProviderSetup>,
}

impl ControlPlane {
    pub fn new(builder: Arc<AgentBuilder>, external: Vec<Arc<dyn Connector>>) -> Self {
        let native = Arc::new(NativeConnector::new(Arc::clone(&builder)));
        Self {
            builder,
            native,
            external,
            setup: Arc::new(ReadOnlySetup),
        }
    }

    /// Injects the binary's config and keychain writer (ADR-006).
    pub fn with_setup(mut self, setup: Arc<dyn ProviderSetup>) -> Self {
        self.setup = setup;
        self
    }

    pub fn setup(&self) -> &Arc<dyn ProviderSetup> {
        &self.setup
    }

    /// Every configured profile with credential availability, never a
    /// value.
    pub fn profile_summaries(&self) -> Vec<ProfileSummary> {
        let config = &self.builder.config;
        config
            .profiles
            .iter()
            .map(|(name, profile)| ProfileSummary {
                name: name.clone(),
                protocol: profile.protocol,
                base_url: profile.base_url.clone(),
                credential: match self.builder.keys.key(name, &profile.key) {
                    Ok(_) => CredentialState::Available,
                    Err(_) => CredentialState::Missing {
                        env_var_name: profile.key.env_var_name.clone(),
                    },
                },
                is_default: config.default_profile.as_deref() == Some(name.as_str()),
            })
            .collect()
    }

    /// Warms a known profile's model list and reports its state without
    /// exposing the provider body. An unknown profile is a config error.
    pub async fn warm_catalog(&self, profile: &str) -> Result<CatalogState> {
        if !self.builder.config.profiles.contains_key(profile) {
            return Err(Error::config(format!("unknown profile `{profile}`")));
        }
        let from_status = |status: Option<ModelListStatus>| match status {
            Some(ModelListStatus::Fresh { fetched_at }) => CatalogState::Fresh { fetched_at },
            Some(ModelListStatus::Stale { fetched_at }) => CatalogState::Stale { fetched_at },
            None => CatalogState::Skipped,
        };
        if self.builder.cache.is_none() {
            return Ok(from_status(self.builder.catalog.status(profile)));
        }
        match self.builder.load_catalog(profile).await? {
            None => Ok(from_status(self.builder.catalog.status(profile))),
            Some(error) if error.kind == ErrorKind::MissingModelList => Ok(CatalogState::Missing {
                reason: error.message,
            }),
            Some(error) => Ok(CatalogState::RefreshFailed {
                reason: error.message,
            }),
        }
    }

    /// Warms the profile's list and returns it for the model picker.
    pub async fn catalog(&self, profile: &str) -> Result<ProfileCatalog> {
        let state = self.warm_catalog(profile).await?;
        let protocol = self.builder.config.profiles[profile].protocol;
        let models = self
            .builder
            .catalog
            .list(profile)
            .map(|list| list.models)
            .unwrap_or_default();
        Ok(ProfileCatalog {
            profile: profile.to_owned(),
            protocol,
            state,
            models,
        })
    }

    /// Validates a draft: profile, model, and effort together, after
    /// warming the profile's catalog so model resolution and effort
    /// support use the provider's data. Storage failures are `Err`;
    /// everything a user can fix is a typed [`DraftOutcome::Rejected`].
    pub async fn validate_draft(&self, draft: &SessionDraft) -> Result<DraftOutcome> {
        if let Some(name) = &draft.name {
            if let Some(session) = self.builder.store.find_by_name(name).await? {
                return self.validate_resume(draft, session).await;
            }
        }
        self.validate_new(draft).await
    }

    async fn validate_resume(
        &self,
        draft: &SessionDraft,
        session: Session,
    ) -> Result<DraftOutcome> {
        let rejected = |error: DraftError| {
            Ok(DraftOutcome::Rejected {
                errors: vec![error],
                catalog: None,
            })
        };
        let recorded = session
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| session.workspace.clone());
        if recorded != self.builder.workspace_root() {
            return rejected(DraftError::OtherWorkspace {
                name: session.name,
                workspace: session.workspace,
            });
        }
        let SessionKind::Native {
            provider_profile,
            model,
            effort: stored_effort,
        } = &session.kind
        else {
            let SessionKind::Connector { id } = session.kind else {
                unreachable!("session kinds are native or connector");
            };
            return rejected(DraftError::ConnectorSession {
                name: session.name,
                connector: id,
            });
        };
        if !self.builder.config.profiles.contains_key(provider_profile) {
            return rejected(DraftError::UnknownProfile {
                profile: provider_profile.clone(),
            });
        }
        // Warmed before resolution so the answer does not depend on
        // whether a picker loaded the list earlier.
        let catalog = self.warm_catalog(provider_profile).await?;
        let profile_mismatch = draft
            .profile
            .as_deref()
            .is_some_and(|wanted| wanted != provider_profile);
        let model_mismatch = match draft.model.as_deref() {
            None => false,
            Some(wanted) if wanted == model => false,
            Some(wanted) => match self.resolve_under_profile(provider_profile, wanted) {
                Ok(resolved) => resolved.profile != *provider_profile || resolved.model != *model,
                Err(_) => true,
            },
        };
        if profile_mismatch || model_mismatch {
            return Ok(DraftOutcome::Rejected {
                errors: vec![DraftError::SessionPinned {
                    name: session.name,
                    profile: provider_profile.clone(),
                    model: model.clone(),
                    requested_profile: draft.profile.clone(),
                    requested_model: draft.model.clone(),
                }],
                catalog: Some(catalog),
            });
        }
        let effort = draft.effort.unwrap_or(*stored_effort);
        if let Some(error) = self.effort_error(provider_profile, model, effort) {
            return Ok(DraftOutcome::Rejected {
                errors: vec![error],
                catalog: Some(catalog),
            });
        }
        Ok(DraftOutcome::Ready {
            draft: ResolvedDraft {
                name: Some(session.name.clone()),
                resume: Some(session.id.clone()),
                profile: provider_profile.clone(),
                model: model.clone(),
                effort,
                phase: draft.phase.unwrap_or(session.phase),
            },
            catalog,
            warnings: Vec::new(),
        })
    }

    async fn validate_new(&self, draft: &SessionDraft) -> Result<DraftOutcome> {
        let config = &self.builder.config;
        let Some(profile) = draft
            .profile
            .clone()
            .or_else(|| config.default_profile.clone())
        else {
            return Ok(DraftOutcome::Rejected {
                errors: vec![DraftError::MissingProfile],
                catalog: None,
            });
        };
        if !config.profiles.contains_key(&profile) {
            return Ok(DraftOutcome::Rejected {
                errors: vec![DraftError::UnknownProfile { profile }],
                catalog: None,
            });
        }
        let catalog = self.warm_catalog(&profile).await?;
        let rejected = |error: DraftError, catalog: CatalogState| {
            Ok(DraftOutcome::Rejected {
                errors: vec![error],
                catalog: Some(catalog),
            })
        };
        let Some(model_name) = draft.model.clone().or_else(|| config.default_model.clone()) else {
            return rejected(DraftError::MissingModel, catalog);
        };
        let resolved = match self.resolve_under_profile(&profile, &model_name) {
            Ok(resolved) => resolved,
            Err(error) => {
                return rejected(
                    DraftError::ModelResolution {
                        model: model_name,
                        message: error.message,
                    },
                    catalog,
                );
            }
        };
        if resolved.profile != profile {
            return rejected(
                DraftError::ModelOutsideProfile {
                    model: model_name,
                    model_profile: resolved.profile,
                    profile,
                },
                catalog,
            );
        }
        let mut warnings = Vec::new();
        if let Some(from) = &resolved.remapped_from {
            warnings.push(DraftWarning::DeprecatedModelRemapped {
                from: from.clone(),
                to: resolved.model.clone(),
            });
        }
        if catalog.has_list()
            && self
                .builder
                .catalog
                .model(&profile, &resolved.model)
                .is_none()
        {
            warnings.push(DraftWarning::ModelNotInCatalog {
                profile: profile.clone(),
                model: resolved.model.clone(),
            });
        }
        let effort = draft.effort.unwrap_or_default();
        if let Some(error) = self.effort_error(&profile, &resolved.model, effort) {
            return rejected(error, catalog);
        }
        Ok(DraftOutcome::Ready {
            draft: ResolvedDraft {
                name: draft.name.clone(),
                resume: None,
                profile,
                model: resolved.model,
                effort,
                phase: draft.phase.unwrap_or(Phase::Planning),
            },
            catalog,
            warnings,
        })
    }

    /// Resolves a model name under the selected profile. An id the
    /// profile's catalog lists is taken as that model before any alias or
    /// `profile/model` reading, because catalog ids such as OpenRouter's
    /// `openai/gpt-5-nano` share the qualified-name shape whenever a
    /// profile of the same name is configured. The deprecation policy
    /// still applies to it: a declared or configured replacement is used
    /// and a deprecated id with neither is refused. Anything else goes
    /// through alias resolution with the profile as the hint.
    fn resolve_under_profile(&self, profile: &str, name: &str) -> Result<alias::ModelRef> {
        let config = &self.builder.config;
        let catalog = &self.builder.catalog;
        if catalog.model(profile, name).is_some() {
            return alias::apply_deprecation(config, catalog, profile.to_owned(), name.to_owned());
        }
        alias::resolve(config, catalog, name, Some(profile))
    }

    /// The same rule the adapter applies before a request.
    fn effort_error(
        &self,
        profile: &str,
        model: &str,
        effort: ReasoningEffort,
    ) -> Option<DraftError> {
        let protocol = self.builder.config.profiles.get(profile)?.protocol;
        let capabilities = self.builder.catalog.capabilities(profile, model);
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

    /// Validates the draft and, when it is ready, opens the session: a
    /// resumed one keeps its pinned profile and model and takes the
    /// draft's effort and phase; a new one is created from the resolved
    /// choices.
    pub async fn open_draft(&self, draft: SessionDraft) -> Result<DraftOpen> {
        let (resolved, catalog, warnings) = match self.validate_draft(&draft).await? {
            DraftOutcome::Ready {
                draft,
                catalog,
                warnings,
            } => (draft, catalog, warnings),
            DraftOutcome::Rejected { errors, catalog } => {
                return Ok(DraftOpen::Rejected { errors, catalog });
            }
        };
        let driver: Box<dyn Driver> = match &resolved.resume {
            Some(id) => {
                let session = self
                    .builder
                    .find_session(&SessionSelector::Id(id.clone()), Some(resolved.phase))
                    .await?
                    .ok_or_else(|| Error::storage(format!("session `{}` vanished", id.0)))?;
                let mut agent = self.builder.agent_for(session).await?;
                if agent.effort() != resolved.effort {
                    agent.set_effort(resolved.effort).await?;
                }
                Box::new(agent)
            }
            None => {
                let selector = match &resolved.name {
                    Some(name) => SessionSelector::Named(name.clone()),
                    None => SessionSelector::New { name: None },
                };
                Box::new(
                    self.builder
                        .create_native(
                            &selector,
                            resolved.profile,
                            resolved.model,
                            resolved.effort,
                            Some(resolved.phase),
                        )
                        .await?,
                )
            }
        };
        Ok(DraftOpen::Opened {
            driver,
            catalog,
            warnings,
        })
    }

    /// A control plane with the native connector only.
    pub fn native(builder: Arc<AgentBuilder>) -> Self {
        Self::new(builder, Vec::new())
    }

    pub fn connector(&self, id: ConnectorId) -> Option<Arc<dyn Connector>> {
        if id == ConnectorId::Native {
            let native: Arc<dyn Connector> = Arc::clone(&self.native) as Arc<dyn Connector>;
            return Some(native);
        }
        self.external.iter().find(|c| c.id() == id).cloned()
    }

    /// Every connector in ADR-010 order with its reported state. A probe
    /// failure is shown as such, not hidden.
    pub async fn infos(&self) -> Vec<(ConnectorId, Result<ConnectorInfo>)> {
        let mut out = Vec::new();
        for id in ConnectorId::ORDER {
            match self.connector(id) {
                Some(connector) => out.push((id, connector.info().await)),
                None => out.push((
                    id,
                    Ok(ConnectorInfo {
                        id,
                        version: None,
                        transport: Transport::MachineReadable,
                        capabilities: Default::default(),
                        auth: AuthState::NotInstalled,
                    }),
                )),
            }
        }
        out
    }

    /// Key values the harness can resolve plus every credential-like
    /// variable in this process's environment, redacted out of connector
    /// output. The agents keep their environment (ADR-010); only what they
    /// echo back is filtered.
    pub fn known_secrets(&self) -> Vec<Secret> {
        let blocked: Vec<String> = self
            .builder
            .config
            .profiles
            .values()
            .map(|profile| profile.key.env_var_name.clone())
            .collect();
        let mut secrets: Vec<Secret> = self
            .builder
            .config
            .profiles
            .iter()
            .filter_map(|(name, profile)| self.builder.keys.key(name, &profile.key).ok())
            .collect();
        let vars: Vec<(String, String)> = std::env::vars().collect();
        secrets.extend(gritt_core::secret::secret_env_values(
            vars.iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            &blocked,
        ));
        secrets
    }

    /// Opens or creates a session and returns its driver. An existing
    /// session keeps its own backend; `connector` only applies to a new
    /// one, and `None` means the native path.
    pub async fn open(
        &self,
        selector: SessionSelector,
        connector: Option<ConnectorId>,
        profile: Option<&str>,
        model: Option<&str>,
        phase: Option<Phase>,
    ) -> Result<Box<dyn Driver>> {
        let existing = self.builder.find_session(&selector, phase).await?;
        let existing_id = existing.as_ref().map(|session| session.id.clone());
        let session = match existing {
            Some(session) => {
                if let (Some(wanted), SessionKind::Connector { id }) = (connector, &session.kind) {
                    if wanted != *id {
                        return Err(Error::config(format!(
                            "session `{}` runs on {}, not {}",
                            session.name,
                            id.as_str(),
                            wanted.as_str()
                        )));
                    }
                }
                session
            }
            None => match connector {
                None | Some(ConnectorId::Native) => {
                    let agent = self.builder.open(selector, profile, model, phase).await?;
                    return Ok(Box::new(agent));
                }
                Some(id) => {
                    // Refuse before creating anything the store would then
                    // list as a session that never ran: the connector must
                    // be registered and its executable found.
                    let Some(connector) = self.connector(id) else {
                        return Err(Error::connector(format!(
                            "connector {} is not available",
                            id.as_str()
                        )));
                    };
                    match connector.info().await {
                        Ok(info) if info.auth == AuthState::NotInstalled => {
                            return Err(Error::connector(format!(
                                "connector {} is not installed; nothing was created",
                                id.as_str()
                            )));
                        }
                        Ok(_) => {}
                        Err(error) => {
                            return Err(Error::connector(format!(
                                "connector {} cannot start: {}",
                                id.as_str(),
                                error.message
                            )));
                        }
                    }
                    let now = Utc::now();
                    let session_id = SessionId(uuid::Uuid::new_v4().to_string());
                    let session = Session {
                        name: AgentBuilder::session_name(&selector, &session_id),
                        id: session_id,
                        kind: SessionKind::Connector { id },
                        phase: phase.unwrap_or(Phase::Coding),
                        workspace: self.builder.workspace_root().to_path_buf(),
                        created_at: now,
                        updated_at: now,
                        parent_id: None,
                    };
                    self.builder.store.create(session.clone()).await?;
                    session
                }
            },
        };
        match &session.kind {
            SessionKind::Native { .. } => Ok(Box::new(self.builder.agent_for(session).await?)),
            SessionKind::Connector { id } => {
                let connector = self.connector(*id).ok_or_else(|| {
                    Error::connector(format!("connector {} is not available", id.as_str()))
                })?;
                let created_now = existing_id.is_none();
                let session_id = session.id.clone();
                let opened = ConnectorSession::open(
                    session,
                    connector,
                    Arc::clone(&self.builder.store),
                    Arc::clone(&self.builder.telemetry),
                    self.builder.approval,
                    self.known_secrets(),
                )
                .await;
                match opened {
                    Ok(driver) => Ok(Box::new(driver)),
                    Err(error) => {
                        // A row for a session that never opened is noise in
                        // the list; only one created in this call is removed.
                        if created_now {
                            let _ = self.builder.store.remove(&session_id).await;
                        }
                        Err(error)
                    }
                }
            }
        }
    }
}
