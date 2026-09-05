//! Reducer tests. Every one drives [`App::on_key`] or [`App::on_paste`]
//! the way the runtime does, so what is proved here is what a key does in
//! a real terminal.

use super::*;
use crate::tui::fixture;
use crate::tui::sidebar::SidebarPlacement;

use crate::tui::theme::{Theme, ThemeMode};
use chrono::Utc;
use gritt_core::event::EventSource;
use gritt_core::policy::PolicyOutcome;
use gritt_core::provider::ReasoningEffort;
use gritt_core::tool::{ToolCall, ToolCallId, ToolResult};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn plain() -> App {
    App::new(StatusBar::default(), Theme::new(ThemeMode::NoColor))
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn event(kind: EventKind) -> Event {
    Event {
        session_id: SessionId("s".into()),
        sequence: 0,
        source: EventSource::Native,
        timestamp: Utc::now(),
        kind,
        diagnostic: None,
    }
}

fn approval() -> PendingApproval {
    PendingApproval {
        request: ApprovalRequest {
            id: gritt_core::event::ApprovalId("a".into()),
            tool: "file_write".into(),
            resource: "/ws/a".into(),
            reason: "write".into(),
            call_id: None,
        },
        decision: Decision {
            outcome: PolicyOutcome::Ask,
            reason: "write".into(),
            destructive: false,
            rule: Some(1),
        },
        preview: Some("--- a\n+++ b\n+x\n".into()),
    }
}

#[test]
fn typing_and_submitting_a_prompt() {
    let mut app = plain();
    type_text(&mut app, "hi there");
    app.on_key(ctrl('j'));
    app.on_key(key(KeyCode::Char('!')));
    assert_eq!(app.composer.text(), "hi there\n!");
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.composer.text(), "hi there!");
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("hi there!".into()));
    assert!(app.running);
    assert_eq!(app.entries[0].kind, EntryKind::User);
    assert_eq!(app.layout(), Layout::Conversation);
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert!(app.notice.is_some());
}

#[test]
fn shift_enter_inserts_a_newline_and_ctrl_m_is_not_bound_separately() {
    let mut app = plain();
    type_text(&mut app, "one");
    app.on_key(shift(KeyCode::Enter));
    type_text(&mut app, "two");
    assert_eq!(app.composer.text(), "one\ntwo");
    // Terminals can encode Ctrl-M as Enter; binding it separately would
    // make Enter ambiguous, so Ctrl-M does nothing of its own.
    let before = app.composer.text().to_owned();
    assert_eq!(app.on_key(ctrl('m')), Action::None);
    assert_eq!(app.composer.text(), before);
    assert!(!app.running);
}

#[test]
fn a_slash_command_runs_locally_and_never_becomes_a_prompt() {
    let mut app = plain();
    type_text(&mut app, "/plan");
    // The suggestion list is open, so Enter takes the highlighted row.
    assert!(!app.suggestions().is_empty());
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::SetPhase(Phase::Planning)
    );
    assert!(app.composer.is_empty());
    assert!(
        app.entries.is_empty(),
        "a command must not enter the transcript"
    );
}

#[test]
fn an_unknown_command_shows_a_local_error_and_keeps_the_input() {
    let mut app = plain();
    type_text(&mut app, "/deploy now");
    assert!(app.suggestions().is_empty(), "an argument closes the list");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.composer.text(), "/deploy now");
    assert!(app.notice.as_deref().unwrap().contains("unknown command"));
    assert!(app.entries.is_empty());
}

#[test]
fn a_double_slash_submits_a_literal_line() {
    let mut app = plain();
    type_text(&mut app, "//quit");
    assert!(app.suggestions().is_empty());
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("/quit".into())
    );
    assert_eq!(app.entries[0].text, "/quit");
}

#[test]
fn a_multiline_paste_stays_text_and_cannot_run_a_command() {
    let mut app = plain();
    app.on_paste("/quit\nrm -rf /\n");
    assert!(app.composer.is_multiline());
    assert!(app.suggestions().is_empty());
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(action, Action::Submit("/quit\nrm -rf /\n".into()));
    assert!(!app.quit);
}

