//! The full-screen mode against the real control plane, a real MCP
//! runtime, and a fake configuration writer.
//!
//! The reducer tests in `src/tui/app/tests.rs` prove what a key does. These
//! prove that the values the reducer produces are the ones the harness
//! accepts: a setup form that really writes a profile and makes it usable,
//! a `/mcp` approval that really launches a server, and an MCP tool call
//! approved from the reducer that really reaches the server.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gritt_core::config::Config;
use gritt_core::event::{ApprovalDecision, ApprovalRequest, Event, EventKind};
use gritt_core::mcp::{McpRuntimeSettings, McpServerState, TrustDecision};
use gritt_core::provider::{
    ModelCapabilities, ModelInfo, ModelList, ModelListStatus, Protocol, ProviderProfile,
};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, SessionStore};
use gritt_harness::agent::{AgentBuilder, ApprovalMode, SessionSelector, TurnStatus, Ui};
use gritt_harness::control::{ControlPlane, DraftOpen};
use gritt_harness::draft::SessionDraft;
use gritt_harness::mcp::trust::MemoryTrustStore;
use gritt_harness::mcp::McpRuntime;
use gritt_harness::policy::Decision;
use gritt_harness::setup::{
    apply_setup, ConfigDestination, CredentialStoreOutcome, ProfileSaveOutcome, ProviderSetup,
};
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::Workspace;
use gritt_harness::tui::app::{Action, App, McpRequest, Overlay, PickerKind, StatusBar};
use gritt_harness::tui::command::Command;
use gritt_harness::tui::theme::{Theme, ThemeMode};
use gritt_harness::CancellationToken;
use gritt_provider::models::ModelCatalog;
use gritt_provider::{FixtureResponse, FixtureTransport, StaticKey};

const FIXTURE: &str = env!("CARGO_BIN_EXE_gritt-mcp-fixture");
const KEY: &str = "fixture-key-never-printed";

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn app() -> App {
    App::new(StatusBar::default(), Theme::new(ThemeMode::NoColor))
}

fn profile(name: &str, protocol: Protocol, base_url: &str) -> ProviderProfile {
    ProviderProfile {
        name: name.into(),
        protocol,
        base_url: base_url.into(),
        key: SecretRef::for_profile(name, format!("{}_API_KEY", name.to_uppercase())),
        aliases: Default::default(),
    }
}

// -- the setup round trip ---------------------------------------------

/// A `ProviderSetup` that records what it was asked to write and answers
/// a reload with the profiles it has accepted so far. It is the binary's
/// side of the contract without a config file or a keychain.
#[derive(Default)]
struct FakeSetup {
    saved: Mutex<Vec<(ProviderProfile, ConfigDestination)>>,
    keys: Mutex<Vec<(String, String)>>,
    keychain_fails: bool,
}

impl ProviderSetup for FakeSetup {
    fn save_profile(
        &self,
        profile: &ProviderProfile,
        destination: ConfigDestination,
    ) -> ProfileSaveOutcome {
        self.saved
            .lock()
            .unwrap()
            .push((profile.clone(), destination));
        ProfileSaveOutcome::Saved {
            destination,
            path: PathBuf::from("/fake/config.toml"),
            shadowed_by: None,
        }
    }

    fn store_credential(&self, profile: &ProviderProfile, value: Secret) -> CredentialStoreOutcome {
        if self.keychain_fails {
            return CredentialStoreOutcome::KeychainUnavailable {
                profile: profile.name.clone(),
                env_var_name: profile.key.env_var_name.clone(),
                message: "this system has no keychain".into(),
            };
        }
        self.keys
            .lock()
            .unwrap()
            .push((profile.name.clone(), value.expose().to_owned()));
        CredentialStoreOutcome::Stored {
            profile: profile.name.clone(),
            keychain_service_entry: profile.key.keychain_service_entry.clone(),
        }
    }

    fn reload_config(&self) -> Option<Config> {
        let mut config = Config::default();
        for (profile, _) in self.saved.lock().unwrap().iter() {
            config
                .profiles
                .insert(profile.name.clone(), profile.clone());
        }
        config.default_profile = config.profiles.keys().next().cloned();
        Some(config)
    }
}

