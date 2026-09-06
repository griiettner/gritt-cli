//! External agent connector contract (ADR-010). The native path implements
//! the same contract so the control plane never special-cases it.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::event::{ApprovalDecision, ApprovalId};
use crate::provider::EventStream;
use crate::session::{BoxFuture, ContinuationState, SessionId};
use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorId {
    Native,
    Codex,
    ClaudeCode,
    Cursor,
    OpenCode,
}

impl ConnectorId {
    /// The evaluation and launch order recorded in ADR-010.
    pub const ORDER: [ConnectorId; 5] = [
        ConnectorId::Native,
        ConnectorId::Codex,
        ConnectorId::ClaudeCode,
        ConnectorId::Cursor,
        ConnectorId::OpenCode,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ConnectorId::Native => "native",
            ConnectorId::Codex => "codex",
            ConnectorId::ClaudeCode => "claude_code",
            ConnectorId::Cursor => "cursor",
            ConnectorId::OpenCode => "opencode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    InProcess,
    MachineReadable,
    Pty,
    TerminalScrape,
}

/// Shown, never faked. A missing capability is displayed as such.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorCapabilities {
    pub follow_up_input: bool,
    pub approvals: bool,
    pub cancel: bool,
    pub resume: bool,
    pub inspect: bool,
    pub structured_events: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AuthState {
    Authenticated,
    Unauthenticated,
    Unknown,
    NotInstalled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorInfo {
    pub id: ConnectorId,
    pub version: Option<String>,
    pub transport: Transport,
    pub capabilities: ConnectorCapabilities,
    pub auth: AuthState,
}

/// Coarse task state a connector reports through [`Connector::inspect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Idle,
    Running,
    AwaitingApproval,
    AwaitingInput,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

/// Provider-neutral snapshot of a connector session. Raw connector detail
/// travels only in `diagnostic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorInspection {
    pub session_id: SessionId,
    /// The external agent's own task or thread identifier, when it has one.
    pub external_id: Option<String>,
    pub state: TaskState,
    pub version: Option<String>,
    pub auth: AuthState,
    pub capabilities: ConnectorCapabilities,
    pub diagnostic: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorModel {
    pub id: String,
    /// Human label from the CLI when it reports one. Absent means the
    /// interface should show `id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorModelFreshness {
    Current,
    Stale,
}

/// A connector's model catalog after discovery. `source` names the
/// documented command or interface that produced it, never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorModelCatalog {
    pub connector: ConnectorId,
    pub models: Vec<ConnectorModel>,
    pub source: String,
    pub fetched_at: DateTime<Utc>,
    pub freshness: ConnectorModelFreshness,
}

/// Typed result of asking a connector for its current models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ConnectorModelDiscovery {
    Current {
        catalog: ConnectorModelCatalog,
    },
    CachedStale {
        catalog: ConnectorModelCatalog,
        reason: String,
    },
    Unavailable {
        connector: ConnectorId,
        reason: String,
    },
    Unsupported {
        connector: ConnectorId,
        reason: String,
    },
    CommandFailure {
        connector: ConnectorId,
        reason: String,
    },
    MalformedOutput {
        connector: ConnectorId,
        reason: String,
    },
}

impl ConnectorModelDiscovery {
    pub fn catalog(&self) -> Option<&ConnectorModelCatalog> {
        match self {
            Self::Current { catalog } | Self::CachedStale { catalog, .. } => Some(catalog),
            _ => None,
        }
    }

    pub fn connector(&self) -> ConnectorId {
        match self {
            Self::Current { catalog } | Self::CachedStale { catalog, .. } => catalog.connector,
            Self::Unavailable { connector, .. }
            | Self::Unsupported { connector, .. }
            | Self::CommandFailure { connector, .. }
            | Self::MalformedOutput { connector, .. } => *connector,
        }
    }

