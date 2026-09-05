//! Draws the [`App`]. Every colour comes from a [`Theme`] token, so
//! `NO_COLOR` and the light palette are one decision, not a decision per
//! widget. Layout follows the terminal size on every frame, so a resize
//! needs no handling beyond a redraw.

use ratatui::layout::{Alignment, Constraint, Direction, Layout as RtLayout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{
    App, Entry, EntryKind, Focus, Layout, Metrics, Notice, Overlay, PickerKind, SetupField,
    SetupForm, View,
};
use super::command;
use super::composer::{clusters, display_width};
use super::picker::{ListStatus, Picker};
use super::sidebar::{SidebarPlacement, SIDEBAR_GUTTER, SIDEBAR_MIN_TERMINAL_WIDTH, SIDEBAR_WIDTH};
use super::theme::Theme;
use crate::modes::print::approval_text;
use crate::setup::ConfigDestination;

/// The composer and the home column never grow past this, so a wide
/// terminal reads as a centred document rather than a stretched line.
const CONTENT_WIDTH: u16 = 90;
/// Below this height the wordmark is dropped for transcript and input.
const WORDMARK_MIN_HEIGHT: u16 = 20;
/// Below this width secondary status collapses.
const COMPACT_WIDTH: u16 = 80;

const WORDMARK: [&str; 5] = [
    "  ▟█████▙  ▗█▙  ▗█▙  █▄▄▄▄  █▄▄▄▄  ▗█▙  ▗█▙ ",
    " ▟█▛▘      ▐█▌  ▐█▌  █▌  █  █▌  █    ██▛▛   ",
    " ██▌  ▟██  ▐█████▛▌  █▛▀▀   █▛▀▀      ██    ",
    " ▜█▙   █▌  ▐█▌  ▐█▌  █▌     █▌        ██    ",
    "  ▜█████▛  ▐█▌  ▐█▌  █▌     █▌        ██    ",
];

fn entry_style(theme: &Theme, kind: EntryKind) -> Style {
    match kind {
        EntryKind::User => theme.accent().add_modifier(Modifier::BOLD),
        EntryKind::Assistant => theme.text(),
        EntryKind::Reasoning => theme.reasoning(),
        EntryKind::Tool => theme.muted(),
        EntryKind::System => theme.dim(),
        EntryKind::Error => theme.error(),
    }
}

/// The role label. Whitespace and this label carry the structure; there
/// is no border around a message.
fn prefix(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::User => "you  ",
        EntryKind::Assistant => "gritt",
        EntryKind::Reasoning => "think",
        EntryKind::Tool => "tool ",
        EntryKind::System => "     ",
        EntryKind::Error => "error",
    }
}

/// Wraps `text` to `width` display cells, breaking between words where
/// one fits and inside a word only when it is longer than the line.
/// Widths are display cells, so a CJK glyph counts as two.
fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let width = width.max(1);
    let mut out = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split_inclusive(' ') {
        let trimmed = word.trim_end_matches(' ');
        let word_width = display_width(trimmed);
        if used > 0 && used + word_width > width {
            out.push(std::mem::take(&mut line).trim_end().to_owned());
            used = 0;
        }
        if word_width > width {
            // A word longer than the line is split, but only between whole
            // characters: an accent never lands on the next row alone.
            for (_, cluster) in clusters(word) {
                let cell = display_width(cluster);
                if used + cell > width && used > 0 {
                    out.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push_str(cluster);
                used += cell;
            }
            continue;
        }
        line.push_str(word);
        used += display_width(word);
    }
    if !line.is_empty() {
        out.push(line.trim_end().to_owned());
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn entry_lines(
    theme: &Theme,
    entry: &Entry,
    body_width: usize,
    details: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let style = entry_style(theme, entry.kind);
    let mut first = true;
    let mut blocks: Vec<String> = entry.text.lines().map(str::to_owned).collect();
    if blocks.is_empty() {
        blocks.push(String::new());
    }
    if details {
        if let Some(detail) = &entry.detail {
            blocks.extend(detail.lines().map(|line| format!("  {line}")));
        }
    }
    for raw in blocks {
        for chunk in wrap(&raw, body_width) {
            let label = if first { prefix(entry.kind) } else { "     " };
            first = false;
            lines.push(Line::from(vec![
                Span::styled(format!("{label} "), style),
                Span::styled(chunk, style),
            ]));
        }
    }
    lines
}

/// Transcript lines wrapped to `width`. Called through the app's layout
/// cache, so a frame that changed nothing reuses the previous wrap.
pub fn transcript_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let body_width = width.saturating_sub(7).max(8);
    let mut lines = Vec::new();
    for entry in &app.entries {
        lines.extend(entry_lines(&app.theme, entry, body_width, app.tool_details));
        // Whitespace, not a border, separates messages.
        lines.push(Line::default());
    }
    lines.pop();
    lines
}

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    // One frame, counted for the deterministic timing harness.
    app.count_frame();
    let area = frame.area();
    // The reducer needs the terminal width to know whether `/sidebar`
    // toggles a column or opens a drawer, and only the frame knows it.
    app.set_metrics(Metrics {
        terminal_width: area.width,
        ..app.metrics()
    });
    frame.render_widget(Block::default().style(app.theme.screen()), area);
    match app.layout() {
        Layout::Home => draw_home(frame, app, area),
        Layout::Conversation => draw_conversation(frame, app, area),
    }
    // Overlay priority, bottom to top: the drawer and pickers, then the
    // approval, which is always modal above everything.
    for overlay in &app.overlays {
        draw_overlay(frame, app, overlay, area);
    }
    if app.pending.is_some() {
        draw_approval(frame, app, area);
    }
}

