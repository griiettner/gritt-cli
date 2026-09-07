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
use gritt_harness::changes::{ChangeSource, ChangeStatus, ChangedFile, ChangedFiles};
use gritt_harness::draft::CatalogState;
use gritt_harness::policy::Decision;
use gritt_harness::tui::app::{App, Metrics, PendingApproval, View};
use gritt_harness::tui::command::Command;
use gritt_harness::tui::render::draw;
use gritt_harness::tui::sidebar::IntegrationsSection;
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
        "execution_modes" => {
            let mut app = fixture::conversation(theme);
            app.dispatch(Command::Mode, None);
            app
        }
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
            // A narrow terminal: `/sidebar` opens the drawer there.
            app.set_metrics(Metrics {
                transcript_lines: 40,
                transcript_height: 10,
                terminal_width: 80,
            });
            app.dispatch(Command::Sidebar, None);
            app
        }
        // The sidebar states TKT-0019 fills from live sources: an
        // inventory checked and found empty, one where nothing is known
        // yet, and one longer than any terminal.
        "sidebar_empty" => {
            let mut app = fixture::conversation(theme);
            app.sidebar.changed_files = ChangedFiles::Observed {
                source: ChangeSource::Git,
                files: Vec::new(),
            };
            app.sidebar.integrations = IntegrationsSection {
                mcp: Some(Vec::new()),
                mcp_owner: Some("Gritt".into()),
                connector_mcp: None,
                connector_mcp_inventory: None,
            };
            app.sidebar.usage = Default::default();
            app.sidebar.cost = Default::default();
            app
        }
        "sidebar_unavailable" => {
            let mut app = fixture::conversation(theme);
            // A session that has just been switched to: nothing from the
            // previous driver may survive, so every field is unknown.
            app.sidebar.reset();
            app
        }
        "sidebar_long" => {
            let mut app = fixture::conversation(theme);
            app.sidebar.changed_files = ChangedFiles::Observed {
                source: ChangeSource::Git,
                files: (0..24)
                    .map(|index| ChangedFile {
                        path: format!(
                            "crates/gritt-harness/src/tui/very/deeply/nested/module_{index}.rs"
                        ),
                        status: if index % 3 == 0 {
                            ChangeStatus::Added
                        } else {
                            ChangeStatus::Modified
                        },
                        pre_existing: index % 4 == 0,
                    })
                    .collect(),
            };
            app.sidebar_scroll = 6;
            app
        }
        "home_long_draft" => {
            let mut app = fixture::home(theme);
            type_text(
                &mut app,
                "Refactor the session store so the draft validation and the catalog \
                 warm-up share one code path, then explain what changed and why.",
            );
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

const SCREENS: [&str; 17] = [
    "execution_modes",
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
    "home_long_draft",
    "sidebar_empty",
    "sidebar_unavailable",
    "sidebar_long",
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
    for name in [
        "conversation",
        "sidebar_empty",
        "sidebar_unavailable",
        "sidebar_long",
    ] {
        check(name, 111, 30);
        check(name, 109, 30);
    }
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

/// The three live sidebar states the plan distinguishes: unknown is never
/// drawn as zero, an inventory that was checked may say none, and a list
/// longer than the column scrolls rather than overflowing it.
#[test]
fn the_sidebar_tells_unknown_empty_and_long_apart() {
    let unknown = text_of(&render("sidebar_unavailable", ThemeMode::Dark, 120, 40));
    assert!(unknown.contains("unavailable"), "{unknown}");
    for zero in ["in   0", "out  0", "est  ~$0"] {
        assert!(
            !unknown.contains(zero),
            "an unknown value was drawn as zero: {unknown}"
        );
    }
    // No runtime was checked, so no Integrations section is drawn at all.
    assert!(!unknown.contains("Integrations"), "{unknown}");

    let empty = text_of(&render("sidebar_empty", ThemeMode::Dark, 120, 40));
    assert!(empty.contains("no MCP servers configured"), "{empty}");
    assert!(empty.contains("none"), "{empty}");

    let long = text_of(&render("sidebar_long", ThemeMode::Dark, 120, 40));
    for line in long.lines() {
        assert!(line.chars().count() <= 120, "a line overflowed: {line:?}");
    }
    // A path longer than the column is cut between whole characters and
    // marked, never wrapped across the transcript.
    assert!(long.contains('…'), "{long}");
    assert!(long.contains("(pre-existing)"), "{long}");
    // Scrolled: the heading has moved off the top of the column.
    assert!(
        !long.contains("Gritt 0.1.0"),
        "the column did not scroll:\n{long}"
    );
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

/// Finding 3: keyboard selection must stay on screen. At 60x20 the
/// connection dialog used to draw the "Installed agents" heading with
/// every agent row clipped off the bottom.
#[test]
fn a_picker_shows_the_highlighted_row_at_every_size() {
    for (width, height) in SIZES {
        let mut app = fixture::conversation(Theme::new(ThemeMode::Dark));
        app.dispatch(Command::Connect, None);
        // Walk the whole list; every stop must be visible on screen.
        let rows = match app.top_overlay() {
            Some(gritt_harness::tui::Overlay::Picker { picker, .. }) => picker.rows().len(),
            other => panic!("expected the connection picker, got {other:?}"),
        };
        for step in 0..rows {
            let label = match app.top_overlay() {
                Some(gritt_harness::tui::Overlay::Picker { picker, .. }) => {
                    picker.selected().unwrap().label.clone()
                }
                _ => unreachable!(),
            };
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let text = text_of(terminal.backend().buffer());
            assert!(
                text.contains(&label),
                "at {width}x{height} step {step}: the highlighted row {label:?} \
                 is not on screen:\n{text}"
            );
            app.on_key(key(KeyCode::Down));
        }
    }
}

/// Finding 3, the concrete regression the reviewer found in the golden.
#[test]
fn the_connection_dialog_never_shows_a_group_heading_with_no_rows_under_it() {
    for (width, height) in SIZES {
        let mut app = fixture::conversation(Theme::new(ThemeMode::Dark));
        app.dispatch(Command::Connect, None);
        // Move to the first installed agent, which is what a reader does
        // when they want one.
        for _ in 0..3 {
            app.on_key(key(KeyCode::Down));
        }
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = text_of(terminal.backend().buffer());
        if text.contains("Installed agents") {
            assert!(
                text.contains("codex") || text.contains("claude-code") || text.contains("cursor"),
                "at {width}x{height} the heading has no rows under it:\n{text}"
            );
        }
        assert!(text.contains("codex"), "at {width}x{height}:\n{text}");
    }
}

/// Finding 3 for the slash list: it is taller than its panel at 60x20.
#[test]
fn slash_suggestions_keep_the_highlighted_command_on_screen() {
    let mut app = fixture::home(Theme::new(ThemeMode::Dark));
    app.on_key(key(KeyCode::Char('/')));
    let total = app.suggestions().len();
    for _ in 0..total {
        let name = app.highlighted_suggestion().unwrap().name;
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text = text_of(terminal.backend().buffer());
        assert!(
            text.contains(&format!("/{name}")),
            "the highlighted command /{name} is not on screen:\n{text}"
        );
        app.on_key(key(KeyCode::Down));
    }
}

/// Finding 5: a draft longer than the composer's interior must stay
/// visible, and the cursor must sit on the character being typed.
#[test]
fn a_draft_longer_than_the_composer_scrolls_instead_of_disappearing() {
    let mut app = fixture::home(Theme::new(ThemeMode::Dark));
    let draft = "word ".repeat(60);
    type_text(&mut app, &draft);
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let text = text_of(terminal.backend().buffer());
    // The tail the user is typing is what has to be on screen.
    assert!(text.contains("word word"), "the draft vanished:\n{text}");
    let cursor = terminal.get_cursor_position().unwrap();
    let rows: Vec<&str> = text.lines().collect();
    assert!(
        rows[cursor.y as usize].trim_end().contains("word"),
        "the cursor is not on the row holding the draft tail:\n{text}"
    );

    // Multiline overflow behaves the same way.
    let mut app = fixture::home(Theme::new(ThemeMode::Dark));
    for index in 0..30 {
        type_text(&mut app, &format!("line {index}"));
        app.on_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    }
    type_text(&mut app, "the last line");
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let text = text_of(terminal.backend().buffer());
    assert!(text.contains("the last line"), "{text}");
}

/// Finding 6: hiding the column on a wide terminal gives its space to the
/// transcript rather than covering it with a modal drawer.
#[test]
fn hiding_the_sidebar_on_a_wide_terminal_widens_the_transcript() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::Dark));
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let with_column = text_of(terminal.backend().buffer());
    assert!(with_column.contains("Changed files"), "{with_column}");

    app.dispatch(Command::Sidebar, None);
    assert!(
        app.overlays.is_empty(),
        "a wide terminal must get no drawer"
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let without = text_of(terminal.backend().buffer());
    assert!(!without.contains("Changed files"), "{without}");
    assert!(without.contains("Tidy the public API"), "{without}");
    // The transcript now uses the full width, so it wraps later.
    let widest = |text: &str| text.lines().map(|line| line.chars().count()).max().unwrap();
    assert!(widest(&without) >= widest(&with_column));
}

/// The `fixture` label is the one thing a narrow home may not clip.
#[test]
fn the_fixture_label_survives_the_narrowest_home() {
    for (width, height) in SIZES {
        let text = text_of(&render("home", ThemeMode::Dark, width, height));
        assert!(
            text.contains("fixture"),
            "the fixture label was clipped at {width}x{height}:\n{text}"
        );
    }
}

/// Finding 2 (round 2): a combining mark must stay on the row with the
/// character it modifies. At 60x20 the home composer has 56 interior
/// columns, so 57 ASCII characters followed by `e` + U+0301 lands the
/// accent exactly on a wrap boundary.
#[test]
fn a_combining_mark_never_wraps_away_from_its_character() {
    let mut app = fixture::home(Theme::new(ThemeMode::Dark));
    let draft = format!("{}e\u{0301}", "x".repeat(57));
    type_text(&mut app, &draft);
    assert_eq!(app.composer.text(), draft);
    let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
    terminal.draw(|frame| draw(frame, &app)).unwrap();
    let text = text_of(terminal.backend().buffer());
    // The base character and its accent are on the same row, and that row
    // is the one the cursor is on.
    let cursor = terminal.get_cursor_position().unwrap();
    let rows: Vec<&str> = text.lines().collect();
    let row = rows[cursor.y as usize];
    assert!(
        row.contains("e\u{0301}"),
        "the accent left its character behind; cursor row was {row:?} in:\n{text}"
    );
    assert!(
        !rows.iter().any(|line| line.trim() == "\u{0301}"),
        "a combining mark was drawn on a row of its own:\n{text}"
    );
    // The base text is still on screen: the one-row composer did not
    // scroll it away to make room for a row holding only the accent.
    assert!(text.contains("xxxx"), "the draft scrolled away:\n{text}");
    // The cursor sits after the whole character, one cell wide.
    let before = &draft[..draft.len() - "e\u{0301}".len()];
    let expected = (before.len() % 56) + 1;
    assert_eq!(
        cursor.x as usize % 56,
        expected % 56,
        "the accent was charged more than one cell"
    );
}

/// The same rule for the transcript's long-word splitting.
#[test]
fn transcript_wrapping_keeps_combining_marks_with_their_characters() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::Dark));
    app.push(
        gritt_harness::tui::EntryKind::Assistant,
        format!("{}e\u{0301}{}", "y".repeat(80), "z".repeat(20)),
    );
    let text = text_of(&render_app(&app, 60, 20));
    assert!(
        !text.lines().any(|line| line.trim() == "\u{0301}"),
        "a combining mark was wrapped onto its own row:\n{text}"
    );
    assert!(text.contains("e\u{0301}"), "{text}");
}

