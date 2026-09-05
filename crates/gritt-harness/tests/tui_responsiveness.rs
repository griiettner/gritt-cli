//! Deterministic responsiveness harness for the full-screen mode (TKT-0020).
//!
//! The feature plan makes responsiveness an acceptance requirement and names
//! the scenario: a 10,000-message transcript, 1,000 incoming text deltas per
//! second, several active MCP servers plus one hung server, a 1 MiB tool
//! result, cancellation under load, and history paging. This file is that
//! scenario, driven through `App::on_key`, `App::on_event`, and `render::draw`
//! against a `TestBackend`, so the numbers come from the same code the binary
//! runs and not from a mock.
//!
//! Two modes:
//!
//! - Default. Sizes are scaled down so the suite stays fast, and every
//!   assertion is a loose bound chosen well above the measured value. This is
//!   the regression test; it must not be flaky.
//! - `GRITT_BENCH=1`. Full sizes, more samples, and a recorded table on
//!   stdout. This is what produced the numbers in the ticket report.
//!
//! ```text
//! cargo test --release -p gritt-harness --test tui_responsiveness -- --nocapture
//! GRITT_BENCH=1 cargo test --release -p gritt-harness --test tui_responsiveness -- --nocapture --test-threads 1
//! ```
//!
//! A latency sample is one input event plus the frame that answers it: the
//! reducer call and the `draw` that follows, which is exactly what the run
//! loop does between reading a key and putting a frame on the terminal. It is
//! not presentation latency; the plan asks for a real-terminal run separately
//! and neither alone proves the other.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gritt_core::event::{Event, EventKind, EventSource};
use gritt_core::mcp::{McpRuntimeSettings, McpServerState};
use gritt_core::session::SessionId;
use gritt_core::tool::{ToolCall, ToolCallId, ToolResult};
use gritt_harness::mcp::trust::MemoryTrustStore;
use gritt_harness::mcp::McpRuntime;
use gritt_harness::tui::app::{App, EntryKind, StatusBar};
use gritt_harness::tui::render::draw;
use gritt_harness::tui::theme::{Theme, ThemeMode};
use gritt_harness::CancellationToken;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const FIXTURE: &str = env!("CARGO_BIN_EXE_gritt-mcp-fixture");

/// The reference viewport the plan's render budget is stated at.
const WIDTH: u16 = 120;
const HEIGHT: u16 = 40;

// -- budgets ----------------------------------------------------------
//
// The plan's numbers. They are the comparison the report makes; the
// assertions below use the separate, looser regression bounds so a busy
// machine cannot fail the suite.

const BUDGET_INPUT_P95: Duration = Duration::from_millis(50);
const BUDGET_RENDER_P95: Duration = Duration::from_millis(16);
const BUDGET_CANCEL: Duration = Duration::from_millis(100);

/// Regression bounds. Deliberately far above the measured values: this
/// suite runs on shared machines and its job is to catch an order-of-
/// magnitude regression, not to police a few milliseconds.
const BOUND_INPUT_P95: Duration = Duration::from_millis(400);
const BOUND_RENDER_P95: Duration = Duration::from_millis(200);
const BOUND_CANCEL: Duration = Duration::from_millis(400);

fn recording() -> bool {
    std::env::var("GRITT_BENCH").is_ok_and(|value| value == "1")
}

/// Transcript size. The plan's figure when recording; enough to keep the
/// cost shape visible otherwise.
fn transcript_messages() -> usize {
    if recording() {
        10_000
    } else {
        1_000
    }
}

fn samples(full: usize) -> usize {
    if recording() {
        full
    } else {
        (full / 5).max(20)
    }
}