/// Height the composer needs, bounded so a long draft cannot squeeze the
/// transcript out.
fn composer_height(app: &App, area: Rect) -> u16 {
    let lines = app.composer.line_count().max(1) as u16;
    lines.saturating_add(2).clamp(3, (area.height / 2).max(3))
}

fn centered_column(area: Rect, width: u16) -> Rect {
    let width = width.min(area.width);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y,
        width,
        height: area.height,
    }
}

fn draw_home(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let margin = if area.width >= COMPACT_WIDTH { 1 } else { 0 };
    let area = area.inner(Margin::new(margin, 0));
    let show_wordmark = area.height >= WORDMARK_MIN_HEIGHT;
    let wordmark_height = if show_wordmark {
        WORDMARK.len() as u16 + 1
    } else {
        0
    };
    let composer = composer_height(app, area);
    // Suggestions sit directly under the composer on the home screen too.
    let suggestions = app.suggestions();
    let suggest_height = if suggestions.is_empty() {
        0
    } else {
        (suggestions.len() as u16 + 2).min(area.height / 3)
    };
    let block_height = wordmark_height + 2 + composer + suggest_height + 2;
    let top = area.height.saturating_sub(block_height) / 2;
    let rows = RtLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top),
            Constraint::Length(wordmark_height),
            Constraint::Length(2),
            Constraint::Length(composer),
            Constraint::Length(suggest_height),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(area);
    let column = centered_column(rows[1], CONTENT_WIDTH);
    if show_wordmark {
        let lines: Vec<Line<'static>> = WORDMARK
            .iter()
            .map(|row| Line::from(Span::styled((*row).to_owned(), app.theme.accent())))
            .collect();
        frame.render_widget(
            Paragraph::new(Text::from(lines)).alignment(Alignment::Center),
            column,
        );
    }
    let subtitle = centered_column(rows[2], CONTENT_WIDTH);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            home_subtitle(app),
            if app.is_connected() {
                app.theme.muted()
            } else {
                app.theme.accent()
            },
        )]))
        .alignment(Alignment::Center),
        subtitle,
    );
    let composer_area = centered_column(rows[3], CONTENT_WIDTH);
    draw_composer(frame, app, composer_area);
    if suggest_height > 0 {
        draw_suggestions(frame, app, centered_column(rows[4], CONTENT_WIDTH));
    }
    let status = centered_column(rows[5], CONTENT_WIDTH);
    frame.render_widget(
        Paragraph::new(Text::from(home_status(app, area.width))).alignment(Alignment::Center),
        status,
    );
}

fn home_subtitle(app: &App) -> String {
    match (&app.notice, app.is_connected()) {
        (Some(notice), _) => notice.clone(),
        (None, false) => "Use /connect to get started.".to_owned(),
        (None, true) => "Type a prompt, or / for commands.".to_owned(),
    }
}

