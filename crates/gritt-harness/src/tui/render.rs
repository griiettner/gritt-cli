//! Draws the [`App`]. Colors are dropped when `NO_COLOR` is set; layout
//! follows the terminal size on every frame so resize needs no handling
//! beyond a redraw.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use super::app::{App, EntryKind, View, PALETTE};
use crate::modes::print::approval_text;

fn style(app: &App, color: Color, modifier: Modifier) -> Style {
    let mut style = Style::default().add_modifier(modifier);
    if app.color {
        style = style.fg(color);
    }
    style
}

fn entry_style(app: &App, kind: EntryKind) -> Style {
    match kind {
        EntryKind::User => style(app, Color::Cyan, Modifier::BOLD),
        EntryKind::Assistant => Style::default(),
        EntryKind::Reasoning => style(app, Color::DarkGray, Modifier::ITALIC),
        EntryKind::Tool => style(app, Color::Yellow, Modifier::empty()),
        EntryKind::System => style(app, Color::DarkGray, Modifier::empty()),
        EntryKind::Error => style(app, Color::Red, Modifier::BOLD),
    }
}

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

/// Transcript lines wrapped to `width`, bottom-aligned with scroll.
pub fn transcript_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let body_width = width.saturating_sub(7).max(10);
    for entry in &app.entries {
        let style = entry_style(app, entry.kind);
        let mut first = true;
        for raw in entry.text.lines().chain(if entry.text.is_empty() {
            Some("")
        } else {
            None
        }) {
            for chunk in wrap(raw, body_width) {
                let label = if first { prefix(entry.kind) } else { "     " };
                first = false;
                lines.push(Line::from(vec![
                    Span::styled(format!("{label} "), style),
                    Span::styled(
                        chunk,
                        if entry.kind == EntryKind::Assistant {
                            Style::default()
                        } else {
                            style
                        },
                    ),
                ]));
            }
        }
    }
    lines
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut out = Vec::new();
    let mut current = String::new();
    let mut count = 0;
    for c in text.chars() {
        if count == width {
            out.push(std::mem::take(&mut current));
            count = 0;
        }
        current.push(c);
        count += 1;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let input_height = (app.input.lines().count().max(1) as u16 + 2).min(8);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);
    draw_transcript(frame, app, chunks[0]);
    draw_input(frame, app, chunks[1]);
    draw_status(frame, app, chunks[2]);
    if app.pending.is_some() {
        draw_approval(frame, app, area);
    }
    match app.view {
        View::Palette => draw_palette(frame, app, area),
        View::Sessions => draw_sessions(frame, app, area),
        _ => {}
    }
}

fn draw_transcript(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let lines = transcript_lines(app, area.width.saturating_sub(2) as usize);
    let total = lines.len();
    let end = total.saturating_sub(app.scroll);
    let start = end.saturating_sub(inner_height);
    let visible: Vec<Line<'static>> = lines[start..end].to_vec();
    let title = if app.running {
        " transcript (running, Esc cancels) "
    } else {
        " transcript "
    };
    let paragraph = Paragraph::new(Text::from(visible))
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);
}

fn draw_input(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = match &app.notice {
        Some(notice) => format!(" {notice} "),
        None => " prompt (Enter sends, Ctrl-J newline, Ctrl-P palette, Ctrl-S sessions) ".into(),
    };
    let paragraph = Paragraph::new(app.input.as_str())
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(paragraph, area);
    let before = &app.input[..app.cursor];
    let row = before
        .lines()
        .count()
        .saturating_sub(if before.ends_with('\n') { 0 } else { 1 }) as u16;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count() as u16;
    let x = area.x + 1 + column.min(area.width.saturating_sub(3));
    let y = area.y + 1 + row.min(area.height.saturating_sub(3));
    frame.set_cursor_position((x, y));
}

fn draw_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let usage = &app.status.usage;
    let text = format!(
        " {} | {} | {} [{}] | in {} out {} | {}",
        app.status.profile,
        app.status.model,
        app.status.session,
        app.status.phase,
        usage.input_tokens.unwrap_or(0),
        usage.output_tokens.unwrap_or(0),
        if app.status.connection.is_empty() {
            "idle"
        } else {
            &app.status.connection
        }
    );
    let paragraph = Paragraph::new(text).style(style(app, Color::Black, Modifier::REVERSED));
    frame.render_widget(paragraph, area);
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area)[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical)[1]
}

fn draw_approval(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let Some(pending) = &app.pending else {
        return;
    };
    let popup = centered(area, 80, if app.view == View::Diff { 90 } else { 50 });
    frame.render_widget(Clear, popup);
    let mut text = if app.view == View::Diff {
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
    let block = Block::default().borders(Borders::ALL).title(title).style(
        if pending.decision.destructive {
            style(app, Color::Red, Modifier::BOLD)
        } else {
            style(app, Color::Yellow, Modifier::empty())
        },
    );
    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.diff_scroll as u16, 0));
    frame.render_widget(paragraph, popup);
}

fn draw_palette(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let popup = centered(area, 50, 40);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem<'_>> = PALETTE
        .iter()
        .enumerate()
        .map(|(index, (name, description))| {
            let marker = if index == app.palette_index { ">" } else { " " };
            ListItem::new(format!("{marker} {name:<9} {description}"))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" command palette (Enter runs, Esc closes) "),
    );
    frame.render_widget(list, popup);
}

fn draw_sessions(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let popup = centered(area, 70, 60);
    frame.render_widget(Clear, popup);
    let items: Vec<ListItem<'_>> = if app.sessions.is_empty() {
        vec![ListItem::new("no sessions yet")]
    } else {
        app.sessions
            .iter()
            .enumerate()
            .map(|(index, session)| {
                let marker = if index == app.session_index { ">" } else { " " };
                ListItem::new(format!(
                    "{marker} {:<24} {:?} {}",
                    session.name,
                    session.phase,
                    session.updated_at.format("%Y-%m-%d %H:%M")
                ))
            })
            .collect()
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" sessions (Enter resumes, Esc closes) "),
    );
    frame.render_widget(list, popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::StatusBar;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn wrapping_and_rendering_without_a_terminal() {
        let mut app = App::new(StatusBar::default(), false);
        app.push(EntryKind::User, "a".repeat(30));
        app.push(EntryKind::Assistant, "line one\nline two");
        let lines = transcript_lines(&app, 20);
        assert!(lines.len() >= 5);
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rendered = terminal.backend().buffer().clone();
        let text: String = rendered.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("transcript"));
        assert!(text.contains("prompt"));
    }
}