#[test]
fn tab_completes_a_suggestion_and_otherwise_moves_focus() {
    let mut app = plain();
    type_text(&mut app, "/mod");
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.composer.text(), "/models");
    assert_eq!(
        app.focus,
        Focus::Composer,
        "Tab completed instead of moving"
    );
    app.composer.clear();
    // A terminal wide enough for the column, so focus can rest on it.
    app.set_metrics(Metrics {
        transcript_lines: 40,
        transcript_height: 10,
        terminal_width: 120,
    });
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Transcript);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Sidebar);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Composer);
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(app.focus, Focus::Sidebar);
}

/// Finding 3 (round 2): Tab must not park the keyboard on a sidebar that
/// is not on screen, or the scroll keys go somewhere invisible.
#[test]
fn focus_skips_the_sidebar_when_its_column_is_not_on_screen() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    // A narrow conversation: the column is collapsed.
    app.set_metrics(Metrics {
        transcript_lines: 60,
        transcript_height: 10,
        terminal_width: 80,
    });
    assert!(!app.sidebar_column_visible());
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Transcript);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(
        app.focus,
        Focus::Composer,
        "Tab stopped on a hidden sidebar"
    );
    app.on_key(key(KeyCode::BackTab));
    assert_eq!(app.focus, Focus::Transcript);

    // The reviewer's sequence: Tab twice then PageUp must move the
    // transcript, not a sidebar offset nobody can see.
    app.focus = Focus::Composer;
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Tab));
    let before = app.top;
    app.on_key(key(KeyCode::PageUp));
    assert_ne!(app.top, before, "PageUp moved nothing");
    assert_eq!(app.sidebar_scroll, 0, "the hidden sidebar ate the scroll");
}

/// Finding 3 (round 2): hiding the column while it holds focus.
#[test]
fn hiding_the_column_moves_focus_off_it() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.set_metrics(Metrics {
        transcript_lines: 60,
        transcript_height: 10,
        terminal_width: 120,
    });
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Sidebar);
    app.dispatch(Command::Sidebar, None);
    assert!(!app.sidebar_enabled);
    assert_eq!(app.focus, Focus::Composer);

    // And the same when a resize takes the column away.
    app.dispatch(Command::Sidebar, None);
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Sidebar);
    app.on_resize(80, 24);
    assert_eq!(app.focus, Focus::Composer);
    let before = app.top;
    app.on_key(key(KeyCode::PageUp));
    assert_ne!(app.top, before);
}

/// Finding 1 (round 2): a drawer opened narrow is not drawn once the
/// column fits, so it must not stay on the stack eating keys.
#[test]
fn a_drawer_left_open_by_a_resize_does_not_swallow_input() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.set_metrics(Metrics {
        transcript_lines: 60,
        transcript_height: 10,
        terminal_width: 80,
    });
    app.dispatch(Command::Sidebar, None);
    assert!(
        matches!(app.top_overlay(), Some(Overlay::Drawer { .. })),
        "a narrow terminal opens the drawer"
    );
    // The terminal grows past the column threshold.
    app.on_resize(120, 40);
    assert!(app.overlays.is_empty(), "the invisible drawer stayed open");
    assert!(app.sidebar_enabled, "the drawer became the column");
    assert_eq!(app.sidebar_placement(120), SidebarPlacement::Column);
    // Typing reaches the composer instead of an overlay nobody can see.
    type_text(&mut app, "hello");
    assert_eq!(app.composer.text(), "hello");
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Submit("hello".into())
    );
}

/// The same defect reached without a resize event, in case one is missed:
/// the reducer reconciles from the geometry the last frame measured.
#[test]
fn a_stale_drawer_is_reconciled_before_the_next_key_is_routed() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.set_metrics(Metrics {
        transcript_lines: 60,
        transcript_height: 10,
        terminal_width: 80,
    });
    app.dispatch(Command::Sidebar, None);
    assert_eq!(app.overlays.len(), 1);
    // A frame is drawn at the new width but no resize event arrives.
    app.set_metrics(Metrics {
        transcript_lines: 60,
        transcript_height: 10,
        terminal_width: 120,
    });
    type_text(&mut app, "x");
    assert!(app.overlays.is_empty());
    assert_eq!(app.composer.text(), "x");
}