/// A path shortened to `width` cells from the left, so the directory a
/// user recognises is the part that survives.
fn short_path(path: &str, width: usize) -> String {
    use crate::tui::composer::display_width;
    if display_width(path) <= width {
        return path.to_owned();
    }
    let mut kept = String::new();
    for part in path.rsplit('/') {
        if part.is_empty() {
            continue;
        }
        let candidate = format!("/{part}{kept}");
        if display_width(&candidate) + 1 > width {
            break;
        }
        kept = candidate;
    }
    if kept.is_empty() {
        return "…".to_owned();
    }
    format!("…{kept}")
}

fn home_status(app: &App, width: u16) -> Vec<Line<'static>> {
    let theme = &app.theme;
    // A narrow home drops the directory and the phase before it drops the
    // fixture label, which is the one thing that must never be cut.
    if width < COMPACT_WIDTH {
        let mut parts = Vec::new();
        if !app.status.session.is_empty() {
            parts.push(Span::styled(app.status.session.clone(), theme.heading()));
            parts.push(Span::styled("  ", theme.dim()));
        }
        parts.push(Span::styled(
            if app.is_connected() {
                app.status.model.clone()
            } else {
                "not connected".to_owned()
            },
            if app.is_connected() {
                theme.text()
            } else {
                theme.dim()
            },
        ));
        parts.push(Span::styled(
            format!("  effort {}", app.status.effort),
            theme.muted(),
        ));
        if let Some(label) = &app.fixture {
            parts.push(Span::styled("  ", theme.dim()));
            parts.push(Span::styled(label.clone(), theme.error()));
        }
        return vec![Line::from(parts)];
    }
    let mut parts = Vec::new();
    // A named session is shown even before its first turn, so `gritt tui
    // --session NAME` says which session the composer will write to.
    if !app.status.session.is_empty() {
        parts.push(Span::styled(app.status.session.clone(), theme.heading()));
        parts.push(Span::styled("  ·  ", theme.dim()));
    }
    parts.extend([
        // A deep workspace path is shortened from its head: the line has
        // to keep the connection, effort, and phase that follow it, and
        // the last components are the ones that identify the directory.
        Span::styled(short_path(&app.status.workspace, 34), theme.muted()),
        Span::styled("  ·  ", theme.dim()),
        Span::styled(
            if app.is_connected() {
                format!("{} · {}", app.status.profile, app.status.model)
            } else {
                "not connected".to_owned()
            },
            if app.is_connected() {
                theme.text()
            } else {
                theme.dim()
            },
        ),
        Span::styled("  ·  ", theme.dim()),
        Span::styled(format!("effort {}", app.status.effort), theme.muted()),
        Span::styled("  ·  ", theme.dim()),
        Span::styled(
            if app.status.phase.is_empty() {
                "plan".to_owned()
            } else {
                app.status.phase.clone()
            },
            theme.muted(),
        ),
    ]);
    if let Some(label) = &app.fixture {
        parts.push(Span::styled("  ·  ", theme.dim()));
        parts.push(Span::styled(label.clone(), theme.error()));
    }
    vec![Line::from(parts)]
}

