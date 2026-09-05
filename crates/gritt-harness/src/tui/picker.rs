//! One searchable selection component. The connection dialog, the model
//! picker, the effort picker, the session list, and the command palette
//! are all this type with different rows, so search, highlight movement,
//! grouping, and the empty and loading states behave the same everywhere.
//!
//! A row carries its own labels. The picker never formats provider,
//! connector, or catalog knowledge itself; the caller builds the rows.

use super::composer::Composer;

/// Whether a row can be chosen, and why not when it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAvailability {
    Available,
    /// Shown, ordered with the rest, but Enter does not take it. The
    /// reason is displayed beside the row.
    Unavailable {
        reason: String,
    },
}

impl RowAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, RowAvailability::Available)
    }

    pub fn reason(&self) -> &str {
        match self {
            RowAvailability::Available => "",
            RowAvailability::Unavailable { reason } => reason,
        }
    }
}

/// One selectable line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerRow {
    /// Returned to the caller on selection. Never displayed on its own.
    pub id: String,
    /// The group heading this row sits under, when the list is grouped.
    pub group: Option<String>,
    /// The primary text: a profile name, a model display name, a level.
    pub label: String,
    /// The secondary text: a model id, an endpoint, a version.
    pub detail: String,
    /// A short right-aligned tag: the row's type or its state.
    pub badge: String,
    /// A quiet third line, used for credential or catalog state.
    pub note: String,
    pub availability: RowAvailability,
    /// Marks the row that is currently in effect.
    pub current: bool,
}

impl PickerRow {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            group: None,
            label: label.into(),
            detail: String::new(),
            badge: String::new(),
            note: String::new(),
            availability: RowAvailability::Available,
            current: false,
        }
    }

    pub fn group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = badge.into();
        self
    }

    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }

    pub fn unavailable(mut self, reason: impl Into<String>) -> Self {
        self.availability = RowAvailability::Unavailable {
            reason: reason.into(),
        };
        self
    }

    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let query = query.to_lowercase();
        self.label.to_lowercase().contains(&query)
            || self.detail.to_lowercase().contains(&query)
            || self.badge.to_lowercase().contains(&query)
            || self
                .group
                .as_deref()
                .is_some_and(|group| group.to_lowercase().contains(&query))
    }
}

/// The state of the list itself, distinct from the rows in it. A loading
/// or failed list renders as that state, never as an empty list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ListStatus {
    #[default]
    Ready,
    /// A refresh is in flight. Any rows present are the cached ones.
    Loading { what: String },
    /// The refresh failed. `cached` says whether the rows below it are a
    /// usable fallback or the list is genuinely unusable.
    Failed { reason: String, cached: bool },
}

impl ListStatus {
    pub fn is_loading(&self) -> bool {
        matches!(self, ListStatus::Loading { .. })
    }
}

/// A searchable list with a query line and a highlighted row.
#[derive(Debug, Clone, Default)]
pub struct Picker {
    pub title: String,
    /// Shown under the title when the list is empty or as a hint.
    pub hint: String,
    pub query: Composer,
    rows: Vec<PickerRow>,
    /// Index into the filtered view, not into `rows`.
    highlight: usize,
    /// First visible filtered index, for a list taller than its area.
    pub scroll: usize,
    pub status: ListStatus,
}

