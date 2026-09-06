//! Session drafts through the control plane: new-session selection, a
//! provider change invalidating the model, resumed-session pinning, the
//! stale catalog fallback, the connector restriction, and effort that
//! persists between turns and reaches the adapter request.

use std::sync::Arc;

use chrono::{Duration, Utc};
use gritt_core::config::Config;
use gritt_core::connector::ConnectorId;
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event};
use gritt_core::provider::{
    EffortUnsupportedReason, ModelCapabilities, ModelInfo, ModelList, ModelListStatus, Protocol,
    ProviderProfile, ReasoningEffort,
};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, Phase, Session, SessionId, SessionKind, SessionStore};
use gritt_harness::agent::{AgentBuilder, ApprovalMode, Ui};
use gritt_harness::control::{ControlPlane, DraftOpen};
use gritt_harness::draft::{CatalogState, DraftError, DraftOutcome, DraftWarning, SessionDraft};
use gritt_harness::driver::EffortOutcome;
use gritt_harness::policy::Decision;
use gritt_harness::setup::CredentialState;
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_provider::models::{CachedModelList, ModelCache, ModelCatalog};
use gritt_provider::{FixtureResponse, FixtureTransport, StaticKey};

const KEY: &str = "fixture-key-never-printed";

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../gritt-provider/tests/fixtures/chat-completions/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

fn profile(name: &str, protocol: Protocol, base_url: &str) -> ProviderProfile {
    ProviderProfile {
        name: name.into(),
        protocol,
        base_url: base_url.into(),
        key: SecretRef::for_profile(name, format!("{}_API_KEY", name.to_uppercase())),
        aliases: Default::default(),
        fallback_model: None,
    }
}

fn config() -> Config {
    let mut config = Config::default();
    config.profiles.insert(
        "openrouter".into(),
        profile(
            "openrouter",
            Protocol::ChatCompletions,
            "https://openrouter.ai/api/v1",
        ),
    );
    config.profiles.insert(
        "other".into(),
        profile("other", Protocol::Responses, "https://other.example/v1"),
    );
    config.profiles.insert(
        "anthropic".into(),
        profile("anthropic", Protocol::Messages, "https://api.anthropic.com"),
    );
    config.aliases.insert("fast".into(), "other/model-x".into());
    config.default_profile = Some("openrouter".into());
    config.default_model = Some("openai/gpt-5-nano".into());
    config
}

/// The shared config plus a profile named like OpenRouter's vendor prefix,
/// so `openai/...` catalog ids look like qualified names.
fn config_with_openai_profile() -> Config {
    let mut config = config();
    config.profiles.insert(
        "openai".into(),
        profile("openai", Protocol::Responses, "https://api.openai.com/v1"),
    );
    config
}

struct Fixture {
    dir: tempfile::TempDir,
    plane: ControlPlane,
    transport: Arc<FixtureTransport>,
}

async fn fixture_plane(responses: Vec<FixtureResponse>, cache: Option<ModelCache>) -> Fixture {
    fixture_plane_with(config(), responses, cache).await
}

async fn fixture_plane_with(
    config: Config,
    responses: Vec<FixtureResponse>,
    cache: Option<ModelCache>,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    let transport = Arc::new(FixtureTransport::new(responses, 17));
    let builder = AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(StaticKey(Secret::new(KEY))),
        transport: transport.clone(),
        catalog: ModelCatalog::new(),
        cache,
        workspace: Workspace::open(dir.path()).unwrap(),
        approval: ApprovalMode::DenyAll,
        mcp: None,
    };
    Fixture {
        dir,
        plane: ControlPlane::native(Arc::new(builder)),
        transport,
    }
}

fn model_info(id: &str, reasoning: Option<bool>) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        display_name: None,
        capabilities: ModelCapabilities {
            reasoning,
            ..Default::default()
        },
        replaced_by: None,
        deprecated: false,
    }
}

/// Puts a fresh list for `profile` in the catalog with one reasoning
/// model.
fn seed_catalog(plane: &ControlPlane, profile: &str, model: &str, reasoning: Option<bool>) {
    plane.builder.catalog.insert(ModelList {
        profile: profile.into(),
        status: ModelListStatus::Fresh {
            fetched_at: Utc::now(),
        },
        models: vec![model_info(model, reasoning)],
    });
}

