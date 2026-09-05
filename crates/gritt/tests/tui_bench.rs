//! Real-binary responsiveness measurements and the scripted walkthrough
//! (TKT-0020).
//!
//! `crates/gritt-harness/tests/tui_responsiveness.rs` measures the reducer
//! and the renderer. Two of the plan's budgets are not properties of either:
//! launch-to-usable-composer and idle CPU belong to the process, its startup
//! path, and its event loop. Those are measured here, against the real binary
//! in a real pseudo-terminal.
//!
//! The walkthrough at the bottom drives the binary through the flows the
//! chain's verification asks about, at both reference sizes, and records what
//! the terminal actually showed. It is machine-driven. It does not replace a
//! human at a terminal, and the ticket report says so.
//!
//! ```text
//! cargo test --release -p gritt --test tui_bench -- --nocapture --test-threads 1
//! ```
//!
//! The idle-CPU sample takes 30 seconds by the plan's definition, so it runs
//! only under `GRITT_BENCH=1`.

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

const ALT_SCREEN_ON: &str = "\x1b[?1049h";
const ALT_SCREEN_OFF: &str = "\x1b[?1049l";

/// The composer placeholder. Its presence is what "usable composer" means:
/// the input is drawn and the loop is reading keys.
const COMPOSER: &str = "Ask Gritt to do something";

fn recording() -> bool {
    std::env::var("GRITT_BENCH").is_ok_and(|value| value == "1")
}

fn record(line: &str) {
    println!("BENCH {line}");
}