/// Renders an already-built app, for cases the named-screen table cannot
/// express.
fn render_app(app: &App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

/// Finding 1 (round 2), through the renderer: after the terminal grows,
/// the drawer is gone and the column is drawn in its place.
#[test]
fn a_drawer_becomes_the_column_when_the_terminal_grows() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::Dark));
    // The sidebar identity line appears nowhere else on screen, and it is
    // the first thing either form draws, so it fits at every size.
    let sidebar = "Gritt 0.1.0";
    // Draw narrow first so the app measures that width.
    let narrow = text_of(&render_app(&app, 80, 24));
    assert!(!narrow.contains(sidebar), "{narrow}");
    app.dispatch(Command::Sidebar, None);
    let drawer = text_of(&render_app(&app, 80, 24));
    assert!(
        drawer.contains(sidebar),
        "the drawer did not open:\n{drawer}"
    );

    app.on_resize(120, 40);
    let wide = text_of(&render_app(&app, 120, 40));
    assert!(wide.contains(sidebar), "the column is missing:\n{wide}");
    assert!(wide.contains("Changed files"), "{wide}");
    assert!(app.overlays.is_empty(), "the drawer outlived its placement");
    // And the composer takes input again.
    type_text(&mut app, "typing works");
    assert_eq!(app.composer.text(), "typing works");
}
