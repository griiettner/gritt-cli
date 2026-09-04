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
use gritt_core::{Error, Result};

pub use supervise::{ExternalConnector, Normalized, Normalizer, Protocol, Timeouts};

/// Credential values in this process's environment. External agents keep
/// their full environment (ADR-010), so anything they echo back has to be
/// redacted instead: these values join the redaction set of every
/// connector. `blocked` lists the configured profile key variables.
pub fn environment_secrets(blocked: &[String]) -> Vec<Secret> {
    let vars: Vec<(String, String)> = std::env::vars().collect();
    gritt_core::secret::secret_env_values(
        vars.iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        blocked,
    )
}

/// Names that mark an option as credential-bearing, matched case-insensitively
/// inside the option name (`--api-key`, `-k`, `--auth-token=`).
const CREDENTIAL_MARKERS: [&str; 6] =
    ["key", "token", "secret", "password", "passwd", "credential"];

/// Whether `arg` is an option (`--flag`, `--flag=value`, `-f`) rather than a
/// positional value.
pub fn is_option(arg: &str) -> bool {
    arg.starts_with('-') && arg.len() > 1
}

/// The option name without any attached `=value`.
fn option_name(arg: &str) -> &str {
    arg.split_once('=').map_or(arg, |(name, _)| name)
}

/// Whether an option name looks like it carries a credential, whether its
/// value is attached with `=` or comes as the next token.
pub fn is_credential_option(arg: &str) -> bool {
    if !is_option(arg) {
        return false;
    }
    let name = option_name(arg)
        .trim_start_matches('-')
        .to_ascii_lowercase();
    CREDENTIAL_MARKERS
        .iter()
        .any(|marker| name.contains(marker))
        || name == "k"
        || name == "p"
}

/// Refuses an `extra_args` entry that carries a credential. Arguments are
/// recorded in launch diagnostics and travel through the process list, so
/// a key belongs in the agent's own credential store, never here. A
/// credential-like option is refused whether its value is attached
/// (`--api-key=...`) or split (`--api-key ...`), and so is any token that
/// contains a known secret value.
pub fn validate_extra_args(settings: &ConnectorSettings, secrets: &[Secret]) -> Result<()> {
    for (connector, args) in &settings.extra_args {
        for arg in args {
            let leaks = secrets
                .iter()
                .any(|secret| !secret.is_empty() && arg.contains(secret.expose()));
            if is_credential_option(arg) || leaks {
                // Only a flag name is worth showing, and only when the
                // argument does not carry a known secret: a secret that
                // begins with `-` parses as an option, so the `leaks`
                // branch always uses a fixed placeholder.
                let shown = if leaks {
                    "[redacted argument]".to_owned()
                } else if is_option(arg) {
                    format!("`{} ...`", option_name(arg))
                } else {
                    "a bare value".to_owned()
                };
                return Err(Error::config(format!(
                    "connectors.extra_args.{connector} contains a credential-bearing argument ({shown}); \
                     put the agent's key in its own credential store, not in Gritt's configuration"
                )));
            }
        }
    }
    Ok(())
}

/// The four external connectors in ADR-010 order, configured from
/// `settings`. Every one is optional at runtime: `info()` reports whether
/// it is installed. Fails only on a credential-bearing `extra_args` entry.
pub fn default_connectors(settings: &ConnectorSettings) -> Result<Vec<Arc<dyn Connector>>> {
    default_connectors_with_secrets(settings, Vec::new())
}

/// The same set, with key values to redact out of every event.
pub fn default_connectors_with_secrets(
    settings: &ConnectorSettings,
    secrets: Vec<Secret>,
) -> Result<Vec<Arc<dyn Connector>>> {
    validate_extra_args(settings, &secrets)?;
    Ok(vec![
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
    ])
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