    /// One line for print, REPL, and TUI diagnostics. Names the CLI and
    /// the catalog source. Never includes a key, a prompt, or tool output.
    pub fn describe(&self) -> String {
        match self {
            Self::Current { catalog } => format!(
                "{} models from {} (fetched {})",
                catalog.connector.as_str(),
                catalog.source,
                catalog.fetched_at.to_rfc3339()
            ),
            Self::CachedStale { catalog, reason } => format!(
                "{} models from {} are stale (cached {}); {reason}",
                catalog.connector.as_str(),
                catalog.source,
                catalog.fetched_at.to_rfc3339()
            ),
            Self::Unavailable { connector, reason } => {
                format!("{} is unavailable: {reason}", connector.as_str())
            }
            Self::Unsupported { connector, reason } => {
                format!("{} does not list models: {reason}", connector.as_str())
            }
            Self::CommandFailure { connector, reason } => {
                format!("{} model listing failed: {reason}", connector.as_str())
            }
            Self::MalformedOutput { connector, reason } => {
                format!(
                    "{} model listing was unreadable: {reason}",
                    connector.as_str()
                )
            }
        }
    }
}

/// Who installed the connector executable, from evidence on disk (a
/// Homebrew Cellar or Caskroom path, an npm `node_modules` package with
/// its `package.json`, a pipx venv with its metadata, a Cargo
/// `.crates.toml` entry, or a vendor installer's own directory). Never a
/// guess from a path prefix alone: two plausible owners are `Ambiguous`,
/// none is `Unknown`, and neither offers an update command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallSource {
    Homebrew { name: String, cask: bool },
    Npm { package: String },
    Pipx { package: String },
    Cargo { crate_name: String },
    Vendor { installer: String },
    Unknown,
    Ambiguous { candidates: Vec<String> },
}

impl InstallSource {
    pub fn label(&self) -> String {
        match self {
            Self::Homebrew { name, cask: true } => format!("Homebrew cask {name}"),
            Self::Homebrew { name, cask: false } => format!("Homebrew formula {name}"),
            Self::Npm { package } => format!("npm package {package}"),
            Self::Pipx { package } => format!("pipx package {package}"),
            Self::Cargo { crate_name } => format!("Cargo crate {crate_name}"),
            Self::Vendor { installer } => installer.clone(),
            Self::Unknown => "unknown installer".to_owned(),
            Self::Ambiguous { candidates } => {
                format!("ambiguous installer ({})", candidates.join(" or "))
            }
        }
    }

    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown | Self::Ambiguous { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionComparison {
    Current,
    Outdated,
    /// Installed is newer than the latest the source reports.
    Newer,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionFreshness {
    Current,
    Stale,
}

/// One executable plus a fixed argument vector. It is displayed for
/// approval and then run as given; it is never joined into a shell
/// string, and no value in it comes from a prompt or a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateAction {
    pub program: String,
    pub args: Vec<String>,
    pub source: InstallSource,
}

impl UpdateAction {
    /// The command as the user should read it before approving. Display
    /// only: arguments with spaces are quoted so the line is unambiguous,
    /// but the vector above is what runs.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(|part| {
                if part.is_empty() || part.chars().any(char::is_whitespace) {
                    format!("'{part}'")
                } else {
                    part.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Provider-neutral version state of an installed connector CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorVersionStatus {
    pub connector: ConnectorId,
    pub installed: Option<String>,
    pub latest: Option<String>,
    pub comparison: VersionComparison,
    pub source: InstallSource,
    /// The documented query that produced `latest`, for example
    /// `npm view @openai/codex version`. Absent when no query ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_source: Option<String>,
    pub checked_at: DateTime<Utc>,
    pub freshness: VersionFreshness,
    /// The fixed update command when the owner is known and documents
    /// one. Absent for an unknown or ambiguous owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateAction>,
    /// What the user can do when Gritt offers no update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
}

impl ConnectorVersionStatus {
    /// One line naming the CLI, the installed and latest versions, and
    /// the owner. Never a key, a prompt, or tool output.
    pub fn summary(&self) -> String {
        let name = self.connector.as_str();
        let installed = self.installed.as_deref().unwrap_or("unknown version");
        let stale = match self.freshness {
            VersionFreshness::Current => "",
            VersionFreshness::Stale => ", stale",
        };
        match (&self.latest, self.comparison) {
            (Some(latest), VersionComparison::Outdated) => format!(
                "{name} {installed} is outdated; latest is {latest} ({}{stale})",
                self.source.label()
            ),
            (Some(_), VersionComparison::Current) => format!(
                "{name} {installed} is current ({}{stale})",
                self.source.label()
            ),
            (Some(latest), VersionComparison::Newer) => format!(
                "{name} {installed} is newer than the published {latest} ({}{stale})",
                self.source.label()
            ),
            (Some(latest), VersionComparison::Unknown) => format!(
                "{name} {installed}; latest reported as {latest}, not comparable ({}{stale})",
                self.source.label()
            ),
            (None, _) => format!(
                "{name} {installed}; latest version not known ({})",
                self.source.label()
            ),
        }
    }
}

/// Why a latest-version lookup produced nothing usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionCheckFailure {
    Network,
    Authentication,
    MalformedResponse,
    /// The owner is unknown, ambiguous, or does not publish a newest
    /// version Gritt can query.
    UnsupportedSource,
    Timeout,
    CommandFailure,
    /// No query has run yet, and the check was not allowed to run one.
    NotChecked,
}

/// How much a version check may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionCheckMode {
    /// Installed version and owner from local evidence, latest from the
    /// cache only. Never runs a package-manager query, so startup can
    /// call it without waiting on a network.
    Offline,
    /// Use a still-fresh cached latest, otherwise query.
    Cached,
    /// Query now.
    Refresh,
}

/// Typed result of checking a connector's version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ConnectorVersionCheck {
    Checked {
        status: ConnectorVersionStatus,
    },
    /// The latest version comes from the cache because the query failed.
    CachedStale {
        status: ConnectorVersionStatus,
        reason: String,
    },
    /// The installed version and owner are known; the latest is not.
    LatestUnavailable {
        status: ConnectorVersionStatus,
        failure: VersionCheckFailure,
        reason: String,
    },
    NotInstalled {
        connector: ConnectorId,
        reason: String,
    },
    Unsupported {
        connector: ConnectorId,
        reason: String,
    },
}

impl ConnectorVersionCheck {
    pub fn status(&self) -> Option<&ConnectorVersionStatus> {
        match self {
            Self::Checked { status }
            | Self::CachedStale { status, .. }
            | Self::LatestUnavailable { status, .. } => Some(status),
            _ => None,
        }
    }

