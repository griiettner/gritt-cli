//! The control plane: picks the backend for a session and hands the modes
//! one [`Driver`] whichever path produced it. The native connector is the
//! first connector; external ones are optional and a missing one only
//! fails its own sessions.

use std::sync::Arc;

use chrono::Utc;
use gritt_core::connector::{AuthState, Connector, ConnectorId, ConnectorInfo, Transport};
use gritt_core::secret::Secret;
use gritt_core::session::{Phase, Session, SessionId, SessionKind, SessionStore};
use gritt_core::{Error, Result};

use crate::agent::{AgentBuilder, SessionSelector};
use crate::connector_session::ConnectorSession;
use crate::driver::Driver;
use crate::native_connector::NativeConnector;

pub struct ControlPlane {
    pub builder: Arc<AgentBuilder>,
    native: Arc<NativeConnector>,
    external: Vec<Arc<dyn Connector>>,
}

impl ControlPlane {
    pub fn new(builder: Arc<AgentBuilder>, external: Vec<Arc<dyn Connector>>) -> Self {
        let native = Arc::new(NativeConnector::new(Arc::clone(&builder)));
        Self {
            builder,
            native,
            external,
        }
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