#[test]
fn overlay_priority_is_approval_then_picker_then_suggestions() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    type_text(&mut app, "/co");
    assert!(!app.suggestions().is_empty());
    app.dispatch(Command::Connect, None);
    // A picker hides the suggestion list even though the composer still
    // holds a slash word.
    assert!(app.suggestions().is_empty());
    assert_eq!(
        app.top_overlay().unwrap().picker_kind(),
        Some(PickerKind::Connect)
    );
    app.request_approval(approval());
    // The approval takes keys above the picker: `j` scrolls the diff
    // instead of filtering the list.
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.diff_scroll, 1);
    assert_eq!(
        app.on_key(key(KeyCode::Char('y'))),
        Action::Approve(ApprovalDecision::Approved)
    );
    // The picker is still there underneath, with its state intact.
    assert_eq!(
        app.top_overlay().unwrap().picker_kind(),
        Some(PickerKind::Connect)
    );
}

#[test]
fn escape_closes_the_top_overlay_first_and_then_cancels_the_turn() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    // Open both while idle: settings overlays are refused during a turn.
    app.dispatch(Command::Help, None);
    app.dispatch(Command::Connect, None);
    assert_eq!(app.overlays.len(), 2);
    app.running = true;
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.overlays.len(), 1);
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::None);
    assert!(app.overlays.is_empty());
    // Only now does Escape reach the running turn.
    assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Cancel);
}

#[test]
fn a_cancelled_approval_cannot_be_answered_by_a_late_key() {
    let mut app = plain();
    app.running = true;
    app.request_approval(approval());
    // The runtime drops the pending approval and its responder on cancel.
    app.pending = None;
    // A `y` that arrives after that is an ordinary character, not an
    // answer to a request that no longer exists.
    assert_eq!(app.on_key(key(KeyCode::Char('y'))), Action::None);
    assert_eq!(app.composer.text(), "y");
}

#[test]
fn scrolling_up_holds_the_same_lines_while_output_streams_in() {
    let mut app = plain();
    for index in 0..40 {
        app.push(EntryKind::Assistant, format!("line {index}"));
    }
    // A frame has to happen before the reducer knows the geometry.
    let held = |app: &App| {
        app.visible_transcript(40, 6, crate::tui::render::transcript_lines)
            .1
    };
    let text = |lines: &[ratatui::text::Line<'static>]| {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("|")
    };
    let bottom = text(&held(&app));
    assert!(bottom.contains("line 39"), "{bottom}");
    assert!(app.follow);

    app.on_key(key(KeyCode::PageUp));
    assert!(!app.follow);
    let before = text(&held(&app));
    assert!(!before.contains("line 39"), "{before}");

    // Ten more lines arrive. The reader is still looking at the same
    // content, not at a viewport that slid down under them.
    for index in 40..50 {
        app.on_event(&event(EventKind::TextDelta {
            text: format!("streamed {index}\n"),
        }));
    }
    let after = text(&held(&app));
    assert_eq!(after, before, "the held viewport moved while streaming");
    assert!(app.new_output);

    // Return to latest is explicit and lands back on the newest line.
    assert_eq!(app.on_key(ctrl('g')), Action::None);
    assert!(app.follow);
    assert!(!app.new_output);
    let latest = text(&held(&app));
    assert!(latest.contains("streamed 49"), "{latest}");
}

#[test]
fn scrolling_back_to_the_bottom_resumes_following() {
    let mut app = plain();
    for index in 0..40 {
        app.push(EntryKind::Assistant, format!("line {index}"));
    }
    app.visible_transcript(40, 6, crate::tui::render::transcript_lines);
    app.on_key(key(KeyCode::PageUp));
    assert!(!app.follow);
    for _ in 0..3 {
        app.on_key(key(KeyCode::PageDown));
        app.visible_transcript(40, 6, crate::tui::render::transcript_lines);
    }
    assert!(app.follow, "reaching the bottom resumes following");
    assert!(!app.new_output);
}

#[test]
fn unicode_cursor_movement_steps_by_character() {
    let mut app = plain();
    type_text(&mut app, "héllo 世界");
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.composer.text(), "héllo 界");
    // Ctrl-Left skips the trailing space and the word before it.
    app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
    assert_eq!(app.composer.cursor(), 0);
    app.on_key(shift(KeyCode::End));
    assert_eq!(app.composer.selected_text(), Some("héllo 界"));
}

