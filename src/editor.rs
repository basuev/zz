use std::cmp::{max, min};
use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::buffer::TextBuffer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
    SearchForward,
    SearchBackward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchDirection {
    Forward,
    Backward,
}

impl SearchDirection {
    fn opposite(self) -> Self {
        match self {
            Self::Forward => Self::Backward,
            Self::Backward => Self::Forward,
        }
    }
}

#[derive(Debug, Clone)]
struct SearchState {
    query: String,
    direction: SearchDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindKind {
    ForwardTo,
    BackwardTo,
    ForwardTill,
    BackwardTill,
}

impl FindKind {
    fn opposite(self) -> Self {
        match self {
            Self::ForwardTo => Self::BackwardTo,
            Self::BackwardTo => Self::ForwardTo,
            Self::ForwardTill => Self::BackwardTill,
            Self::BackwardTill => Self::ForwardTill,
        }
    }

    fn is_forward(self) -> bool {
        matches!(self, Self::ForwardTo | Self::ForwardTill)
    }

    fn is_till(self) -> bool {
        matches!(self, Self::ForwardTill | Self::BackwardTill)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FindState {
    kind: FindKind,
    target: char,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Accept,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Delete,
    Change,
    Yank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    G,
    Z,
    Operator {
        operator: Operator,
        count: usize,
    },
    Find {
        kind: FindKind,
        operator: Option<Operator>,
        count: usize,
    },
}

#[derive(Debug)]
pub struct Editor {
    pub buffer: TextBuffer,
    mode: Mode,
    pending: Pending,
    count: Option<usize>,
    preferred_col: Option<usize>,
    visual_anchor: Option<usize>,
    register: String,
    command: String,
    last_search: Option<SearchState>,
    last_find: Option<FindState>,
    outcome: Option<Outcome>,
}

impl Editor {
    pub fn new(text: &str) -> Self {
        Self {
            buffer: TextBuffer::new(text),
            mode: Mode::Normal,
            pending: Pending::None,
            count: None,
            preferred_col: None,
            visual_anchor: None,
            register: String::new(),
            command: String::new(),
            last_search: None,
            last_find: None,
            outcome: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    pub fn prompt(&self) -> Option<(char, &str)> {
        match self.mode {
            Mode::Command => Some((':', self.command.as_str())),
            Mode::SearchForward => Some(('/', self.command.as_str())),
            Mode::SearchBackward => Some(('?', self.command.as_str())),
            Mode::Normal | Mode::Insert | Mode::Visual | Mode::VisualLine => None,
        }
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.visual_anchor?;
        match self.mode {
            Mode::Visual => {
                let start = min(anchor, self.buffer.cursor());
                let end = max(
                    self.buffer.next_grapheme(anchor),
                    self.buffer.next_grapheme(self.buffer.cursor()),
                );
                Some(start..end)
            }
            Mode::VisualLine => {
                let first_line = min(self.buffer.line_of(anchor), self.buffer.current_line());
                let last_line = max(self.buffer.line_of(anchor), self.buffer.current_line());
                Some(self.buffer.line_start(first_line)..self.line_range_end(last_line))
            }
            Mode::Normal
            | Mode::Insert
            | Mode::Command
            | Mode::SearchForward
            | Mode::SearchBackward => None,
        }
    }

    pub fn replace_text(&mut self, text: &str, cursor: usize) {
        self.buffer = TextBuffer::new(text);
        self.buffer.set_cursor(cursor);
    }

    pub fn handle_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match self.mode {
            Mode::Insert => self.buffer.insert(text),
            Mode::Visual | Mode::VisualLine => {
                if let Some(range) = self.selection() {
                    self.buffer.replace(range, text);
                    self.enter_insert();
                }
            }
            Mode::Normal => {
                self.buffer.begin_group();
                self.buffer.insert(text);
                self.buffer.commit_group();
                self.clamp_normal_cursor();
            }
            Mode::Command | Mode::SearchForward | Mode::SearchBackward => {
                self.command.push_str(text)
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if matches!(key.kind, KeyEventKind::Release) {
            return;
        }

        let expects_find_target = matches!(self.pending, Pending::Find { .. });
        let command_context = !expects_find_target
            && (matches!(
                self.mode,
                Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::Command
            ) || key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER));
        let key = if command_context {
            normalize_command_key(key)
        } else {
            key
        };

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('[') => {
                    self.enter_normal();
                    return;
                }
                KeyCode::Char('r') if self.mode == Mode::Normal => {
                    self.buffer.redo();
                    self.reset_command();
                    return;
                }
                _ => {}
            }
        }

        if expects_find_target {
            self.handle_find_target(key);
            return;
        }

        match self.mode {
            Mode::Insert => self.handle_insert(key),
            Mode::Normal => self.handle_normal(key),
            Mode::Visual | Mode::VisualLine => self.handle_visual(key),
            Mode::Command => self.handle_command(key),
            Mode::SearchForward | Mode::SearchBackward => self.handle_search(key),
        }
    }

    fn handle_find_target(&mut self, key: KeyEvent) {
        let Pending::Find {
            kind,
            operator,
            count,
        } = self.pending
        else {
            return;
        };
        self.pending = Pending::None;
        self.count = None;

        match key.code {
            KeyCode::Esc => self.enter_normal(),
            KeyCode::Char(target)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                let state = FindState { kind, target };
                self.last_find = Some(state);
                self.execute_find(state, count, false, operator);
            }
            _ => {}
        }
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.enter_normal(),
            KeyCode::Enter => self.buffer.insert("\n"),
            KeyCode::Tab => self.buffer.insert("\t"),
            KeyCode::Backspace => {
                let cursor = self.buffer.cursor();
                let previous = self.buffer.prev_grapheme(cursor);
                if previous < cursor {
                    self.buffer.delete(previous..cursor);
                }
            }
            KeyCode::Delete => {
                let cursor = self.buffer.cursor();
                let next = self.buffer.next_grapheme(cursor);
                if next > cursor {
                    self.buffer.delete(cursor..next);
                }
            }
            KeyCode::Left => self.move_left(1),
            KeyCode::Right => self.move_right(1),
            KeyCode::Up => self.move_vertical(-1, 1),
            KeyCode::Down => self.move_vertical(1, 1),
            KeyCode::Home => self.move_line_start(false),
            KeyCode::End => self.move_line_end(true),
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                let mut encoded = [0; 4];
                self.buffer.insert(ch.encode_utf8(&mut encoded));
            }
            _ => {}
        }
    }