impl Picker {
    pub fn new(title: impl Into<String>, rows: Vec<PickerRow>) -> Self {
        let mut picker = Self {
            title: title.into(),
            hint: String::new(),
            query: Composer::new(),
            rows,
            highlight: 0,
            scroll: 0,
            status: ListStatus::Ready,
        };
        picker.highlight_current();
        picker
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn with_status(mut self, status: ListStatus) -> Self {
        self.status = status;
        self
    }

    pub fn rows(&self) -> &[PickerRow] {
        &self.rows
    }

    /// Replaces the rows and starts the highlight over. Used when the
    /// list is a different list, not a fresher copy of the same one.
    pub fn set_rows(&mut self, rows: Vec<PickerRow>) {
        self.rows = rows;
        self.highlight = 0;
        self.scroll = 0;
        self.highlight_current();
    }

    /// Replaces the rows of a list the user is already looking at: the
    /// query stays, and the highlight follows the row it was on by id, or
    /// clamps into the new list when that row is gone.
    pub fn replace_rows(&mut self, rows: Vec<PickerRow>) {
        let held = self.selected().map(|row| row.id.clone());
        self.rows = rows;
        match held
            .and_then(|id| {
                self.visible()
                    .iter()
                    .position(|index| self.rows[*index].id == id)
            })
            .or_else(|| {
                // Nothing was highlighted before, so fall back to the row
                // that is currently in effect.
                self.visible()
                    .iter()
                    .position(|index| self.rows[*index].current)
            }) {
            Some(position) => self.highlight = position,
            None => self.clamp(),
        }
    }

    /// Indices into `rows` that match the query, in row order.
    pub fn visible(&self) -> Vec<usize> {
        let query = self.query.text().trim().to_owned();
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.matches(&query))
            .map(|(index, _)| index)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.visible().is_empty()
    }

    /// The highlighted position in the filtered view.
    pub fn highlight(&self) -> usize {
        self.highlight
    }

    pub fn selected(&self) -> Option<&PickerRow> {
        let visible = self.visible();
        visible.get(self.highlight).map(|index| &self.rows[*index])
    }

    /// The highlighted row when it can actually be chosen.
    pub fn choose(&self) -> Option<&PickerRow> {
        self.selected()
            .filter(|row| row.availability.is_available())
    }

    fn highlight_current(&mut self) {
        if let Some(position) = self
            .visible()
            .iter()
            .position(|index| self.rows[*index].current)
        {
            self.highlight = position;
        }
    }

    fn clamp(&mut self) {
        let count = self.visible().len();
        if count == 0 {
            self.highlight = 0;
            self.scroll = 0;
            return;
        }
        self.highlight = self.highlight.min(count - 1);
        if self.scroll > self.highlight {
            self.scroll = self.highlight;
        }
    }

    pub fn move_down(&mut self) {
        let count = self.visible().len();
        if count == 0 {
            return;
        }
        self.highlight = (self.highlight + 1) % count;
        if self.highlight == 0 {
            self.scroll = 0;
        }
    }

    pub fn move_up(&mut self) {
        let count = self.visible().len();
        if count == 0 {
            return;
        }
        self.highlight = (self.highlight + count - 1) % count;
    }

    pub fn page_down(&mut self, page: usize) {
        let count = self.visible().len();
        if count == 0 {
            return;
        }
        self.highlight = (self.highlight + page.max(1)).min(count - 1);
    }

    pub fn page_up(&mut self, page: usize) {
        self.highlight = self.highlight.saturating_sub(page.max(1));
        self.clamp();
    }

    /// Typing filters. Ordinary letters go to the query, including `j`
    /// and `k`, which never move the highlight in a searchable list.
    pub fn type_char(&mut self, c: char) {
        self.query.insert_char(c);
        self.highlight = 0;
        self.scroll = 0;
    }

    pub fn backspace(&mut self) {
        self.query.backspace();
        self.highlight = 0;
        self.scroll = 0;
    }

    pub fn paste(&mut self, text: &str) {
        // A pasted newline would make the query multiline; the search
        // line takes the first line only and stays text.
        let first = text.lines().next().unwrap_or_default();
        self.query.insert_paste(first);
        self.highlight = 0;
        self.scroll = 0;
    }

