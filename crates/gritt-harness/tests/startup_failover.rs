//! Startup failover and remembered choices through the shared resolver
//! (TKT-0022): the fallback order, every failure class with redaction, the
//! aggregate error, last-used precedence, resume pinning, a legacy
//! database, and the same resolver behind the draft and flag paths.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::Utc;
use gritt_core::config::Config;
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event};
use gritt_core::provider::{
    ModelCapabilities, ModelInfo, ModelList, ModelListStatus, Protocol, ProviderProfile,
    ReasoningEffort,
};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, LastUsedNative, SessionKind};
use gritt_core::{Error, ErrorKind, Result};
use gritt_harness::agent::{AgentBuilder, ApprovalMode, SessionSelector, Ui};
use gritt_harness::control::{ControlPlane, DraftOpen};
use gritt_harness::draft::{CatalogState, DraftError, DraftOutcome, DraftWarning, SessionDraft};
use gritt_harness::policy::Decision;
use gritt_harness::startup::{FailureClass, StartupOutcome, StartupRequest};
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_provider::adapter::KeyProvider;
use gritt_provider::models::{ModelCache, ModelCatalog};
use gritt_provider::{FixtureResponse, FixtureTransport};

const KEY: &str = "fixture-key-never-printed";

/// A key per profile. A profile without one has missing credentials.
struct MapKeys(BTreeMap<String, Secret>);

impl KeyProvider for MapKeys {
    fn key(&self, profile: &str, reference: &SecretRef) -> Result<Secret> {
        self.0
            .get(profile)
            .cloned()
            .ok_or_else(|| Error::missing_key(profile, &reference.env_var_name))
    }
}

fn keys(profiles: &[&str]) -> Arc<dyn KeyProvider> {
    Arc::new(MapKeys(
        profiles
            .iter()
            .map(|name| ((*name).to_owned(), Secret::new(format!("{KEY}-{name}"))))
            .collect(),
    ))
}

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

/// The default profile, two Chat Completions fallbacks, and an Anthropic
/// profile with its own fallback model, in that order.
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
        "backup".into(),
        profile(
            "backup",
            Protocol::ChatCompletions,
            "https://backup.example/v1",
        ),
    );
    config.profiles.insert(
        "local".into(),
        profile("local", Protocol::ChatCompletions, "http://127.0.0.1:1/v1"),
    );
    let mut anthropic = profile("anthropic", Protocol::Messages, "https://api.anthropic.com");
    anthropic.fallback_model = Some("claude-sonnet-5".into());
    config.profiles.insert("anthropic".into(), anthropic);
    config.default_profile = Some("openrouter".into());
    config.default_model = Some("openai/gpt-5-nano".into());
    config.fallback_profiles = vec!["backup".into(), "anthropic".into(), "local".into()];
    config
}

/// A model list answer with reasoning reported for every id.
fn models(ids: &[&str]) -> FixtureResponse {
    let data: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| serde_json::json!({ "id": id, "supported_parameters": ["reasoning", "tools"] }))
        .collect();
    FixtureResponse::json(200, serde_json::json!({ "data": data }).to_string())
}

fn text_turn() -> FixtureResponse {
    FixtureResponse::sse(fixture("stream-text.sse"))
}

struct Fixture {
    _dir: tempfile::TempDir,
    plane: ControlPlane,
    transport: Arc<FixtureTransport>,
}

async fn fixture_plane(
    config: Config,
    responses: Vec<FixtureResponse>,
    keys: Arc<dyn KeyProvider>,
    with_cache: bool,
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
        keys,
        transport: transport.clone(),
        catalog: ModelCatalog::new(),
        cache: with_cache.then(|| ModelCache::new(dir.path().join("models"))),
        workspace: Workspace::open(dir.path()).unwrap(),
        approval: ApprovalMode::DenyAll,
        mcp: None,
    };
    Fixture {
        _dir: dir,
        plane: ControlPlane::native(Arc::new(builder)),
        transport,
    }
}

/// The same plane over the same store with a different configuration,
/// the way a config reload or a later run sees it.
fn reconfigured(fx: &Fixture, edit: impl FnOnce(&mut Config)) -> ControlPlane {
    let mut builder = (*fx.plane.builder).clone();
    edit(&mut builder.config);
    ControlPlane::native(Arc::new(builder))
}