/// A catalog with a current model, a deprecated one the provider
/// replaces, one only a configured alias replaces, and one nobody does.
fn deprecation_models() -> Vec<ModelInfo> {
    vec![
        model_info("openai/gpt-5-nano", Some(true)),
        ModelInfo {
            deprecated: true,
            replaced_by: Some("openai/gpt-5-nano".into()),
            ..model_info("openai/gpt-4-nano", None)
        },
        ModelInfo {
            deprecated: true,
            ..model_info("openai/gpt-3-nano", None)
        },
        ModelInfo {
            deprecated: true,
            ..model_info("openai/gpt-2-nano", None)
        },
    ]
}

fn native_session(
    id: &str,
    name: &str,
    profile: &str,
    model: &str,
    workspace: &std::path::Path,
) -> Session {
    let now = Utc::now();
    Session {
        id: SessionId(id.into()),
        name: name.into(),
        kind: SessionKind::Native {
            provider_profile: profile.into(),
            model: model.into(),
            effort: ReasoningEffort::Auto,
        },
        phase: Phase::Planning,
        workspace: workspace.to_path_buf(),
        created_at: now,
        updated_at: now,
        parent_id: None,
    }
}

#[derive(Default)]
struct RecordingUi {
    events: Vec<Event>,
}

impl Ui for RecordingUi {
    fn event(&mut self, event: &Event) {
        self.events.push(event.clone());
    }

    fn approve<'a>(
        &'a mut self,
        _request: &'a ApprovalRequest,
        _decision: &'a Decision,
        _preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        Box::pin(async { ApprovalDecision::Denied })
    }
}

fn ready(
    outcome: DraftOutcome,
) -> (
    gritt_harness::draft::ResolvedDraft,
    CatalogState,
    Vec<DraftWarning>,
) {
    match outcome {
        DraftOutcome::Ready {
            draft,
            catalog,
            warnings,
        } => (draft, catalog, warnings),
        DraftOutcome::Rejected { errors, .. } => panic!("rejected: {errors:?}"),
    }
}

#[tokio::test]
async fn a_new_session_draft_resolves_profile_model_and_effort_together() {
    let fx = fixture_plane(vec![FixtureResponse::sse(fixture("stream-text.sse"))], None).await;
    seed_catalog(&fx.plane, "openrouter", "openai/gpt-5-nano", Some(true));

    // Defaults fill an empty draft.
    let (resolved, catalog, warnings) = ready(
        fx.plane
            .validate_draft(&SessionDraft::default())
            .await
            .unwrap(),
    );
    assert_eq!(resolved.profile, "openrouter");
    assert_eq!(resolved.model, "openai/gpt-5-nano");
    assert_eq!(resolved.effort, ReasoningEffort::Auto);
    assert_eq!(resolved.phase, Phase::Planning);
    assert_eq!(resolved.resume, None);
    assert!(matches!(catalog, CatalogState::Fresh { .. }));
    assert!(warnings.is_empty());

    // Explicit choices, then the session is created with the effort and
    // the first turn sends it through the adapter.
    let draft = SessionDraft::default()
        .with_name("work")
        .with_profile("openrouter")
        .with_model("openai/gpt-5-nano")
        .with_effort(ReasoningEffort::High)
        .with_phase(Phase::Coding);
    let DraftOpen::Opened {
        mut driver,
        catalog,
        ..
    } = fx.plane.open_draft(draft).await.unwrap()
    else {
        panic!("expected the draft to open");
    };
    assert!(matches!(catalog, CatalogState::Fresh { .. }));
    assert_eq!(driver.effort(), Some(ReasoningEffort::High));
    assert_eq!(driver.phase(), Phase::Coding);
    assert_eq!(driver.session().kind.effort(), Some(ReasoningEffort::High));
    let mut ui = RecordingUi::default();
    driver.run_turn("hello", &mut ui).await.unwrap();
    let body = fx.transport.requests()[0].body_json().unwrap();
    assert_eq!(body["reasoning"], serde_json::json!({ "effort": "high" }));

    let stored = fx
        .plane
        .builder
        .store
        .find_by_name("work")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.kind.effort(), Some(ReasoningEffort::High));
}

