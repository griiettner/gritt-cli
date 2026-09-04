//! One interface over a native agent and a connector session, so the
//! print, REPL, and full-screen modes run either without knowing which.

use gritt_core::session::{BoxFuture, Phase, Session, SessionKind};
use gritt_core::Result;

use crate::agent::{CancelHandle, NativeAgent, TurnOutcome, Ui};

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

    fn info(&self) -> DriverInfo {
        match &NativeAgent::session(self).kind {
            SessionKind::Native {
                provider_profile,
                model,
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
