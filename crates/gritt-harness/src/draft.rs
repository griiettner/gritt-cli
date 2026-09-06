//! Session drafts: the uncommitted provider, model, effort, and phase
//! choices an interface collects before a native session exists, and the
//! typed outcomes of validating them (feature plan, step 3). Everything
//! here is a value an interface can match on; nothing is an error string
//! to parse.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use gritt_core::connector::ConnectorId;
use gritt_core::provider::{EffortUnsupportedReason, ReasoningEffort};
use gritt_core::session::{ExecutionMode, Phase, SessionId};
use gritt_core::{Error, ErrorKind};
use serde::{Deserialize, Serialize};

use crate::startup::SkippedProfile;

/// Uncommitted choices for a native session. Every field is optional:
/// `profile` and `model` fall back to the configured defaults, `effort`
/// to `Auto` for a new session or the stored value for a resumed one,
/// and `phase` to planning for a new session or the stored phase.
///
/// A draft that names an existing native session resumes it. The stored
/// profile and model stay pinned: a draft that asks for different ones is
/// rejected with [`DraftError::SessionPinned`] so the interface can offer a
/// new session instead of silently changing a transcript's meaning.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDraft {
    pub name: Option<String>,
    pub profile: Option<String>,
    /// True when `profile` is the user's own choice (a flag or a picker)
    /// rather than a seeded default. A chosen profile is pinned: startup
    /// does not fail over away from it. See [`crate::startup`].
    #[serde(default)]
    pub explicit_profile: bool,
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
    pub phase: Option<Phase>,
    #[serde(default)]
    pub mode: Option<ExecutionMode>,
}

impl SessionDraft {
    /// Selects a profile and clears the model, because a model belongs to
    /// the profile it was chosen under. The choice pins the profile.
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        let profile = profile.into();
        if self.profile.as_deref() != Some(profile.as_str()) {
            self.model = None;
        }
        self.profile = Some(profile);
        self.explicit_profile = true;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_effort(mut self, effort: ReasoningEffort) -> Self {
        self.effort = Some(effort);
        self
    }

    pub fn with_phase(mut self, phase: Phase) -> Self {
        self.phase = Some(phase);
        self
    }

    pub fn with_mode(mut self, mode: ExecutionMode) -> Self {
        self.mode = Some(mode);
        self.phase = Some(mode.phase());
        self
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
}

/// The state of a profile's model list after warming it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CatalogState {
    Fresh {
        fetched_at: DateTime<Utc>,
    },
    /// The refresh failed and the last cached list is in use.
    Stale {
        fetched_at: DateTime<Utc>,
    },
    /// No list was ever cached and the refresh failed. Capabilities are
    /// unreported; a manually typed model is allowed with a warning.
    Missing {
        reason: String,
    },
    /// A cached list exists but stale fallback is disabled, so it was not
    /// used. Capabilities are unreported.
    RefreshFailed {
        reason: String,
    },
    /// Model list loading is disabled for this run (`--no-models` or no
    /// cache directory). Capabilities are unreported.
    Skipped,
}

impl CatalogState {
    /// Whether a list is available to resolve models against.
    pub fn has_list(&self) -> bool {
        matches!(
            self,
            CatalogState::Fresh { .. } | CatalogState::Stale { .. }
        )
    }
}

/// Why a draft cannot open a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum DraftError {
    /// No profile in the draft and no `default_profile` configured.
    MissingProfile,
    UnknownProfile {
        profile: String,
    },
    /// No model in the draft and no `default_model` configured.
    MissingModel,
    /// The model name (qualified or alias) belongs to another profile.
    /// The interface should clear the model when the profile changes.
    ModelOutsideProfile {
        model: String,
        model_profile: String,
        profile: String,
    },
    /// Alias resolution or deprecation remapping failed.
    ModelResolution {
        model: String,
        message: String,
    },
    /// The adapter has no safe mapping for this level on this model.
    EffortUnsupported {
        profile: String,
        model: String,
        effort: ReasoningEffort,
        reason: EffortUnsupportedReason,
    },
    /// The named session exists and is pinned to another profile or model.
    /// Changing either needs a new session.
    SessionPinned {
        name: String,
        profile: String,
        model: String,
        requested_profile: Option<String>,
        requested_model: Option<String>,
    },
    /// The named session runs on a connector, which owns its own model
    /// and effort; the native picker does not apply.
    ConnectorSession {
        name: String,
        connector: ConnectorId,
    },
    /// The named session belongs to another workspace.
    OtherWorkspace {
        name: String,
        workspace: PathBuf,
    },
    /// Every candidate profile was skipped. Each entry says why, in the
    /// order tried, without a key value.
    NoUsableProfile {
        skipped: Vec<SkippedProfile>,
    },
}

