//! Full-screen state and its reducers. Everything here is plain data so
//! the key handling and transcript logic run under `cargo test`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gritt_core::event::{
    ApprovalDecision, ApprovalRequest, Event, EventKind, SessionStatus, Usage,
};
use gritt_core::session::{Phase, Session, SessionId};

use crate::modes::print::describe_call;
use crate::policy::Decision;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Transcript,
    Palette,
    Sessions,
    Diff,
}

/// An `ask` waiting for the user. `preview` is the write diff.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    pub request: ApprovalRequest,
    pub decision: Decision,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StatusBar {
    pub profile: String,
    pub model: String,
    pub session: String,
    pub phase: String,
    pub usage: Usage,
    pub connection: String,
}

/// What the runtime should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Submit(String),
    Cancel,
    Quit,
    SetPhase(Phase),
    Resume(SessionId),
    Approve(ApprovalDecision),
    RefreshSessions,
}

pub const PALETTE: [(&str, &str); 6] = [
    ("plan", "Switch to the planning phase"),
    ("code", "Switch to the coding phase"),
    ("sessions", "List and resume sessions"),
    ("cancel", "Cancel the running turn"),
    ("clear", "Clear the transcript view"),
    ("quit", "Quit Gritt"),
];

#[derive(Debug)]
pub struct App {
    pub entries: Vec<Entry>,
    pub input: String,
    pub cursor: usize,
    pub status: StatusBar,
    pub pending: Option<PendingApproval>,
    pub view: View,
    pub palette_index: usize,
    pub sessions: Vec<Session>,
    pub session_index: usize,
    /// Lines scrolled up from the bottom of the transcript.
    pub scroll: usize,
    pub diff_scroll: usize,
    pub running: bool,
    pub quit: bool,
    pub color: bool,
    pub notice: Option<String>,
    assistant_open: bool,
}

impl App {
    pub fn new(status: StatusBar, color: bool) -> Self {
        Self {
            entries: Vec::new(),
            input: String::new(),
            cursor: 0,
            status,
            pending: None,
            view: View::Transcript,
            palette_index: 0,
            sessions: Vec::new(),
            session_index: 0,
            scroll: 0,
            diff_scroll: 0,
            running: false,
            quit: false,
            color,
            notice: None,
            assistant_open: false,
        }
    }

    pub fn push(&mut self, kind: EntryKind, text: impl Into<String>) {
        self.entries.push(Entry {
            kind,
            text: text.into(),
        });
        self.assistant_open = kind == EntryKind::Assistant;
    }

    /// Replays stored events into the transcript on resume.
    pub fn load_history(&mut self, events: &[Event]) {
        for event in events {
            self.on_event(event);
        }
        self.assistant_open = false;
        self.running = false;
    }

    pub fn on_event(&mut self, event: &Event) {
        match &event.kind {
            EventKind::TextDelta { text } => {
                if self.assistant_open {
                    if let Some(last) = self.entries.last_mut() {
                        last.text.push_str(text);
                    }
                } else {
                    self.push(EntryKind::Assistant, text.clone());
                }
                self.scroll = 0;
            }
            EventKind::ReasoningSummary { text } => self.push(EntryKind::Reasoning, text.clone()),
            EventKind::ToolCall { call } => {
                self.push(
                    EntryKind::Tool,
                    format!("-> {}", describe_call(&call.name, &call.arguments)),
                );
            }
            EventKind::ToolResult { result } => {
                let first = result.output.lines().next().unwrap_or_default();
                self.push(
                    EntryKind::Tool,
                    format!(
                        "<- {} {} {}",
                        result.name,
                        if result.is_error { "error:" } else { "ok" },
                        if result.is_error { first } else { "" }
                    )
                    .trim_end()
                    .to_owned(),
                );
            }
            EventKind::ApprovalRequested { request } => {
                self.push(
                    EntryKind::System,
                    format!(
                        "approval requested: {} on {}",
                        request.tool, request.resource
                    ),
                );
            }
            EventKind::ApprovalDecided { decision, .. } => {
                self.push(EntryKind::System, format!("{decision:?}"));
            }
            EventKind::Usage { usage } => {
                let total = &mut self.status.usage;
                total.input_tokens =
                    Some(total.input_tokens.unwrap_or(0) + usage.input_tokens.unwrap_or(0));
                total.output_tokens =
                    Some(total.output_tokens.unwrap_or(0) + usage.output_tokens.unwrap_or(0));
            }
            EventKind::StatusChanged { status } => {
                self.status.connection = format!("{status:?}");
                if let Some(phase) = event
                    .diagnostic
                    .as_ref()
                    .and_then(|d| d.get("phase"))
                    .and_then(|p| p.as_str())
                {
                    self.status.phase = phase.to_owned();
                }
                if matches!(
                    status,
                    SessionStatus::Finished | SessionStatus::Failed | SessionStatus::Idle
                ) {
                    self.assistant_open = false;
                }
            }
            EventKind::Error { message, .. } => self.push(EntryKind::Error, message.clone()),
            EventKind::Completed { .. } => self.assistant_open = false,
            EventKind::Cancelled => self.push(EntryKind::System, "cancelled"),
        }
        if let Some(warning) = event
            .diagnostic
            .as_ref()
            .and_then(|d| d.get("capability_warning"))
        {
            self.push(
                EntryKind::System,
                format!(
                    "warning: provider did not report support for {}",
                    warning
                        .get("features")
                        .map(|f| f.to_string())
                        .unwrap_or_default()
                ),
            );
        }
    }