    pub fn connector(&self) -> ConnectorId {
        match self {
            Self::Checked { status }
            | Self::CachedStale { status, .. }
            | Self::LatestUnavailable { status, .. } => status.connector,
            Self::NotInstalled { connector, .. } | Self::Unsupported { connector, .. } => {
                *connector
            }
        }
    }

    /// True only when a fresh, successful query says a newer version
    /// exists. A stale cache is reported as stale, never as current.
    pub fn update_available(&self) -> bool {
        match self {
            Self::Checked { status } => {
                status.update.is_some() && status.comparison == VersionComparison::Outdated
            }
            _ => false,
        }
    }

    /// One line for print, REPL, and TUI diagnostics.
    pub fn describe(&self) -> String {
        match self {
            Self::Checked { status } => status.summary(),
            Self::CachedStale { status, reason } => {
                format!("{}; {reason}", status.summary())
            }
            Self::LatestUnavailable { status, reason, .. } => {
                format!("{}; {reason}", status.summary())
            }
            Self::NotInstalled { connector, reason } => {
                format!("{} is not installed: {reason}", connector.as_str())
            }
            Self::Unsupported { connector, reason } => {
                format!("{} has no version check: {reason}", connector.as_str())
            }
        }
    }
}

/// Typed result of running, declining, or failing an update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ConnectorUpdateOutcome {
    Updated {
        connector: ConnectorId,
        before: Option<String>,
        after: Option<String>,
        /// The version check that ran after the command succeeded.
        recheck: Box<ConnectorVersionCheck>,
    },
    Declined {
        connector: ConnectorId,
    },
    /// No update command exists for this connector or owner.
    NoAction {
        connector: ConnectorId,
        reason: String,
    },
    Failed {
        connector: ConnectorId,
        reason: String,
        /// The last few output lines, redacted and length-capped.
        output: Vec<String>,
    },
    Cancelled {
        connector: ConnectorId,
    },
    TimedOut {
        connector: ConnectorId,
        reason: String,
    },
}