/// Resident set size of this process in KiB, read from `ps`.
///
/// A sampler crate would be one more dependency for one number that the
/// operating system already reports, and `ps` is present wherever this
/// suite runs. `None` on a platform where it is not.
fn rss_kib() -> Option<u64> {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Cumulative CPU time of this process, from `ps -o time=`, as seconds.
///
/// The format is `[[dd-]hh:]mm:ss`. Resolution is one second, which is
/// coarse but sufficient over a soak measured in minutes.
fn cpu_seconds() -> Option<f64> {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    let (days, clock) = match text.split_once('-') {
        Some((days, rest)) => (days.parse::<f64>().ok()?, rest),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in clock.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + seconds)
}

/// A latency series with the two percentiles the plan asks for.
struct Series {
    name: &'static str,
    values: Vec<Duration>,
}

impl Series {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            values: Vec::new(),
        }
    }

    fn push(&mut self, value: Duration) {
        self.values.push(value);
    }

    /// Nearest-rank percentile. With no interpolation the reported value is
    /// always one that was actually observed.
    fn percentile(&self, q: f64) -> Duration {
        let mut sorted = self.values.clone();
        sorted.sort_unstable();
        if sorted.is_empty() {
            return Duration::ZERO;
        }
        let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
        sorted[rank - 1]
    }

    fn p50(&self) -> Duration {
        self.percentile(0.50)
    }

    fn p95(&self) -> Duration {
        self.percentile(0.95)
    }

    fn max(&self) -> Duration {
        self.values.iter().copied().max().unwrap_or_default()
    }

    fn report(&self) {
        record(&format!(
            "{:<34} n={:<6} p50={:>9} p95={:>9} max={:>9}",
            self.name,
            self.values.len(),
            micros(self.p50()),
            micros(self.p95()),
            micros(self.max()),
        ));
    }
}

fn micros(value: Duration) -> String {
    format!("{:.3}ms", value.as_secs_f64() * 1_000.0)
}

/// One recorded line. Always printed, so a default run still shows its own
/// numbers; `--nocapture` is what decides whether they reach the terminal.
fn record(line: &str) {
    println!("BENCH {line}");
}

fn terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("test terminal")
}

fn app() -> App {
    let mut app = App::new(StatusBar::default(), Theme::new(ThemeMode::NoColor));
    app.on_resize(WIDTH, HEIGHT);
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn event(sequence: u64, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("bench".into()),
        sequence,
        source: EventSource::Native,
        timestamp: chrono::Utc::now(),
        kind,
        diagnostic: None,
    }
}

/// A transcript of `count` messages, alternating user and assistant, each a
/// few wrapped lines wide so the layout work is realistic rather than a
/// one-line-per-entry best case.
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

/// One frame, timed. Returns what the draw itself cost.
fn frame(terminal: &mut Terminal<TestBackend>, app: &App) -> Duration {
    let start = Instant::now();
    terminal.draw(|f| draw(f, app)).expect("draw");
    start.elapsed()
}

// -- 1. input-to-frame latency ----------------------------------------

/// Typing, picker navigation, and scrolling against a full transcript.
///
/// Each sample is the reducer call plus the frame that answers it. The
/// transcript is loaded first so the numbers describe the loaded case the
/// plan asks about, not an empty screen.
#[test]
fn input_to_frame_latency_for_typing_picker_navigation_and_scrolling() {
    let mut app = app();
    let mut terminal = terminal();
    let messages = transcript_messages();
    fill_transcript(&mut app, messages);
    // The first frame after a bulk load pays for the whole wrap. It is a
    // load cost, not an input cost, so it is measured on its own and kept
    // out of the series below.
    let first = frame(&mut terminal, &app);
    record(&format!(
        "first frame after loading {messages} messages: {}",
        micros(first)
    ));

    let mut typing = Series::new("typing (loaded transcript)");
    let text: Vec<char> = "explain how the lexer keeps its span table in sync"
        .chars()
        .cycle()
        .take(samples(500))
        .collect();
    for c in text {
        let start = Instant::now();
        app.on_key(key(KeyCode::Char(c)));
        terminal.draw(|f| draw(f, &app)).expect("draw");
        typing.push(start.elapsed());
    }

    // Picker navigation. The session picker is the longest list the
    // interface builds from real data, so it is the honest case.
    let sessions: Vec<gritt_core::session::Session> =
        (0..samples(500)).map(session).collect();
    app.load_sessions(sessions);
    app.dispatch(gritt_harness::tui::command::Command::Sessions, None);
    let mut picker = Series::new("picker navigation");
    for _ in 0..samples(500) {
        let start = Instant::now();
        app.on_key(key(KeyCode::Down));
        terminal.draw(|f| draw(f, &app)).expect("draw");
        picker.push(start.elapsed());
    }
    app.on_key(key(KeyCode::Esc));

    let mut scrolling = Series::new("scrolling");
    for index in 0..samples(500) {
        let start = Instant::now();
        if index % 2 == 0 {
            app.scroll_up(3);
        } else {
            app.scroll_down(3);
        }
        terminal.draw(|f| draw(f, &app)).expect("draw");
        scrolling.push(start.elapsed());
    }

    for series in [&typing, &picker, &scrolling] {
        series.report();
        verdict(series.name, series.p95(), BUDGET_INPUT_P95);
        assert!(
            series.p95() < BOUND_INPUT_P95,
            "{} p95 {} exceeded the regression bound {}",
            series.name,
            micros(series.p95()),
            micros(BOUND_INPUT_P95),
        );
    }
}

