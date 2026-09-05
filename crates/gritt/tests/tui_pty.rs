//! Drives the full-screen mode in a real pseudo-terminal: it must enter the
//! alternate screen, survive a resize, and restore the terminal on quit.

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const ALT_SCREEN_ON: &str = "\x1b[?1049h";
const ALT_SCREEN_OFF: &str = "\x1b[?1049l";

fn write_config(dir: &Path) {
    std::fs::write(
        dir.join("config.toml"),
        "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
         [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
         base_url = \"http://127.0.0.1:9/v1\"\n\
         [profiles.local.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/local\"\n\
         env_var_name = \"GRITT_E2E_KEY\"\n",
    )
    .unwrap();
}

/// The terminal stream with every CSI sequence replaced by one space and
/// runs of spaces collapsed, so words that ratatui draws with cursor moves
/// between them read as ordinary text.
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

fn wait_for(rx: &mpsc::Receiver<Vec<u8>>, seen: &mut String, needle: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !(seen.contains(needle) || plain(seen).contains(needle)) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for {needle:?}; saw {seen:?}"
        );
        if let Ok(chunk) = rx.recv_timeout(remaining) {
            seen.push_str(&String::from_utf8_lossy(&chunk));
        }
    }
}

#[test]
fn full_screen_mode_enters_resizes_and_restores_the_terminal() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
    command.args([
        "--workspace",
        &dir.path().to_string_lossy(),
        "--database",
        &dir.path().join("gritt.db").to_string_lossy(),
        "tui",
        "--no-models",
        "--session",
        "pty",
    ]);
    command.env("GRITT_E2E_KEY", "pty-key");
    command.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
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
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(20));
    // The status bar names the session once the first frame is drawn.
    wait_for(&rx, &mut seen, "pty", Duration::from_secs(20));

    pair.master
        .resize(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    // A redraw after the resize proves the loop handled the event.
    let before = seen.len();
    thread::sleep(Duration::from_millis(300));
    while let Ok(chunk) = rx.try_recv() {
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(seen.len() > before, "no redraw after resize");

    let mut writer = pair.master.take_writer().unwrap();
    // Ctrl-Q quits.
    writer.write_all(&[0x11]).unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, ALT_SCREEN_OFF, Duration::from_secs(20));
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the process did not exit after Ctrl-Q"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status {status:?}");
    assert!(!seen.contains("pty-key"), "the key must never be drawn");
}

/// A one-shot provider stand-in: answers each POST in order with the next
/// canned SSE body.
fn serve(responses: Vec<Vec<u8>>) -> u16 {
    use std::io::BufRead;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for body in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut request = vec![0u8; length];
            reader.read_exact(&mut request).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
                .unwrap();
            stream.write_all(&body).unwrap();
            stream.flush().unwrap();
        }
    });
    port
}