impl DraftError {
    /// The error kind the same failure has outside the draft flow.
    pub fn kind(&self) -> ErrorKind {
        match self {
            DraftError::EffortUnsupported { .. } => ErrorKind::UnsupportedCapability,
            DraftError::NoUsableProfile { .. } => ErrorKind::NoUsableProfile,
            _ => ErrorKind::Config,
        }
    }

    /// The same rejection as an [`Error`], for print and REPL mode, with
    /// the typed value in the diagnostic.
    pub fn into_error(self) -> Error {
        let diagnostic = serde_json::to_value(&self).ok();
        let error = Error::new(self.kind(), self.to_string());
        match diagnostic {
            Some(value) => error.with_diagnostic(value),
            None => error,
        }
    }
}

impl std::fmt::Display for DraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DraftError::MissingProfile => {
                f.write_str("no profile given and no default_profile configured")
            }
            DraftError::UnknownProfile { profile } => write!(f, "unknown profile `{profile}`"),
            DraftError::MissingModel => {
                f.write_str("no model given and no default_model configured")
            }
            DraftError::ModelOutsideProfile {
                model,
                model_profile,
                profile,
            } => write!(
                f,
                "model `{model}` belongs to profile `{model_profile}`, not `{profile}`"
            ),
            DraftError::ModelResolution { model, message } => {
                write!(f, "cannot resolve model `{model}`: {message}")
            }
            DraftError::EffortUnsupported {
                profile,
                model,
                effort,
                reason,
            } => write!(
                f,
                "model `{model}` on `{profile}` does not report support for {}: {}",
                effort.as_str(),
                reason.describe()
            ),
            DraftError::SessionPinned {
                name,
                profile,
                model,
                ..
            } => write!(
                f,
                "session `{name}` is pinned to {model} on {profile}; start a new session to change them"
            ),
            DraftError::ConnectorSession { name, connector } => write!(
                f,
                "session `{name}` runs on {}, which manages its own model and effort",
                connector.as_str()
            ),
            DraftError::OtherWorkspace { name, workspace } => write!(
                f,
                "session `{name}` belongs to workspace {}",
                workspace.display()
            ),
            DraftError::NoUsableProfile { skipped } => {
                f.write_str("no usable provider profile: ")?;
                for (index, entry) in skipped.iter().enumerate() {
                    if index > 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "{entry}")?;
                }
                Ok(())
            }
        }
    }
}

/// Something worth showing that does not block opening the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "warning", rename_all = "snake_case")]
pub enum DraftWarning {
    /// The list is available and does not contain the model. The same
    /// unreported-capability rule as print mode applies.
    ModelNotInCatalog { profile: String, model: String },
    /// A deprecated model id was remapped to its declared replacement.
    DeprecatedModelRemapped { from: String, to: String },
    /// Startup moved past this profile to the next one in the fallback
    /// order. The entry names the failure class, never a key value.
    ProfileSkipped(SkippedProfile),
    /// Which of the new session's choices came from the last successful
    /// session rather than a flag, a picker, or the configuration.
    LastUsedApplied {
        profile: Option<String>,
        model: Option<String>,
        effort: Option<ReasoningEffort>,
    },
    /// The remembered effort has no safe mapping on the selected model,
    /// so the session starts at the provider default instead.
    EffortReset {
        effort: ReasoningEffort,
        profile: String,
        model: String,
        reason: EffortUnsupportedReason,
    },
}

impl std::fmt::Display for DraftWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DraftWarning::ModelNotInCatalog { profile, model } => write!(
                f,
                "{model} is not in {profile}'s list; its capabilities are unreported"
            ),
            DraftWarning::DeprecatedModelRemapped { from, to } => {
                write!(f, "{from} is deprecated; using {to}")
            }
            DraftWarning::ProfileSkipped(entry) => write!(f, "skipped profile {entry}"),
            DraftWarning::LastUsedApplied {
                profile,
                model,
                effort,
            } => {
                let mut parts = Vec::new();
                if let Some(profile) = profile {
                    parts.push(format!("profile {profile}"));
                }
                if let Some(model) = model {
                    parts.push(format!("model {model}"));
                }
                if let Some(effort) = effort {
                    parts.push(format!("effort {}", effort.label()));
                }
                write!(f, "using the last session's {}", parts.join(", "))
            }
            DraftWarning::EffortReset {
                effort,
                profile,
                model,
                reason,
            } => write!(
                f,
                "the remembered effort {} is not supported by {model} on {profile} ({}); using the provider default",
                effort.as_str(),
                reason.describe()
            ),
        }
    }
}