#[tokio::test]
async fn a_provider_change_invalidates_the_model_selection() {
    let fx = fixture_plane(vec![], None).await;
    // The alias belongs to `other`; picked under `openrouter` it is refused.
    let outcome = fx
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_profile("openrouter")
                .with_model("fast"),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.errors(),
        &[DraftError::ModelOutsideProfile {
            model: "fast".into(),
            model_profile: "other".into(),
            profile: "openrouter".into(),
        }]
    );
    // A qualified name from another profile is refused the same way.
    let outcome = fx
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_profile("other")
                .with_model("openrouter/openai/gpt-5-nano"),
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome.errors(),
        [DraftError::ModelOutsideProfile { .. }]
    ));
    // The builder helper clears the model when the profile changes, and
    // an unknown or missing profile is typed too.
    let cleared = SessionDraft::default()
        .with_profile("openrouter")
        .with_model("openai/gpt-5-nano")
        .with_profile("other");
    assert_eq!(cleared.model, None);
    let outcome = fx
        .plane
        .validate_draft(&SessionDraft::default().with_profile("nope"))
        .await
        .unwrap();
    assert_eq!(
        outcome.errors(),
        &[DraftError::UnknownProfile {
            profile: "nope".into()
        }]
    );
    let outcome = fx
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_profile("other")
                .with_model("model-y"),
        )
        .await
        .unwrap();
    let (resolved, catalog, _) = ready(outcome);
    assert_eq!(resolved.profile, "other");
    assert_eq!(resolved.model, "model-y");
    assert_eq!(catalog, CatalogState::Skipped);
}

#[tokio::test]
async fn effort_is_validated_against_the_profile_protocol_and_catalog() {
    let fx = fixture_plane(vec![], None).await;
    // Chat Completions with nothing reported: explicit effort is refused.
    let outcome = fx
        .plane
        .validate_draft(&SessionDraft::default().with_effort(ReasoningEffort::High))
        .await
        .unwrap();
    assert_eq!(
        outcome.errors(),
        &[DraftError::EffortUnsupported {
            profile: "openrouter".into(),
            model: "openai/gpt-5-nano".into(),
            effort: ReasoningEffort::High,
            reason: EffortUnsupportedReason::ReasoningNotReported,
        }]
    );
    // Messages: refused by protocol.
    let outcome = fx
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_profile("anthropic")
                .with_model("claude-sonnet-5")
                .with_effort(ReasoningEffort::Low),
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome.errors(),
        [DraftError::EffortUnsupported {
            reason: EffortUnsupportedReason::Protocol {
                protocol: Protocol::Messages
            },
            ..
        }]
    ));
    // Responses: allowed without list evidence; `auto` always is.
    let (resolved, _, _) = ready(
        fx.plane
            .validate_draft(
                &SessionDraft::default()
                    .with_profile("other")
                    .with_model("model-x")
                    .with_effort(ReasoningEffort::Medium),
            )
            .await
            .unwrap(),
    );
    assert_eq!(resolved.effort, ReasoningEffort::Medium);
    ready(
        fx.plane
            .validate_draft(&SessionDraft::default().with_effort(ReasoningEffort::Auto))
            .await
            .unwrap(),
    );
}