fn seed_catalog(plane: &ControlPlane, profile: &str, ids: &[&str]) {
    plane.builder.catalog.insert(ModelList {
        profile: profile.into(),
        status: ModelListStatus::Fresh {
            fetched_at: Utc::now(),
        },
        models: ids
            .iter()
            .map(|id| ModelInfo {
                id: (*id).into(),
                display_name: None,
                capabilities: ModelCapabilities {
                    reasoning: Some(true),
                    ..Default::default()
                },
                replaced_by: None,
                deprecated: false,
            })
            .collect(),
    });
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

fn ready(outcome: StartupOutcome) -> gritt_harness::startup::StartupSelection {
    match outcome {
        StartupOutcome::Ready(selection) => selection,
        StartupOutcome::Rejected { errors, .. } => panic!("rejected: {errors:?}"),
    }
}

fn skipped(warnings: &[DraftWarning]) -> Vec<(String, FailureClass)> {
    warnings
        .iter()
        .filter_map(|warning| match warning {
            DraftWarning::ProfileSkipped(entry) => Some((entry.profile.clone(), entry.class)),
            _ => None,
        })
        .collect()
}

fn native(kind: &SessionKind) -> (String, String, ReasoningEffort) {
    match kind {
        SessionKind::Native {
            provider_profile,
            model,
            effort,
        } => (provider_profile.clone(), model.clone(), *effort),
        SessionKind::Connector { .. } => panic!("native session expected"),
    }
}

#[tokio::test]
async fn the_default_profile_is_tried_first_then_the_fallback_order() {
    let fx = fixture_plane(
        config(),
        vec![
            FixtureResponse::json(503, r#"{"error":{"message":"overloaded"}}"#),
            models(&["openai/gpt-5-nano"]),
        ],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        true,
    )
    .await;
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::default())
            .await
            .unwrap(),
    );
    assert_eq!(selection.profile, "backup");
    assert_eq!(selection.model, "openai/gpt-5-nano");
    assert_eq!(selection.effort, ReasoningEffort::Auto);
    assert!(matches!(selection.catalog, CatalogState::Fresh { .. }));
    assert_eq!(
        skipped(&selection.warnings),
        vec![("openrouter".to_owned(), FailureClass::Provider)]
    );
    let urls: Vec<String> = fx
        .transport
        .requests()
        .iter()
        .map(|request| request.url.clone())
        .collect();
    assert_eq!(
        urls,
        vec![
            "https://openrouter.ai/api/v1/models",
            "https://backup.example/v1/models"
        ]
    );
    // The probe left a usable list for the selected profile.
    assert!(fx
        .plane
        .builder
        .catalog
        .model("backup", "openai/gpt-5-nano")
        .is_some());
    let text = format!("{:?}", selection.warnings);
    assert!(text.contains("overloaded"), "{text}");
    assert!(!text.contains(KEY), "{text}");
}