#[test]
fn changing_the_provider_clears_the_model_and_the_catalog() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    assert_eq!(app.draft.model.as_deref(), Some("openai/gpt-5-nano"));
    app.select_profile("anthropic");
    assert_eq!(app.draft.profile.as_deref(), Some("anthropic"));
    assert_eq!(app.draft.model, None, "a model belongs to its profile");
    assert!(app.catalog.models.is_empty());
    assert!(app.notice.as_deref().unwrap().contains("cleared"));
}

#[test]
fn changing_the_model_revalidates_the_effort_against_it() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.session_pinned = false;
    app.draft.effort = Some(ReasoningEffort::High);
    // A model the list reports without reasoning support cannot take an
    // explicit level, so the draft returns to the model default.
    app.select_model("openai/gpt-4o-2024-05-13");
    assert_eq!(app.draft.effort, Some(ReasoningEffort::Auto));
    assert!(app.notice.as_deref().unwrap().contains("model default"));
}

#[test]
fn the_effort_picker_offers_auto_and_explains_every_refusal() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    let picker = app.effort_picker();
    assert_eq!(picker.rows()[0].id, "auto");
    assert!(picker.rows()[0].availability.is_available());
    assert!(picker
        .rows()
        .iter()
        .all(|row| row.availability.is_available()));
    // Anthropic's Messages protocol has no safe mapping, so every
    // explicit level is refused with its reason rather than hidden.
    app.select_profile("anthropic");
    let picker = app.effort_picker();
    let refused: Vec<&str> = picker
        .rows()
        .iter()
        .filter(|row| !row.availability.is_available())
        .map(|row| row.availability.reason())
        .collect();
    assert_eq!(refused.len(), 3);
    assert!(refused[0].contains("Messages"), "{refused:?}");
}

#[test]
fn a_pinned_session_explains_that_a_model_change_needs_a_new_session() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    assert!(app.session_pinned);
    let draft_before = app.draft.clone();
    app.select_model("openai/gpt-5");
    assert_eq!(app.draft, draft_before, "the pinned draft is untouched");
    let Some(Overlay::Notice(notice)) = app.top_overlay() else {
        panic!("expected the new-session explanation");
    };
    assert!(notice.body.contains("/new"));
    assert!(notice.body.contains("draft is kept"));
}

#[test]
fn the_model_picker_offers_setup_for_a_profile_with_no_key_and_returns_to_it() {
    let mut app = fixture::home(Theme::new(ThemeMode::NoColor));
    app.select_profile("openrouter");
    app.dispatch(Command::Models, None);
    let picker = match app.top_overlay() {
        Some(Overlay::Picker { picker, .. }) => picker.clone(),
        other => panic!("expected the model picker, got {other:?}"),
    };
    assert_eq!(picker.rows()[0].id, "__setup__");
    // Type a search, then take the setup row.
    type_text(&mut app, "Set up");
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.top_overlay(), Some(Overlay::Setup(_))));
    assert_eq!(app.overlays.len(), 2, "setup opens above the model picker");
    // Escape returns to the picker with its search preserved.
    app.on_key(key(KeyCode::Esc));
    match app.top_overlay() {
        Some(Overlay::Picker { picker, .. }) => assert_eq!(picker.query.text(), "Set up"),
        other => panic!("expected to return to the model picker, got {other:?}"),
    }
}

#[test]
fn the_setup_form_masks_the_key_and_writes_nothing_in_fixture_mode() {
    let mut app = fixture::home(Theme::new(ThemeMode::NoColor));
    app.overlays
        .push(Overlay::Setup(SetupForm::for_profile("openrouter")));
    // Tab to the key field and type.
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Tab));
    let Some(Overlay::Setup(form)) = app.top_overlay() else {
        unreachable!()
    };
    assert_eq!(form.field(), SetupField::Secret);
    assert!(form.field().is_secret());
    type_text(&mut app, "sk-never-echoed");
    app.on_key(key(KeyCode::Enter));
    let Some(Overlay::Setup(form)) = app.top_overlay() else {
        unreachable!()
    };
    assert_eq!(form.secret_len(), "sk-never-echoed".len());
    let outcome = form.outcome.clone().unwrap();
    assert!(outcome.starts_with("fixture:"));
    assert!(!outcome.contains("sk-never"));
    // The value has no accessor, so it cannot reach the transcript.
    assert!(!format!("{:?}", app.entries).contains("sk-never"));
}