fn tool_call_sse(tool: &str, arguments: serde_json::Value) -> Vec<u8> {
    let first = serde_json::json!({
        "id": "chatcmpl-t", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": null,
            "tool_calls": [{"index": 0, "id": format!("call_{tool}"), "type": "function",
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
    let delta = serde_json::json!({
        "id": "chatcmpl-e2e", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let stop = serde_json::json!({
        "id": "chatcmpl-e2e", "object": "chat.completion.chunk", "model": "openai/gpt-5-nano",
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    format!("data: {delta}\n\ndata: {stop}\n\ndata: [DONE]\n\n").into_bytes()
}

/// True when the output contains an SGR sequence that sets a foreground or
/// background color (30 to 37, 40 to 47, 90 to 97, 100 to 107, 38, 48).
fn contains_color_sgr(output: &str) -> bool {
    let mut rest = output;
    while let Some(start) = rest.find("\x1b[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find(|c: char| !(c.is_ascii_digit() || c == ';')) else {
            break;
        };
        if after[end..].starts_with('m') {
            let colored = after[..end].split(';').any(|param| {
                matches!(
                    param.parse::<u16>(),
                    Ok(30..=38 | 40..=48 | 90..=97 | 100..=107)
                )
            });
            if colored {
                return true;
            }
        }
        rest = &after[end..];
    }
    false
}

#[test]
fn approval_diff_palette_and_sessions_views_work_by_keyboard_without_color() {
    let port = serve(vec![
        tool_call_sse(
            "file_write",
            serde_json::json!({"path": "notes.txt", "content": "written by the agent\n"}),
        ),
        text_sse("Write finished."),
    ]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
             [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
             base_url = \"http://127.0.0.1:{port}/v1\"\n\
             [profiles.local.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/local\"\n\
             env_var_name = \"GRITT_E2E_KEY\"\n"
        ),
    )
    .unwrap();
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
    command.args([
        "--workspace",
        &dir.path().to_string_lossy(),
        "--database",
        &dir.path().join("gritt.db").to_string_lossy(),
        "tui",
        "--code",
        "--no-models",
        "--session",
        "views",
    ]);
    command.env("GRITT_E2E_KEY", "pty-key");
    command.env("TERM", "xterm-256color");
    command.env("NO_COLOR", "1");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
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
    let mut seen = String::new();
    let mut writer = pair.master.take_writer().unwrap();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(20));
    wait_for(&rx, &mut seen, "views", Duration::from_secs(20));

    // Keyboard-only: the command palette and the session list open and
    // close with Ctrl-P, Ctrl-S, and Esc.
    writer.write_all(&[0x10]).unwrap();
    writer.flush().unwrap();
    // The panel hint is body text on a freshly cleared area, so it
    // arrives whole; a panel title can be redrawn cell by cell.
    wait_for(&rx, &mut seen, "same registry", Duration::from_secs(20));
    writer.write_all(&[0x1b]).unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));
    writer.write_all(&[0x13]).unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, "Enter resumes", Duration::from_secs(20));
    // The picker opens before the store answers. Its rows carry the
    // session's timestamp, which nothing else on screen shows, so seeing
    // one proves the loaded sessions reached the open list rather than
    // only the state behind it.
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    wait_for(&rx, &mut seen, &today, Duration::from_secs(20));
    writer.write_all(&[0x1b]).unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));

    // A coding prompt reaches the fixture provider, which asks for a write.
    writer.write_all(b"write notes\r").unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, "approve?", Duration::from_secs(30));
    wait_for(&rx, &mut seen, "file_write", Duration::from_secs(20));
    // [d] toggles the diff view, which shows the pending write.
    writer.write_all(b"d").unwrap();
    writer.flush().unwrap();
    wait_for(
        &rx,
        &mut seen,
        "written by the agent",
        Duration::from_secs(20),
    );
    // [y] approves; the file lands and the model finishes the turn.
    writer.write_all(b"y").unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, "Write finished.", Duration::from_secs(30));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
        "written by the agent\n"
    );

    writer.write_all(&[0x11]).unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, ALT_SCREEN_OFF, Duration::from_secs(20));
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the process did not exit after Ctrl-Q"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status {status:?}");
    assert!(!seen.contains("pty-key"), "the key must never be drawn");
    assert!(
        !contains_color_sgr(&seen),
        "NO_COLOR was set but a color SGR sequence was drawn"
    );
}

#[test]
fn plain_text_joins_words_drawn_with_cursor_moves() {
    let drawn = "\x1b[13;33Hcommand\x1b[13;41Hpalette\x1b[13;49H(Enter\x1b[13;56Hruns)";
    assert!(plain(drawn).contains("command palette (Enter runs)"));
}

#[test]
fn color_sgr_detection_recognizes_colors_but_not_modifiers() {
    assert!(contains_color_sgr("\x1b[33mtext\x1b[0m"));
    assert!(contains_color_sgr("\x1b[1;38;5;12mtext"));
    assert!(!contains_color_sgr("\x1b[1m\x1b[7mtext\x1b[0m\x1b[?1049h"));
}

type Master = Box<dyn portable_pty::MasterPty + Send>;
type Child = Box<dyn portable_pty::Child + Send + Sync>;
type Writer = Box<dyn Write + Send>;

/// One fixture run: the terminal, the child, its output, and its input.
struct Fixture {
    master: Master,
    child: Child,
    output: mpsc::Receiver<Vec<u8>>,
    input: Writer,
}