    /// Keeps `highlight` inside a window `height` rows tall and returns
    /// the first filtered index to draw.
    pub fn window(&mut self, height: usize) -> usize {
        let height = height.max(1);
        if self.highlight < self.scroll {
            self.scroll = self.highlight;
        } else if self.highlight >= self.scroll + height {
            self.scroll = self.highlight + 1 - height;
        }
        self.scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> Picker {
        Picker::new(
            "models",
            vec![
                PickerRow::new("a", "GPT-5 nano").detail("openai/gpt-5-nano"),
                PickerRow::new("b", "Claude Sonnet")
                    .detail("anthropic/claude-sonnet-4-5")
                    .current(true),
                PickerRow::new("c", "Jamba large")
                    .detail("ai21/jamba-large")
                    .unavailable("not in this profile's catalog"),
            ],
        )
    }

    #[test]
    fn the_current_row_starts_highlighted_and_movement_wraps() {
        let mut picker = picker();
        assert_eq!(picker.selected().unwrap().id, "b");
        picker.move_down();
        assert_eq!(picker.selected().unwrap().id, "c");
        picker.move_down();
        assert_eq!(picker.selected().unwrap().id, "a");
        picker.move_up();
        assert_eq!(picker.selected().unwrap().id, "c");
        // An unavailable row is highlightable but not choosable.
        assert!(picker.choose().is_none());
        picker.move_up();
        assert_eq!(picker.choose().unwrap().id, "b");
    }

    #[test]
    fn typing_j_and_k_filters_instead_of_moving() {
        let mut picker = picker();
        picker.type_char('j');
        assert_eq!(picker.query.text(), "j");
        assert_eq!(picker.visible().len(), 1);
        assert_eq!(picker.selected().unwrap().id, "c");
        picker.type_char('k');
        assert_eq!(picker.query.text(), "jk");
        assert!(picker.is_empty());
        assert!(picker.selected().is_none());
        picker.backspace();
        assert_eq!(picker.visible().len(), 1);
    }

    #[test]
    fn filtering_matches_display_name_and_id() {
        let mut picker = picker();
        picker.paste("gpt-5\nsecond line");
        assert_eq!(picker.query.text(), "gpt-5");
        assert_eq!(picker.visible().len(), 1);
        assert_eq!(picker.selected().unwrap().id, "a");
        picker.query.clear();
        picker.type_char('C');
        assert_eq!(picker.selected().unwrap().id, "b");
    }

    #[test]
    fn a_loading_or_failed_list_keeps_its_cached_rows() {
        let mut picker = picker().with_status(ListStatus::Loading {
            what: "openai".into(),
        });
        assert!(picker.status.is_loading());
        assert_eq!(picker.visible().len(), 3);
        picker.status = ListStatus::Failed {
            reason: "the provider did not answer".into(),
            cached: true,
        };
        assert!(!picker.status.is_loading());
        assert_eq!(picker.visible().len(), 3);
        picker.set_rows(Vec::new());
        assert!(picker.is_empty());
    }

    #[test]
    fn rows_arriving_late_keep_the_query_and_the_highlighted_row() {
        let mut picker = Picker::new("sessions", Vec::new());
        picker.type_char('a');
        picker.type_char('l');
        // The list was empty when it opened; the load lands afterwards.
        picker.replace_rows(vec![
            PickerRow::new("1", "alpha"),
            PickerRow::new("2", "bravo"),
            PickerRow::new("3", "almond"),
        ]);
        assert_eq!(picker.query.text(), "al");
        assert_eq!(picker.visible().len(), 2);
        assert_eq!(picker.selected().unwrap().id, "1");
        picker.move_down();
        assert_eq!(picker.selected().unwrap().id, "3");
        // A second load keeps the highlight on the same row by id.
        picker.replace_rows(vec![
            PickerRow::new("3", "almond"),
            PickerRow::new("1", "alpha"),
            PickerRow::new("4", "apex"),
        ]);
        assert_eq!(picker.selected().unwrap().id, "3");
        // When that row disappears the highlight clamps instead of
        // pointing past the end.
        picker.replace_rows(vec![PickerRow::new("1", "alpha")]);
        assert_eq!(picker.selected().unwrap().id, "1");
        picker.replace_rows(Vec::new());
        assert!(picker.selected().is_none());
        assert_eq!(picker.highlight(), 0);
    }

    #[test]
    fn the_window_follows_the_highlight() {
        let mut picker = Picker::new(
            "many",
            (0..20)
                .map(|index| PickerRow::new(index.to_string(), format!("row {index}")))
                .collect(),
        );
        assert_eq!(picker.window(5), 0);
        picker.page_down(9);
        assert_eq!(picker.highlight(), 9);
        assert_eq!(picker.window(5), 5);
        picker.page_up(9);
        assert_eq!(picker.highlight(), 0);
        assert_eq!(picker.window(5), 0);
    }
}