#[tokio::test]
async fn every_failure_class_moves_on_and_the_aggregate_names_them_without_a_key() {
    let fx = fixture_plane(
        config(),
        vec![
            // backup: the key is echoed back in a 401 body and must not survive.
            FixtureResponse::json(
                401,
                format!(r#"{{"error":{{"message":"bad key {KEY}-backup"}}}}"#),
            ),
            // anthropic: an answer that is not a model list.
            FixtureResponse::json(200, "<html>maintenance</html>"),
            // local: nothing queued, the request never gets an answer.
        ],
        keys(&["backup", "anthropic", "local"]),
        true,
    )
    .await;
    let outcome = fx
        .plane
        .builder
        .resolve_startup(&StartupRequest::default())
        .await
        .unwrap();
    let StartupOutcome::Rejected { errors, catalog } = outcome else {
        panic!("expected every candidate to fail");
    };
    assert_eq!(catalog, None);
    let [DraftError::NoUsableProfile { skipped }] = errors.as_slice() else {
        panic!("expected one aggregate error: {errors:?}");
    };
    let classes: Vec<(&str, FailureClass)> = skipped
        .iter()
        .map(|entry| (entry.profile.as_str(), entry.class))
        .collect();
    assert_eq!(
        classes,
        vec![
            ("openrouter", FailureClass::MissingCredentials),
            ("backup", FailureClass::Authentication),
            ("anthropic", FailureClass::Provider),
            ("local", FailureClass::Connection),
        ]
    );
    assert!(skipped[0].message.contains("OPENROUTER_API_KEY"));
    assert!(skipped[1].message.contains("401"), "{}", skipped[1].message);
    assert!(
        skipped[2].message.contains("model list"),
        "{}",
        skipped[2].message
    );
    let error = errors[0].clone().into_error();
    assert_eq!(error.kind, ErrorKind::NoUsableProfile);
    for name in ["openrouter", "backup", "anthropic", "local"] {
        assert!(error.message.contains(name), "{}", error.message);
    }
    for text in [
        error.message.clone(),
        format!("{:?}", error.diagnostic),
        format!("{skipped:?}"),
    ] {
        assert!(!text.contains(KEY), "{text}");
    }
    // The key-less default was never asked anything; the chain stopped
    // asking once every candidate had answered.
    assert_eq!(fx.transport.request_count(), 3);
}

#[tokio::test]
async fn an_unavailable_model_falls_back_to_a_profile_that_offers_one() {
    // The requested model is missing from the default's list and present
    // in the next profile's.
    let fx = fixture_plane(
        config(),
        vec![
            models(&["openai/gpt-5-nano"]),
            models(&["openai/gpt-5-nano", "openai/gpt-x"]),
        ],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        true,
    )
    .await;
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                None,
                Some("openai/gpt-x"),
                None,
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("backup", "openai/gpt-x")
    );
    assert_eq!(
        skipped(&selection.warnings),
        vec![("openrouter".to_owned(), FailureClass::ModelUnavailable)]
    );

    // A profile with a configured fallback model uses it when neither the
    // requested nor the default model is in its list; one without is
    // skipped for having no compatible model.
    let fx = fixture_plane(
        config(),
        vec![
            models(&["openai/gpt-5-nano"]),
            models(&["something/else"]),
            models(&["claude-sonnet-5", "claude-haiku-5"]),
        ],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        true,
    )
    .await;
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(None, Some("special"), None))
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("anthropic", "claude-sonnet-5")
    );
    let classes = skipped(&selection.warnings);
    assert_eq!(
        classes,
        vec![
            ("openrouter".to_owned(), FailureClass::ModelUnavailable),
            ("backup".to_owned(), FailureClass::ModelUnavailable),
        ]
    );
    let DraftWarning::ProfileSkipped(entry) = &selection.warnings[1] else {
        panic!("skipped warning expected");
    };
    assert!(
        entry.message.contains("no compatible model"),
        "{}",
        entry.message
    );
    assert!(entry.message.contains("`special`"), "{}", entry.message);

    // A requested model the fallback profile lists beats that profile's
    // own fallback model; the fallback model is for when it does not.
    let fx = fixture_plane(
        config(),
        vec![
            models(&["openai/gpt-5-nano"]),
            models(&["something/else"]),
            models(&["claude-sonnet-5", "claude-haiku-5"]),
        ],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        true,
    )
    .await;
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                None,
                Some("claude-haiku-5"),
                None,
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("anthropic", "claude-haiku-5")
    );
}

#[tokio::test]
async fn a_qualified_default_model_names_its_profile_and_a_removed_remembered_profile_is_dropped() {
    // `default_model` spells out a profile, as it did before failover
    // existed, and outranks `default_profile`.
    let mut config = config();
    config.default_model = Some("backup/model-b".into());
    config.fallback_profiles.clear();
    let fx = fixture_plane(
        config.clone(),
        vec![],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        false,
    )
    .await;
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::default())
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("backup", "model-b")
    );
    assert_eq!(
        fx.plane
            .builder
            .session_profile(&SessionSelector::New { name: None }, None, None)
            .await
            .unwrap(),
        "backup"
    );

    // A remembered profile that is no longer configured does not block
    // the chain: the remaining fallback order is used instead.
    config.default_profile = None;
    config.default_model = None;
    config.fallback_profiles = vec!["backup".into()];
    let fx = fixture_plane(
        config,
        vec![],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        false,
    )
    .await;
    fx.plane
        .builder
        .store
        .set_last_used(
            fx.plane.builder.workspace_root(),
            &LastUsedNative {
                provider_profile: "gone".into(),
                model: "model-g".into(),
                effort: ReasoningEffort::High,
            },
        )
        .await
        .unwrap();
    seed_catalog(&fx.plane, "backup", &["model-b"]);
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(None, Some("model-b"), None))
            .await
            .unwrap(),
    );
    assert_eq!(selection.profile, "backup");
    assert_eq!(selection.effort, ReasoningEffort::High);
    assert!(skipped(&selection.warnings).is_empty());
}