/// Spawns `gritt tui --fixture <screen>` in a pseudo-terminal of the
/// given size.
fn spawn_fixture(screen: &str, cols: u16, rows: u16) -> Fixture {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
    command.args(["tui", "--fixture", screen]);
    command.env("TERM", "xterm-256color");
    let child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
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
    let input = pair.master.take_writer().unwrap();
    Fixture {
        master: pair.master,
        child,
        output: rx,
        input,
    }
}

fn quit(child: &mut Child, rx: &mpsc::Receiver<Vec<u8>>, seen: &mut String, writer: &mut Writer) {
    writer.write_all(&[0x11]).unwrap();
    writer.flush().unwrap();
    wait_for(rx, seen, ALT_SCREEN_OFF, Duration::from_secs(20));
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the process did not exit after Ctrl-Q"
        );
        thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status {status:?}");
}

/// The fixture walkthrough: home, `/connect`, `/models`, `/effort`,
/// `/mcp`, and `/help`, driven by keyboard in a real pseudo-terminal at a
/// wide size and then resized narrow.
#[test]
fn the_fixture_home_walkthrough_runs_by_keyboard_and_never_opens_a_session() {
    let Fixture {
        master,
        mut child,
        output: rx,
        input: mut writer,
    } = spawn_fixture("home", 120, 40);
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(20));
    // The run is labelled a fixture and says what to do first.
    wait_for(
        &rx,
        &mut seen,
        "Use /connect to get started.",
        Duration::from_secs(20),
    );
    wait_for(&rx, &mut seen, "fixture", Duration::from_secs(20));

    // `/` opens suggestions and Enter runs the highlighted command.
    writer.write_all(b"/connect\r").unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, "AI providers", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "Installed agents", Duration::from_secs(20));
    // A connector says who owns its model, and an uninstalled one is not
    // selectable.
    wait_for(&rx, &mut seen, "Managed by agent", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "not installed", Duration::from_secs(20));

    // Typing filters, and selecting a provider opens its model picker.
    writer.write_all(b"openai\r").unwrap();
    writer.flush().unwrap();
    // Needles are body text, not panel titles: ratatui redraws only the
    // cells that changed, so a title over a previous panel's border can
    // reach the stream in pieces.
    wait_for(&rx, &mut seen, "GPT-5 nano", Duration::from_secs(20));
    wait_for(
        &rx,
        &mut seen,
        "openai/gpt-5-codex-preview",
        Duration::from_secs(20),
    );
    // The catalog state is visible, not implied by a blank list.
    wait_for(&rx, &mut seen, "catalog fresh", Duration::from_secs(20));
    writer.write_all(&[0x1b]).unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));

    writer.write_all(b"/effort\r").unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, "Model default", Duration::from_secs(20));
    writer.write_all(&[0x1b]).unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));

    writer.write_all(b"/mcp\r").unwrap();
    writer.flush().unwrap();
    wait_for(
        &rx,
        &mut seen,
        "gritt-local-memory",
        Duration::from_secs(20),
    );
    wait_for(&rx, &mut seen, "awaiting approval", Duration::from_secs(20));
    writer.write_all(&[0x1b]).unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));

    writer.write_all(b"/help\r").unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, "Limitations", Duration::from_secs(20));
    writer.write_all(&[0x1b]).unwrap();
    writer.flush().unwrap();
    // Escape must land on its own: a byte written straight after it is
    // read as part of an escape sequence, not as a keypress.
    thread::sleep(Duration::from_millis(200));

    // An unknown command is refused locally and the input is kept.
    writer.write_all(b"/deploy\r").unwrap();
    writer.flush().unwrap();
    wait_for(&rx, &mut seen, "unknown", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "lists them", Duration::from_secs(20));

    // The narrow size still draws.
    let before = seen.len();
    master
        .resize(PtySize {
            rows: 20,
            cols: 60,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    thread::sleep(Duration::from_millis(400));
    while let Ok(chunk) = rx.try_recv() {
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(seen.len() > before, "no redraw after the resize");

    quit(&mut child, &rx, &mut seen, &mut writer);
}

/// The sidebar column appears at 111 columns and collapses at 109, in a
/// real terminal and not only in a `TestBackend` buffer.
#[test]
fn the_fixture_conversation_shows_the_sidebar_only_above_110_columns() {
    for (cols, expect_sidebar) in [(111u16, true), (109u16, false)] {
        let Fixture {
            master,
            mut child,
            output: rx,
            input: mut writer,
        } = spawn_fixture("conversation", cols, 30);
        let mut seen = String::new();
        wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(20));
        wait_for(&rx, &mut seen, "api-cleanup", Duration::from_secs(20));
        // Give the first frames time to arrive before judging the sidebar.
        thread::sleep(Duration::from_millis(600));
        while let Ok(chunk) = rx.try_recv() {
            seen.push_str(&String::from_utf8_lossy(&chunk));
        }
        let text = plain(&seen);
        assert_eq!(
            text.contains("Changed files"),
            expect_sidebar,
            "at {cols} columns the sidebar column expectation failed"
        );
        // Either way the transcript and the composer keep their space.
        assert!(text.contains("Tidy the public API"), "at {cols} columns");
        assert!(text.contains("effort medium"), "at {cols} columns");
        if !expect_sidebar {
            // `/sidebar` opens the drawer instead, then Escape closes it.
            writer.write_all(b"/sidebar\r").unwrap();
            writer.flush().unwrap();
            wait_for(&rx, &mut seen, "Changed files", Duration::from_secs(20));
            writer.write_all(&[0x1b]).unwrap();
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(200));

            // Reopen it, then grow the terminal past the column threshold.
            // The drawer is no longer drawn, so it must not still be
            // taking the keyboard: what is typed has to reach the
            // composer that is visible.
            writer.write_all(b"/sidebar\r").unwrap();
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(300));
            master
                .resize(PtySize {
                    rows: 40,
                    cols: 120,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .unwrap();
            thread::sleep(Duration::from_millis(500));
            // Ctrl-P is ignored by the drawer and opens the palette from
            // the main view, so the palette appearing proves the key
            // reached the visible interface and not a stale overlay.
            writer.write_all(&[0x10]).unwrap();
            writer.flush().unwrap();
            wait_for(&rx, &mut seen, "same registry", Duration::from_secs(20));
            writer.write_all(&[0x1b]).unwrap();
            writer.flush().unwrap();
            thread::sleep(Duration::from_millis(200));
        }
        quit(&mut child, &rx, &mut seen, &mut writer);
    }
}

// -- TKT-0019: the live paths in a real terminal -----------------------

/// Spawns `gritt tui` against a workspace with the given arguments.
fn spawn_tui(dir: &Path, args: &[&str], cols: u16, rows: u16, env: &[(&str, &str)]) -> Fixture {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
    command.args([
        "--workspace",
        &dir.to_string_lossy(),
        "--database",
        &dir.join("gritt.db").to_string_lossy(),
        "tui",
    ]);
    command.args(args);
    command.env("TERM", "xterm-256color");
    for (name, value) in env {
        command.env(name, value);
    }
    let child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
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
    let input = pair.master.take_writer().unwrap();
    Fixture {
        master: pair.master,
        child,
        output: rx,
        input,
    }
}

fn send(writer: &mut Writer, bytes: &[u8]) {
    writer.write_all(bytes).unwrap();
    writer.flush().unwrap();
}

/// Escape must land on its own: a byte written straight after it is read
/// as part of an escape sequence rather than as a keypress.
fn escape(writer: &mut Writer) {
    send(writer, &[0x1b]);
    thread::sleep(Duration::from_millis(250));
}

/// First run: no configuration at all. `/connect` still opens, the custom
/// endpoint form writes a project profile, and the profile is usable in
/// the same run without a restart.
///
/// The key field is left blank on purpose. A PTY test must not write to
/// the developer's login keychain, and leaving it blank is a real path:
/// the profile is saved and the variable is named as what to export.
#[test]
fn a_first_run_with_no_configuration_can_set_up_a_provider() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!dir.path().join("config.toml").exists());
    let Fixture {
        master: _master,
        mut child,
        output: rx,
        input: mut writer,
    } = spawn_tui(dir.path(), &["--no-models"], 120, 40, &[]);
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(20));
    // Nothing is configured, so the home screen says where to start.
    wait_for(
        &rx,
        &mut seen,
        "Use /connect to get started.",
        Duration::from_secs(20),
    );

    send(&mut writer, b"/connect\r");
    wait_for(&rx, &mut seen, "Add a provider", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "Custom endpoint", Duration::from_secs(20));
    send(&mut writer, b"Custom\r");
    wait_for(&rx, &mut seen, "Provider setup", Duration::from_secs(20));
    // The key is described as masked and keychain-only before it is typed.
    wait_for(&rx, &mut seen, "never echoed", Duration::from_secs(20));

    // name, endpoint, then the variable Gritt derives from the name.
    send(&mut writer, b"ptylocal\r");
    send(&mut writer, b"http://127.0.0.1:9/v1\r");
    send(&mut writer, b"\r");
    // Ctrl-D writes to this workspace instead of the user configuration.
    send(&mut writer, &[0x04]);
    wait_for(&rx, &mut seen, "config.toml", Duration::from_secs(20));
    // Enter on the key field saves with no key typed.
    send(&mut writer, b"\r");
    wait_for(&rx, &mut seen, "saved to", Duration::from_secs(20));

    // The variable Gritt derived from the name is what the profile names,
    // and no key value was written anywhere.
    let written = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(written.contains("ptylocal"), "{written}");
    assert!(written.contains("127.0.0.1:9"), "{written}");
    assert!(written.contains("PTYLOCAL_API_KEY"), "{written}");

    // The reloaded configuration makes it selectable in the same run.
    send(&mut writer, b"/connect\r");
    wait_for(&rx, &mut seen, "AI providers", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "ptylocal", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "no key; set", Duration::from_secs(20));
    escape(&mut writer);

    quit(&mut child, &rx, &mut seen, &mut writer);
    assert!(
        !plain(&seen).contains("Managed by agent · Managed"),
        "the dialog duplicated a row"
    );
}