    pub fn request_approval(&mut self, pending: PendingApproval) {
        self.diff_scroll = 0;
        self.pending = Some(pending);
    }

    fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let previous = self.input[..self.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(1);
        self.cursor -= previous;
        self.input.remove(self.cursor);
    }

    fn move_left(&mut self) {
        if let Some(c) = self.input[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    fn move_right(&mut self) {
        if let Some(c) = self.input[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    /// The reducer. Overlays take keys first; the transcript view edits
    /// the prompt.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.pending.is_some() {
            return self.approval_key(key);
        }
        match self.view {
            View::Palette => return self.palette_key(key),
            View::Sessions => return self.sessions_key(key),
            View::Diff => {
                self.view = View::Transcript;
                return Action::None;
            }
            View::Transcript => {}
        }
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => {
                if self.running {
                    Action::Cancel
                } else {
                    self.quit = true;
                    Action::Quit
                }
            }
            (KeyCode::Char('q'), true) => {
                self.quit = true;
                Action::Quit
            }
            (KeyCode::Char('p'), true) => {
                self.view = View::Palette;
                self.palette_index = 0;
                Action::None
            }
            (KeyCode::Char('s'), true) => {
                self.view = View::Sessions;
                self.session_index = 0;
                Action::RefreshSessions
            }
            (KeyCode::Char('j'), true) => {
                self.insert_char('\n');
                Action::None
            }
            (KeyCode::Esc, _) => {
                if self.running {
                    Action::Cancel
                } else {
                    self.notice = None;
                    Action::None
                }
            }
            (KeyCode::Enter, _) => {
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.insert_char('\n');
                    return Action::None;
                }
                if self.running {
                    self.notice = Some("a turn is running; Esc cancels it".into());
                    return Action::None;
                }
                let prompt = self.input.trim().to_owned();
                if prompt.is_empty() {
                    return Action::None;
                }
                self.input.clear();
                self.cursor = 0;
                self.push(EntryKind::User, prompt.clone());
                self.running = true;
                self.assistant_open = false;
                self.scroll = 0;
                Action::Submit(prompt)
            }
            (KeyCode::Backspace, _) => {
                self.backspace();
                Action::None
            }
            (KeyCode::Left, _) => {
                self.move_left();
                Action::None
            }
            (KeyCode::Right, _) => {
                self.move_right();
                Action::None
            }
            (KeyCode::Home, _) => {
                self.cursor = 0;
                Action::None
            }
            (KeyCode::End, _) => {
                self.cursor = self.input.len();
                Action::None
            }
            (KeyCode::PageUp, _) => {
                self.scroll = self.scroll.saturating_add(10);
                Action::None
            }
            (KeyCode::PageDown, _) => {
                self.scroll = self.scroll.saturating_sub(10);
                Action::None
            }
            (KeyCode::Char(c), false) => {
                self.insert_char(c);
                Action::None
            }
            (KeyCode::Tab, _) => {
                self.insert_char('\t');
                Action::None
            }
            _ => Action::None,
        }
    }

