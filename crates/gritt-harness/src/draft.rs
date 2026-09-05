//! Session drafts: the uncommitted provider, model, effort, and phase
//! choices an interface collects before a native session exists, and the
//! typed outcomes of validating them (feature plan, step 3). Everything
//! here is a value an interface can match on; nothing is an error string
//! to parse.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use gritt_core::connector::ConnectorId;
use gritt_core::provider::{EffortUnsupportedReason, ReasoningEffort};
use gritt_core::session::{Phase, SessionId};
use serde::{Deserialize, Serialize};

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
    pub model: Option<String>,
    pub effort: Option<ReasoningEffort>,
    pub phase: Option<Phase>,
}

impl SessionDraft {
    /// Selects a profile and clears the model, because a model belongs to
    /// the profile it was chosen under.
    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        let profile = profile.into();
        if self.profile.as_deref() != Some(profile.as_str()) {
            self.model = None;
        }
        self.profile = Some(profile);
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
}