#[test]
fn a_connector_selection_says_the_agent_manages_its_own_model() {
    let mut app = fixture::home(Theme::new(ThemeMode::NoColor));
    let picker = app.connection_picker();
    let agent = picker
        .rows()
        .iter()
        .find(|row| row.id == "agent:codex")
        .unwrap();
    assert_eq!(agent.group.as_deref(), Some("Installed agents"));
    assert!(agent.note.contains("Managed by agent"));
    let missing = picker
        .rows()
        .iter()
        .find(|row| row.id == "agent:cursor-agent")
        .unwrap();
    assert!(!missing.availability.is_available());
    // Choosing an installed agent explains its authority instead of
    // opening the native model picker.
    app.dispatch(Command::Connect, None);
    type_text(&mut app, "codex");
    app.on_key(key(KeyCode::Enter));
    let Some(Overlay::Notice(notice)) = app.top_overlay() else {
        panic!("expected the connector explanation");
    };
    assert!(
        notice.body.contains("Managed by agent") || notice.body.contains("managed by the agent")
    );
}

#[test]
fn the_connection_dialog_reports_credential_and_catalog_state_per_profile() {
    let app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    let picker = app.connection_picker();
    let openai = picker
        .rows()
        .iter()
        .find(|row| row.id == "profile:openai")
        .unwrap();
    assert_eq!(openai.group.as_deref(), Some("AI providers"));
    assert!(openai.note.contains("key available"));
    assert!(openai.note.contains("catalog fresh"));
    let openrouter = picker
        .rows()
        .iter()
        .find(|row| row.id == "profile:openrouter")
        .unwrap();
    assert!(openrouter.note.contains("OPENROUTER_API_KEY"));
}

#[test]
fn a_stale_or_missing_catalog_renders_as_state_not_as_a_blank_list() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.catalog.loading = true;
    assert!(app.model_picker().status.is_loading());
    app.catalog.loading = false;
    app.catalog.state = Some(CatalogState::Stale {
        fetched_at: Utc::now(),
    });
    match app.model_picker().status {
        ListStatus::Failed { cached, .. } => assert!(cached, "the cached list is still usable"),
        other => panic!("expected a failed status, got {other:?}"),
    }
    // A missing catalog keeps the state even with no rows to show.
    app.catalog.models.clear();
    app.catalog.state = Some(CatalogState::Missing {
        reason: "the provider did not answer".into(),
    });
    let picker = app.model_picker();
    assert!(picker.is_empty());
    match picker.status {
        ListStatus::Failed { cached, reason } => {
            assert!(!cached);
            assert!(reason.contains("did not answer"));
        }
        other => panic!("expected a failed status, got {other:?}"),
    }
}

#[test]
fn the_palette_and_a_slash_command_reach_the_same_action() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.on_key(ctrl('p'));
    assert_eq!(
        app.top_overlay().unwrap().picker_kind(),
        Some(PickerKind::Commands)
    );
    type_text(&mut app, "sidebar");
    app.on_key(key(KeyCode::Enter));
    assert!(!app.sidebar_enabled || !app.overlays.is_empty());
    let by_palette = app.sidebar_enabled;
    // The same command typed as a slash word lands in the same place.
    let mut other = fixture::conversation(Theme::new(ThemeMode::NoColor));
    other.dispatch(Command::Sidebar, None);
    assert_eq!(other.sidebar_enabled, by_palette);
}

#[test]
fn the_drawer_restores_focus_and_scroll_when_it_closes() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    // A narrow terminal: `/sidebar` opens the drawer here.
    app.set_metrics(Metrics {
        transcript_lines: 40,
        transcript_height: 10,
        terminal_width: 80,
    });
    app.focus = Focus::Transcript;
    app.scroll_up(6);
    let held = app.top;
    app.dispatch(Command::Sidebar, None);
    assert!(matches!(app.top_overlay(), Some(Overlay::Drawer { .. })));
    // The drawer scrolls on its own without touching the transcript.
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.sidebar_scroll, 1);
    assert_eq!(app.top, held);
    app.focus = Focus::Sidebar;
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlays.is_empty());
    assert_eq!(app.focus, Focus::Transcript, "focus is restored");
    assert_eq!(app.top, held, "the transcript position is restored");
}