#[tokio::test]
async fn an_explicit_profile_is_pinned_and_keeps_the_single_candidate_rules() {
    // Nothing is queued: a live probe would fail. The pinned profile is
    // loaded the old way and a missing list is allowed with the model
    // unverified, and no fallback is consulted.
    let fx = fixture_plane(
        config(),
        vec![],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        true,
    )
    .await;
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                Some("openrouter"),
                Some("openai/typed"),
                None,
            ))
            .await
            .unwrap(),
    );
    assert_eq!(selection.profile, "openrouter");
    assert_eq!(selection.model, "openai/typed");
    assert!(matches!(selection.catalog, CatalogState::Missing { .. }));
    assert!(skipped(&selection.warnings).is_empty());
    assert_eq!(fx.transport.request_count(), 1);

    // A qualified model name pins its profile the same way.
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                None,
                Some("backup/model-b"),
                None,
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("backup", "model-b")
    );

    // A pinned profile without a key still opens, as it did before
    // failover existed: the adapter reports the missing key on the first
    // request. Only a chain treats it as a reason to move on.
    let fx = fixture_plane(config(), vec![], keys(&["backup"]), true).await;
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(Some("openrouter"), None, None))
            .await
            .unwrap(),
    );
    assert_eq!(selection.profile, "openrouter");
    assert!(matches!(selection.catalog, CatalogState::Missing { .. }));
    assert!(skipped(&selection.warnings).is_empty());

    // A model that names another profile wins over the `--profile` hint,
    // as alias resolution always did, and pins that profile.
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                Some("openrouter"),
                Some("backup/model-b"),
                None,
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("backup", "model-b")
    );
    // Unless the hinted profile's own list carries that id: then it is that
    // profile's model, not a qualified name.
    seed_catalog(&fx.plane, "backup", &["local/shared-id"]);
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                Some("backup"),
                Some("local/shared-id"),
                None,
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("backup", "local/shared-id")
    );
}

