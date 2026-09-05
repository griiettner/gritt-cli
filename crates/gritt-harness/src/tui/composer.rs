//! The prompt editor: a multiline text buffer with a byte cursor, an
//! optional selection anchor, word navigation, and Unicode-aware display
//! width. It is plain data, so every editing rule is tested without a
//! terminal.
//!
//! The cursor is a byte offset that always sits on a `char` boundary.
//! Widths come from `ratatui`'s own text measurement, so a wide CJK glyph
//! or an emoji occupies the same number of cells here as when drawn.

use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;

/// Display width of a string in terminal cells.
pub fn display_width(text: &str) -> usize {
    Span::raw(text).width()
}

/// The user-perceived characters of `text` with their byte offsets. A
/// combining mark belongs to the character it modifies, so `e` followed by
/// U+0301 is one cluster one cell wide, never two things to move over,
/// delete separately, or break a line between.
pub fn clusters(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.grapheme_indices(true)
}

/// The byte offset of the cluster boundary at or before `at`.
fn cluster_start(text: &str, at: usize) -> usize {
    clusters(text)
        .take_while(|(offset, _)| *offset < at)
        .last()
        .map(|(offset, _)| offset)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composer {
    text: String,
    cursor: usize,
    /// The other end of a selection, when one is being made.
    anchor: Option<usize>,
    /// The display column a vertical move tries to keep.
    goal_column: Option<usize>,
}

impl Composer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let cursor = text.len();
        Self {
            text,
            cursor,
            anchor: None,
            goal_column: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn is_multiline(&self) -> bool {
        self.text.contains('\n')
    }

    pub fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    /// Replaces the whole buffer, for restoring a preserved draft.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.anchor = None;
        self.goal_column = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.anchor = None;
        self.goal_column = None;
    }

    /// Empties the buffer and hands back what it held.
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.anchor = None;
        self.goal_column = None;
        text
    }

    // -- selection ------------------------------------------------------

    pub fn has_selection(&self) -> bool {
        self.selection().is_some()
    }

    /// The selected byte range, ordered, or `None` when empty.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        Some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(start, end)| &self.text[start..end])
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// The whole buffer, or the selection when there is one: what a
    /// keyboard copy takes.
    pub fn copy_target(&self) -> &str {
        self.selected_text().unwrap_or(&self.text)
    }

    fn begin(&mut self, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = None;
        true
    }

    // -- editing --------------------------------------------------------

    pub fn insert_char(&mut self, c: char) {
        self.delete_selection();
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.goal_column = None;
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    /// Inserts pasted text verbatim. Carriage returns are normalized so a
    /// bracketed paste from another terminal does not leave stray `\r`,
    /// and the content stays text: the caller never treats it as a
    /// command, whatever it starts with.
    pub fn insert_paste(&mut self, pasted: &str) {
        self.delete_selection();
        let normalized = pasted.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(self.cursor, &normalized);
        self.cursor += normalized.len();
        self.goal_column = None;
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let previous = self.prev_boundary(self.cursor);
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        self.goal_column = None;
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor >= self.text.len() {
            return;
        }
        let next = self.next_boundary(self.cursor);
        self.text.replace_range(self.cursor..next, "");
        self.goal_column = None;
    }

    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        let start = self.word_start(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.goal_column = None;
    }

    pub fn delete_word_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let end = self.word_end(self.cursor);
        self.text.replace_range(self.cursor..end, "");
        self.goal_column = None;
    }

    /// Deletes from the cursor back to the start of its line.
    pub fn delete_to_line_start(&mut self) {
        if self.delete_selection() {
            return;
        }
        let start = self.line_start(self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.goal_column = None;
    }

    // -- movement -------------------------------------------------------

    pub fn move_left(&mut self, extend: bool) {
        self.begin(extend);
        if self.cursor > 0 {
            self.cursor = self.prev_boundary(self.cursor);
        }
        self.goal_column = None;
    }

    pub fn move_right(&mut self, extend: bool) {
        self.begin(extend);
        if self.cursor < self.text.len() {
            self.cursor = self.next_boundary(self.cursor);
        }
        self.goal_column = None;
    }

    pub fn move_word_left(&mut self, extend: bool) {
        self.begin(extend);
        self.cursor = self.word_start(self.cursor);
        self.goal_column = None;
    }

    pub fn move_word_right(&mut self, extend: bool) {
        self.begin(extend);
        self.cursor = self.word_end(self.cursor);
        self.goal_column = None;
    }

    pub fn move_line_start(&mut self, extend: bool) {
        self.begin(extend);
        self.cursor = self.line_start(self.cursor);
        self.goal_column = None;
    }

    pub fn move_line_end(&mut self, extend: bool) {
        self.begin(extend);
        self.cursor = self.line_end(self.cursor);
        self.goal_column = None;
    }

    pub fn move_text_start(&mut self, extend: bool) {
        self.begin(extend);
        self.cursor = 0;
        self.goal_column = None;
    }

    pub fn move_text_end(&mut self, extend: bool) {
        self.begin(extend);
        self.cursor = self.text.len();
        self.goal_column = None;
    }

    /// Moves up one line, keeping the display column where it can.
    /// Returns false when the cursor is already on the first line, so the
    /// caller can use Up for something else (history, a picker).
    pub fn move_up(&mut self, extend: bool) -> bool {
        let start = self.line_start(self.cursor);
        if start == 0 {
            return false;
        }
        self.begin(extend);
        let column = self.goal_column.unwrap_or_else(|| self.column());
        let previous_start = self.line_start(start - 1);
        self.cursor = self.offset_for_column(previous_start, column);
        self.goal_column = Some(column);
        true
    }

    pub fn move_down(&mut self, extend: bool) -> bool {
        let end = self.line_end(self.cursor);
        if end >= self.text.len() {
            return false;
        }
        self.begin(extend);
        let column = self.goal_column.unwrap_or_else(|| self.column());
        self.cursor = self.offset_for_column(end + 1, column);
        self.goal_column = Some(column);
        true
    }

    /// The cursor's zero-based display column on its line.
    pub fn column(&self) -> usize {
        display_width(&self.text[self.line_start(self.cursor)..self.cursor])
    }

    /// The cursor's zero-based line index.
    pub fn row(&self) -> usize {
        self.text[..self.cursor].matches('\n').count()
    }

    // -- boundaries -----------------------------------------------------

    /// The previous cluster boundary: Left and Backspace step over a whole
    /// character, accents included.
    fn prev_boundary(&self, at: usize) -> usize {
        cluster_start(&self.text[..at], at)
    }

    fn next_boundary(&self, at: usize) -> usize {
        at + clusters(&self.text[at..])
            .next()
            .map(|(_, cluster)| cluster.len())
            .unwrap_or(1)
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..]
            .find('\n')
            .map(|index| at + index)
            .unwrap_or(self.text.len())
    }

    /// The byte offset on the line starting at `start` closest to display
    /// column `column`, never past the end of that line.
    fn offset_for_column(&self, start: usize, column: usize) -> usize {
        let end = self.line_end(start);
        let mut width = 0;
        for (offset, cluster) in clusters(&self.text[start..end]) {
            let next = display_width(cluster);
            if width + next > column {
                return start + offset;
            }
            width += next;
        }
        end
    }

    /// Whether the cluster starting at `offset` is whitespace. A cluster
    /// is whitespace when the character it is built on is.
    fn is_space_at(&self, start: usize, end: usize) -> bool {
        self.text[start..end]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    }

    /// Skips whitespace backwards, then the word before the cursor.
    fn word_start(&self, at: usize) -> usize {
        let mut offset = at;
        while offset > 0 {
            let previous = self.prev_boundary(offset);
            if self.is_space_at(previous, offset) {
                offset = previous;
            } else {
                break;
            }
        }
        while offset > 0 {
            let previous = self.prev_boundary(offset);
            if self.is_space_at(previous, offset) {
                break;
            }
            offset = previous;
        }
        offset
    }

    fn word_end(&self, at: usize) -> usize {
        let mut offset = at;
        while offset < self.text.len() {
            let next = self.next_boundary(offset);
            if self.is_space_at(offset, next) {
                offset = next;
            } else {
                break;
            }
        }
        while offset < self.text.len() {
            let next = self.next_boundary(offset);
            if self.is_space_at(offset, next) {
                break;
            }
            offset = next;
        }
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_movement_and_deletion_step_by_character_not_byte() {
        let mut composer = Composer::new();
        for c in "héllo 世界".chars() {
            composer.insert_char(c);
        }
        assert_eq!(composer.text(), "héllo 世界");
        assert_eq!(composer.cursor(), composer.text().len());
        // Display width counts a CJK glyph as two cells.
        assert_eq!(display_width("世界"), 4);
        assert_eq!(composer.column(), 10);
        composer.move_left(false);
        assert_eq!(composer.cursor(), "héllo 世".len());
        composer.backspace();
        assert_eq!(composer.text(), "héllo 界");
        composer.move_line_start(false);
        composer.move_right(false);
        composer.delete_forward();
        assert_eq!(composer.text(), "hllo 界");
    }

    #[test]
    fn vertical_movement_keeps_the_goal_column_across_a_short_line() {
        let mut composer = Composer::from_text("first line\nab\nthird line");
        composer.move_text_start(false);
        composer.move_line_end(false);
        assert_eq!(composer.column(), 10);
        assert!(composer.move_down(false));
        // The short middle line clamps to its end...
        assert_eq!(composer.column(), 2);
        // ...but the goal column survives to the next long line.
        assert!(composer.move_down(false));
        assert_eq!(composer.column(), 10);
        assert_eq!(composer.row(), 2);
        assert!(!composer.move_down(false));
        composer.move_text_start(false);
        assert!(!composer.move_up(false));
    }

    #[test]
    fn word_navigation_and_word_delete() {
        let mut composer = Composer::from_text("alpha beta  gamma");
        composer.move_word_left(false);
        assert_eq!(&composer.text()[composer.cursor()..], "gamma");
        composer.move_word_left(false);
        assert_eq!(&composer.text()[composer.cursor()..], "beta  gamma");
        composer.move_word_right(false);
        assert_eq!(&composer.text()[composer.cursor()..], "  gamma");
        composer.move_text_end(false);
        composer.delete_word_back();
        assert_eq!(composer.text(), "alpha beta  ");
        composer.delete_word_back();
        assert_eq!(composer.text(), "alpha ");
    }

    #[test]
    fn selection_extends_replaces_and_copies() {
        let mut composer = Composer::from_text("hello world");
        composer.move_text_start(false);
        for _ in 0..5 {
            composer.move_right(true);
        }
        assert_eq!(composer.selected_text(), Some("hello"));
        assert_eq!(composer.copy_target(), "hello");
        composer.insert_char('H');
        assert_eq!(composer.text(), "H world");
        assert!(!composer.has_selection());
        composer.select_all();
        assert_eq!(composer.copy_target(), "H world");
        composer.clear_selection();
        // With nothing selected the copy path takes the whole draft.
        assert_eq!(composer.copy_target(), "H world");
    }

    #[test]
    fn pasted_text_is_inserted_verbatim_with_normalized_line_endings() {
        let mut composer = Composer::from_text("draft ");
        composer.insert_paste("/quit\r\nsecond\rthird");
        assert_eq!(composer.text(), "draft /quit\nsecond\nthird");
        assert!(composer.is_multiline());
        assert_eq!(composer.line_count(), 3);
        // A failed submission puts the draft back untouched.
        let held = composer.take();
        assert!(composer.is_empty());
        composer.set_text(held);
        assert_eq!(composer.text(), "draft /quit\nsecond\nthird");
    }

    #[test]
    fn a_combining_mark_travels_with_the_character_it_modifies() {
        // `e` + U+0301 is one character one cell wide, not two.
        let mut composer = Composer::from_text("cafe\u{0301}");
        assert_eq!(display_width(composer.text()), 4);
        assert_eq!(composer.column(), 4);
        composer.move_left(false);
        assert_eq!(composer.cursor(), "caf".len(), "Left split the accent off");
        assert_eq!(composer.column(), 3);
        composer.move_right(false);
        assert_eq!(composer.cursor(), composer.text().len());
        // Backspace removes the whole character, not just its accent.
        composer.backspace();
        assert_eq!(composer.text(), "caf");
        // And it is one cluster to the wrapper as well.
        let text = "cafe\u{0301}";
        assert_eq!(clusters(text).count(), 4);
        assert_eq!(clusters(text).last().unwrap().1, "e\u{0301}");
    }

    #[test]
    fn line_edits_stay_inside_their_line() {
        let mut composer = Composer::from_text("one\ntwo three");
        composer.move_line_start(false);
        assert_eq!(composer.row(), 1);
        composer.move_line_end(false);
        composer.delete_to_line_start();
        assert_eq!(composer.text(), "one\n");
    }
}
