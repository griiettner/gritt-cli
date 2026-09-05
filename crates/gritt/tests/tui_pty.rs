//! Drives the full-screen mode in a real pseudo-terminal: it must enter the
//! alternate screen, survive a resize, and restore the terminal on quit.

#![cfg(unix)]

use std::io::{Read, Write};
use std::path::Path;
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
    wait_for(&rx, &mut seen, "command palette", Duration::from_secs(20));
    writer.write_all(&[0x1b]).unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));
    writer.write_all(&[0x13]).unwrap();
    writer.flush().unwrap();
    wait_for(
        &rx,
        &mut seen,
        "sessions (Enter resumes",
        Duration::from_secs(20),
    );
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
