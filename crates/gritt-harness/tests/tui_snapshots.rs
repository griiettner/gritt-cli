//! Ratatui `TestBackend` snapshots of the full-screen mode.
//!
//! Each golden file holds one screen at one size: the drawn text once,
//! then the distinct styles each palette used for it. Text is shared
//! because a palette must never change what is written; the style
//! sections are what prove dark, light, and `NO_COLOR` differ, and that
//! the no-colour palette sets no colour at all.
//!
//! Regenerate after an intended change:
//!
//! ```text
//! GRITT_UPDATE_SNAPSHOTS=1 cargo test -p gritt-harness --test tui_snapshots
//! ```

use std::collections::BTreeSet;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gritt_core::event::{ApprovalId, ApprovalRequest};
use gritt_core::policy::PolicyOutcome;
use gritt_harness::draft::CatalogState;
use gritt_harness::policy::Decision;
use gritt_harness::tui::app::{App, PendingApproval, View};
use gritt_harness::tui::command::Command;
use gritt_harness::tui::render::draw;
use gritt_harness::tui::theme::{Theme, ThemeMode};
use gritt_harness::tui::{fixture, PickerKind};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

const THEMES: [ThemeMode; 3] = [ThemeMode::Dark, ThemeMode::Light, ThemeMode::NoColor];
/// The three review sizes: a wide desktop terminal, a default one, and a
/// small split pane.
const SIZES: [(u16, u16); 3] = [(120, 40), (80, 24), (60, 20)];

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

/// Builds one named screen. Every screen is fixture state.
fn build(name: &str, theme: Theme) -> App {
    match name {
        "home" => fixture::home(theme),
        "conversation" => {
            let mut app = fixture::conversation(theme);
            app.tool_details = true;
            app
        }
        "command_search" => {
            let mut app = fixture::conversation(theme);
            app.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
            type_text(&mut app, "se");
            app
        }
        "slash_suggestions" => {
            let mut app = fixture::home(theme);
            type_text(&mut app, "/c");
            app
        }
        "connect" => {
            let mut app = fixture::conversation(theme);
            app.dispatch(Command::Connect, None);
            app
        }
        "models" => {
            let mut app = fixture::conversation(theme);
            app.dispatch(Command::Models, None);
            app
        }
        "models_error" => {
            let mut app = fixture::conversation(theme);
            app.catalog.models.clear();
            app.catalog.state = Some(CatalogState::Missing {
                reason: "the provider did not answer within 30s".into(),
            });
            app.dispatch(Command::Models, None);
            app
        }
        "effort" => {
            let mut app = fixture::conversation(theme);
            // Anthropic's Messages protocol refuses every explicit level,
            // so the picker has to explain each one.
            app.select_profile("anthropic");
            app.dispatch(Command::Effort, None);
            app
        }
        "approval_diff" => {
            let mut app = fixture::conversation(theme);
            app.request_approval(PendingApproval {
                request: ApprovalRequest {
                    id: ApprovalId("a1".into()),
                    tool: "file_write".into(),
                    resource: "/work/gritt/crates/gritt-harness/src/store/mod.rs".into(),
                    reason: "writes inside the workspace ask first".into(),
                    call_id: None,
                },
                decision: Decision {
                    outcome: PolicyOutcome::Ask,
                    reason: "writes inside the workspace ask first".into(),
                    destructive: false,
                    rule: Some(2),
                },
                preview: Some(
                    "--- a/crates/gritt-harness/src/store/mod.rs\n\
                     +++ b/crates/gritt-harness/src/store/mod.rs\n\
                     @@ -1,4 +1,4 @@\n\
                     -pub fn seed() -> Result<()> {\n\
                     +pub(crate) fn seed() -> Result<()> {\n\
                      \u{1b}[31m escape sequences are shown, not run\n"
                        .into(),
                ),
            });
            app.view = View::Diff;
            app
        }
        "mcp" => {
            let mut app = fixture::conversation(theme);
            app.dispatch(Command::Mcp, None);
            app
        }
        "help" => {
            let mut app = fixture::conversation(theme);
            app.dispatch(Command::Help, None);
            app
        }
        "sidebar_drawer" => {
            let mut app = fixture::conversation(theme);
            app.dispatch(Command::Sidebar, None);
            app
        }
        other => panic!("unknown screen {other}"),
    }
}