/// A configured workspace: one profile with a key variable that resolves, so
/// startup does the work it does for a real user rather than a short path
/// through "nothing is configured".
fn write_config(dir: &Path) {
    std::fs::write(
        dir.join("config.toml"),
        "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
         [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
         base_url = \"http://127.0.0.1:9/v1\"\n\
         [profiles.local.key]\nkeychain_service_entry = \"gritt-bench-no-such-entry/local\"\n\
         env_var_name = \"GRITT_BENCH_KEY\"\n",
    )
    .unwrap();
}

/// Every CSI sequence replaced by a space, so words ratatui draws with cursor
/// moves between them read as ordinary text. Copied from `tui_pty.rs`; the two
/// harnesses are deliberately independent.
fn plain(output: &str) -> String {
    let mut text = String::with_capacity(output.len());
    let mut rest = output;
    while let Some(start) = rest.find('\x1b') {
        text.push_str(&rest[..start]);
        text.push(' ');
        let after = &rest[start + 1..];
        let skip = match after.strip_prefix('[') {
            Some(params) => params
                .find(|c: char| c.is_ascii_alphabetic() || c == '@' || c == '~')
                .map(|end| end + 2)
                .unwrap_or(after.len()),
            None => 1,
        };
        rest = &after[skip.min(after.len())..];
    }
    text.push_str(rest);
    let mut collapsed = String::with_capacity(text.len());
    let mut last_space = false;
    for c in text.chars() {
        if c == ' ' {
            if !last_space {
                collapsed.push(' ');
            }
            last_space = true;
        } else {
            collapsed.push(c);
            last_space = false;
        }
    }
    collapsed
}

/// A running binary in a pseudo-terminal, with its output collected on a
/// thread so a read can never block the test.
struct Session {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    seen: String,
    pid: u32,
}

impl Session {
    fn start(dir: &Path, rows: u16, cols: u16, extra: &[&str]) -> Self {
        write_config(dir);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
        command.args(
            [
                "--workspace",
                &dir.to_string_lossy(),
                "--database",
                &dir.join("gritt.db").to_string_lossy(),
                "tui",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain(extra.iter().map(|s| (*s).to_owned()))
            .collect::<Vec<_>>(),
        );
        command.env("GRITT_BENCH_KEY", "bench-key-never-drawn");
        command.env("TERM", "xterm-256color");
        command.env("NO_COLOR", "1");
        // Startup probes every installed agent, and each probe runs a real
        // executable under a 15 second deadline. An empty `PATH` means none
        // is found, so startup work finishes on a schedule this test
        // controls rather than on whatever is installed on the machine.
        command.env(
            "PATH",
            dir.join("no-such-bin").to_string_lossy().to_string(),
        );
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let pid = child.process_id().unwrap_or_default();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let writer = pair.master.take_writer().unwrap();
        Self {
            master: pair.master,
            child,
            rx,
            writer,
            seen: String::new(),
            pid,
        }
    }

    fn wait_for(&mut self, needle: &str, timeout: Duration) -> Duration {
        let start = Instant::now();
        let deadline = start + timeout;
        while !(self.seen.contains(needle) || plain(&self.seen).contains(needle)) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {needle:?}; saw {:?}",
                plain(&self.seen)
            );
            if let Ok(chunk) = self.rx.recv_timeout(remaining) {
                self.seen.push_str(&String::from_utf8_lossy(&chunk));
            }
        }
        start.elapsed()
    }

    fn drain(&mut self) {
        while let Ok(chunk) = self.rx.try_recv() {
            self.seen.push_str(&String::from_utf8_lossy(&chunk));
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn type_text(&mut self, text: &str) {
        self.send(text.as_bytes());
    }

    /// Bytes written since the last check. Zero over a window is what "no
    /// continuous redraw" means at the terminal.
    fn bytes_seen(&self) -> usize {
        self.seen.len()
    }

    fn resize(&self, rows: u16, cols: u16) {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
    }

    /// Waits until the terminal has been silent for `quiet`, or gives up
    /// after `cap`. Returns whether it went quiet.
    ///
    /// Startup work legitimately redraws: profile discovery can wait on the
    /// keychain, the workspace scan runs `git`, and each lands as its own
    /// message. Assuming a fixed settling time makes the idle measurement a
    /// race against that work; waiting for actual silence does not.
    fn wait_until_quiet(&mut self, quiet: Duration, cap: Duration) -> bool {
        let deadline = Instant::now() + cap;
        let mut last_change = Instant::now();
        let mut seen = self.bytes_seen();
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
            self.drain();
            if self.bytes_seen() != seen {
                seen = self.bytes_seen();
                last_change = Instant::now();
            } else if last_change.elapsed() >= quiet {
                return true;
            }
        }
        false
    }

    fn quit(&mut self) -> portable_pty::ExitStatus {
        // Ctrl-Q.
        self.send(&[0x11]);
        self.wait_for(ALT_SCREEN_OFF, Duration::from_secs(20));
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the process did not exit");
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// CPU percentage of one core, as the operating system reports it for the
/// interval since the process started. Sampled twice, the difference is the
/// interval's own usage.
fn cpu_seconds(pid: u32) -> Option<f64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
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

fn rss_kib(pid: u32) -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

// -- launch -----------------------------------------------------------

/// Time from spawn to a composer the user can type into, with a provider
/// already configured.
///
/// The plan's budget is 500 ms and states it must be independent of provider
/// and MCP readiness. `--no-models` is not passed to make it easy: the run
/// below still builds the store, runs migrations, resolves the workspace, and
/// opens the control plane. What it must not do is wait for a network.
#[test]
fn launch_to_usable_composer_with_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let mut session = Session::start(dir.path(), 40, 120, &["--no-models"]);
    session.wait_for(ALT_SCREEN_ON, Duration::from_secs(30));
    let alt = start.elapsed();
    // The home status line is the first thing that proves a composer is on
    // screen and the loop is reading keys.
    session.wait_for(COMPOSER, Duration::from_secs(30));
    let usable = start.elapsed();
    record(&format!(
        "launch: alternate screen at {:.0}ms, usable composer at {:.0}ms",
        alt.as_secs_f64() * 1_000.0,
        usable.as_secs_f64() * 1_000.0,
    ));
    record(&format!(
        "  budget {:<32} {:.0}ms vs 500ms -> {}",
        "launch to usable composer",
        usable.as_secs_f64() * 1_000.0,
        if usable <= Duration::from_millis(500) {
            "MET"
        } else {
            "NOT MET"
        },
    ));
    // Loose: the budget comparison above is the real statement. This only
    // catches a startup that has begun waiting on something.
    assert!(
        usable < Duration::from_secs(10),
        "the composer took {usable:?} to appear"
    );
    assert!(session.quit().success());
    assert!(
        !session.seen.contains("bench-key-never-drawn"),
        "the key must never be drawn"
    );
}

/// A provider that accepts the connection and then never answers.
///
/// This is what "independent of provider readiness" has to survive: the
/// model list is requested and does not come back.
fn stalling_provider() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            // Held open, never answered, never closed.
            held.push(stream);
        }
    });
    port
}

