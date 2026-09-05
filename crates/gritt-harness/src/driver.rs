//! One interface over a native agent and a connector session, so the
//! print, REPL, and full-screen modes run either without knowing which.

use gritt_core::connector::ConnectorId;
use gritt_core::provider::{EffortUnsupportedReason, ReasoningEffort};
use gritt_core::session::{BoxFuture, Phase, Session, SessionKind};
use gritt_core::Result;

use crate::agent::{CancelHandle, NativeAgent, TurnOutcome, Ui};

/// What changing effort on a driver did. Typed so an interface can show
/// the right message without parsing one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortOutcome {
    /// Stored on the native session; the next turn sends it.
    Applied { effort: ReasoningEffort },
    /// A connector session: the external agent owns its own effort, so
    /// the request is not applied and nothing is stored.
    ManagedByConnector { id: ConnectorId },
    /// The adapter has no safe mapping for this level on the session's
    /// model. Nothing is stored.
    Unsupported {
        effort: ReasoningEffort,
        reason: EffortUnsupportedReason,
    },
}

/// What the status bar shows for a session's backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriverInfo {
    /// Provider profile or connector name.
    pub backend: String,
    /// Model id or connector version.
    pub detail: String,
}

pub trait Driver: Send {
    fn session(&self) -> &Session;
    fn phase(&self) -> Phase;
    fn set_phase(&mut self, phase: Phase) -> BoxFuture<'_, Result<()>>;
    fn handle(&self) -> CancelHandle;
    fn run_turn<'a>(
        &'a mut self,
        prompt: &'a str,
        ui: &'a mut dyn Ui,
    ) -> BoxFuture<'a, Result<TurnOutcome>>;
    fn info(&self) -> DriverInfo;
    /// The native effort, `None` on a connector session (managed by the
    /// agent). Interfaces show `auto` rather than hiding a default.
    fn effort(&self) -> Option<ReasoningEffort>;
    /// Changes the effort for later turns. Native only; see
    /// [`EffortOutcome`].
    fn set_effort(&mut self, effort: ReasoningEffort) -> BoxFuture<'_, Result<EffortOutcome>>;
}

impl Driver for NativeAgent {
    fn session(&self) -> &Session {
        NativeAgent::session(self)
    }

    fn phase(&self) -> Phase {
        NativeAgent::phase(self)
    }

    fn set_phase(&mut self, phase: Phase) -> BoxFuture<'_, Result<()>> {
        Box::pin(NativeAgent::set_phase(self, phase))
    }

    fn handle(&self) -> CancelHandle {
        NativeAgent::handle(self)
    }

    fn run_turn<'a>(
        &'a mut self,
        prompt: &'a str,
        ui: &'a mut dyn Ui,
    ) -> BoxFuture<'a, Result<TurnOutcome>> {
        Box::pin(NativeAgent::run_turn(self, prompt, ui))
    }

    fn effort(&self) -> Option<ReasoningEffort> {
        Some(NativeAgent::effort(self))
    }

    fn set_effort(&mut self, effort: ReasoningEffort) -> BoxFuture<'_, Result<EffortOutcome>> {
        Box::pin(NativeAgent::set_effort(self, effort))
    }

    fn info(&self) -> DriverInfo {
        match &NativeAgent::session(self).kind {
            SessionKind::Native {
                provider_profile,
                model,
                ..
            } => DriverInfo {
                backend: provider_profile.clone(),
                detail: model.clone(),
            },
            SessionKind::Connector { id } => DriverInfo {
                backend: id.as_str().to_owned(),
                detail: String::new(),
            },
        }
    }
}