fn draw_conversation(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let margin = if area.width >= COMPACT_WIDTH { 1 } else { 0 };
    let area = area.inner(Margin::new(margin, 0));
    let composer = composer_height(app, area);
    let suggestions = app.suggestions();
    let suggest_height = if suggestions.is_empty() {
        0
    } else {
        (suggestions.len() as u16 + 2).min(area.height / 3)
    };
    let rows = RtLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(suggest_height),
            Constraint::Length(composer),
            Constraint::Length(1),
        ])
        .split(area);
    draw_header(frame, app, rows[0]);
    let body = rows[1];
    // The sidebar is a column only on a wide terminal; below 110 columns
    // it collapses and `/sidebar` opens the drawer instead.
    let transcript = if app.sidebar_placement(frame.area().width) == SidebarPlacement::Column
        && body.width > SIDEBAR_WIDTH + SIDEBAR_GUTTER + 20
    {
        let columns = RtLayout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(20),
                Constraint::Length(SIDEBAR_GUTTER),
                Constraint::Length(SIDEBAR_WIDTH),
            ])
            .split(body);
        draw_sidebar(frame, app, columns[2], false);
        columns[0]
    } else {
        body
    };
    draw_transcript(frame, app, transcript);
    if suggest_height > 0 {
        draw_suggestions(frame, app, rows[2]);
    }
    draw_composer(frame, app, rows[3]);
    draw_footer(frame, app, rows[4]);
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = &app.theme;
    let mut spans = vec![
        Span::styled(
            if app.status.session.is_empty() {
                "new session".to_owned()
            } else {
                app.status.session.clone()
            },
            theme.heading(),
        ),
        Span::styled("  ", theme.dim()),
        Span::styled(app.status.phase.clone(), theme.muted()),
    ];
    if area.width >= COMPACT_WIDTH {
        spans.push(Span::styled("  ", theme.dim()));
        spans.push(Span::styled(app.status.workspace.clone(), theme.dim()));
    }
    if let Some(label) = &app.fixture {
        spans.push(Span::styled("  ", theme.dim()));
        spans.push(Span::styled(label.clone(), theme.error()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_transcript(frame: &mut Frame<'_>, app: &App, area: Rect) {
    // The app owns the viewport arithmetic so a test can assert on the
    // same visible lines the reader gets.
    let (_, mut visible) =
        app.visible_transcript(area.width as usize, area.height as usize, transcript_lines);
    // A held viewport says so, and offers the way back.
    if app.new_output && !visible.is_empty() {
        visible.pop();
        visible.push(Line::from(Span::styled(
            "new output below — Ctrl-G returns to latest".to_owned(),
            app.theme.accent().add_modifier(Modifier::REVERSED),
        )));
    }
    let mut paragraph = Paragraph::new(Text::from(visible));
    if app.focus == Focus::Transcript {
        paragraph = paragraph.block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(app.theme.accent()),
        );
    }
    frame.render_widget(paragraph, area);
}

fn draw_composer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = &app.theme;
    let focused = app.focus == Focus::Composer && app.overlays.is_empty() && app.pending.is_none();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused {
            theme.accent()
        } else {
            theme.muted()
        })
        .style(theme.raised());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if app.composer.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                if app.is_connected() {
                    "Ask Gritt to do something…"
                } else {
                    "Type here; /connect chooses a provider or an installed agent"
                }
                .to_owned(),
                theme.dim(),
            ))),
            inner,
        );
        if focused {
            frame.set_cursor_position((inner.x, inner.y));
        }
        return;
    }
    // The composer wraps here rather than in `Paragraph`, so the cursor's
    // visual position is computed from exactly the rows that are drawn.
    let width = inner.width.max(1) as usize;
    let rows = composer_rows(app.composer.text(), width);
    let cursor_row = rows
        .iter()
        .rposition(|row| row.start <= app.composer.cursor())
        .unwrap_or(0);
    let height = inner.height.max(1) as usize;
    // Keep the cursor's row on screen; a draft longer than the box scrolls
    // instead of disappearing under it.
    let top = if cursor_row >= height {
        cursor_row + 1 - height
    } else {
        0
    };
    let visible: Vec<Line<'static>> = rows
        .iter()
        .skip(top)
        .take(height)
        .map(|row| Line::from(Span::styled(row.text.clone(), theme.text())))
        .collect();
    frame.render_widget(Paragraph::new(Text::from(visible)), inner);
    if focused {
        let row = &rows[cursor_row];
        let column = display_width(&row.text[..app.composer.cursor() - row.start]);
        let x = inner.x + (column as u16).min(inner.width.saturating_sub(1));
        let y = inner.y + (cursor_row - top) as u16;
        frame.set_cursor_position((x, y));
    }
}

/// One drawn row of the composer: where it starts in the buffer and the
/// text on it.
struct ComposerRow {
    start: usize,
    text: String,
}

/// Splits the composer buffer into the rows it is drawn on. Logical lines
/// break first, then each is hard-wrapped at `width` display cells; an
/// editor's cursor has to land on a predictable cell, so this does not
/// wrap on words the way the transcript does.
///
/// Breaks fall on character boundaries, not scalar boundaries: `e` plus a
/// combining acute is one character one cell wide, so it is never split
/// across two rows and never charged two cells.
fn composer_rows(text: &str, width: usize) -> Vec<ComposerRow> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        let mut chunk = String::new();
        let mut chunk_start = line_start;
        let mut used = 0;
        for (offset, cluster) in clusters(line) {
            let cell = display_width(cluster);
            // `used > 0` keeps a zero-width cluster with what precedes it
            // and stops a wide glyph from looping on an empty row.
            if used + cell > width && used > 0 {
                rows.push(ComposerRow {
                    start: chunk_start,
                    text: std::mem::take(&mut chunk),
                });
                chunk_start = line_start + offset;
                used = 0;
            }
            chunk.push_str(cluster);
            used += cell;
        }
        rows.push(ComposerRow {
            start: chunk_start,
            text: chunk,
        });
        line_start += line.len() + 1;
    }
    rows
}