/// Launch with a model list that is requested and never arrives.
///
/// The earlier measurement passed `--no-models`, which skips the request
/// entirely and so cannot show whether the composer waits for it. This one
/// makes the request and lets it hang.
#[test]
fn launch_to_usable_composer_with_a_pending_catalog_request() {
    let port = stalling_provider();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
             [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
             base_url = \"http://127.0.0.1:{port}/v1\"\n\
             [profiles.local.key]\nkeychain_service_entry = \"gritt-bench-no-such-entry/local\"\n\
             env_var_name = \"GRITT_BENCH_KEY\"\n"
        ),
    )
    .unwrap();
    let start = Instant::now();
    // No `--no-models`: the catalog is requested and will not answer.
    let mut session = Session::start(dir.path(), 40, 120, &[]);
    session.wait_for(ALT_SCREEN_ON, Duration::from_secs(30));
    session.wait_for(COMPOSER, Duration::from_secs(30));
    let usable = start.elapsed();
    record(&format!(
        "launch with a pending catalog: usable composer at {:.0}ms",
        usable.as_secs_f64() * 1_000.0
    ));
    record(&format!(
        "  budget {:<32} {:.0}ms vs 500ms -> {}",
        "launch, catalog pending",
        usable.as_secs_f64() * 1_000.0,
        if usable <= Duration::from_millis(500) {
            "MET"
        } else {
            "NOT MET"
        },
    ));
    assert!(
        usable < Duration::from_secs(10),
        "the composer waited {usable:?} for a model list that never arrived"
    );
    assert!(session.quit().success());
}

// -- idle -------------------------------------------------------------

/// CPU and terminal traffic over 30 idle seconds.
///
/// The plan asks for no continuous full redraw and average CPU below 1% of
/// one core. Both are measured: `ps` for the process, and the byte count the
/// terminal received for the redraw claim.
#[test]
fn idle_cpu_and_redraw_over_thirty_seconds() {
    if !recording() {
        record("idle: skipped, set GRITT_BENCH=1 to run the 30 second sample");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start(dir.path(), 40, 120, &["--no-models"]);
    session.wait_for(ALT_SCREEN_ON, Duration::from_secs(30));
    session.wait_for(COMPOSER, Duration::from_secs(30));
    // Measure only once the terminal has actually gone silent.
    assert!(
        session.wait_until_quiet(Duration::from_secs(2), Duration::from_secs(60)),
        "startup never went quiet, so there is no idle period to measure"
    );

    let Some(before_cpu) = cpu_seconds(session.pid) else {
        record("idle: `ps` reported no CPU time, skipped rather than recorded as zero");
        assert!(session.quit().success());
        return;
    };
    let before_bytes = session.bytes_seen();
    let before_rss = rss_kib(session.pid);
    let window = Duration::from_secs(30);
    let start = Instant::now();
    while start.elapsed() < window {
        thread::sleep(Duration::from_millis(200));
        session.drain();
    }
    let elapsed = start.elapsed();
    let Some(after_cpu) = cpu_seconds(session.pid) else {
        record("idle: `ps` stopped reporting, skipped rather than recorded as zero");
        assert!(session.quit().success());
        return;
    };
    let cpu = after_cpu - before_cpu;
    let bytes = session.bytes_seen() - before_bytes;
    let percent = cpu / elapsed.as_secs_f64() * 100.0;
    record(&format!(
        "idle {:.0}s: cpu {cpu:.0}s ({percent:.1}% of one core), \
         {bytes} bytes written to the terminal, rss {:?} -> {:?} KiB",
        elapsed.as_secs_f64(),
        before_rss,
        rss_kib(session.pid),
    ));
    record(&format!(
        "  budget {:<32} {percent:.1}% vs 1.0% -> {}",
        "idle CPU over 30s",
        if percent <= 1.0 { "MET" } else { "NOT MET" },
    ));
    record(&format!(
        "  budget {:<32} {bytes} bytes -> {}",
        "no continuous full redraw",
        if bytes == 0 { "MET" } else { "NOT MET" },
    ));
    assert!(session.quit().success());
}

/// The regression form of the budget above, short enough to run every time.
///
/// An idle full-screen mode must write nothing at all to the terminal. Before
/// TKT-0020 the loop drew on every wakeup, including a 50 ms tick with no
/// input behind it, so an idle session wrote roughly 540 bytes a second
/// forever. Nothing in the interface is drawn from a clock, so zero is the
/// correct number and any non-zero result is the same defect returning.
#[test]
fn an_idle_session_writes_nothing_to_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::start(dir.path(), 40, 120, &["--no-models"]);
    session.wait_for(ALT_SCREEN_ON, Duration::from_secs(30));
    session.wait_for(COMPOSER, Duration::from_secs(30));
    // Startup's background work (profiles, agent probes, the change scan)
    // each land as a message and legitimately redraw. Wait for real
    // silence rather than assuming a settling time: a probe finishing a
    // second late is a correct redraw, not the defect this guards.
    assert!(
        session.wait_until_quiet(Duration::from_secs(2), Duration::from_secs(60)),
        "startup never went quiet, so there is no idle period to measure"
    );

    let before = session.bytes_seen();
    thread::sleep(Duration::from_secs(3));
    session.drain();
    let written = session.bytes_seen() - before;
    record(&format!(
        "idle regression: {written} bytes over 3 idle seconds"
    ));
    assert_eq!(
        written, 0,
        "an idle session wrote {written} bytes; the redraw gate has regressed"
    );
    assert!(session.quit().success());
}

