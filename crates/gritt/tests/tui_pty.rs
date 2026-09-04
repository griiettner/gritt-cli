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
    std::fs::create_dir_all(dir.join(".gritt")).unwrap();
    std::fs::write(
        dir.join(".gritt/config.toml"),
        "default_profile = \"local\"\ndefault_model = \"openai/gpt-5-nano\"\n\
         [profiles.local]\nname = \"local\"\nprotocol = \"chat_completions\"\n\
         base_url = \"http://127.0.0.1:9/v1\"\n\
         [profiles.local.key]\nkeychain_service_entry = \"gritt-e2e-no-such-entry/local\"\n\
         env_var_name = \"GRITT_E2E_KEY\"\n",
    )
    .unwrap();
}

fn wait_for(rx: &mpsc::Receiver<Vec<u8>>, seen: &mut String, needle: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !seen.contains(needle) {
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