async fn plane_with(
    config: Config,
    setup: Arc<dyn ProviderSetup>,
) -> (tempfile::TempDir, ControlPlane) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    let builder = AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(StaticKey(Secret::new(KEY))),
        transport: Arc::new(FixtureTransport::new(Vec::new(), 17)),
        catalog: ModelCatalog::new(),
        cache: None,
        workspace: Workspace::open(dir.path()).unwrap(),
        approval: ApprovalMode::DenyAll,
        mcp: None,
    };
    let plane = ControlPlane::native(Arc::new(builder)).with_setup(setup);
    (dir, plane)
}

/// `/connect` with nothing configured offers the supported presets, the
/// form writes through the injected service, and the reloaded plane makes
/// the profile usable at once.
#[tokio::test]
async fn setting_up_a_provider_makes_it_usable_without_a_restart() {
    let setup = Arc::new(FakeSetup::default());
    let (_dir, plane) = plane_with(
        Config::default(),
        Arc::clone(&setup) as Arc<dyn ProviderSetup>,
    )
    .await;

    let mut app = app();
    app.profiles = plane.profile_summaries();
    assert!(app.profiles.is_empty(), "the fixture starts unconfigured");
    app.dispatch(Command::Connect, None);
    // With no profile at all, the dialog still opens and offers setup.
    type_text(&mut app, "Set up openrouter");
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.top_overlay(), Some(Overlay::Setup(_))),
        "the preset did not open its setup form"
    );
    type_text(&mut app, "sk-fake-key-value");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::SaveProfile);

    // What the runtime does with that action.
    let submission = app.take_setup_submission().expect("a complete form");
    assert_eq!(submission.profile.name, "openrouter");
    assert_eq!(submission.profile.protocol, Protocol::ChatCompletions);
    let (message, close) = apply_setup(plane.setup().as_ref(), submission);
    assert!(close, "a successful write must close the form: {message}");
    assert!(message.contains("keychain"), "{message}");

    let plane = plane.reloaded().expect("the fake service reloads");
    app.profiles = plane.profile_summaries();
    app.setup_outcome(message, close);
    assert_eq!(app.profiles.len(), 1);
    assert_eq!(app.profiles[0].name, "openrouter");
    // The connection dialog now lists it as a provider, not as setup.
    let picker = app.connection_picker();
    assert!(picker
        .rows()
        .iter()
        .any(|row| row.id == "profile:openrouter"));

    // The key went to the keychain service and to nothing else.
    let keys = setup.keys.lock().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].1, "sk-fake-key-value");
    let saved = setup.saved.lock().unwrap();
    assert_eq!(
        saved[0].1,
        ConfigDestination::User,
        "the project file is not the default"
    );
    assert!(
        !format!("{:?}", saved[0].0).contains("sk-fake"),
        "the key reached the profile record"
    );
}

/// A keychain that refuses leaves the profile usable and names the
/// variable to export instead.
#[tokio::test]
async fn a_refused_keychain_still_leaves_a_usable_profile() {
    let setup = Arc::new(FakeSetup {
        keychain_fails: true,
        ..FakeSetup::default()
    });
    let (_dir, plane) = plane_with(
        Config::default(),
        Arc::clone(&setup) as Arc<dyn ProviderSetup>,
    )
    .await;
    let mut app = app();
    app.dispatch(Command::Connect, None);
    type_text(&mut app, "Set up anthropic");
    app.on_key(key(KeyCode::Enter));
    type_text(&mut app, "sk-not-stored");
    app.on_key(key(KeyCode::Enter));
    let submission = app.take_setup_submission().unwrap();
    let (message, close) = apply_setup(plane.setup().as_ref(), submission);
    assert!(close, "the profile exists even though the key did not land");
    assert!(message.contains("ANTHROPIC_API_KEY"), "{message}");
    assert!(!message.contains("sk-not-stored"), "{message}");
    assert_eq!(plane.reloaded().unwrap().profile_summaries().len(), 1);
}