/// Finding 6: on a wide terminal `/sidebar` hides and shows the column.
/// It must not replace it with a modal drawer.
#[test]
fn on_a_wide_terminal_sidebar_toggles_the_column_and_never_opens_a_drawer() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.set_metrics(Metrics {
        transcript_lines: 40,
        transcript_height: 10,
        terminal_width: 120,
    });
    assert!(app.sidebar_enabled);
    app.dispatch(Command::Sidebar, None);
    assert!(app.overlays.is_empty(), "a wide terminal gets no drawer");
    assert!(!app.sidebar_enabled);
    assert_eq!(app.sidebar_placement(120), SidebarPlacement::Hidden);
    app.dispatch(Command::Sidebar, None);
    assert!(app.sidebar_enabled);
    assert_eq!(app.sidebar_placement(120), SidebarPlacement::Column);
}

/// Finding 6: the column has its own scroll offset, reached with Tab.
#[test]
fn the_sidebar_column_scrolls_without_moving_the_transcript_or_the_draft() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.set_metrics(Metrics {
        transcript_lines: 40,
        transcript_height: 10,
        terminal_width: 120,
    });
    type_text(&mut app, "a draft");
    app.scroll_up(4);
    let held = app.top;
    app.focus = Focus::Sidebar;
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.sidebar_scroll, 2);
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.sidebar_scroll, 1);
    app.on_key(key(KeyCode::PageDown));
    assert_eq!(app.sidebar_scroll, 11);
    assert_eq!(app.top, held, "the transcript did not move");
    assert_eq!(app.composer.text(), "a draft", "the draft did not change");
}

#[test]
fn details_expands_tool_output_and_new_clears_back_to_home() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    assert!(!app.tool_details);
    app.dispatch(Command::Details, None);
    assert!(app.tool_details);
    assert!(app.entries.iter().any(|entry| entry.detail.is_some()));
    type_text(&mut app, "a draft worth keeping");
    app.dispatch(Command::New, None);
    assert_eq!(app.layout(), Layout::Home);
    assert!(!app.session_pinned);
    assert_eq!(app.composer.text(), "a draft worth keeping");
    assert_eq!(app.sessions.len(), 2, "/new does not delete a session");
}

#[test]
fn a_tool_result_keeps_its_output_and_escape_sequences_are_rendered_not_executed() {
    let mut app = plain();
    app.on_event(&event(EventKind::ToolCall {
        call: ToolCall {
            id: ToolCallId("c".into()),
            name: "shell".into(),
            arguments: serde_json::json!({"command": "ls"}),
        },
    }));
    app.on_event(&event(EventKind::ToolResult {
        result: ToolResult {
            call_id: ToolCallId("c".into()),
            name: "shell".into(),
            output: "\u{1b}[2Jcleared\u{7}".into(),
            is_error: false,
        },
    }));
    let detail = app.entries.last().unwrap().detail.clone().unwrap();
    assert!(!detail.contains('\u{1b}'), "{detail:?}");
    assert!(!detail.contains('\u{7}'), "{detail:?}");
    assert!(detail.contains("cleared"));
}

#[test]
fn streamed_text_accumulates_into_one_entry() {
    let mut app = plain();
    app.on_event(&event(EventKind::TextDelta { text: "Hel".into() }));
    app.on_event(&event(EventKind::TextDelta { text: "lo".into() }));
    assert_eq!(app.entries.len(), 1);
    assert_eq!(app.entries[0].text, "Hello");
}

#[test]
fn the_wrapped_layout_is_cached_until_the_width_or_transcript_changes() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    assert!(!app.layout_cache_hit(80));
    let first = app.transcript_lines(80, crate::tui::render::transcript_lines);
    assert!(app.layout_cache_hit(80));
    let again = app.transcript_lines(80, |_, _| panic!("the cache must be reused"));
    assert_eq!(first.len(), again.len());
    // A different width and a new entry both invalidate it.
    assert!(!app.layout_cache_hit(60));
    app.push(EntryKind::Assistant, "more");
    assert!(!app.layout_cache_hit(80));
}