    fn handle_command(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.enter_normal(),
            KeyCode::Enter => {
                let command = self.command.trim();
                match command {
                    "q" | "q!" => self.outcome = Some(Outcome::Cancel),
                    "w" | "wq" | "x" => self.outcome = Some(Outcome::Accept),
                    _ => self.enter_normal(),
                }
            }
            KeyCode::Backspace => {
                if self.command.pop().is_none() {
                    self.enter_normal();
                }
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.command.push(ch);
            }
            _ => {}
        }
    }

    fn handle_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.enter_normal(),
            KeyCode::Enter => {
                let direction = match self.mode {
                    Mode::SearchForward => SearchDirection::Forward,
                    Mode::SearchBackward => SearchDirection::Backward,
                    _ => unreachable!("search handler requires search mode"),
                };
                let query = if self.command.is_empty() {
                    self.last_search.as_ref().map(|search| search.query.clone())
                } else {
                    Some(self.command.clone())
                };
                self.mode = Mode::Normal;
                self.command.clear();
                self.reset_command();
                if let Some(query) = query {
                    self.last_search = Some(SearchState { query, direction });
                    self.repeat_search(direction, 1);
                }
            }
            KeyCode::Backspace => {
                if self.command.pop().is_none() {
                    self.enter_normal();
                }
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.command.push(ch);
            }
            _ => {}
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) {
        if let Pending::Z = self.pending {
            self.pending = Pending::None;
            match key.code {
                KeyCode::Char('Z') => self.outcome = Some(Outcome::Accept),
                KeyCode::Char('Q') => self.outcome = Some(Outcome::Cancel),
                _ => {}
            }
            return;
        }

        if let Pending::G = self.pending {
            self.pending = Pending::None;
            if key.code == KeyCode::Char('g') {
                self.buffer.set_cursor(0);
            }
            self.reset_command();
            return;
        }

        if let Pending::Operator { operator, count } = self.pending {
            if self.handle_operator_key(operator, count, key) {
                self.pending = Pending::None;
                self.count = None;
            }
            return;
        }

        if let KeyCode::Char(ch @ '1'..='9') = key.code {
            self.push_count(ch);
            return;
        }
        if key.code == KeyCode::Char('0') && self.count.is_some() {
            self.push_count('0');
            return;
        }

        let count = self.take_count();
        match key.code {
            KeyCode::Esc => self.reset_command(),
            KeyCode::Char(':') => {
                self.command.clear();
                self.mode = Mode::Command;
            }
            KeyCode::Char('/') => self.start_search(SearchDirection::Forward),
            KeyCode::Char('?') => self.start_search(SearchDirection::Backward),
            KeyCode::Char('n') => {
                if let Some(direction) = self.last_search.as_ref().map(|search| search.direction) {
                    self.repeat_search(direction, count.unwrap_or(1));
                }
            }
            KeyCode::Char('N') => {
                if let Some(direction) = self.last_search.as_ref().map(|search| search.direction) {
                    self.repeat_search(direction.opposite(), count.unwrap_or(1));
                }
            }
            KeyCode::Char('f') => self.start_find(FindKind::ForwardTo, None, count.unwrap_or(1)),
            KeyCode::Char('F') => self.start_find(FindKind::BackwardTo, None, count.unwrap_or(1)),
            KeyCode::Char('t') => self.start_find(FindKind::ForwardTill, None, count.unwrap_or(1)),
            KeyCode::Char('T') => self.start_find(FindKind::BackwardTill, None, count.unwrap_or(1)),
            KeyCode::Char(';') => {
                if let Some(state) = self.last_find {
                    self.execute_find(state, count.unwrap_or(1), true, None);
                }
            }
            KeyCode::Char(',') => {
                if let Some(state) = self.last_find {
                    self.execute_find(
                        FindState {
                            kind: state.kind.opposite(),
                            target: state.target,
                        },
                        count.unwrap_or(1),
                        true,
                        None,
                    );
                }
            }
            KeyCode::Char('Z') => self.pending = Pending::Z,
            KeyCode::Char('g') => self.pending = Pending::G,
            KeyCode::Char('G') => {
                let line = count
                    .map(|value| value.saturating_sub(1))
                    .unwrap_or_else(|| self.buffer.len_lines().saturating_sub(1));
                self.buffer.set_cursor(self.buffer.line_start(line));
            }
            KeyCode::Char('h') | KeyCode::Left => self.move_left(count.unwrap_or(1)),
            KeyCode::Char('l') | KeyCode::Right => self.move_right(count.unwrap_or(1)),
            KeyCode::Char('j') | KeyCode::Down => self.move_vertical(1, count.unwrap_or(1)),
            KeyCode::Char('k') | KeyCode::Up => self.move_vertical(-1, count.unwrap_or(1)),
            KeyCode::Char('w') => {
                let target = self
                    .buffer
                    .next_word_start(self.buffer.cursor(), count.unwrap_or(1));
                self.buffer.set_cursor(target);
                self.clamp_normal_cursor();
            }
            KeyCode::Char('W') => {
                let target = self
                    .buffer
                    .next_big_word_start(self.buffer.cursor(), count.unwrap_or(1));
                self.buffer.set_cursor(target);
                self.clamp_normal_cursor();
            }
            KeyCode::Char('b') => {
                let target = self
                    .buffer
                    .prev_word_start(self.buffer.cursor(), count.unwrap_or(1));
                self.buffer.set_cursor(target);
            }
            KeyCode::Char('B') => {
                let target = self
                    .buffer
                    .prev_big_word_start(self.buffer.cursor(), count.unwrap_or(1));
                self.buffer.set_cursor(target);
            }
            KeyCode::Char('e') => {
                let target = self
                    .buffer
                    .word_end(self.buffer.cursor(), count.unwrap_or(1));
                self.buffer.set_cursor(target);
                self.clamp_normal_cursor();
            }
            KeyCode::Char('E') => {
                let target = self
                    .buffer
                    .big_word_end(self.buffer.cursor(), count.unwrap_or(1));
                self.buffer.set_cursor(target);
                self.clamp_normal_cursor();
            }
            KeyCode::Char('0') | KeyCode::Home => self.move_line_start(false),
            KeyCode::Char('^') => self.move_line_start(true),
            KeyCode::Char('$') | KeyCode::End => self.move_line_end(false),
            KeyCode::Char('i') => self.enter_insert(),
            KeyCode::Char('a') => {
                let next = self.buffer.next_grapheme(self.buffer.cursor());
                self.buffer.set_cursor(next);
                self.enter_insert();
            }
            KeyCode::Char('I') => {
                self.move_line_start(true);
                self.enter_insert();
            }
            KeyCode::Char('A') => {
                self.move_line_end(true);
                self.enter_insert();
            }
            KeyCode::Char('o') => self.open_line_below(),
            KeyCode::Char('O') => self.open_line_above(),
            KeyCode::Char('v') => self.enter_visual(Mode::Visual),
            KeyCode::Char('V') => self.enter_visual(Mode::VisualLine),
            KeyCode::Char('d') => {
                self.pending = Pending::Operator {
                    operator: Operator::Delete,
                    count: count.unwrap_or(1),
                }
            }
            KeyCode::Char('c') => {
                self.pending = Pending::Operator {
                    operator: Operator::Change,
                    count: count.unwrap_or(1),
                }
            }
            KeyCode::Char('y') => {
                self.pending = Pending::Operator {
                    operator: Operator::Yank,
                    count: count.unwrap_or(1),
                }
            }
            KeyCode::Char('x') | KeyCode::Delete => self.delete_under_cursor(count.unwrap_or(1)),
            KeyCode::Char('p') => self.paste_after(),
            KeyCode::Char('P') => self.paste_before(),
            KeyCode::Char('u') => {
                self.buffer.undo();
                self.clamp_normal_cursor();
            }
            _ => {}
        }
    }

    fn handle_visual(&mut self, key: KeyEvent) {
        let count = self.take_count().unwrap_or(1);
        match key.code {
            KeyCode::Esc => self.enter_normal(),
            KeyCode::Char('h') | KeyCode::Left => self.move_left(count),
            KeyCode::Char('l') | KeyCode::Right => self.move_right(count),
            KeyCode::Char('j') | KeyCode::Down => self.move_vertical(1, count),
            KeyCode::Char('k') | KeyCode::Up => self.move_vertical(-1, count),
            KeyCode::Char('w') => {
                let target = self.buffer.next_word_start(self.buffer.cursor(), count);
                self.buffer.set_cursor(target);
                self.clamp_normal_cursor();
            }
            KeyCode::Char('W') => {
                let target = self.buffer.next_big_word_start(self.buffer.cursor(), count);
                self.buffer.set_cursor(target);
                self.clamp_normal_cursor();
            }
            KeyCode::Char('b') => {
                let target = self.buffer.prev_word_start(self.buffer.cursor(), count);
                self.buffer.set_cursor(target);
            }
            KeyCode::Char('B') => {
                let target = self.buffer.prev_big_word_start(self.buffer.cursor(), count);
                self.buffer.set_cursor(target);
            }
            KeyCode::Char('e') => {
                let target = self.buffer.word_end(self.buffer.cursor(), count);
                self.buffer.set_cursor(target);
            }
            KeyCode::Char('E') => {
                let target = self.buffer.big_word_end(self.buffer.cursor(), count);
                self.buffer.set_cursor(target);
            }
            KeyCode::Char('0') | KeyCode::Home => self.move_line_start(false),
            KeyCode::Char('$') | KeyCode::End => self.move_line_end(false),
            KeyCode::Char('f') => self.start_find(FindKind::ForwardTo, None, count),
            KeyCode::Char('F') => self.start_find(FindKind::BackwardTo, None, count),
            KeyCode::Char('t') => self.start_find(FindKind::ForwardTill, None, count),
            KeyCode::Char('T') => self.start_find(FindKind::BackwardTill, None, count),
            KeyCode::Char(';') => {
                if let Some(state) = self.last_find {
                    self.execute_find(state, count, true, None);
                }
            }
            KeyCode::Char(',') => {
                if let Some(state) = self.last_find {
                    self.execute_find(
                        FindState {
                            kind: state.kind.opposite(),
                            target: state.target,
                        },
                        count,
                        true,
                        None,
                    );
                }
            }
            KeyCode::Char('v') if self.mode == Mode::Visual => self.enter_normal(),
            KeyCode::Char('V') => self.mode = Mode::VisualLine,
            KeyCode::Char('d') | KeyCode::Char('x') => self.consume_selection(Operator::Delete),
            KeyCode::Char('c') => self.consume_selection(Operator::Change),
            KeyCode::Char('y') => self.consume_selection(Operator::Yank),
            _ => {}
        }
    }

    fn handle_operator_key(
        &mut self,
        operator: Operator,
        operator_count: usize,
        key: KeyEvent,
    ) -> bool {
        let count = operator_count
            .saturating_mul(self.take_count().unwrap_or(1))
            .max(1);
        match key.code {
            KeyCode::Char('f') => {
                self.start_find(FindKind::ForwardTo, Some(operator), count);
                return false;
            }
            KeyCode::Char('F') => {
                self.start_find(FindKind::BackwardTo, Some(operator), count);
                return false;
            }
            KeyCode::Char('t') => {
                self.start_find(FindKind::ForwardTill, Some(operator), count);
                return false;
            }
            KeyCode::Char('T') => {
                self.start_find(FindKind::BackwardTill, Some(operator), count);
                return false;
            }
            KeyCode::Char(';') => {
                if let Some(state) = self.last_find {
                    self.execute_find(state, count, true, Some(operator));
                }
                return true;
            }
            KeyCode::Char(',') => {
                if let Some(state) = self.last_find {
                    self.execute_find(
                        FindState {
                            kind: state.kind.opposite(),
                            target: state.target,
                        },
                        count,
                        true,
                        Some(operator),
                    );
                }
                return true;
            }
            _ => {}
        }

        let repeated = matches!(
            (operator, key.code),
            (Operator::Delete, KeyCode::Char('d'))
                | (Operator::Change, KeyCode::Char('c'))
                | (Operator::Yank, KeyCode::Char('y'))
        );
        if repeated {
            let start_line = self.buffer.current_line();
            let end_line = (start_line + count - 1).min(self.buffer.len_lines().saturating_sub(1));
            self.apply_operator(
                operator,
                self.buffer.line_start(start_line)..self.line_range_end(end_line),
            );
            return true;
        }

        let origin = self.buffer.cursor();
        let range = match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                let mut target = origin;
                for _ in 0..count {
                    target = self.buffer.prev_grapheme(target);
                }
                target..origin
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let mut target = origin;
                for _ in 0..count {
                    target = self.buffer.next_grapheme(target);
                }
                origin..target
            }
            KeyCode::Char('w') => origin..self.buffer.next_word_start(origin, count),
            KeyCode::Char('W') => origin..self.buffer.next_big_word_start(origin, count),
            KeyCode::Char('b') => {
                let target = self.buffer.prev_word_start(origin, count);
                target..self.buffer.next_grapheme(origin)
            }
            KeyCode::Char('B') => {
                let target = self.buffer.prev_big_word_start(origin, count);
                target..self.buffer.next_grapheme(origin)
            }
            KeyCode::Char('e') => {
                let target = self.buffer.word_end(origin, count);
                origin..self.buffer.next_grapheme(target)
            }
            KeyCode::Char('E') => {
                let target = self.buffer.big_word_end(origin, count);
                origin..self.buffer.next_grapheme(target)
            }
            KeyCode::Char('0') | KeyCode::Home => {
                self.buffer.line_start(self.buffer.current_line())..origin
            }
            KeyCode::Char('$') | KeyCode::End => {
                origin..self.buffer.line_end(self.buffer.current_line())
            }
            _ => return true,
        };
        self.apply_operator(operator, normalize_range(range));
        true
    }

    fn apply_operator(&mut self, operator: Operator, range: Range<usize>) {
        let start = range.start.min(self.buffer.len_chars());
        let end = range.end.min(self.buffer.len_chars()).max(start);
        if start == end && operator != Operator::Yank {
            return;
        }
        self.register = self
            .buffer
            .as_string()
            .chars()
            .skip(start)
            .take(end - start)
            .collect();
        match operator {
            Operator::Yank => {
                self.buffer.set_cursor(start);
                self.enter_normal();
            }
            Operator::Delete => {
                self.buffer.delete(start..end);
                self.buffer.set_cursor(start.min(self.buffer.len_chars()));
                self.enter_normal();
                self.clamp_normal_cursor();
            }
            Operator::Change => {
                self.buffer.delete(start..end);
                self.buffer.set_cursor(start.min(self.buffer.len_chars()));
                self.enter_insert();
            }
        }
    }

    fn consume_selection(&mut self, operator: Operator) {
        if let Some(range) = self.selection() {
            self.apply_operator(operator, range);
        }
    }

    fn delete_under_cursor(&mut self, count: usize) {
        let start = self.buffer.cursor();
        let mut end = start;
        for _ in 0..count {
            let next = self.buffer.next_grapheme(end);
            if next == end || self.buffer.char_at(end) == Some('\n') {
                break;
            }
            end = next;
        }
        if end > start {
            self.register = self.buffer.delete(start..end);
            self.clamp_normal_cursor();
        }
    }

    fn paste_after(&mut self) {
        if self.register.is_empty() {
            return;
        }
        let cursor = if self.register.ends_with('\n') {
            self.line_range_end(self.buffer.current_line())
        } else {
            self.buffer.next_grapheme(self.buffer.cursor())
        };
        self.buffer.set_cursor(cursor);
        self.buffer.insert(&self.register.clone());
        self.clamp_normal_cursor();
    }

    fn paste_before(&mut self) {
        if self.register.is_empty() {
            return;
        }
        let cursor = if self.register.ends_with('\n') {
            self.buffer.line_start(self.buffer.current_line())
        } else {
            self.buffer.cursor()
        };
        self.buffer.set_cursor(cursor);
        self.buffer.insert(&self.register.clone());
        self.clamp_normal_cursor();
    }

    fn open_line_below(&mut self) {
        let insertion = self.line_range_end(self.buffer.current_line());
        let inserts_before_existing_line =
            insertion > 0 && self.buffer.char_at(insertion - 1) == Some('\n');
        self.buffer.set_cursor(insertion);
        self.buffer.begin_group();
        self.buffer.insert("\n");
        if inserts_before_existing_line {
            self.buffer.set_cursor(insertion);
        }
        self.enter_insert_with_open_group();
    }

    fn open_line_above(&mut self) {
        let insertion = self.buffer.line_start(self.buffer.current_line());
        self.buffer.set_cursor(insertion);
        self.buffer.begin_group();
        self.buffer.insert("\n");
        self.buffer.set_cursor(insertion);
        self.enter_insert_with_open_group();
    }

    fn start_find(&mut self, kind: FindKind, operator: Option<Operator>, count: usize) {
        self.pending = Pending::Find {
            kind,
            operator,
            count: count.max(1),
        };
    }

    fn execute_find(
        &mut self,
        state: FindState,
        count: usize,
        repeating: bool,
        operator: Option<Operator>,
    ) {
        let origin = self.buffer.cursor();
        let Some(destination) = self.find_destination(state, count, repeating) else {
            return;
        };

        if let Some(operator) = operator {
            let start = min(origin, destination);
            let end = self.buffer.next_grapheme(max(origin, destination));
            self.apply_operator(operator, start..end);
        } else {
            self.buffer.set_cursor(destination);
            self.preferred_col = None;
            self.clamp_normal_cursor();
        }
    }

    fn find_destination(&self, state: FindState, count: usize, repeating: bool) -> Option<usize> {
        let origin = self.buffer.cursor();
        let line = self.buffer.current_line();
        let line_start = self.buffer.line_start(line);
        let line_end = self.buffer.line_end(line);
        let adjacent = if state.kind.is_forward() {
            self.buffer.next_grapheme(origin)
        } else {
            self.buffer.prev_grapheme(origin)
        };
        let skip_adjacent = repeating
            && state.kind.is_till()
            && adjacent != origin
            && self.buffer.char_at(adjacent) == Some(state.target);

        let mut remaining = count.max(1).saturating_add(usize::from(skip_adjacent));
        let target = if state.kind.is_forward() {
            ((origin + 1).min(line_end)..line_end).find(|index| {
                if self.buffer.char_at(*index) == Some(state.target) {
                    remaining -= 1;
                    remaining == 0
                } else {
                    false
                }
            })
        } else {
            (line_start..origin.min(line_end)).rev().find(|index| {
                if self.buffer.char_at(*index) == Some(state.target) {
                    remaining -= 1;
                    remaining == 0
                } else {
                    false
                }
            })
        }?;

        Some(match state.kind {
            FindKind::ForwardTo | FindKind::BackwardTo => target,
            FindKind::ForwardTill => self.buffer.prev_grapheme(target),
            FindKind::BackwardTill => self.buffer.next_grapheme(target),
        })
    }

    fn start_search(&mut self, direction: SearchDirection) {
        self.buffer.commit_group();
        self.command.clear();
        self.mode = match direction {
            SearchDirection::Forward => Mode::SearchForward,
            SearchDirection::Backward => Mode::SearchBackward,
        };
        self.visual_anchor = None;
        self.reset_command();
    }

    fn repeat_search(&mut self, direction: SearchDirection, count: usize) {
        let Some(query) = self.last_search.as_ref().map(|search| search.query.clone()) else {
            return;
        };
        if query.is_empty() {
            return;
        }

        let text = self.buffer.as_string();
        let mut cursor = self.buffer.cursor();
        for _ in 0..count.max(1) {
            let Some(found) = find_search_match(&text, &query, cursor, direction) else {
                break;
            };
            cursor = found;
        }
        self.buffer.set_cursor(cursor);
        self.preferred_col = None;
        self.clamp_normal_cursor();
    }

    fn enter_insert(&mut self) {
        self.buffer.begin_group();
        self.enter_insert_with_open_group();
    }

    fn enter_insert_with_open_group(&mut self) {
        self.mode = Mode::Insert;
        self.visual_anchor = None;
        self.reset_command();
    }

    fn enter_visual(&mut self, mode: Mode) {
        self.buffer.commit_group();
        self.mode = mode;
        self.visual_anchor = Some(self.buffer.cursor());
        self.reset_command();
    }

    fn enter_normal(&mut self) {
        let was_insert = self.mode == Mode::Insert;
        self.buffer.commit_group();
        self.mode = Mode::Normal;
        self.visual_anchor = None;
        self.command.clear();
        self.reset_command();
        if was_insert {
            let line_start = self.buffer.line_start(self.buffer.current_line());
            if self.buffer.cursor() > line_start {
                self.buffer
                    .set_cursor(self.buffer.prev_grapheme(self.buffer.cursor()));
            }
        }
        self.clamp_normal_cursor();
    }

    fn move_left(&mut self, count: usize) {
        let line_start = self.buffer.line_start(self.buffer.current_line());
        let mut cursor = self.buffer.cursor();
        for _ in 0..count {
            let previous = self.buffer.prev_grapheme(cursor);
            if previous < line_start {
                break;
            }
            cursor = previous;
        }
        self.buffer.set_cursor(cursor);
        self.preferred_col = None;
    }

    fn move_right(&mut self, count: usize) {
        let line_end = self.buffer.line_end(self.buffer.current_line());
        let limit = if self.mode == Mode::Insert {
            line_end
        } else {
            self.line_last_cursor(self.buffer.current_line())
        };
        let mut cursor = self.buffer.cursor();
        for _ in 0..count {
            let next = self.buffer.next_grapheme(cursor);
            if next > limit {
                break;
            }
            cursor = next;
        }
        self.buffer.set_cursor(cursor);
        self.preferred_col = None;
    }

    fn move_vertical(&mut self, direction: isize, count: usize) {
        let column = self
            .preferred_col
            .unwrap_or_else(|| self.buffer.char_column(self.buffer.cursor()));
        let target = self.buffer.move_vertical(
            self.buffer.cursor(),
            direction.saturating_mul(count as isize),
            column,
        );
        self.buffer.set_cursor(target);
        self.preferred_col = Some(column);
        if self.mode != Mode::Insert {
            self.clamp_normal_cursor();
        }
    }

    fn move_line_start(&mut self, first_nonblank: bool) {
        let line = self.buffer.current_line();
        let mut cursor = self.buffer.line_start(line);
        if first_nonblank {
            let end = self.buffer.line_end(line);
            while cursor < end && self.buffer.char_at(cursor).is_some_and(char::is_whitespace) {
                cursor += 1;
            }
        }
        self.buffer.set_cursor(cursor);
        self.preferred_col = None;
    }

    fn move_line_end(&mut self, insertion_point: bool) {
        let line = self.buffer.current_line();
        let cursor = if insertion_point || self.mode == Mode::Insert {
            self.buffer.line_end(line)
        } else {
            self.line_last_cursor(line)
        };
        self.buffer.set_cursor(cursor);
        self.preferred_col = None;
    }

    fn line_last_cursor(&self, line: usize) -> usize {
        let start = self.buffer.line_start(line);
        let end = self.buffer.line_end(line);
        if end > start {
            self.buffer.prev_grapheme(end)
        } else {
            start
        }
    }

    fn line_range_end(&self, line: usize) -> usize {
        if line + 1 < self.buffer.len_lines() {
            self.buffer.line_start(line + 1)
        } else {
            self.buffer.line_end(line)
        }
    }

    fn clamp_normal_cursor(&mut self) {
        if self.mode == Mode::Insert || self.buffer.len_chars() == 0 {
            return;
        }
        let line = self.buffer.current_line();
        let end = self.buffer.line_end(line);
        let start = self.buffer.line_start(line);
        if self.buffer.cursor() >= end && end > start {
            self.buffer.set_cursor(self.buffer.prev_grapheme(end));
        }
    }

    fn push_count(&mut self, digit: char) {
        let value = digit.to_digit(10).unwrap_or_default() as usize;
        self.count = Some(
            self.count
                .unwrap_or_default()
                .saturating_mul(10)
                .saturating_add(value),
        );
    }

    fn take_count(&mut self) -> Option<usize> {
        self.count.take().filter(|value| *value > 0)
    }

    fn reset_command(&mut self) {
        self.pending = Pending::None;
        self.count = None;
        self.preferred_col = None;
    }
}