#[tokio::test]
async fn a_resumed_session_stays_pinned_and_loads_its_stored_effort() {
    let fx = fixture_plane(
        vec![
            FixtureResponse::sse(fixture("stream-text.sse")),
            FixtureResponse::sse(fixture("stream-text.sse")),
        ],
        None,
    )
    .await;
    seed_catalog(&fx.plane, "openrouter", "openai/gpt-5-nano", Some(true));
    let DraftOpen::Opened { mut driver, .. } = fx
        .plane
        .open_draft(SessionDraft::default().with_name("pinned"))
        .await
        .unwrap()
    else {
        panic!("expected open");
    };
    let id = driver.session().id.clone();
    // Effort changes between turns through the driver and persists.
    assert_eq!(
        driver.set_effort(ReasoningEffort::Low).await.unwrap(),
        EffortOutcome::Applied {
            effort: ReasoningEffort::Low
        }
    );
    let mut ui = RecordingUi::default();
    driver.run_turn("first", &mut ui).await.unwrap();
    assert_eq!(
        fx.transport.requests()[0].body_json().unwrap()["reasoning"]["effort"],
        "low"
    );
    drop(driver);

    // Asking for another profile or model is refused with the pin.
    let outcome = fx
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_name("pinned")
                .with_profile("other"),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.errors(),
        &[DraftError::SessionPinned {
            name: "pinned".into(),
            profile: "openrouter".into(),
            model: "openai/gpt-5-nano".into(),
            requested_profile: Some("other".into()),
            requested_model: None,
        }]
    );
    let outcome = fx
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_name("pinned")
                .with_model("fast"),
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome.errors(),
        [DraftError::SessionPinned { .. }]
    ));

    // The same profile and model, or nothing, resumes with the stored
    // effort and phase; the interface never has to restate them.
    let (resolved, _, _) = ready(
        fx.plane
            .validate_draft(
                &SessionDraft::default()
                    .with_name("pinned")
                    .with_profile("openrouter")
                    .with_model("openai/gpt-5-nano"),
            )
            .await
            .unwrap(),
    );
    assert_eq!(resolved.resume, Some(id.clone()));
    assert_eq!(resolved.effort, ReasoningEffort::Low);
    assert_eq!(resolved.phase, Phase::Planning);
    let DraftOpen::Opened { mut driver, .. } = fx
        .plane
        .open_draft(
            SessionDraft::default()
                .with_name("pinned")
                .with_phase(Phase::Coding),
        )
        .await
        .unwrap()
    else {
        panic!("expected resume");
    };
    assert_eq!(driver.session().id, id);
    assert_eq!(driver.effort(), Some(ReasoningEffort::Low));
    assert_eq!(driver.phase(), Phase::Coding);
    let mut ui = RecordingUi::default();
    driver.run_turn("second", &mut ui).await.unwrap();
    assert_eq!(
        fx.transport.requests()[1].body_json().unwrap()["reasoning"]["effort"],
        "low"
    );
    assert_eq!(fx.plane.builder.store.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_unsupported_effort_is_refused_by_the_driver_and_not_stored() {
    let fx = fixture_plane(vec![], None).await;
    let DraftOpen::Opened { mut driver, .. } = fx
        .plane
        .open_draft(SessionDraft::default().with_name("plain"))
        .await
        .unwrap()
    else {
        panic!("expected open");
    };
    assert_eq!(
        driver.set_effort(ReasoningEffort::High).await.unwrap(),
        EffortOutcome::Unsupported {
            effort: ReasoningEffort::High,
            reason: EffortUnsupportedReason::ReasoningNotReported,
        }
    );
    assert_eq!(driver.effort(), Some(ReasoningEffort::Auto));
    let stored = fx
        .plane
        .builder
        .store
        .find_by_name("plain")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.kind.effort(), Some(ReasoningEffort::Auto));
}

#[tokio::test]
async fn a_stale_catalog_falls_back_and_flags_an_unlisted_model() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path().join("models"));
    let fetched_at = Utc::now() - Duration::days(3);
    cache
        .write(
            "openrouter",
            &CachedModelList {
                fetched_at: Some(fetched_at),
                last_attempt_at: Some(fetched_at),
                models: vec![ModelInfo {
                    id: "openai/gpt-5-nano".into(),
                    display_name: None,
                    capabilities: ModelCapabilities {
                        reasoning: Some(true),
                        ..Default::default()
                    },
                    replaced_by: None,
                    deprecated: false,
                }],
            },
        )
        .unwrap();
    // The transport has no response, so the refresh fails.
    let fx = fixture_plane(vec![], Some(cache)).await;
    let catalog = fx.plane.catalog("openrouter").await.unwrap();
    assert_eq!(catalog.state, CatalogState::Stale { fetched_at });
    assert_eq!(catalog.models.len(), 1);
    // The stale list still validates a listed model's effort.
    let (resolved, state, warnings) = ready(
        fx.plane
            .validate_draft(&SessionDraft::default().with_effort(ReasoningEffort::Medium))
            .await
            .unwrap(),
    );
    assert_eq!(resolved.effort, ReasoningEffort::Medium);
    assert!(matches!(state, CatalogState::Stale { .. }));
    assert!(warnings.is_empty());
    // An unlisted model is allowed with a warning, as in print mode.
    let (_, _, warnings) = ready(
        fx.plane
            .validate_draft(&SessionDraft::default().with_model("openai/other"))
            .await
            .unwrap(),
    );
    assert_eq!(
        warnings,
        vec![DraftWarning::ModelNotInCatalog {
            profile: "openrouter".into(),
            model: "openai/other".into(),
        }]
    );
    // A profile with no cache and a failed refresh is reported missing.
    let other = fx.plane.catalog("other").await.unwrap();
    assert!(matches!(other.state, CatalogState::Missing { .. }));
    assert!(other.models.is_empty());
    let text = format!("{other:?}");
    assert!(!text.contains(KEY));
    drop(fx);
}