#[tokio::test]
async fn a_completed_turn_remembers_the_choices_and_flags_and_config_win_over_them() {
    // No configured defaults: the remembered choices are what fills the
    // gaps.
    let mut config = config();
    config.default_profile = None;
    config.default_model = None;
    config.fallback_profiles.clear();
    let fx = fixture_plane(
        config,
        vec![text_turn(), text_turn()],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        false,
    )
    .await;
    seed_catalog(
        &fx.plane,
        "openrouter",
        &["openai/gpt-5-nano", "openai/gpt-5-mini"],
    );
    let workspace = fx.plane.builder.workspace_root().to_path_buf();
    assert_eq!(fx.plane.builder.last_used().await.unwrap(), None);
    // Nothing chosen, nothing configured, nothing remembered: no session.
    let error = match fx
        .plane
        .open_with(
            SessionSelector::New { name: None },
            None,
            StartupRequest::default(),
            None,
        )
        .await
    {
        Ok(_) => panic!("nothing should open"),
        Err(error) => error,
    };
    assert_eq!(error.kind, ErrorKind::Config);
    assert!(
        error.message.contains("no profile given"),
        "{}",
        error.message
    );

    let opened = fx
        .plane
        .open_with(
            SessionSelector::New { name: None },
            None,
            StartupRequest::from_flags(
                Some("openrouter"),
                Some("openai/gpt-5-nano"),
                Some(ReasoningEffort::High),
            ),
            None,
        )
        .await
        .unwrap();
    assert!(opened.warnings.is_empty());
    assert!(matches!(opened.catalog, Some(CatalogState::Fresh { .. })));
    let mut driver = opened.driver;
    // Opening alone remembers nothing; a completed turn does.
    assert_eq!(
        fx.plane.builder.store.last_used(&workspace).await.unwrap(),
        None
    );
    let mut ui = RecordingUi::default();
    driver.run_turn("hello", &mut ui).await.unwrap();
    assert_eq!(
        fx.plane.builder.store.last_used(&workspace).await.unwrap(),
        Some(LastUsedNative {
            provider_profile: "openrouter".into(),
            model: "openai/gpt-5-nano".into(),
            effort: ReasoningEffort::High,
        })
    );

    // The next new session, asked for nothing, starts on those choices.
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::default())
            .await
            .unwrap(),
    );
    assert_eq!(
        (
            selection.profile.as_str(),
            selection.model.as_str(),
            selection.effort
        ),
        ("openrouter", "openai/gpt-5-nano", ReasoningEffort::High)
    );
    assert_eq!(
        selection.warnings,
        vec![DraftWarning::LastUsedApplied {
            profile: Some("openrouter".into()),
            model: Some("openai/gpt-5-nano".into()),
            effort: Some(ReasoningEffort::High),
        }]
    );

    // Explicit flags win field by field; the remembered profile still
    // fills the gap they leave.
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                None,
                Some("openai/gpt-5-mini"),
                Some(ReasoningEffort::Low),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        (
            selection.profile.as_str(),
            selection.model.as_str(),
            selection.effort
        ),
        ("openrouter", "openai/gpt-5-mini", ReasoningEffort::Low)
    );
    assert_eq!(
        selection.warnings,
        vec![DraftWarning::LastUsedApplied {
            profile: Some("openrouter".into()),
            model: None,
            effort: None,
        }]
    );

    // A qualified model name pins its own profile; nothing about that
    // choice is remembered, so no note claims it was.
    let selection = ready(
        fx.plane
            .builder
            .resolve_startup(&StartupRequest::from_flags(
                None,
                Some("openrouter/openai/gpt-5-mini"),
                Some(ReasoningEffort::Low),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        (selection.profile.as_str(), selection.model.as_str()),
        ("openrouter", "openai/gpt-5-mini")
    );
    assert!(selection.warnings.is_empty(), "{:?}", selection.warnings);

    // A draft that chose a model wins the same way.
    let outcome = fx
        .plane
        .validate_draft(&SessionDraft::default().with_model("openai/gpt-5-mini"))
        .await
        .unwrap();
    let DraftOutcome::Ready { draft, .. } = outcome else {
        panic!("draft rejected: {outcome:?}");
    };
    assert_eq!(draft.model, "openai/gpt-5-mini");
    assert_eq!(draft.effort, ReasoningEffort::High);

    // The remembered profile and model beat configured defaults; the
    // defaults fill in only what nothing else names.
    let configured = reconfigured(&fx, |config| {
        config.default_profile = Some("backup".into());
        config.default_model = Some("openai/gpt-5-nano".into());
    });
    let selection = ready(
        configured
            .builder
            .resolve_startup(&StartupRequest::default())
            .await
            .unwrap(),
    );
    assert_eq!(
        (
            selection.profile.as_str(),
            selection.model.as_str(),
            selection.effort
        ),
        ("openrouter", "openai/gpt-5-nano", ReasoningEffort::High)
    );
    assert_eq!(
        selection.warnings,
        vec![DraftWarning::LastUsedApplied {
            profile: Some("openrouter".into()),
            model: Some("openai/gpt-5-nano".into()),
            effort: Some(ReasoningEffort::High),
        }]
    );
    // An explicit profile takes the remembered effort along, and returns
    // it to the provider default with a note where the model cannot take
    // it.
    let selection = ready(
        configured
            .builder
            .resolve_startup(&StartupRequest::from_flags(Some("backup"), None, None))
            .await
            .unwrap(),
    );
    assert_eq!(selection.profile, "backup");
    assert_eq!(selection.model, "openai/gpt-5-nano");
    assert_eq!(selection.effort, ReasoningEffort::Auto);
    assert!(matches!(
        selection.warnings.as_slice(),
        [DraftWarning::EffortReset {
            effort: ReasoningEffort::High,
            ..
        }]
    ));
    seed_catalog(&configured, "backup", &["openai/gpt-5-nano"]);
    let selection = ready(
        configured
            .builder
            .resolve_startup(&StartupRequest::from_flags(Some("backup"), None, None))
            .await
            .unwrap(),
    );
    assert_eq!(selection.effort, ReasoningEffort::High);
    assert_eq!(
        selection.warnings,
        vec![DraftWarning::LastUsedApplied {
            profile: None,
            model: None,
            effort: Some(ReasoningEffort::High),
        }]
    );

    // A failed turn on a new session records nothing new.
    let failing = fx
        .plane
        .open_with(
            SessionSelector::New { name: None },
            None,
            StartupRequest::from_flags(Some("openrouter"), Some("openai/gpt-5-mini"), None),
            None,
        )
        .await
        .unwrap();
    let mut driver = failing.driver;
    // One turn is queued; the second finds nothing and fails.
    driver.run_turn("first", &mut ui).await.unwrap();
    let remembered = fx.plane.builder.store.last_used(&workspace).await.unwrap();
    assert_eq!(
        remembered.as_ref().map(|last| last.model.as_str()),
        Some("openai/gpt-5-mini")
    );
    let _ = driver.run_turn("second", &mut ui).await;
    assert_eq!(
        fx.plane.builder.store.last_used(&workspace).await.unwrap(),
        remembered
    );
}

#[tokio::test]
async fn a_resumed_session_keeps_its_profile_and_model_when_the_configuration_changes() {
    let fx = fixture_plane(
        config(),
        vec![models(&["openai/gpt-5-nano"]), text_turn(), text_turn()],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        true,
    )
    .await;
    let opened = fx
        .plane
        .open_with(
            SessionSelector::Named("keep".into()),
            None,
            StartupRequest::default(),
            None,
        )
        .await
        .unwrap();
    let mut driver = opened.driver;
    assert_eq!(native(&driver.session().kind).0, "openrouter");
    let mut ui = RecordingUi::default();
    driver.run_turn("one", &mut ui).await.unwrap();
    drop(driver);
    let probes_before = fx.transport.request_count();

    // The default, the fallback order, and the remembered choices all
    // point elsewhere now.
    let changed = reconfigured(&fx, |config| {
        config.default_profile = Some("backup".into());
        config.default_model = Some("openai/gpt-x".into());
        config.fallback_profiles = vec!["anthropic".into()];
    });
    changed
        .builder
        .store
        .set_last_used(
            fx.plane.builder.workspace_root(),
            &LastUsedNative {
                provider_profile: "anthropic".into(),
                model: "claude-sonnet-5".into(),
                effort: ReasoningEffort::Low,
            },
        )
        .await
        .unwrap();
    let resumed = changed
        .open_with(
            SessionSelector::Named("keep".into()),
            None,
            StartupRequest::from_flags(None, None, Some(ReasoningEffort::High)),
            None,
        )
        .await
        .unwrap();
    assert!(resumed.warnings.is_empty());
    assert_eq!(
        native(&resumed.driver.session().kind),
        (
            "openrouter".to_owned(),
            "openai/gpt-5-nano".to_owned(),
            ReasoningEffort::Auto
        )
    );
    // No profile was probed for a resume.
    assert_eq!(fx.transport.request_count(), probes_before);

    // The draft path pins the same way, and a draft asking for the new
    // default is refused rather than moved.
    let outcome = changed
        .validate_draft(&SessionDraft::default().with_name("keep"))
        .await
        .unwrap();
    let DraftOutcome::Ready { draft, .. } = outcome else {
        panic!("resume rejected: {outcome:?}");
    };
    assert_eq!(draft.profile, "openrouter");
    assert_eq!(draft.model, "openai/gpt-5-nano");
    assert!(draft.resume.is_some());
    let outcome = changed
        .validate_draft(
            &SessionDraft::default()
                .with_name("keep")
                .with_profile("backup"),
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome.errors(),
        [DraftError::SessionPinned { .. }]
    ));
    // A completed turn on the resumed session leaves the remembered
    // choices alone: only a new session records them.
    let mut driver = resumed.driver;
    driver.run_turn("two", &mut ui).await.unwrap();
    assert_eq!(
        changed
            .builder
            .store
            .last_used(fx.plane.builder.workspace_root())
            .await
            .unwrap()
            .map(|last| last.provider_profile),
        Some("anthropic".to_owned())
    );
}