impl ConnectorUpdateOutcome {
    pub fn connector(&self) -> ConnectorId {
        match self {
            Self::Updated { connector, .. }
            | Self::Declined { connector }
            | Self::NoAction { connector, .. }
            | Self::Failed { connector, .. }
            | Self::Cancelled { connector }
            | Self::TimedOut { connector, .. } => *connector,
        }
    }

    pub fn describe(&self) -> String {
        let name = self.connector().as_str();
        match self {
            Self::Updated { before, after, .. } => match (before, after) {
                (Some(before), Some(after)) if before == after => {
                    format!("{name} update ran; still {after}")
                }
                (_, Some(after)) => format!("{name} updated to {after}"),
                (_, None) => format!("{name} update ran; version not reported"),
            },
            Self::Declined { .. } => format!("{name} update declined; nothing was run"),
            Self::NoAction { reason, .. } => format!("{name}: no update run; {reason}"),
            Self::Failed { reason, .. } => format!("{name} update failed: {reason}"),
            Self::Cancelled { .. } => format!("{name} update cancelled"),
            Self::TimedOut { reason, .. } => format!("{name} update timed out: {reason}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub session_id: SessionId,
    pub prompt: String,
    pub workspace: PathBuf,
    /// State a previous turn left behind the session interface, so a
    /// connector can pick its external thread back up after a restart.
    /// Opaque to everything above the connector that wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<ContinuationState>,
    /// Explicit model for a new connector session. Absent means the
    /// external CLI's own default. Never guessed from a display label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub trait Connector: Send + Sync {
    fn id(&self) -> ConnectorId;
    fn info(&self) -> BoxFuture<'_, Result<ConnectorInfo>>;
    /// Discovers the models this connector currently exposes. Default is
    /// [`ConnectorModelDiscovery::Unsupported`]. Cache, freshness, and
    /// stale fallback belong to the implementation that talks to the CLI.
    fn discover_models(&self, _refresh: bool) -> BoxFuture<'_, ConnectorModelDiscovery> {
        let connector = self.id();
        Box::pin(async move {
            ConnectorModelDiscovery::Unsupported {
                connector,
                reason: format!(
                    "{} does not document a model listing command",
                    connector.as_str()
                ),
            }
        })
    }
    /// Reports the installed version, its owner, and the newest version
    /// that owner publishes. Default is [`ConnectorVersionCheck::Unsupported`].
    /// Advisory: a failure here never stops a session from starting.
    fn check_version(&self, _mode: VersionCheckMode) -> BoxFuture<'_, ConnectorVersionCheck> {
        let connector = self.id();
        Box::pin(async move {
            ConnectorVersionCheck::Unsupported {
                connector,
                reason: format!(
                    "{} has no installed executable to check",
                    connector.as_str()
                ),
            }
        })
    }
    /// Runs an already-approved update action and rechecks the version.
    /// Approval is the caller's job; this never prompts and never runs
    /// anything other than `action`.
    fn update(&self, action: UpdateAction) -> BoxFuture<'_, ConnectorUpdateOutcome> {
        let connector = self.id();
        Box::pin(async move {
            ConnectorUpdateOutcome::NoAction {
                connector,
                reason: format!(
                    "{} cannot run `{}`: updates are not supported here",
                    connector.as_str(),
                    action.display()
                ),
            }
        })
    }
    fn start(&self, request: TaskRequest) -> BoxFuture<'_, Result<EventStream<'_>>>;
    fn send_input(&self, session_id: &SessionId, input: String) -> BoxFuture<'_, Result<()>>;
    fn answer_approval(
        &self,
        session_id: &SessionId,
        request_id: ApprovalId,
        decision: ApprovalDecision,
    ) -> BoxFuture<'_, Result<()>>;
    fn cancel(&self, session_id: &SessionId) -> BoxFuture<'_, Result<()>>;
    /// Resumes a session when the connector supports it; otherwise returns a
    /// connector error naming the missing capability.
    fn resume(&self, session_id: &SessionId) -> BoxFuture<'_, Result<EventStream<'_>>>;
    /// Reports the current state of a session when
    /// [`ConnectorCapabilities::inspect`] is set; otherwise returns a
    /// connector error naming the missing capability.
    fn inspect(&self, session_id: &SessionId) -> BoxFuture<'_, Result<ConnectorInspection>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_outcomes_round_trip_and_never_claim_stale_is_current() {
        let catalog = ConnectorModelCatalog {
            connector: ConnectorId::Codex,
            models: vec![ConnectorModel {
                id: "gpt-5.4".into(),
                display_label: Some("GPT-5.4".into()),
            }],
            source: "codex debug models".into(),
            fetched_at: DateTime::parse_from_rfc3339("2026-09-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            freshness: ConnectorModelFreshness::Stale,
        };
        let stale = ConnectorModelDiscovery::CachedStale {
            catalog: catalog.clone(),
            reason: "codex debug models exited 1".into(),
        };
        let text = serde_json::to_string(&stale).unwrap();
        assert!(text.contains("cached_stale"));
        assert!(!text.contains("\"current\""));
        assert_eq!(
            stale.catalog().unwrap().freshness,
            ConnectorModelFreshness::Stale
        );
        assert!(stale.describe().contains("codex"));
        assert!(stale.describe().contains("stale"));
        let back: ConnectorModelDiscovery = serde_json::from_str(&text).unwrap();
        assert_eq!(back, stale);
    }

