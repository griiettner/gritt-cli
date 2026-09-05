//! The combined load scenario, driven through the real event loop (TKT-0020).
//!
//! `tui_responsiveness.rs` measures the reducer and the renderer in
//! isolation. Those are microbenchmarks: they say what one frame costs, not
//! what the loop does with a queue behind it. The loop takes **one** message
//! per wakeup and redraws, and streaming output arrives on the same unbounded
//! channel as every other completion, so batching, queue depth, and how long
//! a keypress waits behind a stream are properties of `tui/run.rs` and cannot
//! be seen from `App` at all.
//!
//! This file runs the feature plan's scenario as one workload against
//! `LoopHarness`, which forwards to the same `on_message` and `on_action` the
//! product calls: a 10,000-message transcript, text deltas produced at the
//! plan's rate through the production `Ui`, four real MCP fixture servers of
//! which one never answers, a 1 MiB tool result, a synthetic user typing
//! throughout, and a cancellation executed through the runtime handler with
//! its effect asserted.
//!
//! ```text
//! GRITT_BENCH=1 cargo test --release -p gritt-harness --test tui_load -- --nocapture --test-threads 1
//! ```

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gritt_core::config::Config;
use gritt_core::event::{Event, EventKind, EventSource};
use gritt_core::mcp::{McpRuntimeSettings, McpServerState};
use gritt_core::provider::{Protocol, ProviderProfile};
use gritt_core::secret::{Secret, SecretRef};
use gritt_core::session::{BoxFuture, Phase, Session, SessionId, SessionKind};
use gritt_core::tool::{ToolCall, ToolCallId, ToolResult};
use gritt_core::Result;
use gritt_harness::agent::{AgentBuilder, ApprovalMode, CancelHandle, TurnOutcome, TurnStatus, Ui};
use gritt_harness::control::ControlPlane;
use gritt_harness::driver::{Driver, DriverInfo};
use gritt_harness::mcp::trust::MemoryTrustStore;
use gritt_harness::mcp::McpRuntime;
use gritt_harness::store::{DatabaseLocation, Store};
use gritt_harness::telemetry::Telemetry;
use gritt_harness::tools::{ProcessRegistry, Workspace};
use gritt_harness::tui::app::{App, EntryKind, StatusBar};
use gritt_harness::tui::render::draw;
use gritt_harness::tui::run::LoopHarness;
use gritt_harness::tui::theme::{Theme, ThemeMode};
use gritt_harness::CancellationToken;
use gritt_provider::models::ModelCatalog;
use gritt_provider::{FixtureTransport, StaticKey};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const FIXTURE: &str = env!("CARGO_BIN_EXE_gritt-mcp-fixture");
const WIDTH: u16 = 120;
const HEIGHT: u16 = 40;

fn recording() -> bool {
    std::env::var("GRITT_BENCH").is_ok_and(|value| value == "1")
}

fn record(line: &str) {
    println!("BENCH {line}");
}

fn millis(value: Duration) -> String {
    format!("{:.3}ms", value.as_secs_f64() * 1_000.0)
}

