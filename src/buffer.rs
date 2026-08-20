use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone)]
struct PrimitiveEdit {
    start: usize,
    deleted: String,
    inserted: String,
}

#[derive(Debug, Clone)]
struct UndoEntry {
    edits: Vec<PrimitiveEdit>,
    cursor_before: usize,
    cursor_after: usize,
}

#[derive(Debug, Clone)]
pub struct TextBuffer {
    rope: Rope,
    cursor: usize,
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    active_group: Option<UndoEntry>,
    revision: u64,
}

impl TextBuffer {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            cursor: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            active_group: None,
            revision: 0,
        }
    }

    pub fn as_string(&self) -> String {
        self.rope.to_string()
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.len_chars());
    }

    pub fn line_of(&self, char_idx: usize) -> usize {
        let idx = char_idx.min(self.len_chars());
        if idx == self.len_chars() && idx > 0 && self.char_at(idx - 1) == Some('\n') {
            self.len_lines().saturating_sub(1)
        } else {
            self.rope.char_to_line(idx)
        }
    }

    pub fn current_line(&self) -> usize {
        self.line_of(self.cursor)
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope
            .line_to_char(line.min(self.len_lines().saturating_sub(1)))
    }

    pub fn line_end(&self, line: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        let start = self.rope.line_to_char(line);
        let slice = self.rope.line(line);
        let mut end = start + slice.len_chars();
        while end > start && matches!(self.char_at(end - 1), Some('\n' | '\r')) {
            end -= 1;
        }
        end
    }

    pub fn line_text(&self, line: usize) -> String {
        if line >= self.len_lines() {
            return String::new();
        }
        self.rope.line(line).to_string()
    }

    pub fn char_at(&self, idx: usize) -> Option<char> {
        (idx < self.len_chars()).then(|| self.rope.char(idx))
    }

    pub fn begin_group(&mut self) {
        if self.active_group.is_none() {
            self.active_group = Some(UndoEntry {
                edits: Vec::new(),
                cursor_before: self.cursor,
                cursor_after: self.cursor,
            });
        }
    }

    pub fn commit_group(&mut self) {
        let Some(mut entry) = self.active_group.take() else {
            return;
        };
        if entry.edits.is_empty() {
            return;
        }
        entry.cursor_after = self.cursor;
        self.undo.push(entry);
        self.redo.clear();
    }

    pub fn replace(&mut self, range: Range<usize>, inserted: &str) {
        let start = range.start.min(self.len_chars());
        let end = range.end.min(self.len_chars()).max(start);
        let deleted = self.rope.slice(start..end).to_string();
        let before = self.cursor;

        self.apply_raw(start..end, inserted);
        self.cursor = start + inserted.chars().count();
        self.revision = self.revision.wrapping_add(1);

        let edit = PrimitiveEdit {
            start,
            deleted,
            inserted: inserted.to_owned(),
        };
        if let Some(group) = &mut self.active_group {
            group.edits.push(edit);
            group.cursor_after = self.cursor;
        } else {
            self.undo.push(UndoEntry {
                edits: vec![edit],
                cursor_before: before,
                cursor_after: self.cursor,
            });
            self.redo.clear();
        }
    }

    pub fn insert(&mut self, text: &str) {
        self.replace(self.cursor..self.cursor, text);
    }

    pub fn delete(&mut self, range: Range<usize>) -> String {
        let start = range.start.min(self.len_chars());
        let end = range.end.min(self.len_chars()).max(start);
        let deleted = self.rope.slice(start..end).to_string();
        self.replace(start..end, "");
        deleted
    }

    pub fn undo(&mut self) -> bool {
        self.commit_group();
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        for edit in entry.edits.iter().rev() {
            let inserted_len = edit.inserted.chars().count();
            self.apply_raw(edit.start..edit.start + inserted_len, &edit.deleted);
        }
        self.cursor = entry.cursor_before.min(self.len_chars());
        self.revision = self.revision.wrapping_add(1);
        self.redo.push(entry);
        true
    }

    pub fn redo(&mut self) -> bool {
        self.commit_group();
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        for edit in &entry.edits {
            let deleted_len = edit.deleted.chars().count();
            self.apply_raw(edit.start..edit.start + deleted_len, &edit.inserted);
        }
        self.cursor = entry.cursor_after.min(self.len_chars());
        self.revision = self.revision.wrapping_add(1);
        self.undo.push(entry);
        true
    }

    pub fn next_grapheme(&self, idx: usize) -> usize {
        let idx = idx.min(self.len_chars());
        if idx == self.len_chars() {
            return idx;
        }
        let line = self.line_of(idx);
        let start = self.line_start(line);
        let text = self.line_text(line);
        let local = idx.saturating_sub(start);
        let mut char_pos = 0;
        for grapheme in text.graphemes(true) {
            let next = char_pos + grapheme.chars().count();
            if char_pos >= local || next > local {
                return (start + next).min(self.len_chars());
            }
            char_pos = next;
        }
        (idx + 1).min(self.len_chars())
    }

    pub fn prev_grapheme(&self, idx: usize) -> usize {
        let idx = idx.min(self.len_chars());
        if idx == 0 {
            return 0;
        }
        if self.char_at(idx - 1) == Some('\n') {
            return idx - 1;
        }
        let line = self.line_of(idx.saturating_sub(1));
        let start = self.line_start(line);
        let text = self.line_text(line);
        let local = idx.saturating_sub(start);
        let mut char_pos = 0;
        let mut previous = 0;
        for grapheme in text.graphemes(true) {
            let next = char_pos + grapheme.chars().count();
            if next >= local {
                return start + char_pos;
            }
            previous = char_pos;
            char_pos = next;
        }
        start + previous
    }

    pub fn move_vertical(&self, idx: usize, delta: isize, preferred_col: usize) -> usize {
        let line = self.line_of(idx);
        let target = line
            .saturating_add_signed(delta)
            .min(self.len_lines().saturating_sub(1));
        let start = self.line_start(target);
        let end = self.line_end(target);
        (start + preferred_col).min(end)
    }

    pub fn char_column(&self, idx: usize) -> usize {
        idx.min(self.len_chars()) - self.line_start(self.line_of(idx))
    }

    pub fn next_word_start(&self, mut idx: usize, count: usize) -> usize {
        for _ in 0..count.max(1) {
            if idx >= self.len_chars() {
                break;
            }
            let class = self.char_at(idx).map(char_class).unwrap_or(0);
            while idx < self.len_chars() && self.char_at(idx).map(char_class).unwrap_or(0) == class
            {
                idx += 1;
            }
            while idx < self.len_chars() && self.char_at(idx).is_some_and(char::is_whitespace) {
                idx += 1;
            }
        }
        idx.min(self.len_chars())
    }

    pub fn prev_word_start(&self, mut idx: usize, count: usize) -> usize {
        for _ in 0..count.max(1) {
            if idx == 0 {
                break;
            }
            idx -= 1;
            while idx > 0 && self.char_at(idx).is_some_and(char::is_whitespace) {
                idx -= 1;
            }
            let class = self.char_at(idx).map(char_class).unwrap_or(0);
            while idx > 0 && self.char_at(idx - 1).map(char_class).unwrap_or(0) == class {
                idx -= 1;
            }
        }
        idx
    }

    pub fn word_end(&self, mut idx: usize, count: usize) -> usize {
        for _ in 0..count.max(1) {
            if idx < self.len_chars() && !self.char_at(idx).is_some_and(char::is_whitespace) {
                let class = self.char_at(idx).map(char_class).unwrap_or(0);
                let at_end = idx + 1 >= self.len_chars()
                    || self.char_at(idx + 1).map(char_class).unwrap_or(0) != class;
                if at_end {
                    idx += 1;
                }
            }
            while idx < self.len_chars() && self.char_at(idx).is_some_and(char::is_whitespace) {
                idx += 1;
            }
            if idx >= self.len_chars() {
                return self.len_chars();
            }
            let class = self.char_at(idx).map(char_class).unwrap_or(0);
            while idx + 1 < self.len_chars()
                && self.char_at(idx + 1).map(char_class).unwrap_or(0) == class
            {
                idx += 1;
            }
        }
        idx.min(self.len_chars())
    }

    pub fn next_big_word_start(&self, mut idx: usize, count: usize) -> usize {
        for _ in 0..count.max(1) {
            while idx < self.len_chars() && !self.char_at(idx).is_some_and(char::is_whitespace) {
                idx += 1;
            }
            while idx < self.len_chars() && self.char_at(idx).is_some_and(char::is_whitespace) {
                idx += 1;
            }
        }
        idx.min(self.len_chars())
    }

    pub fn prev_big_word_start(&self, mut idx: usize, count: usize) -> usize {
        for _ in 0..count.max(1) {
            if idx == 0 {
                break;
            }
            idx -= 1;
            while idx > 0 && self.char_at(idx).is_some_and(char::is_whitespace) {
                idx -= 1;
            }
            while idx > 0 && !self.char_at(idx - 1).is_some_and(char::is_whitespace) {
                idx -= 1;
            }
        }
        idx
    }

    pub fn big_word_end(&self, mut idx: usize, count: usize) -> usize {
        for _ in 0..count.max(1) {
            if idx < self.len_chars() && !self.char_at(idx).is_some_and(char::is_whitespace) {
                let at_end = idx + 1 >= self.len_chars()
                    || self.char_at(idx + 1).is_some_and(char::is_whitespace);
                if at_end {
                    idx += 1;
                }
            }
            while idx < self.len_chars() && self.char_at(idx).is_some_and(char::is_whitespace) {
                idx += 1;
            }
            if idx >= self.len_chars() {
                return self.len_chars();
            }
            while idx + 1 < self.len_chars()
                && !self.char_at(idx + 1).is_some_and(char::is_whitespace)
            {
                idx += 1;
            }
        }
        idx.min(self.len_chars())
    }

    fn apply_raw(&mut self, range: Range<usize>, inserted: &str) {
        let start = range.start;
        if start < range.end {
            self.rope.remove(range);
        }
        if !inserted.is_empty() {
            self.rope.insert(start, inserted);
        }
    }
}