/// The lazy path: `gritt tui` with no named session opens on a draft and
/// creates the session when the first prompt is submitted. `/new` then
/// returns to a fresh draft without deleting it, and `/sessions` still
/// lists it.
#[test]
fn the_first_prompt_creates_the_session_and_new_keeps_it() {
    let port = serve(vec![text_sse("First answer."), text_sse("Second answer.")]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
             [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
             base_url = \"http://127.0.0.1:{port}/v1\"\n\
             [profiles.local.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/local\"\n\
             env_var_name = \"GRITT_E2E_KEY\"\n"
        ),
    )
    .unwrap();
    let Fixture {
        master: _master,
        mut child,
        output: rx,
        input: mut writer,
    } = spawn_tui(
        dir.path(),
        &["--no-models"],
        120,
        40,
        &[("GRITT_E2E_KEY", "pty-key")],
    );
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(20));
    // Home, with the configured selection already drafted and no session.
    wait_for(&rx, &mut seen, "local", Duration::from_secs(20));

    // The first prompt is what opens the session.
    send(&mut writer, b"hello\r");
    wait_for(&rx, &mut seen, "First answer.", Duration::from_secs(30));
    // The sidebar is a column at this width and names the live session.
    wait_for(&rx, &mut seen, "Changed files", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "openai/gpt-5-nano", Duration::from_secs(20));

    // `/new` clears the view and keeps the session.
    send(&mut writer, b"/new\r");
    wait_for(
        &rx,
        &mut seen,
        "the previous session is still listed",
        Duration::from_secs(20),
    );
    send(&mut writer, &[0x13]);
    wait_for(&rx, &mut seen, "Enter resumes", Duration::from_secs(20));
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    wait_for(&rx, &mut seen, &today, Duration::from_secs(20));
    // Resuming reloads the history the first session produced.
    send(&mut writer, b"\r");
    wait_for(&rx, &mut seen, "First answer.", Duration::from_secs(30));

    quit(&mut child, &rx, &mut seen, &mut writer);
    assert!(!seen.contains("pty-key"), "the key must never be drawn");
}