fn normalize_range(range: Range<usize>) -> Range<usize> {
    min(range.start, range.end)..max(range.start, range.end)
}

fn find_search_match(
    text: &str,
    query: &str,
    origin: usize,
    direction: SearchDirection,
) -> Option<usize> {
    let text: Vec<char> = text.chars().collect();
    let query: Vec<char> = query.chars().collect();
    if query.is_empty() || query.len() > text.len() {
        return None;
    }

    let matches: Vec<usize> = text
        .windows(query.len())
        .enumerate()
        .filter_map(|(index, window)| (window == query).then_some(index))
        .collect();
    match direction {
        SearchDirection::Forward => matches
            .iter()
            .copied()
            .find(|index| *index > origin)
            .or_else(|| matches.first().copied()),
        SearchDirection::Backward => matches
            .iter()
            .rev()
            .copied()
            .find(|index| *index < origin)
            .or_else(|| matches.last().copied()),
    }
}

fn normalize_command_key(mut key: KeyEvent) -> KeyEvent {
    if let Some(base_code) = key.base_code {
        key.code = match base_code {
            KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::SHIFT) => {
                KeyCode::Char(shift_us_key(ch))
            }
            code => code,
        };
        key.modifiers.remove(KeyModifiers::SHIFT);
        return key;
    }

    if let KeyCode::Char(ch) = key.code
        && let Some(mapped) = cyrillic_key_to_us(ch)
    {
        key.code = KeyCode::Char(mapped);
        key.modifiers.remove(KeyModifiers::SHIFT);
    }
    key
}