#[test]
fn resuming_a_session_is_refused_while_a_turn_runs() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.running = true;
    assert_eq!(
        app.dispatch(Command::Sessions, None),
        Action::RefreshSessions
    );
    app.on_key(key(KeyCode::Enter));
    assert!(app.notice.as_deref().unwrap().contains("running turn"));
    app.running = false;
    app.dispatch(Command::Sessions, None);
    assert!(matches!(app.on_key(key(KeyCode::Enter)), Action::Resume(_)));
}

#[test]
fn ctrl_c_cancels_a_running_turn_and_quits_when_idle() {
    let mut app = plain();
    app.running = true;
    assert_eq!(app.on_key(ctrl('c')), Action::Cancel);
    app.running = false;
    assert_eq!(app.on_key(ctrl('c')), Action::Quit);
    assert!(app.quit);
    let mut app = plain();
    assert_eq!(app.dispatch(Command::Quit, None), Action::Quit);
}

#[test]
fn the_keyboard_copy_path_takes_the_selection_or_the_transcript() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    type_text(&mut app, "copy me");
    app.on_key(ctrl('a'));
    app.on_key(ctrl('y'));
    assert_eq!(app.clipboard.as_deref(), Some("copy me"));
    app.focus = Focus::Transcript;
    app.on_key(ctrl('y'));
    assert!(app
        .clipboard
        .as_deref()
        .unwrap()
        .contains("Tidy the public API"));
}

#[test]
fn setting_a_connector_session_marks_the_model_as_managed_by_the_agent() {
    let mut app = plain();
    let now = Utc::now();
    let session = Session {
        id: SessionId("c".into()),
        name: "codex-run".into(),
        kind: gritt_core::session::SessionKind::Connector {
            id: gritt_core::connector::ConnectorId::Codex,
        },
        phase: Phase::Coding,
        workspace: "/ws".into(),
        created_at: now,
        updated_at: now,
        parent_id: None,
    };
    app.set_session(&session);
    assert!(app.sidebar.model.managed_by_agent);
    assert_eq!(app.sidebar.model.model, None);
    assert!(app.status.model.is_empty());
}

#[test]
fn the_mcp_overlay_accounts_for_every_configured_entry() {
    let app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    let picker = app.mcp_picker();
    assert_eq!(picker.rows().len(), app.mcp.len());
    for row in picker.rows() {
        assert!(!row.badge.is_empty(), "every entry has a visible state");
        assert!(!row.note.is_empty(), "every entry explains itself");
    }
    let unsupported = picker
        .rows()
        .iter()
        .find(|row| row.id == "legacy-sse")
        .unwrap();
    assert!(unsupported.note.contains("Streamable HTTP"));
}

/// Finding 1, the reviewer's exact sequence: `/`, Down, Tab, Enter. Tab
/// completes `/models`, which narrows the list to one row; the highlight
/// must come back with it instead of pointing past the end.
#[test]
fn completing_a_suggestion_resets_the_highlight_so_enter_cannot_run_off_the_list() {
    let mut app = plain();
    app.on_key(key(KeyCode::Char('/')));
    assert_eq!(app.suggestions().len(), crate::tui::command::COMMANDS.len());
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.suggestion_index, 1);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.composer.text(), "/models");
    assert_eq!(app.suggestions().len(), 1);
    assert_eq!(app.suggestion_index, 0);
    // This is the keystroke that used to index a one-row list at 1.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.top_overlay().and_then(Overlay::picker_kind),
        Some(PickerKind::Connect),
        "with no provider drafted, /models routes to /connect first"
    );
}