// -- the scripted walkthrough -----------------------------------------

/// The chain's flows at both reference sizes, driven through the real
/// binary: home, `/connect`, `/models`, `/effort`, a fixture conversation,
/// `/mcp`, `/sidebar`, and quit.
///
/// Every assertion is on text the terminal actually received. What this
/// cannot do is judge spacing, colour, font rendering, or perceived latency,
/// and it cannot send Shift-Enter, because the harness writes bytes and a
/// terminal is what turns a chord into one. The report carries that list.
#[test]
fn scripted_walkthrough_at_both_reference_sizes() {
    for (rows, cols) in [(40u16, 120u16), (24, 80)] {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::start(dir.path(), rows, cols, &["--no-models"]);
        session.wait_for(ALT_SCREEN_ON, Duration::from_secs(30));
        session.wait_for(COMPOSER, Duration::from_secs(30));
        record(&format!("walkthrough {cols}x{rows}: home drawn"));

        // /connect: the provider and agent picker.
        session.type_text("/connect\r");
        session.wait_for("local", Duration::from_secs(10));
        record(&format!(
            "walkthrough {cols}x{rows}: /connect listed the configured profile"
        ));
        session.send(&[0x1b]); // Esc
        thread::sleep(Duration::from_millis(200));
        session.drain();

        // /models with no catalog: the picker still opens and explains.
        session.type_text("/models\r");
        thread::sleep(Duration::from_millis(400));
        session.drain();
        record(&format!(
            "walkthrough {cols}x{rows}: /models opened, screen mentions models: {}",
            plain(&session.seen).contains("model")
        ));
        session.send(&[0x1b]);
        thread::sleep(Duration::from_millis(200));

        // /effort.
        session.type_text("/effort\r");
        thread::sleep(Duration::from_millis(400));
        session.drain();
        record(&format!(
            "walkthrough {cols}x{rows}: /effort opened, screen mentions effort: {}",
            plain(&session.seen).contains("effort")
        ));
        session.send(&[0x1b]);
        thread::sleep(Duration::from_millis(200));

        // /mcp: this workspace has no .mcp.json, which must be stated
        // rather than shown as an empty list.
        session.type_text("/mcp\r");
        thread::sleep(Duration::from_millis(600));
        session.drain();
        record(&format!(
            "walkthrough {cols}x{rows}: /mcp opened, screen mentions servers: {}",
            plain(&session.seen).contains("server")
        ));
        session.send(&[0x1b]);
        thread::sleep(Duration::from_millis(200));

        // /sidebar: a column at 120 columns, a drawer at 80.
        let before = session.bytes_seen();
        session.type_text("/sidebar\r");
        thread::sleep(Duration::from_millis(400));
        session.drain();
        assert!(
            session.bytes_seen() > before,
            "{cols}x{rows}: /sidebar drew nothing"
        );
        record(&format!(
            "walkthrough {cols}x{rows}: /sidebar redrew ({} bytes)",
            session.bytes_seen() - before
        ));
        session.send(&[0x1b]);
        thread::sleep(Duration::from_millis(200));

        // A prompt with no reachable provider: the composer accepts it and
        // the failure is reported in the transcript, not a crash.
        session.type_text("hello from the walkthrough\r");
        thread::sleep(Duration::from_millis(1_500));
        session.drain();
        record(&format!(
            "walkthrough {cols}x{rows}: prompt submitted, transcript shows it: {}",
            plain(&session.seen).contains("hello from the walkthrough")
        ));

        // A resize between the two reference sizes, then quit.
        session.resize(
            if rows == 40 { 24 } else { 40 },
            if cols == 120 { 80 } else { 120 },
        );
        thread::sleep(Duration::from_millis(400));
        session.drain();

        let status = session.quit();
        record(&format!(
            "walkthrough {cols}x{rows}: quit restored the terminal, status {status:?}"
        ));
        assert!(status.success());
        assert!(
            !session.seen.contains("bench-key-never-drawn"),
            "{cols}x{rows}: the key must never be drawn"
        );
    }
}