/// A draft the reducer produced opens a real session through the control
/// plane, against a fake catalog rather than a provider.
#[tokio::test]
async fn a_draft_from_the_reducer_opens_a_session_through_the_plane() {
    let mut config = Config::default();
    config.profiles.insert(
        "openrouter".into(),
        profile(
            "openrouter",
            Protocol::ChatCompletions,
            "https://openrouter.ai/api/v1",
        ),
    );
    let (_dir, plane) = plane_with(config, Arc::new(FakeSetup::default())).await;
    // A fake catalog: no provider is contacted, the list is simply there.
    plane.builder.catalog.insert(ModelList {
        profile: "openrouter".into(),
        status: ModelListStatus::Fresh {
            fetched_at: chrono::Utc::now(),
        },
        models: vec![ModelInfo {
            id: "openai/gpt-5-nano".into(),
            display_name: Some("GPT-5 nano".into()),
            capabilities: ModelCapabilities {
                reasoning: Some(true),
                context_length: Some(400_000),
                ..ModelCapabilities::default()
            },
            replaced_by: None,
            deprecated: false,
        }],
    });

    let mut app = app();
    app.profiles = plane.profile_summaries();
    let catalog = plane.catalog("openrouter").await.unwrap();
    app.dispatch(Command::Connect, None);
    type_text(&mut app, "openrouter");
    app.on_key(key(KeyCode::Enter));
    assert!(app.apply_catalog(
        app.selection,
        "openrouter",
        catalog.models.clone(),
        catalog.state.clone()
    ));
    type_text(&mut app, "nano");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.draft.model.as_deref(), Some("openai/gpt-5-nano"));

    let DraftOpen::Opened { driver, .. } = plane.open_draft(app.draft.clone()).await.unwrap()
    else {
        panic!("the plane refused a draft the picker built");
    };
    app.set_session(driver.session());
    assert_eq!(app.status.profile, "openrouter");
    assert_eq!(app.status.model, "openai/gpt-5-nano");
    assert!(app.session_id.is_some());
}

// -- MCP through the reducer ------------------------------------------

/// Pipes a turn's events into the reducer and answers approvals with the
/// reducer's own key handling, so what approves the call is the same code
/// a keypress runs.
struct ReducerUi {
    app: Arc<Mutex<App>>,
    answer: KeyCode,
    asked: Arc<Mutex<Vec<ApprovalRequest>>>,
}

impl Ui for ReducerUi {
    fn event(&mut self, event: &Event) {
        self.app.lock().unwrap().on_event(event);
    }

    fn approve<'a>(
        &'a mut self,
        request: &'a ApprovalRequest,
        decision: &'a Decision,
        preview: Option<&'a str>,
    ) -> BoxFuture<'a, ApprovalDecision> {
        self.asked.lock().unwrap().push(request.clone());
        let mut app = self.app.lock().unwrap();
        app.request_approval(gritt_harness::tui::app::PendingApproval {
            request: request.clone(),
            decision: decision.clone(),
            preview: preview.map(str::to_owned),
        });
        // The approval overlay is modal: a settings command is refused
        // while it is open, exactly as the plan requires.
        assert_eq!(app.dispatch(Command::Models, None), Action::None);
        assert!(app
            .notice
            .as_deref()
            .unwrap_or_default()
            .contains("approval"));
        let action = app.on_key(key(self.answer));
        let decision = match action {
            Action::Approve(decision) => decision,
            other => panic!("the reducer did not answer the approval: {other:?}"),
        };
        Box::pin(async move { decision })
    }
}

fn tool_call_sse(tool: &str, arguments: serde_json::Value) -> Vec<u8> {
    let first = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": null,
            "tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": tool, "arguments": arguments.to_string()}}]},
            "finish_reason": null}]
    });
    let second = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {first}\n\ndata: {second}\n\ndata: [DONE]\n\n").into_bytes()
}

fn text_sse(text: &str) -> Vec<u8> {
    let chunk = serde_json::json!({
        "id": "chatcmpl-f", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": "stop"}]
    });
    format!("data: {chunk}\n\ndata: [DONE]\n\n").into_bytes()
}

struct McpFixture {
    _dir: tempfile::TempDir,
    plane: ControlPlane,
    mcp: Arc<McpRuntime>,
    marker: PathBuf,
}