fn draw_suggestions(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = &app.theme;
    let suggestions = app.suggestions();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.muted())
        .title(Span::styled(" commands ".to_owned(), theme.muted()));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    // The list can be taller than the panel, so it scrolls with the
    // highlight instead of clipping it off the bottom.
    let height = inner.height.max(1) as usize;
    let focus = app
        .suggestion_index
        .min(suggestions.len().saturating_sub(1));
    let top = if focus >= height {
        focus + 1 - height
    } else {
        0
    };
    let lines: Vec<Line<'static>> = suggestions
        .iter()
        .enumerate()
        .skip(top)
        .take(height)
        .map(|(index, spec)| {
            let style = if index == focus {
                theme.selection()
            } else {
                theme.text()
            };
            Line::from(vec![
                Span::styled(format!(" /{:<10}", spec.name), style),
                Span::styled(format!(" {}", spec.summary), theme.muted()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let theme = &app.theme;
    let mut spans = Vec::new();
    if let Some(notice) = &app.notice {
        spans.push(Span::styled(notice.clone(), theme.error()));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    // Asynchronous work is visible and cancellable, never a frozen frame.
    if let Some(loading) = app.loading() {
        spans.push(Span::styled(format!("{loading}… "), theme.accent()));
        spans.push(Span::styled("Esc cancels".to_owned(), theme.dim()));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    spans.push(Span::styled(
        if app.is_connected() {
            format!("{} · {}", app.status.profile, app.status.model)
        } else {
            "not connected".to_owned()
        },
        theme.muted(),
    ));
    spans.push(Span::styled(
        format!("  effort {}", app.status.effort),
        theme.muted(),
    ));
    // Secondary status collapses before the input or transcript does.
    if area.width >= COMPACT_WIDTH {
        let usage = &app.status.usage;
        spans.push(Span::styled(
            format!(
                "  in {} out {}",
                usage
                    .input_tokens
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "—".into()),
                usage
                    .output_tokens
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "—".into())
            ),
            theme.dim(),
        ));
        spans.push(Span::styled(
            format!(
                "  {}",
                if app.running {
                    "running · Esc cancels"
                } else {
                    "Enter sends · Ctrl-J newline · / commands"
                }
            ),
            theme.dim(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_sidebar(frame: &mut Frame<'_>, app: &App, area: Rect, drawer: bool) {
    let theme = &app.theme;
    // The column scrolls on its own too: Tab reaches it and the arrow
    // keys move it without touching the composer or the transcript.
    let scroll = app.sidebar_scroll;
    let _ = drawer;
    let block = Block::default()
        .borders(if drawer { Borders::ALL } else { Borders::LEFT })
        .border_type(BorderType::Rounded)
        .border_style(if app.focus == Focus::Sidebar {
            theme.accent()
        } else {
            theme.muted()
        })
        .style(if drawer {
            theme.raised()
        } else {
            theme.screen()
        });
    let inner = block.inner(area);
    if drawer {
        frame.render_widget(Clear, area);
    }
    frame.render_widget(block, area);
    let lines = app
        .sidebar
        .lines(theme, inner.width.saturating_sub(1) as usize);
    let start = scroll.min(lines.len());
    frame.render_widget(
        Paragraph::new(Text::from(lines[start..].to_vec())),
        inner.inner(Margin::new(1, 0)),
    );
}

fn overlay_area(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn draw_overlay(frame: &mut Frame<'_>, app: &App, overlay: &Overlay, area: Rect) {
    match overlay {
        Overlay::Picker { kind, picker } => draw_picker(frame, app, *kind, picker, area),
        Overlay::Setup(form) => draw_setup(frame, app, form, area),
        Overlay::Notice(notice) => draw_notice(frame, app, notice, area),
        Overlay::Help { scroll } => draw_help(frame, app, *scroll, area),
        Overlay::FileDiff { path, body, scroll } => {
            draw_file_diff(frame, app, path, body, *scroll, area)
        }
        Overlay::Drawer { .. } => {
            // At 110 columns or more the column is the sidebar, so a
            // drawer left open by a resize is not drawn over it.
            if area.width >= SIDEBAR_MIN_TERMINAL_WIDTH {
                return;
            }
            let width = (area.width * 3 / 4).min(SIDEBAR_WIDTH + 8);
            let drawer = Rect {
                x: area.x + area.width.saturating_sub(width),
                y: area.y,
                width,
                height: area.height,
            };
            draw_sidebar(frame, app, drawer, true);
        }
    }
}

fn panel<'a>(theme: &Theme, title: String, hint: &str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.accent())
        .style(theme.raised())
        .title(Span::styled(format!(" {title} "), theme.heading()))
        .title_bottom(Span::styled(format!(" {hint} "), theme.muted()))
}

fn draw_picker(frame: &mut Frame<'_>, app: &App, kind: PickerKind, picker: &Picker, area: Rect) {
    let theme = &app.theme;
    let width = match kind {
        PickerKind::Effort => 60,
        _ => 76,
    };
    let target = overlay_area(area, width, (area.height * 4 / 5).max(8));
    frame.render_widget(Clear, target);
    let block = panel(
        theme,
        picker.title.clone(),
        "type to filter · Enter selects · Esc closes",
    );
    let inner = block.inner(target);
    frame.render_widget(block, target);
    let rows = RtLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ".to_owned(), theme.accent()),
            Span::styled(picker.query.text().to_owned(), theme.text()),
            Span::styled("▏".to_owned(), theme.accent()),
        ])),
        rows[0],
    );
    // Loading and failure are list state, never a blank list.
    let status = match &picker.status {
        ListStatus::Ready => Span::styled(picker.hint.clone(), theme.muted()),
        ListStatus::Loading { what } => Span::styled(
            format!("loading {what}'s models… Esc cancels"),
            theme.accent(),
        ),
        ListStatus::Failed { reason, cached } => Span::styled(
            if *cached {
                format!("refresh failed: {reason}")
            } else {
                format!("unavailable: {reason}")
            },
            theme.error(),
        ),
    };
    frame.render_widget(Paragraph::new(Line::from(status)), rows[1]);

    let visible = picker.visible();
    if visible.is_empty() {
        let message = match &picker.status {
            ListStatus::Loading { .. } => "nothing cached yet",
            ListStatus::Failed { .. } => "no list is available; the state above says why",
            ListStatus::Ready if picker.query.is_empty() => "nothing configured yet",
            ListStatus::Ready => "no match for this search",
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(message.to_owned(), theme.dim()))),
            rows[2],
        );
        return;
    }
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Which lines are a group heading or blank, so the window never ends
    // on a heading with nothing under it.
    let mut headings: Vec<bool> = Vec::new();
    // Where each filtered row's block begins and ends in `lines`, so the
    // viewport can keep a whole row visible, group heading and all.
    let mut extents: Vec<(usize, usize)> = Vec::new();
    let mut group: Option<String> = None;
    let body_width = rows[2].width as usize;
    for (position, index) in visible.iter().enumerate() {
        let block_start = lines.len();
        let row = &picker.rows()[*index];
        if row.group != group {
            group.clone_from(&row.group);
            if let Some(name) = &group {
                if !lines.is_empty() {
                    lines.push(Line::default());
                    headings.push(true);
                }
                lines.push(Line::from(Span::styled(name.clone(), theme.heading())));
                headings.push(true);
            }
        }
        let selected = position == picker.highlight();
        let base = if !row.availability.is_available() {
            theme.dim()
        } else if selected {
            theme.selection()
        } else {
            theme.text()
        };
        let marker = if row.current { "●" } else { " " };
        let label = format!("{marker} {}", row.label);
        let badge = if row.badge.is_empty() {
            String::new()
        } else {
            format!("[{}]", row.badge)
        };
        let pad = body_width
            .saturating_sub(display_width(&label) + display_width(&badge) + 1)
            .max(1);
        lines.push(Line::from(vec![
            Span::styled(label, base),
            Span::styled(" ".repeat(pad), base),
            Span::styled(badge, theme.muted()),
        ]));
        headings.push(false);
        let mut second = Vec::new();
        if !row.detail.is_empty() {
            second.push(Span::styled(format!("    {}", row.detail), theme.muted()));
        }
        if !row.availability.is_available() {
            second.push(Span::styled(
                format!("  — {}", row.availability.reason()),
                theme.error(),
            ));
        }
        if !second.is_empty() {
            lines.push(Line::from(second));
            headings.push(false);
        }
        if !row.note.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("    {}", row.note),
                theme.dim(),
            )));
            headings.push(false);
        }
        extents.push((block_start, lines.len()));
    }
    // Rows are several lines tall and a group heading belongs to the row
    // under it, so the window is computed in lines, not in rows.
    let height = rows[2].height.max(1) as usize;
    let focus = picker.highlight().min(extents.len().saturating_sub(1));
    let (row_start, row_end) = extents[focus];
    let mut top = row_end.saturating_sub(height);
    if top > row_start {
        top = row_start;
    }
    let mut end = (top + height).min(lines.len());
    // A heading is only worth its line when a row follows it on screen.
    while end > top && end <= headings.len() && headings[end - 1] && end - 1 > row_end - 1 {
        end -= 1;
    }
    let window: Vec<Line<'static>> = lines[top..end].to_vec();
    frame.render_widget(Paragraph::new(Text::from(window)), rows[2]);
}