fn shift_us_key(ch: char) -> char {
    match ch {
        'a'..='z' => ch.to_ascii_uppercase(),
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        '`' => '~',
        _ => ch,
    }
}

fn cyrillic_key_to_us(ch: char) -> Option<char> {
    const LOWER_CYRILLIC: &str = "йцукенгшщзхъфывапролджэячсмитьбюёіїєґ";
    const LOWER_US: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`s]'`";
    const UPPER_CYRILLIC: &str = "ЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮЁІЇЄҐ";
    const UPPER_US: &str = "QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>~S}\"~";

    LOWER_CYRILLIC
        .chars()
        .zip(LOWER_US.chars())
        .chain(UPPER_CYRILLIC.chars().zip(UPPER_US.chars()))
        .find_map(|(layout, us)| (layout == ch).then_some(us))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyEvent, KeyModifiers};
    use pretty_assertions::assert_eq;

    use super::*;

    fn key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    #[test]
    fn insert_session_is_one_undo_transaction() {
        let mut editor = Editor::new("");
        editor.handle_key(key('i'));
        editor.handle_key(key('h'));
        editor.handle_key(key('i'));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        editor.handle_key(key('u'));
        assert_eq!(editor.buffer.as_string(), "");
    }

    #[test]
    fn dd_deletes_and_p_restores_a_line() {
        let mut editor = Editor::new("one\ntwo\n");
        editor.handle_key(key('d'));
        editor.handle_key(key('d'));
        assert_eq!(editor.buffer.as_string(), "two\n");
        editor.handle_key(key('P'));
        assert_eq!(editor.buffer.as_string(), "one\ntwo\n");

        let mut counted = Editor::new("one\ntwo\nthree\n");
        counted.handle_key(key('2'));
        counted.handle_key(key('d'));
        counted.handle_key(key('d'));
        assert_eq!(counted.buffer.as_string(), "three\n");
    }

    #[test]
    fn zz_accepts_and_zq_cancels() {
        let mut accept = Editor::new("");
        accept.handle_key(key('Z'));
        accept.handle_key(key('Z'));
        assert_eq!(accept.outcome(), Some(Outcome::Accept));

        let mut cancel = Editor::new("");
        cancel.handle_key(key('Z'));
        cancel.handle_key(key('Q'));
        assert_eq!(cancel.outcome(), Some(Outcome::Cancel));
    }

    #[test]
    fn visual_delete_uses_inclusive_selection() {
        let mut editor = Editor::new("abc");
        editor.handle_key(key('v'));
        editor.handle_key(key('l'));
        editor.handle_key(key('d'));
        assert_eq!(editor.buffer.as_string(), "c");
    }

    #[test]
    fn backward_visual_selection_includes_anchor() {
        let mut editor = Editor::new("abc");
        editor.handle_key(key('l'));
        editor.handle_key(key('l'));
        editor.handle_key(key('v'));
        editor.handle_key(key('h'));
        editor.handle_key(key('h'));
        editor.handle_key(key('d'));
        assert_eq!(editor.buffer.as_string(), "");
    }

    #[test]
    fn dh_deletes_only_the_previous_character() {
        let mut editor = Editor::new("abc");
        editor.handle_key(key('l'));
        editor.handle_key(key('d'));
        editor.handle_key(key('h'));
        assert_eq!(editor.buffer.as_string(), "bc");
    }

    #[test]
    fn open_below_inserts_a_blank_line() {
        let mut editor = Editor::new("one\ntwo");
        editor.handle_key(key('o'));
        editor.handle_key(key('x'));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(editor.buffer.as_string(), "one\nx\ntwo");
    }

    #[test]
    fn uppercase_e_advances_across_big_words() {
        let mut editor = Editor::new("one two-three four");
        editor.handle_key(key('E'));
        assert_eq!(editor.buffer.cursor(), 2);
        editor.handle_key(key('E'));
        assert_eq!(editor.buffer.cursor(), 12);
        editor.handle_key(key('E'));
        assert_eq!(editor.buffer.cursor(), 17);
    }

    #[test]
    fn command_line_supports_q_w_and_wq() {
        for command in ["w", "wq"] {
            let mut editor = Editor::new("");
            editor.handle_key(key(':'));
            for ch in command.chars() {
                editor.handle_key(key(ch));
            }
            editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert_eq!(editor.outcome(), Some(Outcome::Accept));
        }

        let mut editor = Editor::new("");
        editor.handle_key(key(':'));
        editor.handle_key(key('q'));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.outcome(), Some(Outcome::Cancel));
    }

    #[test]
    fn character_find_supports_counts_and_directional_repeats() {
        let mut editor = Editor::new("a-b-c-d");
        editor.handle_key(key('2'));
        editor.handle_key(key('f'));
        editor.handle_key(key('-'));
        assert_eq!(editor.buffer.cursor(), 3);

        editor.handle_key(key(';'));
        assert_eq!(editor.buffer.cursor(), 5);
        editor.handle_key(key(','));
        assert_eq!(editor.buffer.cursor(), 3);
    }

    #[test]
    fn till_repeat_skips_the_adjacent_previous_target() {
        let mut editor = Editor::new("abxcxdx");
        editor.handle_key(key('t'));
        editor.handle_key(key('x'));
        assert_eq!(editor.buffer.cursor(), 1);
        editor.handle_key(key(';'));
        assert_eq!(editor.buffer.cursor(), 3);
        editor.handle_key(key(';'));
        assert_eq!(editor.buffer.cursor(), 5);
    }

    #[test]
    fn character_find_target_uses_the_logical_layout_character() {
        let mut editor = Editor::new("aфbф");
        editor.handle_key(key('f'));
        editor.handle_key(key('ф'));
        assert_eq!(editor.buffer.cursor(), 1);
        editor.handle_key(key(';'));
        assert_eq!(editor.buffer.cursor(), 3);
    }

    #[test]
    fn character_find_works_with_operators_and_visual_mode() {
        let mut deleted = Editor::new("abc:def");
        deleted.handle_key(key('d'));
        deleted.handle_key(key('f'));
        deleted.handle_key(key(':'));
        assert_eq!(deleted.buffer.as_string(), "def");

        let mut counted = Editor::new("a:b:c:d");
        counted.handle_key(key('2'));
        counted.handle_key(key('d'));
        counted.handle_key(key('f'));
        counted.handle_key(key(':'));
        assert_eq!(counted.buffer.as_string(), "c:d");

        let mut changed_till = Editor::new("abc:def");
        changed_till.handle_key(key('d'));
        changed_till.handle_key(key('t'));
        changed_till.handle_key(key(':'));
        assert_eq!(changed_till.buffer.as_string(), ":def");

        let mut visual = Editor::new("abc:def");
        visual.handle_key(key('v'));
        visual.handle_key(key('f'));
        visual.handle_key(key(':'));
        visual.handle_key(key('d'));
        assert_eq!(visual.buffer.as_string(), "def");
    }

    #[test]
    fn character_find_stays_on_the_current_line() {
        let mut editor = Editor::new("abc\ndef:c");
        editor.handle_key(key('f'));
        editor.handle_key(key(':'));
        assert_eq!(editor.buffer.cursor(), 0);
    }

    #[test]
    fn forward_search_repeats_wraps_and_reverses() {
        let mut editor = Editor::new("one two one three one");
        editor.handle_key(key('/'));
        for ch in "one".chars() {
            editor.handle_key(key(ch));
        }
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.buffer.cursor(), 8);

        editor.handle_key(key('n'));
        assert_eq!(editor.buffer.cursor(), 18);
        editor.handle_key(key('n'));
        assert_eq!(editor.buffer.cursor(), 0);
        editor.handle_key(key('N'));
        assert_eq!(editor.buffer.cursor(), 18);
    }

    #[test]
    fn backward_search_honors_counts() {
        let mut editor = Editor::new("one two one three one");
        editor.buffer.set_cursor(18);
        editor.handle_key(key('?'));
        for ch in "one".chars() {
            editor.handle_key(key(ch));
        }
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.buffer.cursor(), 8);

        editor.handle_key(key('2'));
        editor.handle_key(key('n'));
        assert_eq!(editor.buffer.cursor(), 18);
    }

    #[test]
    fn search_keeps_logical_unicode_input() {
        let mut editor = Editor::new("начало фраза конец фраза");
        editor.handle_key(key('/'));
        for ch in "фраза".chars() {
            editor.handle_key(key(ch));
        }
        assert_eq!(editor.prompt(), Some(('/', "фраза")));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.buffer.cursor(), 7);
    }

    #[test]
    fn russian_layout_uses_physical_keys_for_commands_but_not_inserted_text() {
        let mut editor = Editor::new("");
        editor.handle_key(key('ш')); // Physical I.
        assert_eq!(editor.mode(), Mode::Insert);
        editor.handle_key(key('ш'));
        assert_eq!(editor.buffer.as_string(), "ш");
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(editor.mode(), Mode::Normal);

        editor.handle_key(key('Ж')); // Physical Shift+; -> :.
        assert_eq!(editor.mode(), Mode::Command);
        editor.handle_key(key('й')); // Physical Q.
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.outcome(), Some(Outcome::Cancel));
    }

    #[test]
    fn kitty_base_layout_key_takes_precedence_for_commands() {
        let mut event = key('ш');
        event.base_code = Some(KeyCode::Char('i'));
        let mut editor = Editor::new("");
        editor.handle_key(event);
        assert_eq!(editor.mode(), Mode::Insert);

        let mut colon = key('Ж');
        colon.base_code = Some(KeyCode::Char(';'));
        colon.modifiers = KeyModifiers::SHIFT;
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        editor.handle_key(colon);
        assert_eq!(editor.mode(), Mode::Command);
    }
}