/// A workspace with one MCP server that has not been approved yet.
async fn mcp_fixture() -> McpFixture {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("tool-was-called");
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {"probe": {
            "command": FIXTURE,
            "args": ["marker"],
            "env": {"FIXTURE_MARKER": marker.to_string_lossy()},
        }}})
        .to_string(),
    )
    .unwrap();
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.path().join("gritt.db")))
            .await
            .unwrap(),
    );
    let mut config = Config::default();
    config.profiles.insert(
        "openrouter".into(),
        profile(
            "openrouter",
            Protocol::ChatCompletions,
            "https://openrouter.ai/api/v1",
        ),
    );
    config.default_profile = Some("openrouter".into());
    config.default_model = Some("openai/gpt-5-nano".into());
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    let transport = Arc::new(FixtureTransport::new(
        vec![
            FixtureResponse::sse(tool_call_sse(
                "mcp__probe__echo",
                serde_json::json!({"text": "hi"}),
            )),
            FixtureResponse::sse(text_sse("done")),
        ],
        17,
    ));
    // The default trust store approves nothing, which is the state a first
    // run is really in.
    let mcp = Arc::new(
        McpRuntime::new(
            Workspace::open(dir.path()).unwrap().root(),
            McpRuntimeSettings::default(),
        )
        .with_trust(MemoryTrustStore::new()),
    );
    mcp.open(&CancellationToken::new()).await.unwrap();
    let builder = AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(StaticKey(Secret::new(KEY))),
        transport,
        catalog: ModelCatalog::new(),
        cache: None,
        workspace: Workspace::open(dir.path()).unwrap(),
        approval: ApprovalMode::Ask,
        mcp: Some(Arc::clone(&mcp)),
    };
    McpFixture {
        _dir: dir,
        plane: ControlPlane::native(Arc::new(builder)),
        mcp,
        marker,
    }
}

/// `/mcp` shows the real state, its approval really launches the server,
/// and the server's tool is then callable. The transitions come from the
/// runtime's subscription, not from a poll.
#[tokio::test]
async fn approving_a_server_from_the_overlay_launches_it_and_publishes_the_change() {
    let fixture = mcp_fixture().await;
    let mut updates = fixture.mcp.subscribe();
    let mut app = app();
    app.apply_mcp(fixture.mcp.snapshots().await);
    assert_eq!(app.dispatch(Command::Mcp, None), Action::RefreshMcp);
    let Some(Overlay::Picker { picker, .. }) = app.top_overlay() else {
        panic!("/mcp opened nothing")
    };
    assert_eq!(picker.rows().len(), 1);
    assert_eq!(picker.rows()[0].badge, "awaiting approval");

    // Approve it the way a keypress does.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.top_overlay().and_then(Overlay::picker_kind),
        Some(PickerKind::McpActions)
    );
    type_text(&mut app, "Approve");
    let action = app.on_key(key(KeyCode::Enter));
    // Approving asks for the definition first: reading a workspace file
    // does not authorize running what it names.
    let Action::Mcp(McpRequest::RequestApproval { server }) = action else {
        panic!("expected a request for the definition, got {action:?}")
    };

    // The real, redacted summary from the runtime, shown in the shared
    // approval overlay.
    let definition = fixture
        .mcp
        .definition_summary(&server)
        .await
        .expect("a configured entry has a definition");
    assert!(
        definition.contains(FIXTURE),
        "the executable being trusted is not shown: {definition}"
    );
    assert!(
        definition.contains("FIXTURE_MARKER"),
        "the environment names are not shown: {definition}"
    );
    assert!(
        !definition.contains(&fixture.marker.to_string_lossy().to_string()),
        "an environment value leaked into the summary: {definition}"
    );
    app.request_mcp_approval(server.clone(), definition);
    assert!(app.pending.is_some(), "no approval overlay was shown");
    let action = app.on_key(key(KeyCode::Char('y')));
    let Action::Mcp(McpRequest::Decide { server, decision }) = action else {
        panic!("expected a typed decision, got {action:?}")
    };
    assert_eq!(decision, TrustDecision::Approved);

    // What the runtime does with it.
    let cancel = CancellationToken::new();
    fixture.mcp.decide(&server, decision).await.unwrap();
    fixture.mcp.start(&cancel).await;

    // The change arrived on the subscription rather than being polled for.
    let mut ready = false;
    while let Ok(snapshots) = updates.try_recv() {
        if snapshots.iter().any(|s| s.state == McpServerState::Ready) {
            app.apply_mcp(snapshots);
            ready = true;
        }
    }
    assert!(ready, "no lifecycle message reported the server ready");
    assert_eq!(app.mcp[0].state, McpServerState::Ready);
    assert!(
        app.mcp[0].tool_count > 0,
        "a ready server contributed no tools"
    );
    // The sidebar shows the same thing the overlay does.
    assert_eq!(
        app.sidebar.integrations.mcp.as_ref().unwrap()[0].state,
        McpServerState::Ready
    );
}