#[tokio::test]
async fn connector_sessions_are_outside_the_native_draft() {
    let fx = fixture_plane(vec![], None).await;
    let now = Utc::now();
    fx.plane
        .builder
        .store
        .create(Session {
            id: SessionId("c1".into()),
            name: "codex-work".into(),
            kind: SessionKind::Connector {
                id: ConnectorId::Codex,
            },
            phase: Phase::Coding,
            workspace: fx.dir.path().to_path_buf(),
            created_at: now,
            updated_at: now,
            parent_id: None,
        })
        .await
        .unwrap();
    let outcome = fx
        .plane
        .validate_draft(&SessionDraft::default().with_name("codex-work"))
        .await
        .unwrap();
    assert_eq!(
        outcome.errors(),
        &[DraftError::ConnectorSession {
            name: "codex-work".into(),
            connector: ConnectorId::Codex,
        }]
    );
    // A session from another workspace is typed too.
    fx.plane
        .builder
        .store
        .create(Session {
            id: SessionId("w1".into()),
            name: "elsewhere".into(),
            kind: SessionKind::Native {
                provider_profile: "openrouter".into(),
                model: "openai/gpt-5-nano".into(),
                effort: ReasoningEffort::Auto,
            },
            phase: Phase::Planning,
            workspace: "/nonexistent/other-workspace".into(),
            created_at: now,
            updated_at: now,
            parent_id: None,
        })
        .await
        .unwrap();
    let outcome = fx
        .plane
        .validate_draft(&SessionDraft::default().with_name("elsewhere"))
        .await
        .unwrap();
    assert!(matches!(
        outcome.errors(),
        [DraftError::OtherWorkspace { .. }]
    ));
}

#[tokio::test]
async fn profile_summaries_report_availability_without_values() {
    let fx = fixture_plane(vec![], None).await;
    let summaries = fx.plane.profile_summaries();
    assert_eq!(summaries.len(), 3);
    let openrouter = summaries.iter().find(|s| s.name == "openrouter").unwrap();
    assert!(openrouter.is_default);
    assert_eq!(openrouter.credential, CredentialState::Available);
    assert_eq!(openrouter.protocol, Protocol::ChatCompletions);
    let text = serde_json::to_string(&summaries).unwrap();
    assert!(!text.contains(KEY));
    // No setup writer was injected: writes are unavailable, never a panic.
    let outcome = fx.plane.setup().store_credential(
        &fx.plane.builder.config.profiles["openrouter"],
        Secret::new("sk-x"),
    );
    assert!(matches!(
        outcome,
        gritt_harness::setup::CredentialStoreOutcome::Unavailable { .. }
    ));
}