fn percentile(values: &[Duration], q: f64) -> Duration {
    if values.is_empty() {
        return Duration::ZERO;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

fn session(name: &str) -> Session {
    Session {
        id: SessionId(name.into()),
        parent_id: None,
        name: name.into(),
        workspace: std::path::PathBuf::from("/tmp/bench"),
        kind: SessionKind::Native {
            provider_profile: "openrouter".into(),
            model: "openai/gpt-5-nano".into(),
            effort: Default::default(),
        },
        phase: Phase::Planning,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// A driver that exists to own a cancellation token. Cancelling the turn
/// must reach it, which is what the cancellation case asserts.
struct StubDriver {
    session: Session,
    token: CancellationToken,
}

impl Driver for StubDriver {
    fn session(&self) -> &Session {
        &self.session
    }
    fn phase(&self) -> Phase {
        self.session.phase
    }
    fn set_phase(&mut self, _phase: Phase) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn handle(&self) -> CancelHandle {
        CancelHandle::new(self.token.clone(), ProcessRegistry::new())
    }
    fn run_turn<'a>(
        &'a mut self,
        _prompt: &'a str,
        _ui: &'a mut dyn Ui,
    ) -> BoxFuture<'a, Result<TurnOutcome>> {
        Box::pin(async {
            Ok(TurnOutcome {
                status: TurnStatus::Completed,
                text: String::new(),
                usage: Default::default(),
                tool_calls: 0,
                error: None,
            })
        })
    }
    fn info(&self) -> DriverInfo {
        DriverInfo {
            backend: "openrouter".into(),
            detail: "openai/gpt-5-nano".into(),
        }
    }
    fn effort(&self) -> Option<gritt_core::provider::ReasoningEffort> {
        Some(Default::default())
    }
    fn set_effort(
        &mut self,
        _effort: gritt_core::provider::ReasoningEffort,
    ) -> BoxFuture<'_, Result<gritt_harness::driver::EffortOutcome>> {
        Box::pin(async {
            Ok(gritt_harness::driver::EffortOutcome::Applied {
                effort: Default::default(),
            })
        })
    }
}

async fn plane_with_mcp(dir: &Path, mcp: Option<Arc<McpRuntime>>) -> ControlPlane {
    let store = Arc::new(
        Store::open(DatabaseLocation::Explicit(dir.join("gritt.db")))
            .await
            .unwrap(),
    );
    let mut config = Config::default();
    config.profiles.insert(
        "openrouter".into(),
        ProviderProfile {
            name: "openrouter".into(),
            protocol: Protocol::ChatCompletions,
            base_url: "https://openrouter.ai/api/v1".into(),
            key: SecretRef::for_profile("openrouter", "OPENROUTER_API_KEY"),
            aliases: Default::default(),
        },
    );
    config.default_profile = Some("openrouter".into());
    config.default_model = Some("openai/gpt-5-nano".into());
    let telemetry = Arc::new(Telemetry::new(Arc::clone(&store), config.logging.clone()));
    let builder = AgentBuilder {
        config,
        store,
        telemetry,
        keys: Arc::new(StaticKey(Secret::new("k"))),
        transport: Arc::new(FixtureTransport::new(Vec::new(), 17)),
        catalog: ModelCatalog::new(),
        cache: None,
        workspace: Workspace::open(dir).unwrap(),
        approval: ApprovalMode::DenyAll,
        mcp,
    };
    ControlPlane::native(Arc::new(builder))
}

fn text_delta(sequence: u64, text: &str) -> Event {
    Event {
        session_id: SessionId("bench".into()),
        sequence,
        source: EventSource::Native,
        timestamp: chrono::Utc::now(),
        kind: EventKind::TextDelta { text: text.into() },
        diagnostic: None,
    }
}

fn fill_transcript(app: &mut App, count: usize) {
    for index in 0..count {
        if index % 2 == 0 {
            app.push(
                EntryKind::User,
                format!("message {index}: rerun the parser split and report what changed"),
            );
        } else {
            app.push(
                EntryKind::Assistant,
                format!(
                    "message {index}: the parser module splits into a lexer and a \
                     grammar layer. The lexer keeps its own span table so the \
                     grammar never re-scans a token it already produced."
                ),
            );
        }
    }
}

/// The plan's whole scenario at once, on the real loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_combined_workload_on_the_real_event_loop() {
    let messages = if recording() { 10_000 } else { 1_000 };
    let seconds = if recording() { 10 } else { 3 };
    let target_rate = 1_000f64;

    // Four real servers, one of which never answers `initialize`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {
            "load-alpha": {"command": FIXTURE, "args": ["basic"]},
            "load-beta": {"command": FIXTURE, "args": ["basic"]},
            "load-gamma": {"command": FIXTURE, "args": ["basic"]},
            "load-hung": {"command": FIXTURE, "args": ["silent"]},
        }})
        .to_string(),
    )
    .unwrap();
    let mcp = Arc::new(
        McpRuntime::new(
            dir.path(),
            McpRuntimeSettings {
                init_timeout: Duration::from_secs(5),
                shutdown_grace: Duration::from_millis(300),
                ..McpRuntimeSettings::default()
            },
        )
        .with_trust(MemoryTrustStore::trust_all()),
    );

    let plane = plane_with_mcp(dir.path(), Some(Arc::clone(&mcp))).await;
    let mut app = App::new(StatusBar::default(), Theme::new(ThemeMode::NoColor));
    app.on_resize(WIDTH, HEIGHT);
    fill_transcript(&mut app, messages);
    let mut harness = LoopHarness::new(plane, dir.path(), app);
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();

    // The real lifecycle subscription, and the real launch, both running
    // while the loop is under load.
    harness.subscribe_mcp();
    let opening = tokio::spawn({
        let mcp = Arc::clone(&mcp);
        async move { mcp.open(&CancellationToken::new()).await }
    });

    // The producer: a turn streaming through the production `Ui`.
    let produced = Arc::new(AtomicUsize::new(0));
    let mut ui = harness.channel_ui(0);
    let streaming = tokio::spawn({
        let produced = Arc::clone(&produced);
        async move {
            let interval = Duration::from_secs_f64(1.0 / target_rate);
            let start = Instant::now();
            let mut sequence = 0u64;
            // One 1 MiB tool result partway through, as the plan asks.
            let mut sent_big = false;
            while start.elapsed() < Duration::from_secs(seconds) {
                sequence += 1;
                ui.event(&text_delta(sequence, "token "));
                produced.fetch_add(1, Ordering::Relaxed);
                if !sent_big && start.elapsed() > Duration::from_secs(seconds / 2) {
                    sent_big = true;
                    let call = ToolCall {
                        id: ToolCallId("big".into()),
                        name: "mcp__load-alpha__dump".into(),
                        arguments: serde_json::json!({}),
                    };
                    ui.event(&Event {
                        kind: EventKind::ToolCall { call },
                        ..text_delta(sequence, "")
                    });
                    ui.event(&Event {
                        kind: EventKind::ToolResult {
                            result: ToolResult {
                                call_id: ToolCallId("big".into()),
                                name: "mcp__load-alpha__dump".into(),
                                output: "x".repeat(1024 * 1024),
                                is_error: false,
                            },
                        },
                        ..text_delta(sequence, "")
                    });
                    produced.fetch_add(2, Ordering::Relaxed);
                }
                // Pace the producer without spinning a core.
                if sequence.is_multiple_of(50) {
                    tokio::time::sleep(interval * 50).await;
                }
            }
        }
    });

    // The loop, shaped exactly as `event_loop` is: draw when the screen can
    // have changed, then take one wakeup.
    let mut dirty = true;
    let mut frames = 0usize;
    let mut handled = 0usize;
    let mut queue_samples: Vec<usize> = Vec::new();
    let mut input_latency: Vec<Duration> = Vec::new();
    let mut pending_key: Option<Instant> = None;
    let mut next_key = Instant::now() + Duration::from_millis(100);
    let run_start = Instant::now();
    let deadline = Duration::from_secs(seconds);

    // The frame cap and the coalescing drain the loop uses. Mirrored here so
    // the model and the product cannot drift; if `run.rs` changes shape,
    // these numbers stop describing it and this comment is the pointer.
    let frame_interval = Duration::from_millis(33);
    let mut last_draw = Instant::now() - frame_interval;

    while run_start.elapsed() < deadline {
        if dirty && last_draw.elapsed() >= frame_interval {
            terminal.draw(|f| draw(f, harness.app())).unwrap();
            frames += 1;
            dirty = false;
            last_draw = Instant::now();
            // A key is answered by the first frame drawn after it.
            if let Some(pressed) = pending_key.take() {
                input_latency.push(pressed.elapsed());
            }
        }
        queue_samples.push(harness.queue_depth());

        // The synthetic user types about ten characters a second.
        let key_ready = pending_key.is_none() && Instant::now() >= next_key;
        let msg_ready = harness.queue_depth() > 0;
        // The loop's `select!` is `biased` with input first, so a key that
        // is ready always wins over a queued message.
        let take_key = key_ready;
        if take_key {
            let pressed = Instant::now();
            harness
                .press(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
                .await;
            pending_key = Some(pressed);
            next_key = Instant::now() + Duration::from_millis(100);
            dirty = true;
        } else if msg_ready {
            // One wakeup, then the coalescing drain: everything already
            // waiting is handled before the next frame.
            harness.pump_one().await;
            handled += 1;
            let mut coalesced = 0;
            while coalesced < 4_096 && harness.pump_one().await {
                coalesced += 1;
                handled += 1;
            }
            dirty = true;
        } else {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    streaming.abort();
    let snapshots = opening.await.unwrap().unwrap();
    let elapsed = run_start.elapsed();
    let produced_total = produced.load(Ordering::Relaxed);
    let peak_queue = queue_samples.iter().copied().max().unwrap_or(0);
    let final_queue = harness.queue_depth();
    let drain_rate = handled as f64 / elapsed.as_secs_f64();
    let frame_rate = frames as f64 / elapsed.as_secs_f64();

    record(&format!(
        "combined load ({messages}-message transcript, {:.0}s): produced {produced_total} \
         messages, handled {handled} ({drain_rate:.0}/s), frames {frames} ({frame_rate:.0} fps)",
        elapsed.as_secs_f64()
    ));
    record(&format!(
        "combined load queue: peak {peak_queue}, final {final_queue}"
    ));
    if !input_latency.is_empty() {
        record(&format!(
            "combined load input-to-frame under load: n={} p50={} p95={} max={}",
            input_latency.len(),
            millis(percentile(&input_latency, 0.50)),
            millis(percentile(&input_latency, 0.95)),
            millis(percentile(&input_latency, 1.0)),
        ));
        let p95 = percentile(&input_latency, 0.95);
        record(&format!(
            "  budget {:<34} {:>10} vs   50.000ms -> {}",
            "input to frame under load",
            millis(p95),
            if p95 <= Duration::from_millis(50) {
                "MET"
            } else {
                "NOT MET"
            },
        ));
    }
    record(&format!(
        "  budget {:<34} {drain_rate:.0}/s vs {target_rate:.0}/s -> {}",
        "sustained delta drain rate",
        if drain_rate >= target_rate * 0.9 {
            "MET"
        } else {
            "NOT MET"
        },
    ));

    // Every MCP entry is accounted for while the loop is under load.
    let ready = snapshots
        .iter()
        .filter(|s| s.state == McpServerState::Ready)
        .count();
    let hung = snapshots.iter().find(|s| s.name == "load-hung").unwrap();
    record(&format!(
        "combined load mcp: {ready} of {} ready, hung entry {:?}",
        snapshots.len(),
        hung.state
    ));
    assert_eq!(ready, 3, "healthy servers must not wait for the hung one");
    assert!(!matches!(hung.state, McpServerState::Ready));

    // The 1 MiB result reached the transcript through the real path.
    assert!(
        harness
            .app()
            .entries
            .iter()
            .any(|e| e.detail.as_ref().is_some_and(|d| d.len() >= 1024 * 1024)),
        "the 1 MiB tool result never arrived"
    );

    // Cancellation, executed through the runtime handler, under this load.
    let token = CancellationToken::new();
    harness.set_driver(Box::new(StubDriver {
        session: session("bench"),
        token: token.clone(),
    }));
    harness.app_mut().running = true;
    let before = terminal.backend().buffer().clone();
    let cancel_start = Instant::now();
    harness
        .press(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .await;
    terminal.draw(|f| draw(f, harness.app())).unwrap();
    let cancel_latency = cancel_start.elapsed();
    let after = terminal.backend().buffer().clone();
    record(&format!(
        "combined load cancel: {} to a redrawn frame, token cancelled={}",
        millis(cancel_latency),
        token.is_cancelled()
    ));
    record(&format!(
        "  budget {:<34} {:>10} vs  100.000ms -> {}",
        "cancel visible under load",
        millis(cancel_latency),
        if cancel_latency <= Duration::from_millis(100) {
            "MET"
        } else {
            "NOT MET"
        },
    ));
    assert!(
        token.is_cancelled(),
        "Escape did not reach the running turn's cancellation token"
    );
    assert_ne!(
        before, after,
        "cancelling under load produced no visible change"
    );
    assert!(
        cancel_latency < Duration::from_millis(1_000),
        "cancel took {}",
        millis(cancel_latency)
    );

    // Loose regression bounds. The budget verdicts above are the statement;
    // these only catch an order-of-magnitude change.
    assert!(frames > 0, "the loop never drew");
    assert!(handled > 0, "the loop never handled a message");
    // The two that matter. Without the coalescing drain the loop handled 69
    // messages a second against 1,000 produced and ended holding 8,862, so
    // both of these fail loudly if that regresses.
    assert!(
        drain_rate >= target_rate * 0.5,
        "the loop drained only {drain_rate:.0} messages/s of {target_rate:.0} produced"
    );
    assert!(
        peak_queue < 2_000,
        "the message queue reached {peak_queue}; it is not keeping up"
    );
    // The frame cap. Under a saturating stream the loop must not draw a
    // frame per event; 30 fps plus a margin is the ceiling.
    assert!(
        frame_rate <= 45.0,
        "the loop drew at {frame_rate:.0} fps, above the 30 fps cap"
    );

    mcp.shutdown().await;
}