/// The wire protocol, spelled the way the configuration file does.
fn protocol_label(protocol: gritt_core::provider::Protocol) -> &'static str {
    match protocol {
        gritt_core::provider::Protocol::ChatCompletions => "chat_completions",
        gritt_core::provider::Protocol::Responses => "responses",
        gritt_core::provider::Protocol::Messages => "messages",
    }
}

fn draw_setup(frame: &mut Frame<'_>, app: &App, form: &SetupForm, area: Rect) {
    let theme = &app.theme;
    let target = overlay_area(area, 70, 16);
    frame.render_widget(Clear, target);
    let block = panel(
        theme,
        "Provider setup".to_owned(),
        "Tab moves · Ctrl-T protocol · Ctrl-D destination · Esc returns",
    );
    let inner = block.inner(target);
    frame.render_widget(block, target);
    let mut lines = vec![
        Line::from(Span::styled(
            "The key is typed masked, is never echoed, and goes to the keychain only.".to_owned(),
            theme.muted(),
        )),
        Line::default(),
    ];
    for field in SetupField::ORDER {
        let value = match field {
            SetupField::Name => form.name.text().to_owned(),
            SetupField::BaseUrl => form.base_url.text().to_owned(),
            SetupField::EnvVar => form.env_var.text().to_owned(),
            // Only the length leaves the form.
            SetupField::Secret => "•".repeat(form.secret_len()),
        };
        let focused = form.field() == field;
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>14}  ", field.label()),
                if focused {
                    theme.accent()
                } else {
                    theme.muted()
                },
            ),
            Span::styled(
                if value.is_empty() {
                    "—".to_owned()
                } else {
                    value
                },
                if focused {
                    theme.selection()
                } else {
                    theme.text()
                },
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled(format!("{:>14}  ", "protocol"), theme.muted()),
        Span::styled(protocol_label(form.protocol).to_owned(), theme.text()),
    ]));
    lines.push(Line::from(vec![
        Span::styled(format!("{:>14}  ", "saves to"), theme.muted()),
        Span::styled(
            match form.destination {
                ConfigDestination::User => "the user configuration",
                ConfigDestination::Project => "this workspace's config.toml",
            }
            .to_owned(),
            theme.text(),
        ),
    ]));
    if let Some(outcome) = &form.outcome {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(outcome.clone(), theme.error())));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_notice(frame: &mut Frame<'_>, app: &App, notice: &Notice, area: Rect) {
    let theme = &app.theme;
    let target = overlay_area(area, 70, 12);
    frame.render_widget(Clear, target);
    let block = panel(theme, notice.title.clone(), "Enter closes");
    let inner = block.inner(target);
    frame.render_widget(block, target);
    frame.render_widget(
        Paragraph::new(Text::from(vec![Line::from(Span::styled(
            notice.body.clone(),
            if notice.is_error {
                theme.error()
            } else {
                theme.text()
            },
        ))]))
        .wrap(Wrap { trim: false }),
        inner,
    );
}