/// A draft after validation: concrete profile and model ids, the effort
/// that will be stored, and whether it resumes an existing session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDraft {
    pub name: Option<String>,
    /// The session this draft resumes, `None` for a new one.
    pub resume: Option<SessionId>,
    pub profile: String,
    pub model: String,
    pub effort: ReasoningEffort,
    pub phase: Phase,
}

/// The result of validating a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DraftOutcome {
    Ready {
        draft: ResolvedDraft,
        catalog: CatalogState,
        warnings: Vec<DraftWarning>,
    },
    Rejected {
        errors: Vec<DraftError>,
        /// Present when the profile was known and its catalog was warmed
        /// before the rejection.
        catalog: Option<CatalogState>,
    },
}

impl DraftOutcome {
    pub fn is_ready(&self) -> bool {
        matches!(self, DraftOutcome::Ready { .. })
    }

    pub fn errors(&self) -> &[DraftError] {
        match self {
            DraftOutcome::Ready { .. } => &[],
            DraftOutcome::Rejected { errors, .. } => errors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_profile_clears_the_model() {
        let draft = SessionDraft::default()
            .with_profile("openai")
            .with_model("gpt-5-nano")
            .with_effort(ReasoningEffort::High);
        assert_eq!(draft.model.as_deref(), Some("gpt-5-nano"));
        assert!(draft.explicit_profile);
        let seeded: SessionDraft =
            serde_json::from_value(serde_json::json!({ "profile": "openai" })).unwrap();
        assert!(!seeded.explicit_profile);
        let same = draft.clone().with_profile("openai");
        assert_eq!(same.model.as_deref(), Some("gpt-5-nano"));
        let changed = draft.with_profile("anthropic");
        assert_eq!(changed.model, None);
        assert_eq!(changed.effort, Some(ReasoningEffort::High));
    }

    #[test]
    fn outcomes_serialize_with_tags_an_interface_can_match_on() {
        let rejected = DraftOutcome::Rejected {
            errors: vec![DraftError::ModelOutsideProfile {
                model: "other/model-x".into(),
                model_profile: "other".into(),
                profile: "openrouter".into(),
            }],
            catalog: Some(CatalogState::Skipped),
        };
        let json = serde_json::to_value(&rejected).unwrap();
        assert_eq!(json["outcome"], "rejected");
        assert_eq!(json["errors"][0]["error"], "model_outside_profile");
        assert_eq!(json["catalog"]["state"], "skipped");
        assert!(!rejected.is_ready());
        assert_eq!(rejected.errors().len(), 1);
        assert!(!CatalogState::Missing { reason: "x".into() }.has_list());
    }

    #[test]
    fn rejections_convert_to_errors_with_the_matching_kind_and_no_secret() {
        let missing = DraftError::MissingModel;
        let error = missing.clone().into_error();
        assert_eq!(error.kind, ErrorKind::Config);
        assert!(error.message.contains("default_model"));
        assert_eq!(error.diagnostic.unwrap()["error"], "missing_model");
        let aggregate = DraftError::NoUsableProfile {
            skipped: vec![
                SkippedProfile {
                    profile: "openrouter".into(),
                    class: crate::startup::FailureClass::Connection,
                    message: "connection refused".into(),
                },
                SkippedProfile {
                    profile: "anthropic".into(),
                    class: crate::startup::FailureClass::Authentication,
                    message: "provider returned 401: bad key".into(),
                },
            ],
        };
        let error = aggregate.into_error();
        assert_eq!(error.kind, ErrorKind::NoUsableProfile);
        assert_eq!(
            error.message,
            "no usable provider profile: openrouter (connection failed: connection refused); \
             anthropic (authentication failed: provider returned 401: bad key)"
        );
        assert_eq!(DraftError::MissingModel.kind(), ErrorKind::Config);
        let note = DraftWarning::LastUsedApplied {
            profile: Some("anthropic".into()),
            model: None,
            effort: Some(ReasoningEffort::High),
        };
        assert_eq!(
            note.to_string(),
            "using the last session's profile anthropic, effort high"
        );
    }
}
