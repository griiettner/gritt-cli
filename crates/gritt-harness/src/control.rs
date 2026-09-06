//! The control plane: picks the backend for a session and hands the modes
//! one [`Driver`] whichever path produced it. The native connector is the
//! first connector; external ones are optional and a missing one only
//! fails its own sessions. It also validates and opens session drafts and
//! reports profile and catalog state for the setup flow.
//!
//! This is also the seam ADR-011 names for the first non-terminal
//! frontend: `ControlPlane` plus [`AgentBuilder`] own provider/profile/
//! model resolution, session lifecycle, execution mode and effort
//! selection, permission decisions (via [`crate::policy::PolicyEngine`]
//! and the [`crate::agent::Ui`] trait), last-used preferences, and the
//! normalized [`gritt_core::event::Event`] stream. None of it references
//! Ratatui, Crossterm, terminal dimensions, or escape sequences; the CLI,
//! REPL, and TUI consume this module, and it does not depend on any of
//! them. `crates/gritt-harness/tests/control_plane_client.rs` is a
//! non-terminal Rust client fixture built directly against this API.

use std::sync::Arc;

use chrono::Utc;
use gritt_core::connector::{
    AuthState, Connector, ConnectorId, ConnectorInfo, ConnectorModelDiscovery, Transport,
};
use gritt_core::provider::{ModelInfo, Protocol};
use gritt_core::secret::Secret;
use gritt_core::session::{Phase, Session, SessionId, SessionKind, SessionStore};
use gritt_core::{Error, Result};

use crate::agent::{AgentBuilder, SessionSelector};
use crate::connector_session::ConnectorSession;
use crate::draft::{
    CatalogState, DraftError, DraftOutcome, DraftWarning, ResolvedDraft, SessionDraft,
};
use crate::driver::Driver;
use crate::native_connector::NativeConnector;
use crate::setup::{CredentialState, ProfileSummary, ProviderSetup, ReadOnlySetup};
use crate::startup::{StartupOutcome, StartupRequest};

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

/// A session opened through the control plane with the startup notes a
/// new native session produced and the model-list state its choices were
/// resolved against. A resumed or connector session has neither.
pub struct Opened {
    pub driver: Box<dyn Driver>,
    pub warnings: Vec<DraftWarning>,
    pub catalog: Option<CatalogState>,
    /// Present for a new connector session after discovery ran. A resumed
    /// session keeps its stored model and does not discover again.
    pub connector_models: Option<ConnectorModelDiscovery>,
}