fn render(name: &str, theme: ThemeMode, width: u16, height: u16) -> Buffer {
    let app = build(name, Theme::new(theme));
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    terminal.backend().buffer().clone()
}

/// The drawn text, one line per row, trailing blanks removed. The cell
/// after a double-width glyph is its continuation and is skipped, so a
/// CJK name reads as it does on screen.
fn text_of(buffer: &Buffer) -> String {
    let area = buffer.area();
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut skip = false;
            for x in 0..area.width {
                if skip {
                    skip = false;
                    continue;
                }
                let symbol = buffer[(x, y)].symbol();
                skip = ratatui::text::Span::raw(symbol).width() > 1;
                line.push_str(symbol);
            }
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a style names a real colour. `Color::Reset` is the terminal's
/// own default and is what an unstyled cell carries.
fn is_colored(color: Option<ratatui::style::Color>) -> bool {
    !matches!(color, None | Some(ratatui::style::Color::Reset))
}

/// Every distinct style the buffer used, sorted, one per line.
fn styles_of(buffer: &Buffer) -> String {
    let area = buffer.area();
    let mut seen = BTreeSet::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buffer[(x, y)];
            let style = cell.style();
            seen.insert(format!(
                "fg={:?} bg={:?} mod={:?}",
                style.fg, style.bg, style.add_modifier
            ));
        }
    }
    seen.into_iter().collect::<Vec<_>>().join("\n")
}

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
}

fn check(name: &str, width: u16, height: u16) {
    let dark = render(name, ThemeMode::Dark, width, height);
    let text = text_of(&dark);
    let mut document = format!("# {name} {width}x{height}\n\n## text\n\n{text}\n");
    for theme in THEMES {
        let buffer = render(name, theme, width, height);
        assert_eq!(
            text_of(&buffer),
            text,
            "{name} {width}x{height}: the {} palette changed the text",
            theme.name()
        );
        document.push_str(&format!(
            "\n## styles {}\n\n{}\n",
            theme.name(),
            styles_of(&buffer)
        ));
    }
    let path = snapshot_dir().join(format!("{name}_{width}x{height}.txt"));
    if std::env::var_os("GRITT_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(snapshot_dir()).unwrap();
        std::fs::write(&path, &document).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing snapshot {}: {error}. Regenerate with \
             GRITT_UPDATE_SNAPSHOTS=1 cargo test -p gritt-harness --test tui_snapshots",
            path.display()
        )
    });
    assert_eq!(
        document,
        expected,
        "{} differs; review the change and regenerate with GRITT_UPDATE_SNAPSHOTS=1",
        path.display()
    );
}

const SCREENS: [&str; 12] = [
    "home",
    "conversation",
    "command_search",
    "slash_suggestions",
    "connect",
    "models",
    "models_error",
    "effort",
    "approval_diff",
    "mcp",
    "help",
    "sidebar_drawer",
];

#[test]
fn every_screen_matches_its_snapshot_at_every_review_size() {
    for name in SCREENS {
        for (width, height) in SIZES {
            check(name, width, height);
        }
    }
}

/// The sidebar column appears at 110 columns and not at 109.
#[test]
fn the_sidebar_boundary_is_snapshotted_on_both_sides_of_110_columns() {
    check("conversation", 111, 30);
    check("conversation", 109, 30);
    let wide = text_of(&render("conversation", ThemeMode::Dark, 111, 30));
    let narrow = text_of(&render("conversation", ThemeMode::Dark, 109, 30));
    assert!(wide.contains("Changed files"), "{wide}");
    assert!(
        !narrow.contains("Changed files"),
        "the column must collapse below 110 columns:\n{narrow}"
    );
    // Collapsing does not cost the transcript or the composer.
    assert!(narrow.contains("api-cleanup"), "{narrow}");
    assert!(narrow.contains("effort"), "{narrow}");
}