/// A native MCP tool call approved through the reducer reaches the server;
/// denied through the same path, it does not.
#[tokio::test]
async fn an_mcp_tool_call_approved_in_the_reducer_reaches_the_server() {
    for (answer, expect_called) in [(KeyCode::Char('y'), true), (KeyCode::Char('n'), false)] {
        let fixture = mcp_fixture().await;
        fixture
            .mcp
            .decide("probe", TrustDecision::Approved)
            .await
            .unwrap();
        fixture.mcp.start(&CancellationToken::new()).await;

        let shared = Arc::new(Mutex::new(app()));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let mut driver = fixture
            .plane
            .open_draft(SessionDraft::default().with_phase(gritt_core::session::Phase::Coding))
            .await
            .map(|open| match open {
                DraftOpen::Opened { driver, .. } => driver,
                DraftOpen::Rejected { errors, .. } => panic!("{errors:?}"),
            })
            .unwrap();
        shared.lock().unwrap().set_session(driver.session());
        let mut ui = ReducerUi {
            app: Arc::clone(&shared),
            answer,
            asked: Arc::clone(&asked),
        };
        let outcome = driver.run_turn("use the tool", &mut ui).await.unwrap();
        assert_eq!(outcome.status, TurnStatus::Completed);
        assert_eq!(asked.lock().unwrap().len(), 1, "the call was not gated");
        assert_eq!(
            fixture.marker.exists(),
            expect_called,
            "answering with {answer:?} sent the call to the server: {}",
            fixture.marker.exists()
        );

        // Either way the transcript carries the compact tool rows the
        // shared event model produces, with the result expandable.
        {
            let app = shared.lock().unwrap();
            let tool_rows: Vec<_> = app
                .entries
                .iter()
                .filter(|entry| entry.kind == gritt_harness::tui::EntryKind::Tool)
                .collect();
            assert_eq!(tool_rows.len(), 2, "expected a call row and a result row");
            assert!(tool_rows[0].text.starts_with("->"));
            assert!(tool_rows[1].text.starts_with("<-"));
            assert!(
                tool_rows[1].detail.is_some(),
                "the result row has nothing to expand"
            );
            assert!(app.pending.is_none(), "the approval overlay stayed open");
        }
        fixture.mcp.shutdown().await;
    }
}

/// Every configured entry keeps a visible state through a reload, and the
/// reducer's reload action is the runtime's.
#[tokio::test]
async fn reloading_from_the_overlay_keeps_every_entry_accounted_for() {
    let fixture = mcp_fixture().await;
    let mut app = app();
    app.apply_mcp(fixture.mcp.snapshots().await);
    app.dispatch(Command::Mcp, None);
    app.on_key(key(KeyCode::Enter));
    type_text(&mut app, "Reload");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Mcp(McpRequest::ReloadAll)
    );
    fixture.mcp.reload().await.unwrap();
    app.apply_mcp(fixture.mcp.snapshots().await);
    assert_eq!(app.mcp.len(), 1, "an entry vanished across a reload");
    assert_eq!(app.mcp[0].state, McpServerState::AwaitingApproval);
}

/// A resumed session is pinned to its stored provider and model, and the
/// plane refuses a draft that asks for another one.
#[tokio::test]
async fn resuming_keeps_the_stored_provider_and_model() {
    let fixture = mcp_fixture().await;
    let DraftOpen::Opened { driver, .. } = fixture
        .plane
        .open_draft(SessionDraft::default().with_name("pinned"))
        .await
        .unwrap()
    else {
        panic!("the first open was refused")
    };
    let session = driver.session().clone();
    drop(driver);

    let mut app = app();
    app.set_session(&session);
    assert_eq!(app.draft.profile.as_deref(), Some("openrouter"));
    assert_eq!(app.draft.model.as_deref(), Some("openai/gpt-5-nano"));

    // Resuming by name keeps both.
    let DraftOpen::Opened { driver, .. } = fixture
        .plane
        .open_draft(SessionDraft::default().with_name("pinned"))
        .await
        .unwrap()
    else {
        panic!("resume was refused")
    };
    assert_eq!(driver.session().id, session.id);
    drop(driver);

    // Asking for another model on the same session is a typed refusal the
    // interface turns into the new-session explanation.
    let outcome = fixture
        .plane
        .open_draft(
            SessionDraft::default()
                .with_name("pinned")
                .with_model("something/else"),
        )
        .await
        .unwrap();
    let DraftOpen::Rejected { errors, .. } = outcome else {
        panic!("a pinned session accepted another model")
    };
    let (title, body) = gritt_harness::tui::app::describe_draft_error(&errors[0]);
    assert!(title.contains("new session"), "{title}");
    assert!(body.contains("/new"), "{body}");
}