/// Cloning shares every handle, which is what
/// [`ControlPlane::reloaded`] and the asynchronous TUI work both need: a
/// task holds its own handle without opening a second store or runtime.
#[derive(Clone)]
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

    /// Re-reads configuration through the injected setup service and
    /// rebuilds the plane around it.
    ///
    /// Saving a profile writes a file; it does not change the `Config` this
    /// plane was built with. Without this, a profile created from the setup
    /// flow would not appear in the connection picker until the next run.
    /// The store, telemetry, catalog, cache, workspace, and MCP runtime are
    /// shared handles and survive the rebuild, so nothing already open is
    /// disturbed: only the configuration-derived parts change.
    ///
    /// Returns whether a reload happened. A service that cannot reload
    /// answers `false` rather than pretending.
    pub fn reload_config(&mut self) -> bool {
        match self.reloaded() {
            Some(plane) => {
                *self = plane;
                true
            }
            None => false,
        }
    }

    /// The same reload as a new value, for a caller that shares this plane
    /// behind an `Arc` and replaces the handle rather than mutating it.
    pub fn reloaded(&self) -> Option<ControlPlane> {
        let config = self.setup.reload_config()?;
        let mut plane = self.clone();
        let mut builder = (*self.builder).clone();
        builder.config = config;
        plane.builder = Arc::new(builder);
        plane.native = Arc::new(NativeConnector::new(Arc::clone(&plane.builder)));
        Some(plane)
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
        self.builder.warm_catalog(profile).await
    }

    /// Discovers the models an installed connector currently exposes.
    /// Native uses the provider catalog instead.
    pub async fn connector_models(
        &self,
        id: ConnectorId,
        refresh: bool,
    ) -> ConnectorModelDiscovery {
        if id == ConnectorId::Native {
            return ConnectorModelDiscovery::Unsupported {
                connector: id,
                reason: "native sessions use the provider model catalog".into(),
            };
        }
        match self.connector(id) {
            Some(connector) => connector.discover_models(refresh).await,
            None => ConnectorModelDiscovery::Unavailable {
                connector: id,
                reason: format!("{} is not available in this control plane", id.as_str()),
            },
        }
    }

    /// Status and catalog lines print and REPL show for a discovery
    /// result. The TUI picker renders the same value.
    pub fn connector_model_lines(discovery: &ConnectorModelDiscovery) -> Vec<String> {
        let mut lines = vec![discovery.describe()];
        if let Some(catalog) = discovery.catalog() {
            for model in &catalog.models {
                match &model.display_label {
                    Some(label) if label != &model.id => {
                        lines.push(format!("  {}  {label}", model.id));
                    }
                    _ => lines.push(format!("  {}", model.id)),
                }
            }
        }
        lines
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
            let SessionKind::Connector { id, .. } = session.kind else {
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
            Some(wanted) => match self.builder.resolve_under_profile(provider_profile, wanted) {
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
        if let Some(error) = self.builder.effort_error(provider_profile, model, effort) {
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

    /// A new session goes through the startup resolver, so the draft gets
    /// the same failover, remembered choices, and model rules as print and
    /// REPL mode. Skipped profiles come back as warnings on a ready draft
    /// and as one aggregate error when nothing was usable.
    async fn validate_new(&self, draft: &SessionDraft) -> Result<DraftOutcome> {
        match self
            .builder
            .resolve_startup(&StartupRequest::from_draft(draft))
            .await?
        {
            StartupOutcome::Ready(selection) => Ok(DraftOutcome::Ready {
                draft: ResolvedDraft {
                    name: draft.name.clone(),
                    resume: None,
                    profile: selection.profile,
                    model: selection.model,
                    effort: selection.effort,
                    phase: draft.phase.unwrap_or(Phase::Planning),
                },
                catalog: selection.catalog,
                warnings: selection.warnings,
            }),
            StartupOutcome::Rejected { errors, catalog } => {
                Ok(DraftOutcome::Rejected { errors, catalog })
            }
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
        let mut driver: Box<dyn Driver> = match &resolved.resume {
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
        if let Some(mode) = draft.mode {
            driver.set_mode(mode).await?;
        }
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
    /// one, and `None` means the native path. The startup notes are
    /// dropped; see [`ControlPlane::open_with`].
    pub async fn open(
        &self,
        selector: SessionSelector,
        connector: Option<ConnectorId>,
        profile: Option<&str>,
        model: Option<&str>,
        phase: Option<Phase>,
    ) -> Result<Box<dyn Driver>> {
        self.open_with(
            selector,
            connector,
            StartupRequest::from_flags(profile, model, None),
            phase,
            false,
        )
        .await
        .map(|opened| opened.driver)
    }

    /// [`ControlPlane::open`] with the full startup request and the notes
    /// a new native session produced, for a mode that reports them.
    /// `refresh_models` is forwarded to [`ControlPlane::connector_models`]
    /// for a new connector session.
    pub async fn open_with(
        &self,
        selector: SessionSelector,
        connector: Option<ConnectorId>,
        request: StartupRequest,
        phase: Option<Phase>,
        refresh_models: bool,
    ) -> Result<Opened> {
        let existing = self.builder.find_session(&selector, phase).await?;
        let existing_id = existing.as_ref().map(|session| session.id.clone());
        let session = match existing {
            Some(session) => {
                if let (Some(wanted), SessionKind::Connector { id, .. }) =
                    (connector, &session.kind)
                {
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
                    // The lookup above already established that nothing
                    // exists under this selector.
                    let opened = self.builder.start_native(&selector, request, phase).await?;
                    return Ok(Opened {
                        driver: Box::new(opened.agent),
                        warnings: opened.warnings,
                        catalog: opened.catalog,
                        connector_models: None,
                    });
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
                    let model = request
                        .model
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                    let session = Session {
                        name: AgentBuilder::session_name(&selector, &session_id),
                        id: session_id,
                        kind: SessionKind::Connector { id, model },
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
            SessionKind::Native { .. } => Ok(Opened {
                driver: Box::new(self.builder.agent_for(session).await?),
                warnings: Vec::new(),
                catalog: None,
                connector_models: None,
            }),
            SessionKind::Connector { id, .. } => {
                let connector = self.connector(*id).ok_or_else(|| {
                    Error::connector(format!("connector {} is not available", id.as_str()))
                })?;
                let created_now = existing_id.is_none();
                let session_id = session.id.clone();
                let connector_models = if created_now {
                    Some(self.connector_models(*id, refresh_models).await)
                } else {
                    None
                };
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
                    Ok(driver) => Ok(Opened {
                        driver: Box::new(driver),
                        warnings: Vec::new(),
                        catalog: None,
                        connector_models,
                    }),
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