/// A changed file's diff, read only. The text came from the harness; this
/// draws it and nothing here can write to the workspace.
fn draw_file_diff(
    frame: &mut Frame<'_>,
    app: &App,
    path: &str,
    body: &str,
    scroll: usize,
    area: Rect,
) {
    let theme = &app.theme;
    let target = overlay_area(area, 92, (area.height * 4 / 5).max(8));
    frame.render_widget(Clear, target);
    let block = panel(
        theme,
        format!("{path} · read only"),
        "j/k scrolls · Esc closes",
    );
    let inner = block.inner(target);
    frame.render_widget(block, target);
    let lines: Vec<Line<'static>> = body
        .lines()
        .map(|line| {
            let style = match line.as_bytes().first() {
                Some(b'+') => theme.success(),
                Some(b'-') => theme.error(),
                Some(b'@') => theme.accent(),
                _ => theme.text(),
            };
            Line::from(Span::styled(crate::tui::app::sanitize(line), style))
        })
        .collect();
    let start = scroll.min(lines.len());
    frame.render_widget(Paragraph::new(Text::from(lines[start..].to_vec())), inner);
}

fn draw_help(frame: &mut Frame<'_>, app: &App, scroll: usize, area: Rect) {
    let theme = &app.theme;
    let target = overlay_area(area, 78, (area.height * 4 / 5).max(8));
    frame.render_widget(Clear, target);
    let block = panel(theme, "Help".to_owned(), "j/k scrolls · Esc closes");
    let inner = block.inner(target);
    frame.render_widget(block, target);
    let mut lines = vec![Line::from(Span::styled(
        "Commands".to_owned(),
        theme.heading(),
    ))];
    for spec in command::COMMANDS {
        lines.push(Line::from(vec![
            Span::styled(format!(" /{:<10}", spec.name), theme.accent()),
            Span::styled(format!(" {}", spec.summary), theme.text()),
            Span::styled(
                spec.shortcut
                    .map(|key| format!("  [{key}]"))
                    .unwrap_or_default(),
                theme.muted(),
            ),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Keys".to_owned(), theme.heading())));
    for (key, what) in [
        ("Enter", "send the prompt"),
        ("Ctrl-J / Shift-Enter", "insert a newline"),
        ("Tab", "move focus, or complete a suggestion"),
        ("Esc", "close the top overlay, then cancel a running turn"),
        ("Ctrl-P", "the command palette"),
        ("Ctrl-G", "return to the latest output"),
        ("Ctrl-Y", "copy the draft or transcript to the Gritt buffer"),
        (
            "Ctrl-A / Ctrl-W / Ctrl-U",
            "select all, delete word, delete to line start",
        ),
        ("Ctrl-Q", "quit"),
    ] {
        lines.push(Line::from(vec![
            Span::styled(format!(" {key:<26}"), theme.accent()),
            Span::styled(what.to_owned(), theme.text()),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "Limitations".to_owned(),
        theme.heading(),
    )));
    for limitation in [
        "A session is pinned to its provider and model; /new changes them.",
        "An installed agent manages its own model, effort, and permissions.",
        "Cost is an estimate from listed prices, never a billed amount.",
        "Ctrl-Y copies inside Gritt; it does not write the system clipboard.",
    ] {
        lines.push(Line::from(Span::styled(
            format!(" · {limitation}"),
            theme.muted(),
        )));
    }
    let start = scroll.min(lines.len());
    frame.render_widget(
        Paragraph::new(Text::from(lines[start..].to_vec())).wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_approval(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(pending) = &app.pending else {
        return;
    };
    let theme = &app.theme;
    let diff = app.view == View::Diff;
    let target = overlay_area(
        area,
        (area.width * 4 / 5).max(30),
        if diff {
            (area.height * 9 / 10).max(8)
        } else {
            (area.height / 2).max(8)
        },
    );
    frame.render_widget(Clear, target);
    let mut text = if diff {
        pending
            .preview
            .clone()
            .unwrap_or_else(|| "no diff for this call".into())
    } else {
        approval_text(&pending.request, &pending.decision, None)
    };
    text.push_str("\n[y] approve  [n] deny  [d] toggle diff  [j/k] scroll");
    let title = if pending.decision.destructive {
        " DESTRUCTIVE: approve? "
    } else {
        " approve? "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            title.to_owned(),
            if pending.decision.destructive {
                theme.error()
            } else {
                theme.heading()
            },
        ))
        .border_style(if pending.decision.destructive {
            theme.error()
        } else {
            theme.accent()
        })
        .style(theme.raised());
    let lines: Vec<Line<'static>> = crate::tui::app::sanitize(&text)
        .lines()
        .map(|line| {
            let style = if line.starts_with('+') {
                theme.success()
            } else if line.starts_with('-') {
                theme.error()
            } else {
                theme.text()
            };
            Line::from(Span::styled(line.to_owned(), style))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.diff_scroll as u16, 0)),
        target,
    );
}