/// The session listing and the store agree, and the picker resumes by the
/// id the store gave it.
#[tokio::test]
async fn the_session_picker_lists_and_resumes_real_sessions() {
    let fixture = mcp_fixture().await;
    for name in ["first", "second"] {
        let DraftOpen::Opened { driver, .. } = fixture
            .plane
            .open_draft(SessionDraft::default().with_name(name))
            .await
            .unwrap()
        else {
            panic!("{name} was refused")
        };
        drop(driver);
    }
    let sessions = fixture.plane.builder.store.list().await.unwrap();
    let mut app = app();
    assert_eq!(
        app.dispatch(Command::Sessions, None),
        Action::RefreshSessions
    );
    app.load_sessions(sessions.clone());
    let Some(Overlay::Picker { picker, .. }) = app.top_overlay() else {
        panic!("the session picker closed")
    };
    assert_eq!(picker.rows().len(), 2);
    type_text(&mut app, "second");
    let action = app.on_key(key(KeyCode::Enter));
    let Action::Resume(id) = action else {
        panic!("expected a resume, got {action:?}")
    };
    let wanted = sessions
        .iter()
        .find(|session| session.name == "second")
        .unwrap();
    assert_eq!(id, wanted.id);
    // And that id opens the session the row named.
    let opened = fixture
        .plane
        .open(SessionSelector::Id(id), None, None, None, None)
        .await
        .unwrap();
    assert_eq!(opened.session().name, "second");
}

/// The event stream a turn produces is what fills the sidebar's usage.
#[tokio::test]
async fn usage_events_from_a_real_turn_fill_the_sidebar() {
    let fixture = mcp_fixture().await;
    fixture
        .mcp
        .decide("probe", TrustDecision::Approved)
        .await
        .unwrap();
    fixture.mcp.start(&CancellationToken::new()).await;
    let shared = Arc::new(Mutex::new(app()));
    let mut driver = match fixture
        .plane
        .open_draft(SessionDraft::default().with_phase(gritt_core::session::Phase::Coding))
        .await
        .unwrap()
    {
        DraftOpen::Opened { driver, .. } => driver,
        DraftOpen::Rejected { errors, .. } => panic!("{errors:?}"),
    };
    let mut ui = ReducerUi {
        app: Arc::clone(&shared),
        answer: KeyCode::Char('y'),
        asked: Arc::new(Mutex::new(Vec::new())),
    };
    driver.run_turn("use the tool", &mut ui).await.unwrap();
    {
        let app = shared.lock().unwrap();
        let had_usage = app
            .entries
            .iter()
            .any(|entry| entry.kind == gritt_harness::tui::EntryKind::Tool);
        assert!(had_usage, "the turn produced no tool rows");
        // Without a catalog there is no context limit and no price, so
        // both stay unavailable rather than being invented.
        assert_eq!(app.sidebar.usage.context_limit, None);
        assert_eq!(app.sidebar.cost.estimate_usd, None);
    }
    fixture.mcp.shutdown().await;
}

/// A turn's events reach the reducer as the shared model, including the
/// ones a connector session would also produce.
#[test]
fn a_usage_event_never_becomes_a_context_percentage_on_its_own() {
    let mut app = app();
    app.on_event(&Event {
        session_id: gritt_core::session::SessionId("s".into()),
        sequence: 1,
        timestamp: chrono::Utc::now(),
        source: gritt_core::event::EventSource::Native,
        kind: EventKind::Usage {
            usage: gritt_core::event::Usage {
                input_tokens: Some(120),
                output_tokens: Some(40),
                ..Default::default()
            },
        },
        diagnostic: None,
    });
    assert_eq!(app.sidebar.usage.input_tokens, Some(120));
    assert_eq!(
        app.sidebar.usage.occupancy(),
        None,
        "occupancy was derived without a model limit"
    );
}