#[tokio::test]
async fn catalog_ids_with_a_profile_name_prefix_stay_in_the_selected_profile() {
    let fx = fixture_plane_with(config_with_openai_profile(), vec![], None).await;
    seed_catalog(&fx.plane, "openrouter", "openai/gpt-5-nano", Some(true));
    // Creation: the OpenRouter catalog id is not read as `openai/...`.
    let (resolved, _, warnings) = ready(
        fx.plane
            .validate_draft(
                &SessionDraft::default()
                    .with_profile("openrouter")
                    .with_model("openai/gpt-5-nano")
                    .with_effort(ReasoningEffort::Low),
            )
            .await
            .unwrap(),
    );
    assert_eq!(resolved.profile, "openrouter");
    assert_eq!(resolved.model, "openai/gpt-5-nano");
    assert!(warnings.is_empty());
    // A qualified name that is not a catalog id still routes by profile.
    let outcome = fx
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_profile("openrouter")
                .with_model("openai/gpt-5-mini"),
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.errors(),
        &[DraftError::ModelOutsideProfile {
            model: "openai/gpt-5-mini".into(),
            model_profile: "openai".into(),
            profile: "openrouter".into(),
        }]
    );
    let DraftOpen::Opened { driver, .. } = fx
        .plane
        .open_draft(
            SessionDraft::default()
                .with_name("prefixed")
                .with_profile("openrouter")
                .with_model("openai/gpt-5-nano"),
        )
        .await
        .unwrap()
    else {
        panic!("expected open");
    };
    let id = driver.session().id.clone();
    drop(driver);
    // Resume with the same catalog id, with and without the profile.
    for draft in [
        SessionDraft::default()
            .with_name("prefixed")
            .with_model("openai/gpt-5-nano"),
        SessionDraft::default()
            .with_name("prefixed")
            .with_profile("openrouter")
            .with_model("openai/gpt-5-nano"),
    ] {
        let (resolved, _, _) = ready(fx.plane.validate_draft(&draft).await.unwrap());
        assert_eq!(resolved.resume, Some(id.clone()));
        assert_eq!(resolved.profile, "openrouter");
    }

    // Resume compares the exact stored id even when no catalog is loaded.
    let cold = fixture_plane_with(config_with_openai_profile(), vec![], None).await;
    let now = Utc::now();
    cold.plane
        .builder
        .store
        .create(Session {
            id: SessionId("p1".into()),
            name: "stored".into(),
            kind: SessionKind::Native {
                provider_profile: "openrouter".into(),
                model: "openai/gpt-5-nano".into(),
                effort: ReasoningEffort::Auto,
            },
            phase: Phase::Planning,
            workspace: cold.dir.path().to_path_buf(),
            created_at: now,
            updated_at: now,
            parent_id: None,
        })
        .await
        .unwrap();
    let (resolved, catalog, _) = ready(
        cold.plane
            .validate_draft(
                &SessionDraft::default()
                    .with_name("stored")
                    .with_model("openai/gpt-5-nano"),
            )
            .await
            .unwrap(),
    );
    assert_eq!(resolved.resume, Some(SessionId("p1".into())));
    assert_eq!(catalog, CatalogState::Skipped);
    let outcome = cold
        .plane
        .validate_draft(
            &SessionDraft::default()
                .with_name("stored")
                .with_model("openai/gpt-5-mini"),
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome.errors(),
        [DraftError::SessionPinned { .. }]
    ));
}