/// The same class of defect from the other direction: a backspace can
/// widen or narrow the list under a highlight set a keystroke ago.
#[test]
fn editing_the_slash_word_never_leaves_the_highlight_out_of_range() {
    let mut app = plain();
    type_text(&mut app, "/s");
    let count = app.suggestions().len();
    assert!(count > 1);
    for _ in 0..count - 1 {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Backspace));
    assert_eq!(app.suggestion_index, 0);
    assert!(app.highlighted_suggestion().is_some());
    // Every reachable highlight resolves to a real row.
    type_text(&mut app, "quit");
    assert_eq!(app.suggestions().len(), 1);
    assert_eq!(app.highlighted_suggestion().unwrap().name, "quit");
    assert_eq!(app.on_key(key(KeyCode::Enter)), Action::Quit);
}

/// Finding 2: the picker opens before the store answers, so the rows have
/// to arrive into the list the user is already looking at.
#[test]
fn sessions_loaded_after_the_picker_opened_appear_in_it() {
    let mut app = plain();
    let loaded = fixture::conversation(Theme::new(ThemeMode::NoColor)).sessions;
    assert_eq!(
        app.dispatch(Command::Sessions, None),
        Action::RefreshSessions
    );
    match app.top_overlay() {
        Some(Overlay::Picker { picker, .. }) => assert!(picker.rows().is_empty()),
        other => panic!("expected the session picker, got {other:?}"),
    }
    // What the runtime does when the store answers.
    app.load_sessions(loaded.clone());
    match app.top_overlay() {
        Some(Overlay::Picker { picker, .. }) => {
            assert_eq!(picker.rows().len(), loaded.len());
            assert_eq!(picker.selected().unwrap().label, "api-cleanup");
        }
        other => panic!("expected the session picker, got {other:?}"),
    }
    // And it resumes without being closed and reopened first.
    assert_eq!(
        app.on_key(key(KeyCode::Enter)),
        Action::Resume(loaded[0].id.clone())
    );
}

#[test]
fn a_late_session_load_keeps_the_query_and_the_highlight() {
    let mut app = plain();
    let loaded = fixture::conversation(Theme::new(ThemeMode::NoColor)).sessions;
    app.dispatch(Command::Sessions, None);
    type_text(&mut app, "docs");
    app.load_sessions(loaded);
    match app.top_overlay() {
        Some(Overlay::Picker { picker, .. }) => {
            assert_eq!(picker.query.text(), "docs");
            assert_eq!(picker.visible().len(), 1);
            assert_eq!(picker.selected().unwrap().label, "docs-pass");
        }
        other => panic!("expected the session picker, got {other:?}"),
    }
}

/// Finding 7: the runtime refuses a phase change during a turn, so the
/// interface must not show one either.
#[test]
fn a_running_turn_refuses_settings_commands_and_shows_no_phase_change() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    assert_eq!(app.status.phase, "coding");
    app.running = true;
    assert_eq!(app.dispatch(Command::Plan, None), Action::None);
    assert_eq!(app.status.phase, "coding", "a refused change showed anyway");
    assert!(app.notice.as_deref().unwrap().contains("a turn is running"));
    for command in [
        Command::Connect,
        Command::Models,
        Command::Effort,
        Command::New,
    ] {
        assert_eq!(app.dispatch(command, None), Action::None);
        assert!(app.overlays.is_empty(), "{command:?} opened during a turn");
    }
    // Reading the interface is still allowed.
    app.dispatch(Command::Help, None);
    assert!(matches!(app.top_overlay(), Some(Overlay::Help { .. })));
    app.overlays.clear();

    // Idle, the command is dispatched and the phase still waits for the
    // runtime to apply it.
    app.running = false;
    assert_eq!(
        app.dispatch(Command::Plan, None),
        Action::SetPhase(Phase::Planning)
    );
    assert_eq!(
        app.status.phase, "coding",
        "the phase moved before the runtime applied it"
    );
    // `set_session` is where the runtime commits it.
    let mut session = app.sessions[0].clone();
    session.phase = Phase::Planning;
    app.set_session(&session);
    assert_eq!(app.status.phase, "planning");
}

#[test]
fn a_pending_approval_refuses_settings_commands_too() {
    let mut app = fixture::conversation(Theme::new(ThemeMode::NoColor));
    app.request_approval(approval());
    assert_eq!(app.dispatch(Command::Effort, None), Action::None);
    assert!(app.notice.as_deref().unwrap().contains("approval"));
    assert!(app.overlays.is_empty());
    assert!(!app.settings_are_editable());
}