/// A connector session shows the connector's identity and refuses the
/// native pickers. Skipped honestly when no connector executable is
/// available on this machine.
#[test]
fn a_connector_session_shows_its_identity_and_refuses_the_native_pickers() {
    // A connector Gritt can actually start is required; a stub would
    // prove nothing about the connector path.
    let Some(executable) = ["codex", "claude", "cursor-agent"]
        .into_iter()
        .find_map(which)
    else {
        eprintln!(
            "skipped: no connector executable (codex, claude, cursor-agent) is installed here"
        );
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    let connector = match executable.file_name().unwrap().to_string_lossy().as_ref() {
        "codex" => "codex",
        "claude" => "claude-code",
        _ => "cursor",
    };
    let Fixture {
        master: _master,
        mut child,
        output: rx,
        input: mut writer,
    } = spawn_tui(
        dir.path(),
        &[
            "--no-models",
            "--session",
            "connector-pty",
            "--connector",
            connector,
        ],
        120,
        40,
        &[("GRITT_E2E_KEY", "pty-key")],
    );
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(30));
    wait_for(&rx, &mut seen, "connector-pty", Duration::from_secs(30));
    // The native pickers do not apply to a connector session, and the
    // refusal names the agent that owns the setting.
    send(&mut writer, b"/models\r");
    wait_for(
        &rx,
        &mut seen,
        &format!("runs on {connector}"),
        Duration::from_secs(30),
    );
    wait_for(
        &rx,
        &mut seen,
        "managed by the agent",
        Duration::from_secs(20),
    );
    quit(&mut child, &rx, &mut seen, &mut writer);
}