fn session(index: usize) -> gritt_core::session::Session {
    gritt_core::session::Session {
        id: SessionId(format!("s{index}")),
        parent_id: None,
        name: format!("session-{index}"),
        workspace: std::path::PathBuf::from("/tmp/bench"),
        kind: gritt_core::session::SessionKind::Native {
            provider_profile: "local".into(),
            model: "openai/gpt-5-nano".into(),
            effort: Default::default(),
        },
        phase: gritt_core::session::Phase::Planning,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

/// Prints how a measured value compares to the plan's budget. The report
/// quotes these lines; a miss is named rather than softened.
fn verdict(name: &str, measured: Duration, budget: Duration) {
    record(&format!(
        "  budget {:<32} {:>9} vs {:>9} -> {}",
        name,
        micros(measured),
        micros(budget),
        if measured <= budget { "MET" } else { "NOT MET" },
    ));
}

// -- 2. sustained output ----------------------------------------------

/// 1,000 text deltas per second into a loaded transcript.
///
/// Render work per frame is the plan's 16 ms figure at 120x40. The backlog
/// number beside it is the queue evidence: deltas are produced at the plan's
/// rate and drained by the same loop that draws, so a backlog that grows
/// without bound is a queue that is not bounded.
#[test]
fn sustained_output_render_work_and_queue_depth() {
    let mut app = app();
    let mut terminal = terminal();
    fill_transcript(&mut app, transcript_messages());
    frame(&mut terminal, &app);

    let seconds = if recording() { 10 } else { 2 };
    let per_second = 1_000usize;
    let mut render = Series::new("render work at 120x40");
    let mut backlog_peak = 0usize;
    let mut delivered = 0usize;
    let start = Instant::now();
    let mut sequence = 0u64;
    while start.elapsed() < Duration::from_secs(seconds) {
        // How many deltas the producer would have queued by now, minus what
        // this loop has already applied.
        let due = (start.elapsed().as_secs_f64() * per_second as f64) as usize;
        let backlog = due.saturating_sub(delivered);
        backlog_peak = backlog_peak.max(backlog);
        // The loop applies whatever is waiting, then draws once. That is the
        // coalescing the plan asks for, expressed as a drain.
        for _ in 0..backlog.max(1) {
            sequence += 1;
            app.on_event(&event(
                sequence,
                EventKind::TextDelta {
                    text: "token ".into(),
                },
            ));
            delivered += 1;
        }
        render.push(frame(&mut terminal, &app));
    }
    let rate = delivered as f64 / start.elapsed().as_secs_f64();
    record(&format!(
        "sustained output: {delivered} deltas in {:.1}s ({rate:.0}/s), \
         peak backlog {backlog_peak}, frames {}",
        start.elapsed().as_secs_f64(),
        render.values.len(),
    ));
    render.report();
    verdict("render work p95", render.p95(), BUDGET_RENDER_P95);
    assert!(
        rate >= per_second as f64 * 0.9,
        "the loop drained only {rate:.0} deltas/s, below the plan's 1000/s"
    );
    assert!(
        render.p95() < BOUND_RENDER_P95,
        "render p95 {} exceeded the regression bound {}",
        micros(render.p95()),
        micros(BOUND_RENDER_P95),
    );
}

// -- 3. a large tool result -------------------------------------------

/// A 1 MiB tool result must not cost a frame proportional to its size.
///
/// The result is held as entry detail and only drawn when tool details are
/// expanded, so the frame after it arrives is the interesting one.
#[test]
fn a_one_mebibyte_tool_result_does_not_stall_the_frame() {
    let mut app = app();
    let mut terminal = terminal();
    fill_transcript(&mut app, transcript_messages() / 10);
    frame(&mut terminal, &app);

    let payload = "x".repeat(1024 * 1024);
    let call = ToolCall {
        id: ToolCallId("big".into()),
        name: "mcp__bench__dump".into(),
        arguments: serde_json::json!({}),
    };
    app.on_event(&event(1, EventKind::ToolCall { call }));
    let start = Instant::now();
    app.on_event(&event(
        2,
        EventKind::ToolResult {
            result: ToolResult {
                call_id: ToolCallId("big".into()),
                name: "mcp__bench__dump".into(),
                output: payload,
                is_error: false,
            },
        },
    ));
    let reduce = start.elapsed();
    let collapsed = frame(&mut terminal, &app);
    record(&format!(
        "1 MiB tool result: reduce {}, collapsed frame {}",
        micros(reduce),
        micros(collapsed)
    ));
    verdict("1 MiB result frame", collapsed, BUDGET_RENDER_P95);
    assert!(
        collapsed < BOUND_RENDER_P95,
        "the frame after a 1 MiB result cost {}",
        micros(collapsed)
    );
}

// -- 4. cancellation under load ---------------------------------------

/// Escape while output is streaming into a full transcript.
///
/// The plan's budget is a visible canceling state within 100 ms, with the
/// cleanup itself asynchronous. What is timed here is the reducer plus the
/// frame that shows it, which is the visible part.
#[test]
fn cancellation_under_load_shows_its_state_immediately() {
    let mut app = app();
    let mut terminal = terminal();
    fill_transcript(&mut app, transcript_messages());
    let mut worst = Duration::ZERO;
    for round in 0..samples(50) {
        app.begin_work(gritt_harness::tui::app::Work::Catalog, "streaming");
        for index in 0..200u64 {
            app.on_event(&event(
                round as u64 * 1_000 + index,
                EventKind::TextDelta {
                    text: "token ".into(),
                },
            ));
        }
        let start = Instant::now();
        app.on_key(key(KeyCode::Esc));
        terminal.draw(|f| draw(f, &app)).expect("draw");
        worst = worst.max(start.elapsed());
    }
    record(&format!("cancel under load: worst {}", micros(worst)));
    verdict("cancel under load", worst, BUDGET_CANCEL);
    assert!(
        worst < BOUND_CANCEL,
        "cancel took {} at worst",
        micros(worst)
    );
}

// -- 5. history paging and the memory plateau -------------------------

/// Resident memory over a bounded soak with history paging active.
///
/// The plan asks for a plateau rather than a ceiling: what matters is that
/// the curve stops climbing, not what number it stops at. The full run is
/// five minutes; the default run is short, and the soak length is recorded
/// beside the result so a scaled run is never mistaken for the full one.
#[test]
fn resident_memory_reaches_a_plateau_over_a_soak() {
    let Some(baseline) = rss_kib() else {
        record("resident memory: `ps` unavailable on this platform, skipped");
        return;
    };
    let soak = match std::env::var("GRITT_BENCH_SOAK_SECS") {
        Ok(value) => Duration::from_secs(value.parse().unwrap_or(30)),
        Err(_) if recording() => Duration::from_secs(300),
        Err(_) => Duration::from_secs(5),
    };
    let mut app = app();
    let mut terminal = terminal();
    fill_transcript(&mut app, transcript_messages());
    let cpu_start = cpu_seconds();

    let mut readings = Vec::new();
    let start = Instant::now();
    let mut sequence = 0u64;
    while start.elapsed() < soak {
        for _ in 0..1_000 {
            sequence += 1;
            app.on_event(&event(
                sequence,
                EventKind::TextDelta {
                    text: "token ".into(),
                },
            ));
        }
        frame(&mut terminal, &app);
        if let Some(rss) = rss_kib() {
            readings.push((start.elapsed().as_secs_f64(), rss));
        }
    }
    let cpu = match (cpu_start, cpu_seconds()) {
        (Some(before), Some(after)) => Some(after - before),
        _ => None,
    };
    let peak = readings
        .iter()
        .map(|(_, rss)| *rss)
        .max()
        .unwrap_or(baseline);
    // The plateau test: the last third of the run against the middle third.
    // A curve that is still climbing shows a rising step between them.
    let third = readings.len() / 3;
    let middle = readings
        .get(third..2 * third)
        .and_then(|slice| slice.iter().map(|(_, rss)| *rss).max())
        .unwrap_or(peak);
    let last = readings
        .get(2 * third..)
        .and_then(|slice| slice.iter().map(|(_, rss)| *rss).max())
        .unwrap_or(peak);
    record(&format!(
        "soak {:.0}s: {sequence} deltas, rss baseline {baseline} KiB, \
         peak {peak} KiB, middle-third {middle} KiB, last-third {last} KiB, \
         growth over the last two thirds {} KiB, cpu {}",
        soak.as_secs_f64(),
        last as i64 - middle as i64,
        cpu.map(|c| format!("{c:.0}s"))
            .unwrap_or_else(|| "unknown".into()),
    ));
    // The plan's budget is a plateau. Reported, never asserted: the value
    // is a property of the machine and the soak length, and this suite must
    // not fail on either. The verdict is what the report quotes.
    let plateau = last <= middle;
    record(&format!(
        "  budget {:<32} last-third {last} KiB vs middle-third {middle} KiB -> {}",
        "resident memory plateau",
        if plateau { "MET" } else { "NOT MET" },
    ));

    // What is asserted is the growth *rate*, which is a property of the
    // code rather than the machine: how much resident memory one delta
    // costs. A regression that changes the shape of the transcript or its
    // layout cache moves this by orders of magnitude; a busy machine does
    // not move it at all.
    let growth = peak.saturating_sub(baseline);
    let per_delta = (growth as f64 * 1024.0) / sequence.max(1) as f64;
    record(&format!(
        "soak growth: {growth} KiB over {sequence} deltas = {per_delta:.0} bytes/delta"
    ));
    assert!(
        per_delta < 2_048.0,
        "resident memory grew {per_delta:.0} bytes per delta, above the 2 KiB bound"
    );
}

// -- 6. MCP under load ------------------------------------------------

/// Several fake servers plus one that never answers.
///
/// The hung server must not hold the frame loop: while it sits at its
/// deadline the other servers reach `Ready`, tools are discoverable, and
/// frames keep being produced at the same cost as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_servers_and_one_hung_server_do_not_block_the_frame_loop() {
    let dir = tempfile::tempdir().unwrap();
    let entries = serde_json::json!({
        "bench-alpha": {"command": FIXTURE, "args": ["basic"]},
        "bench-beta": {"command": FIXTURE, "args": ["basic"]},
        "bench-gamma": {"command": FIXTURE, "args": ["basic"]},
        "bench-hung": {"command": FIXTURE, "args": ["silent"]},
    });
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": entries}).to_string(),
    )
    .unwrap();
    let runtime = McpRuntime::new(
        dir.path(),
        McpRuntimeSettings {
            // Short enough that the hung server resolves inside the test,
            // long enough that a loaded machine cannot fail a healthy one.
            init_timeout: Duration::from_secs(5),
            shutdown_grace: Duration::from_millis(300),
            ..McpRuntimeSettings::default()
        },
    )
    .with_trust(MemoryTrustStore::trust_all());

    let mut app = app();
    let mut terminal = terminal();
    fill_transcript(&mut app, transcript_messages() / 10);
    frame(&mut terminal, &app);

    // Frames are produced while the runtime opens, which is what the loop
    // does: `open` is background work and the interface keeps drawing.
    let opening = tokio::spawn({
        let cancel = CancellationToken::new();
        async move { runtime.open(&cancel).await.map(|s| (runtime, s)) }
    });
    let mut render = Series::new("render while MCP initializes");
    while !opening.is_finished() {
        render.push(frame(&mut terminal, &app));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (runtime, snapshots) = opening.await.unwrap().unwrap();
    let ready = snapshots
        .iter()
        .filter(|s| s.state == McpServerState::Ready)
        .count();
    let hung = snapshots
        .iter()
        .find(|s| s.name == "bench-hung")
        .expect("the hung entry is accounted for");
    record(&format!(
        "mcp load: {ready} of {} servers ready, hung entry state {:?}",
        snapshots.len(),
        hung.state
    ));
    render.report();
    assert_eq!(
        ready, 3,
        "the healthy servers must not wait for the hung one"
    );
    assert!(
        !matches!(hung.state, McpServerState::Ready),
        "the silent server cannot be ready"
    );
    assert!(
        render.p95() < BOUND_RENDER_P95,
        "frames during MCP startup cost {} at p95",
        micros(render.p95())
    );
    app.apply_mcp(snapshots);
    frame(&mut terminal, &app);
    runtime.shutdown().await;
}