    #[test]
    fn update_actions_are_argument_vectors_and_stale_checks_never_say_current() {
        let action = UpdateAction {
            program: "npm".into(),
            args: vec!["install".into(), "-g".into(), "@openai/codex@latest".into()],
            source: InstallSource::Npm {
                package: "@openai/codex".into(),
            },
        };
        // The vector is the contract; `display` only quotes for reading.
        assert_eq!(action.args.len(), 3);
        assert_eq!(action.display(), "npm install -g @openai/codex@latest");
        let spaced = UpdateAction {
            program: "/opt/my tools/agent".into(),
            args: vec!["update".into()],
            source: InstallSource::Vendor {
                installer: "vendor installer".into(),
            },
        };
        assert_eq!(spaced.display(), "'/opt/my tools/agent' update");
        assert_eq!(spaced.args, vec!["update".to_owned()]);

        let status = ConnectorVersionStatus {
            connector: ConnectorId::Codex,
            installed: Some("0.150.0".into()),
            latest: Some("0.153.4".into()),
            comparison: VersionComparison::Outdated,
            source: action.source.clone(),
            latest_source: Some("npm view @openai/codex version".into()),
            checked_at: DateTime::parse_from_rfc3339("2026-09-06T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            freshness: VersionFreshness::Stale,
            update: Some(action),
            next_step: None,
        };
        let stale = ConnectorVersionCheck::CachedStale {
            status,
            reason: "npm view failed".into(),
        };
        assert!(
            !stale.update_available(),
            "a stale cache must not offer an update as current"
        );
        let text = serde_json::to_string(&stale).unwrap();
        assert!(text.contains("cached_stale"));
        assert!(stale.describe().contains("stale"));
        assert!(stale.describe().contains("outdated"));
        let back: ConnectorVersionCheck = serde_json::from_str(&text).unwrap();
        assert_eq!(back, stale);

        let unknown = ConnectorVersionStatus {
            comparison: VersionComparison::Unknown,
            source: InstallSource::Ambiguous {
                candidates: vec!["npm".into(), "Homebrew".into()],
            },
            update: None,
            latest: None,
            next_step: Some("update it with the installer you used".into()),
            ..stale.status().unwrap().clone()
        };
        assert!(unknown.summary().contains("ambiguous"));
        assert!(unknown.update.is_none());
    }

    #[test]
    fn task_request_without_model_still_loads() {
        let raw = serde_json::json!({
            "session_id": "s1",
            "prompt": "hi",
            "workspace": "/tmp"
        });
        let request: TaskRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(request.model, None);
    }
}