/// The first executable of that name on `PATH`, or `None`.
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The reference walkthrough against the live control plane at a narrow
/// size: home, a prompt that opens the session, the sidebar as a drawer,
/// `/mcp` accounting for a broken entry, `/help`, and Ctrl-J for a
/// newline. The wide size is covered by the tests above.
///
/// This is the automated half of the walkthrough. What it cannot cover is
/// recorded with it: Shift-Enter is only distinguishable on terminals that
/// report it, and this harness writes bytes rather than key events, so
/// Ctrl-J is the newline the test can prove.
#[test]
fn the_live_walkthrough_runs_at_eighty_by_twenty_four() {
    let port = serve(vec![text_sse("Narrow answer.")]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
             [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
             base_url = \"http://127.0.0.1:{port}/v1\"\n\
             [profiles.local.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/local\"\n\
             env_var_name = \"GRITT_E2E_KEY\"\n"
        ),
    )
    .unwrap();
    // An entry that cannot run: no process is started and `/mcp` must
    // still account for it with a reason.
    std::fs::write(
        dir.path().join(".mcp.json"),
        r#"{"mcpServers": {"broken-entry": {"args": ["x"]}}}"#,
    )
    .unwrap();
    let Fixture {
        master: _master,
        mut child,
        output: rx,
        input: mut writer,
    } = spawn_tui(
        dir.path(),
        &["--no-models"],
        80,
        24,
        &[("GRITT_E2E_KEY", "pty-key")],
    );
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(20));
    wait_for(&rx, &mut seen, "Type a prompt", Duration::from_secs(20));

    // Ctrl-J inserts a newline instead of submitting: the prompt that
    // arrives has both lines in it, which is what the transcript shows
    // once the layout switches and the area is redrawn whole.
    send(&mut writer, b"first line");
    send(&mut writer, &[0x0a]);
    send(&mut writer, b"second line");
    thread::sleep(Duration::from_millis(300));
    send(&mut writer, b"\r");
    wait_for(&rx, &mut seen, "Narrow answer.", Duration::from_secs(30));
    let transcript = plain(&seen);
    assert!(transcript.contains("first line"), "{transcript}");
    assert!(
        transcript.contains("second line"),
        "Ctrl-J submitted instead of inserting a newline:\n{transcript}"
    );
    // Below 110 columns the column is collapsed; `/sidebar` is the drawer.
    let before = plain(&seen);
    assert!(
        !before.contains("Changed files"),
        "the column was drawn at 80 columns"
    );
    send(&mut writer, b"/sidebar\r");
    wait_for(&rx, &mut seen, "Session", Duration::from_secs(20));
    // A 24-row drawer cannot show every section at once, so it scrolls on
    // its own: the sections below the fold are reachable rather than cut.
    send(&mut writer, b"jjjjjjjj");
    wait_for(&rx, &mut seen, "Changed files", Duration::from_secs(20));
    // Unknown usage is drawn as unavailable, never as a zero, and the
    // last request's prompt tokens have their own label.
    let drawer = plain(&seen);
    assert!(drawer.contains("in unavailable"), "{drawer}");
    assert!(
        !drawer.contains("in 0"),
        "an unknown count was drawn as zero"
    );
    escape(&mut writer);

    // Every configured MCP entry is accounted for, including one that
    // cannot run.
    send(&mut writer, b"/mcp\r");
    wait_for(&rx, &mut seen, "broken-entry", Duration::from_secs(20));
    wait_for(&rx, &mut seen, "invalid", Duration::from_secs(20));
    escape(&mut writer);

    send(&mut writer, b"/help\r");
    wait_for(&rx, &mut seen, "Commands", Duration::from_secs(20));
    // A 24-row terminal cannot show the whole help, so it scrolls: the
    // capability limitations are reachable rather than cut off.
    send(&mut writer, b"jjjjjjjjjjjjjjjjjjjjjjjj");
    wait_for(&rx, &mut seen, "Limitations", Duration::from_secs(20));
    escape(&mut writer);

    quit(&mut child, &rx, &mut seen, &mut writer);
    assert!(!seen.contains("pty-key"), "the key must never be drawn");
}