fn char_class(ch: char) -> u8 {
    if ch.is_whitespace() {
        0
    } else if ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn grouped_edits_undo_as_one_change() {
        let mut buffer = TextBuffer::new("hello");
        buffer.set_cursor(5);
        buffer.begin_group();
        buffer.insert(" ");
        buffer.insert("world");
        buffer.commit_group();

        assert_eq!(buffer.as_string(), "hello world");
        assert!(buffer.undo());
        assert_eq!(buffer.as_string(), "hello");
        assert!(buffer.redo());
        assert_eq!(buffer.as_string(), "hello world");
    }

    #[test]
    fn grapheme_motion_does_not_split_emoji() {
        let buffer = TextBuffer::new("a👨‍👩‍👧‍👦b");
        let after_a = buffer.next_grapheme(0);
        let after_emoji = buffer.next_grapheme(after_a);
        assert_eq!(after_a, 1);
        assert_eq!(buffer.char_at(after_emoji), Some('b'));
        assert_eq!(buffer.prev_grapheme(after_emoji), after_a);
    }

    #[test]
    fn word_motions_cross_punctuation_and_space() {
        let buffer = TextBuffer::new("one::two three");
        assert_eq!(buffer.next_word_start(0, 1), 3);
        assert_eq!(buffer.next_word_start(3, 1), 5);
        assert_eq!(buffer.prev_word_start(8, 1), 5);
    }

    #[test]
    fn repeated_end_motions_advance_to_following_words() {
        let buffer = TextBuffer::new("one two-three four");
        assert_eq!(buffer.word_end(0, 1), 2);
        assert_eq!(buffer.word_end(2, 1), 6);
        assert_eq!(buffer.big_word_end(0, 1), 2);
        assert_eq!(buffer.big_word_end(2, 1), 12);
        assert_eq!(buffer.big_word_end(12, 1), 17);
    }
}