    fn approval_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.pending = None;
                self.view = View::Transcript;
                Action::Approve(ApprovalDecision::Approved)
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending = None;
                self.view = View::Transcript;
                Action::Approve(ApprovalDecision::Denied)
            }
            KeyCode::Char('d') => {
                self.view = if self.view == View::Diff {
                    View::Transcript
                } else {
                    View::Diff
                };
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.diff_scroll = self.diff_scroll.saturating_add(1);
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.diff_scroll = self.diff_scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::PageDown => {
                self.diff_scroll = self.diff_scroll.saturating_add(10);
                Action::None
            }
            KeyCode::PageUp => {
                self.diff_scroll = self.diff_scroll.saturating_sub(10);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn palette_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.view = View::Transcript;
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.palette_index = (self.palette_index + 1) % PALETTE.len();
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.palette_index = (self.palette_index + PALETTE.len() - 1) % PALETTE.len();
                Action::None
            }
            KeyCode::Enter => {
                self.view = View::Transcript;
                match PALETTE[self.palette_index].0 {
                    "plan" => Action::SetPhase(Phase::Planning),
                    "code" => Action::SetPhase(Phase::Coding),
                    "sessions" => {
                        self.view = View::Sessions;
                        self.session_index = 0;
                        Action::RefreshSessions
                    }
                    "cancel" => Action::Cancel,
                    "clear" => {
                        self.entries.clear();
                        Action::None
                    }
                    _ => {
                        self.quit = true;
                        Action::Quit
                    }
                }
            }
            _ => Action::None,
        }
    }

    fn sessions_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.view = View::Transcript;
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') if !self.sessions.is_empty() => {
                self.session_index = (self.session_index + 1) % self.sessions.len();
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') if !self.sessions.is_empty() => {
                self.session_index =
                    (self.session_index + self.sessions.len() - 1) % self.sessions.len();
                Action::None
            }
            KeyCode::Enter => {
                self.view = View::Transcript;
                match self.sessions.get(self.session_index) {
                    Some(session) if !self.running => Action::Resume(session.id.clone()),
                    Some(_) => {
                        self.notice = Some("finish or cancel the running turn first".into());
                        Action::None
                    }
                    None => Action::None,
                }
            }
            _ => Action::None,
        }
    }

    pub fn set_session(&mut self, session: &Session) {
        self.status.session = session.name.clone();
        self.status.phase = match session.phase {
            Phase::Planning => "planning".into(),
            Phase::Coding => "coding".into(),
        };
        match &session.kind {
            gritt_core::session::SessionKind::Native {
                provider_profile,
                model,
            } => {
                self.status.profile = provider_profile.clone();
                self.status.model = model.clone();
            }
            gritt_core::session::SessionKind::Connector { id } => {
                self.status.profile = id.as_str().to_owned();
                self.status.model.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use gritt_core::event::EventSource;
    use gritt_core::policy::PolicyOutcome;
    use gritt_core::tool::{ToolCall, ToolCallId};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
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

    #[test]
    fn typing_and_submitting_a_prompt() {
        let mut app = App::new(StatusBar::default(), true);
        for c in "hi there".chars() {
            assert_eq!(app.on_key(key(KeyCode::Char(c))), Action::None);
        }
        app.on_key(ctrl('j'));
        app.on_key(key(KeyCode::Char('!')));
        assert_eq!(app.input, "hi there\n!");
        app.on_key(key(KeyCode::Left));
        app.on_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "hi there!");
        let action = app.on_key(key(KeyCode::Enter));
        assert_eq!(action, Action::Submit("hi there!".into()));
        assert!(app.running);
        assert_eq!(app.entries[0].kind, EntryKind::User);
        assert_eq!(app.on_key(key(KeyCode::Enter)), Action::None);
        assert!(app.notice.is_some());
    }

    #[test]
    fn streamed_text_accumulates_into_one_entry() {
        let mut app = App::new(StatusBar::default(), false);
        app.on_event(&event(EventKind::TextDelta { text: "Hel".into() }));
        app.on_event(&event(EventKind::TextDelta { text: "lo".into() }));
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].text, "Hello");
        app.on_event(&event(EventKind::ToolCall {
            call: ToolCall {
                id: ToolCallId("c".into()),
                name: "shell".into(),
                arguments: serde_json::json!({"command": "ls"}),
            },
        }));
        app.on_event(&event(EventKind::TextDelta {
            text: "done".into(),
        }));
        assert_eq!(app.entries.len(), 3);
        assert_eq!(app.entries[1].text, "-> shell ls");
        assert_eq!(app.entries[2].text, "done");
    }

    #[test]
    fn approval_keys_answer_and_diff_toggles() {
        let mut app = App::new(StatusBar::default(), true);
        app.request_approval(PendingApproval {
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
        });
        assert_eq!(app.on_key(key(KeyCode::Char('d'))), Action::None);
        assert_eq!(app.view, View::Diff);
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.diff_scroll, 1);
        assert_eq!(
            app.on_key(key(KeyCode::Char('y'))),
            Action::Approve(ApprovalDecision::Approved)
        );
        assert!(app.pending.is_none());
        assert_eq!(app.view, View::Transcript);
    }

    #[test]
    fn palette_and_quit_and_cancel() {
        let mut app = App::new(StatusBar::default(), true);
        assert_eq!(app.on_key(ctrl('p')), Action::None);
        assert_eq!(app.view, View::Palette);
        app.on_key(key(KeyCode::Down));
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::SetPhase(Phase::Coding)
        );
        app.running = true;
        assert_eq!(app.on_key(key(KeyCode::Esc)), Action::Cancel);
        assert_eq!(app.on_key(ctrl('c')), Action::Cancel);
        app.running = false;
        assert_eq!(app.on_key(ctrl('c')), Action::Quit);
        assert!(app.quit);
    }

    #[test]
    fn sessions_view_resumes_when_idle() {
        let mut app = App::new(StatusBar::default(), true);
        assert_eq!(app.on_key(ctrl('s')), Action::RefreshSessions);
        let now = Utc::now();
        app.sessions = vec![Session {
            id: SessionId("abc".into()),
            name: "work".into(),
            kind: gritt_core::session::SessionKind::Native {
                provider_profile: "p".into(),
                model: "m".into(),
            },
            phase: Phase::Coding,
            workspace: "/ws".into(),
            created_at: now,
            updated_at: now,
            parent_id: None,
        }];
        assert_eq!(
            app.on_key(key(KeyCode::Enter)),
            Action::Resume(SessionId("abc".into()))
        );
        app.set_session(&app.sessions[0].clone());
        assert_eq!(app.status.phase, "coding");
        assert_eq!(app.status.model, "m");
    }
}