/// Quitting the full-screen mode kills an MCP server that never answers.
///
/// The launch path and the interrupt path are already covered against
/// `gritt mcp trust` (`e2e.rs`). This is the third exit the chain added: a
/// server trusted for this workspace is started when the full-screen mode
/// opens on the native path, and Ctrl-Q has to take it with it. The server
/// never speaks MCP, so it is still inside initialization when the quit
/// arrives, and it sits in its own process group, so only a group kill ends
/// it (TKT-0020).
#[cfg(unix)]
#[test]
fn quitting_the_full_screen_mode_leaves_no_mcp_server_running() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    let database = dir.path().join("gritt.db");
    let pidfile = dir.path().join("server.pid");
    std::fs::write(
        dir.path().join(".mcp.json"),
        serde_json::json!({"mcpServers": {"never-answers": {
            "command": "sh",
            "args": ["-c", format!("echo $$ > {}; sleep 300", pidfile.display())],
        }}})
        .to_string(),
    )
    .unwrap();

    // Approve the definition for this workspace. The command launches the
    // server, waits out its initialization deadline, records the decision,
    // and cleans up; what matters here is the record it leaves behind.
    let trust = std::process::Command::new(env!("CARGO_BIN_EXE_gritt"))
        .args([
            "--workspace",
            &dir.path().to_string_lossy(),
            "--database",
            &database.to_string_lossy(),
            "mcp",
            "trust",
            "never-answers",
        ])
        .output()
        .unwrap();
    assert!(
        trust.status.success(),
        "trust failed: {}",
        String::from_utf8_lossy(&trust.stderr)
    );
    let _ = std::fs::remove_file(&pidfile);

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
    command.args([
        "--workspace",
        &dir.path().to_string_lossy(),
        "--database",
        &database.to_string_lossy(),
        "tui",
        "--no-models",
        "--session",
        "mcp-quit",
    ]);
    command.env("GRITT_E2E_KEY", "pty-key");
    command.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
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
    let mut writer = pair.master.take_writer().unwrap();
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(30));

    // The server the full-screen mode started, by its own pid.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !pidfile.exists() {
        assert!(
            Instant::now() < deadline,
            "the full-screen mode never started the trusted server"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let server: u32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // Ctrl-Q, while the server is still inside its handshake.
    writer.write_all(&[0x11]).unwrap();
    writer.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the process did not exit on Ctrl-Q"
        );
        thread::sleep(Duration::from_millis(50));
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let alive = std::process::Command::new("kill")
            .args(["-0", &server.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !alive {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "an MCP server outlived the full-screen mode"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(!seen.contains("pty-key"), "the key must never be drawn");
}

/// A model list served after a delay, on `GET /v1/models`.
///
/// The delay is the point: it forces the question of whether the eager path
/// resolves before or after the catalog arrives.
fn serve_models(delay: Duration, body: String) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line == "\r\n" {
                    break;
                }
            }
            thread::sleep(delay);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    port
}

