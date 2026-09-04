//! External agent connectors for Gritt (ADR-010).
//!
//! Each installed agent keeps its own command and tool authority. Gritt
//! launches it through its documented machine-readable interface (a PTY
//! when configured), supervises the process, normalizes its output into
//! the shared event model, relays approvals where the protocol exposes
//! them, and reports capabilities, version, and auth state without faking
//! parity. A missing or outdated agent never breaks the native path: it
//! only reports `NotInstalled` or an error event for its own sessions.
//!
//! The native connector lives in `gritt-harness`, because this crate
//! depends on `gritt-core` only.

pub mod health;
pub mod process;
pub mod protocols;
pub mod pty;
pub mod redact;
pub mod supervise;

use std::sync::Arc;

use gritt_core::config::ConnectorSettings;
use gritt_core::connector::{Connector, ConnectorId};
use gritt_core::secret::Secret;

pub use supervise::{ExternalConnector, Normalized, Normalizer, Protocol, Timeouts};

/// The four external connectors in ADR-010 order, configured from
/// `settings`. Every one is optional at runtime: `info()` reports whether
/// it is installed.
pub fn default_connectors(settings: &ConnectorSettings) -> Vec<Arc<dyn Connector>> {
    default_connectors_with_secrets(settings, Vec::new())
}

/// The same set, with key values to redact out of every event.
pub fn default_connectors_with_secrets(
    settings: &ConnectorSettings,
    secrets: Vec<Secret>,
) -> Vec<Arc<dyn Connector>> {
    vec![
        Arc::new(
            ExternalConnector::new(protocols::codex::Codex, settings).with_secrets(secrets.clone()),
        ),
        Arc::new(
            ExternalConnector::new(protocols::claude::ClaudeCode, settings)
                .with_secrets(secrets.clone()),
        ),
        Arc::new(
            ExternalConnector::new(protocols::cursor::Cursor, settings)
                .with_secrets(secrets.clone()),
        ),
        Arc::new(
            ExternalConnector::new(protocols::opencode::OpenCode, settings).with_secrets(secrets),
        ),
    ]
}

/// Parses a connector name as the user types it.
pub fn parse_connector_id(name: &str) -> Option<ConnectorId> {
    match name.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "native" => Some(ConnectorId::Native),
        "codex" => Some(ConnectorId::Codex),
        "claude" | "claude_code" | "claudecode" => Some(ConnectorId::ClaudeCode),
        "cursor" => Some(ConnectorId::Cursor),
        "opencode" | "open_code" => Some(ConnectorId::OpenCode),
        _ => None,
    }
}