#[tokio::test]
async fn a_removed_profile_rejects_resume_without_touching_the_session() {
    let fx = fixture_plane(vec![], None).await;
    let now = Utc::now();
    let stored = Session {
        id: SessionId("g1".into()),
        name: "orphan".into(),
        kind: SessionKind::Native {
            provider_profile: "gone".into(),
            model: "some-model".into(),
            effort: ReasoningEffort::Auto,
        },
        phase: Phase::Planning,
        workspace: fx.dir.path().to_path_buf(),
        created_at: now,
        updated_at: now,
        parent_id: None,
    };
    fx.plane.builder.store.create(stored.clone()).await.unwrap();
    let draft = SessionDraft::default()
        .with_name("orphan")
        .with_effort(ReasoningEffort::High)
        .with_phase(Phase::Coding);
    let outcome = fx.plane.validate_draft(&draft).await.unwrap();
    assert_eq!(
        outcome,
        DraftOutcome::Rejected {
            errors: vec![DraftError::UnknownProfile {
                profile: "gone".into()
            }],
            catalog: None,
        }
    );
    assert!(matches!(
        fx.plane.open_draft(draft).await.unwrap(),
        DraftOpen::Rejected { .. }
    ));
    let after = fx
        .plane
        .builder
        .store
        .get(&SessionId("g1".into()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, stored);
    assert!(fx
        .plane
        .builder
        .store
        .read_events(&SessionId("g1".into()))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(fx.plane.builder.store.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn deprecated_catalog_ids_are_remapped_or_rejected_on_creation_and_resume() {
    let mut config = config_with_openai_profile();
    config
        .profiles
        .get_mut("openrouter")
        .unwrap()
        .aliases
        .insert("openai/gpt-3-nano".into(), "openai/gpt-5-nano".into());
    let fx = fixture_plane_with(config, vec![], None).await;
    fx.plane.builder.catalog.insert(ModelList {
        profile: "openrouter".into(),
        status: ModelListStatus::Fresh {
            fetched_at: Utc::now(),
        },
        models: deprecation_models(),
    });
    let draft = |model: &str| {
        SessionDraft::default()
            .with_profile("openrouter")
            .with_model(model)
    };

    // Creation: provider-declared replacement, then configured alias.
    for old in ["openai/gpt-4-nano", "openai/gpt-3-nano"] {
        let (resolved, _, warnings) = ready(fx.plane.validate_draft(&draft(old)).await.unwrap());
        assert_eq!(resolved.profile, "openrouter");
        assert_eq!(resolved.model, "openai/gpt-5-nano", "{old}");
        assert_eq!(
            warnings,
            vec![DraftWarning::DeprecatedModelRemapped {
                from: old.into(),
                to: "openai/gpt-5-nano".into(),
            }]
        );
    }
    // Creation: no replacement anywhere is a typed rejection, and nothing
    // is created.
    let outcome = fx
        .plane
        .validate_draft(&draft("openai/gpt-2-nano"))
        .await
        .unwrap();
    assert!(
        matches!(
            outcome.errors(),
            [DraftError::ModelResolution { model, message }]
                if model == "openai/gpt-2-nano" && message.contains("deprecated")
        ),
        "{outcome:?}"
    );
    assert!(matches!(
        fx.plane
            .open_draft(draft("openai/gpt-2-nano").with_name("dead"))
            .await
            .unwrap(),
        DraftOpen::Rejected { .. }
    ));
    assert!(fx.plane.builder.store.list().await.unwrap().is_empty());
    // The stored session carries the replacement, never the deprecated id.
    let DraftOpen::Opened { driver, .. } = fx
        .plane
        .open_draft(draft("openai/gpt-4-nano").with_name("remapped"))
        .await
        .unwrap()
    else {
        panic!("expected open");
    };
    assert!(matches!(
        &driver.session().kind,
        SessionKind::Native { model, .. } if model == "openai/gpt-5-nano"
    ));
    let id = driver.session().id.clone();
    drop(driver);

    // Resume: a deprecated name whose replacement is the stored model
    // resumes; one with no replacement cannot match the pin.
    let (resolved, _, _) = ready(
        fx.plane
            .validate_draft(&draft("openai/gpt-4-nano").with_name("remapped"))
            .await
            .unwrap(),
    );
    assert_eq!(resolved.resume, Some(id.clone()));
    assert_eq!(resolved.model, "openai/gpt-5-nano");
    let outcome = fx
        .plane
        .validate_draft(&draft("openai/gpt-2-nano").with_name("remapped"))
        .await
        .unwrap();
    assert!(matches!(
        outcome.errors(),
        [DraftError::SessionPinned { .. }]
    ));
    let stored = fx.plane.builder.store.get(&id).await.unwrap().unwrap();
    assert!(matches!(
        &stored.kind,
        SessionKind::Native { model, .. } if model == "openai/gpt-5-nano"
    ));
}

#[tokio::test]
async fn resume_resolves_against_the_catalog_it_just_warmed() {
    // Nothing is in the in-memory catalog; only the disk cache knows the
    // deprecation, and it is fresh so no refresh is attempted.
    let dir = tempfile::tempdir().unwrap();
    let cache = ModelCache::new(dir.path().join("models"));
    cache
        .write(
            "openrouter",
            &CachedModelList {
                fetched_at: Some(Utc::now()),
                last_attempt_at: Some(Utc::now()),
                models: deprecation_models(),
            },
        )
        .unwrap();
    let fx = fixture_plane_with(config_with_openai_profile(), vec![], Some(cache)).await;
    assert!(fx.plane.builder.catalog.list("openrouter").is_none());
    fx.plane
        .builder
        .store
        .create(native_session(
            "r1",
            "cold",
            "openrouter",
            "openai/gpt-5-nano",
            fx.dir.path(),
        ))
        .await
        .unwrap();
    let (resolved, catalog, _) = ready(
        fx.plane
            .validate_draft(
                &SessionDraft::default()
                    .with_name("cold")
                    .with_model("openai/gpt-4-nano"),
            )
            .await
            .unwrap(),
    );
    assert_eq!(resolved.resume, Some(SessionId("r1".into())));
    assert_eq!(resolved.model, "openai/gpt-5-nano");
    assert!(matches!(catalog, CatalogState::Fresh { .. }));
    assert!(fx.plane.builder.catalog.list("openrouter").is_some());
}