/// A new named session must resolve its model against the model list, not
/// against an empty catalog that has not loaded yet.
///
/// The eager path persists what it resolves. TKT-0020 moved catalog warming
/// off the launch path for responsiveness, and for a moment that made this
/// race: `--model` could resolve before the list arrived, so a retired id
/// would be stored unremapped and the session would be pinned to a model the
/// provider no longer serves. The warm is awaited on the eager path for
/// exactly this reason, and the delay here is what proves it.
#[cfg(unix)]
#[test]
fn a_new_named_session_resolves_its_model_against_the_loaded_catalog() {
    let body = serde_json::json!({
        "data": [
            {"id": "retired-model", "deprecated": true, "replacement": "current-model"},
            {"id": "current-model"},
        ]
    })
    .to_string();
    let port = serve_models(Duration::from_millis(2_500), body);

    // The model cache lives under the user cache directory and is keyed by
    // profile name, so a profile called `local` would be served from
    // whatever an earlier run left there and this race would never appear.
    // A name unique to this run guarantees a cold catalog.
    let profile = format!(
        "cat{}{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        format!(
            "default_profile = \"{profile}\"\ndefault_model = \"fallback-model\"\n\
             [profiles.{profile}]\nname = \"{profile}\"\nprotocol = \"chat_completions\"\n\
             base_url = \"http://127.0.0.1:{port}/v1\"\n\
             [profiles.{profile}.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/x\"\n\
             env_var_name = \"GRITT_E2E_KEY\"\n"
        ),
    )
    .unwrap();

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
    command.args([
        "--workspace",
        &dir.path().to_string_lossy(),
        "--database",
        &dir.path().join("gritt.db").to_string_lossy(),
        "tui",
        "--session",
        "fresh",
        "--model",
        "retired-model",
    ]);
    command.env("GRITT_E2E_KEY", "pty-key");
    command.env("TERM", "xterm-256color");
    command.env("NO_COLOR", "1");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
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
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(30));
    wait_for(&rx, &mut seen, "Ask Gritt", Duration::from_secs(30));
    let mut writer = pair.master.take_writer().unwrap();
    writer.write_all(&[0x11]).unwrap();
    writer.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the process did not exit");
        thread::sleep(Duration::from_millis(50));
    }
    assert!(!seen.contains("pty-key"), "the key must never be drawn");

    // What the session was actually pinned to, read back from the store.
    // The screen is not evidence here: the status line can show a drafted
    // or configured model, while the row is what a resume will use.
    let listed = std::process::Command::new(env!("CARGO_BIN_EXE_gritt"))
        .args([
            "--workspace",
            &dir.path().to_string_lossy(),
            "--database",
            &dir.path().join("gritt.db").to_string_lossy(),
            "session",
            "list",
        ])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    assert!(
        listed.contains("fresh"),
        "the eager session was never created: {listed}"
    );
    assert!(
        listed.contains(&format!("{profile}/current-model")),
        "the session was not pinned to the provider's replacement: {listed}"
    );
    assert!(
        !listed.contains("retired-model"),
        "the session was pinned to the retired id: {listed}"
    );
}

/// A `.mcp.json` that cannot be parsed must say so.
///
/// The failure happens before any entry is published, so the lifecycle
/// subscription delivers an empty list. Reporting nothing would show "no MCP
/// servers configured", which is what a workspace with no file looks like and
/// is the opposite of the truth (TKT-0020).
#[cfg(unix)]
#[test]
fn a_malformed_mcp_configuration_is_reported_in_the_interface() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path());
    std::fs::write(dir.path().join(".mcp.json"), "{ this is not json").unwrap();

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_gritt"));
    command.args([
        "--workspace",
        &dir.path().to_string_lossy(),
        "--database",
        &dir.path().join("gritt.db").to_string_lossy(),
        "tui",
        "--no-models",
        "--session",
        "badmcp",
    ]);
    command.env("GRITT_E2E_KEY", "pty-key");
    command.env("TERM", "xterm-256color");
    command.env("NO_COLOR", "1");
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
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
    let mut seen = String::new();
    wait_for(&rx, &mut seen, ALT_SCREEN_ON, Duration::from_secs(30));
    wait_for(&rx, &mut seen, "MCP configuration", Duration::from_secs(30));
    let mut writer = pair.master.take_writer().unwrap();
    writer.write_all(&[0x11]).unwrap();
    writer.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "the process did not exit");
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !plain(&seen).contains("no MCP servers configured"),
        "a parse failure was shown as an empty configuration"
    );
}