#[test]
fn the_no_color_palette_sets_no_colour_anywhere_on_any_screen() {
    for name in SCREENS {
        let buffer = render(name, ThemeMode::NoColor, 120, 40);
        let area = buffer.area();
        for y in 0..area.height {
            for x in 0..area.width {
                let style = buffer[(x, y)].style();
                assert!(
                    !is_colored(style.fg) && !is_colored(style.bg),
                    "{name} drew colour {:?}/{:?} at {x},{y} with NO_COLOR set",
                    style.fg,
                    style.bg
                );
            }
        }
    }
}

#[test]
fn dark_and_light_differ_and_neither_leaves_the_screen_uncoloured() {
    for name in SCREENS {
        let dark = styles_of(&render(name, ThemeMode::Dark, 120, 40));
        let light = styles_of(&render(name, ThemeMode::Light, 120, 40));
        assert_ne!(dark, light, "{name} looks the same in both palettes");
    }
}

#[test]
fn long_model_names_and_unicode_survive_the_narrowest_size() {
    let text = text_of(&render("models", ThemeMode::Dark, 60, 20));
    // The long identifier is truncated to the panel, not wrapped into it.
    for line in text.lines() {
        assert!(
            line.chars().count() <= 60,
            "a line overflowed 60 columns: {line:?}"
        );
    }
    let wide = text_of(&render("models", ThemeMode::Dark, 120, 40));
    assert!(wide.contains("思考"), "{wide}");
    assert!(wide.contains("long identifier"), "{wide}");
}

#[test]
fn an_escape_sequence_in_a_diff_is_drawn_as_text() {
    let text = text_of(&render("approval_diff", ThemeMode::Dark, 120, 40));
    assert!(!text.contains('\u{1b}'), "an escape reached the buffer");
    assert!(text.contains("escape sequences are shown"), "{text}");
}

#[test]
fn the_home_wordmark_is_dropped_on_a_short_terminal_but_the_composer_is_not() {
    let tall = text_of(&render("home", ThemeMode::Dark, 120, 40));
    let short = text_of(&render("home", ThemeMode::Dark, 120, 12));
    assert!(
        tall.contains('▟'),
        "the wordmark is missing when there is room"
    );
    assert!(
        !short.contains('▟'),
        "the wordmark survived a short terminal"
    );
    for screen in [&tall, &short] {
        assert!(
            screen.contains("Use /connect to get started."),
            "the unconnected prompt is missing:\n{screen}"
        );
        assert!(screen.contains("/connect chooses a provider"), "{screen}");
    }
}

#[test]
fn every_configured_mcp_entry_appears_in_the_overlay() {
    let text = text_of(&render("mcp", ThemeMode::Dark, 120, 40));
    for server in fixture::mcp_servers() {
        assert!(
            text.contains(&server.name),
            "{} is missing:\n{text}",
            server.name
        );
    }
    for word in [
        "ready",
        "starting",
        "awaiting approval",
        "failed",
        "unsupported",
    ] {
        assert!(text.contains(word), "the state {word} is missing:\n{text}");
    }
}

#[test]
fn a_picker_is_reachable_from_both_the_palette_and_a_slash_command() {
    let mut from_palette = fixture::conversation(Theme::new(ThemeMode::Dark));
    from_palette.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    type_text(&mut from_palette, "connect");
    from_palette.on_key(key(KeyCode::Enter));
    assert_eq!(
        from_palette.top_overlay().unwrap().picker_kind(),
        Some(PickerKind::Connect)
    );
    let mut from_slash = fixture::conversation(Theme::new(ThemeMode::Dark));
    type_text(&mut from_slash, "/connect");
    from_slash.on_key(key(KeyCode::Enter));
    assert_eq!(
        from_slash.top_overlay().unwrap().picker_kind(),
        Some(PickerKind::Connect)
    );
}