#[tokio::test]
async fn a_database_from_before_the_last_used_table_upgrades_and_remembers_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let location = DatabaseLocation::Explicit(dir.path().join("gritt.db"));
    {
        let store = Store::open(location.clone()).await.unwrap();
        store
            .connection()
            .execute_batch("DROP TABLE gritt_last_used")
            .await
            .unwrap();
        store
            .connection()
            .execute(
                "DELETE FROM gritt_schema_migrations WHERE name = ?1",
                turso::params!["0005_last_used"],
            )
            .await
            .unwrap();
    }
    let store = Store::open(location).await.unwrap();
    assert!(store
        .applied_migrations()
        .await
        .unwrap()
        .iter()
        .any(|name| name == "0005_last_used"));
    assert_eq!(store.last_used(dir.path()).await.unwrap(), None);
    let last = LastUsedNative {
        provider_profile: "openrouter".into(),
        model: "openai/gpt-5-nano".into(),
        effort: ReasoningEffort::Medium,
    };
    store.set_last_used(dir.path(), &last).await.unwrap();
    assert_eq!(store.last_used(dir.path()).await.unwrap(), Some(last));
}

#[tokio::test]
async fn the_draft_path_and_the_flag_path_share_the_resolver_and_its_notes() {
    let fx = fixture_plane(
        config(),
        vec![
            FixtureResponse::json(503, r#"{"error":{"message":"down"}}"#),
            models(&["openai/gpt-5-nano"]),
            text_turn(),
            FixtureResponse::json(503, r#"{"error":{"message":"down"}}"#),
            models(&["openai/gpt-5-nano"]),
        ],
        keys(&["openrouter", "backup", "anthropic", "local"]),
        true,
    )
    .await;
    // The full-screen draft: seeded, not chosen, so the chain applies and
    // the interface gets the skipped profile as a warning.
    let seeded = SessionDraft {
        profile: Some("openrouter".into()),
        model: Some("openai/gpt-5-nano".into()),
        ..SessionDraft::default()
    };
    let DraftOpen::Opened {
        mut driver,
        warnings,
        ..
    } = fx.plane.open_draft(seeded).await.unwrap()
    else {
        panic!("draft rejected");
    };
    assert_eq!(
        skipped(&warnings),
        vec![("openrouter".to_owned(), FailureClass::Provider)]
    );
    assert_eq!(native(&driver.session().kind).0, "backup");
    let mut ui = RecordingUi::default();
    driver.run_turn("hello", &mut ui).await.unwrap();
    let body = fx.transport.requests()[2].body_json().unwrap();
    assert_eq!(body["model"], "openai/gpt-5-nano");
    assert!(fx.transport.requests()[2]
        .url
        .starts_with("https://backup.example/v1"));

    // Print and REPL mode go through the flags and the same resolver. The
    // completed turn made `backup` the remembered profile, so it now leads
    // the chain; its outage moves startup on to the configured default.
    let opened = fx
        .plane
        .open_with(
            SessionSelector::New { name: None },
            None,
            StartupRequest::default(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        skipped(&opened.warnings),
        vec![("backup".to_owned(), FailureClass::Provider)]
    );
    assert_eq!(native(&opened.driver.session().kind).0, "openrouter");

    // A profile the user chose in the picker is pinned: nothing is
    // probed live and no fallback is consulted.
    let count = fx.transport.request_count();
    let chosen = SessionDraft::default().with_profile("openrouter");
    let DraftOpen::Opened {
        driver, warnings, ..
    } = fx.plane.open_draft(chosen).await.unwrap()
    else {
        panic!("draft rejected");
    };
    assert!(skipped(&warnings).is_empty());
    assert_eq!(native(&driver.session().kind).0, "openrouter");
    assert_eq!(fx.transport.request_count(), count);
}
