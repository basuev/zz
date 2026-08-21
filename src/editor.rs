use std::cmp::{max, min};
use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

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
    History,
    Context,
    ContextPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryScope {
    Workspace,
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextRange {
    None,
    Incomplete,
    Invalid,
    Valid { start: usize, end: usize },
}

#[derive(Debug, Clone, Copy)]
struct ParsedContextQuery<'a> {
    path: &'a str,
    range: ContextRange,
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

#[derive(Debug, Clone)]
enum RepeatInput {
    Key(KeyEvent),
    Paste(String),
}

#[derive(Debug, Clone)]
struct RepeatEvent {
    mode: Mode,
    input: RepeatInput,
}

#[derive(Debug)]
struct ChangeRecording {
    events: Vec<RepeatEvent>,
    revision_before: u64,
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
    TextObject {
        around: bool,
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
    repeat_prefix: Vec<RepeatEvent>,
    recording: Option<ChangeRecording>,
    last_change: Vec<RepeatEvent>,
    replaying: bool,
    history_workspace: Vec<String>,
    history_global: Vec<String>,
    history_scope: HistoryScope,
    history_query: String,
    history_matches: Vec<usize>,
    history_selected: usize,
    history_return_mode: Mode,
    context_files: Vec<String>,
    context_query: String,
    context_matches: Vec<usize>,
    context_selected: usize,
    context_indexing: bool,
    context_search_enabled: bool,
    context_search_generation: u64,
    context_search_request: Option<(u64, String)>,
    context_last_search_query: Option<String>,
    context_matcher: Matcher,
    context_preview_path: Option<String>,
    context_preview_lines: Vec<String>,
    context_preview_cursor: usize,
    context_preview_anchor: Option<usize>,
    context_preview_request: Option<String>,
    context_preview_error: Option<String>,
    context_preview_truncated: bool,
    ctrl_c_armed: bool,
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
            repeat_prefix: Vec::new(),
            recording: None,
            last_change: Vec::new(),
            replaying: false,
            history_workspace: Vec::new(),
            history_global: Vec::new(),
            history_scope: HistoryScope::Workspace,
            history_query: String::new(),
            history_matches: Vec::new(),
            history_selected: 0,
            history_return_mode: Mode::Normal,
            context_files: Vec::new(),
            context_query: String::new(),
            context_matches: Vec::new(),
            context_selected: 0,
            context_indexing: false,
            context_search_enabled: false,
            context_search_generation: 0,
            context_search_request: None,
            context_last_search_query: None,
            context_matcher: Matcher::new(Config::DEFAULT.match_paths()),
            context_preview_path: None,
            context_preview_lines: Vec::new(),
            context_preview_cursor: 0,
            context_preview_anchor: None,
            context_preview_request: None,
            context_preview_error: None,
            context_preview_truncated: false,
            ctrl_c_armed: false,
            outcome: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    pub fn enable_context_search(&mut self) {
        self.context_search_enabled = true;
    }

    pub fn take_context_search_request(&mut self) -> Option<(u64, String)> {
        self.context_search_request.take()
    }

    pub fn fail_context_search(&mut self) {
        self.context_indexing = false;
    }

    pub fn apply_context_search_result(
        &mut self,
        generation: u64,
        files: Vec<String>,
        complete: bool,
    ) -> bool {
        if generation != self.context_search_generation {
            return false;
        }
        self.context_files = files;
        self.context_indexing = !complete;
        self.recompute_context_matches();
        true
    }

    pub fn set_context_files(&mut self, files: Vec<String>) {
        self.context_files = files;
        self.context_indexing = false;
        self.recompute_context_matches();
    }

    pub fn context_query(&self) -> Option<&str> {
        (self.mode == Mode::Context).then_some(self.context_query.as_str())
    }

    pub fn context_match_count(&self) -> usize {
        self.context_matches.len()
    }

    pub fn context_selected(&self) -> usize {
        self.context_selected
    }

    pub fn context_item(&self, index: usize) -> Option<&str> {
        let candidate = *self.context_matches.get(index)?;
        self.context_files.get(candidate).map(String::as_str)
    }

    pub fn context_indexing(&self) -> bool {
        self.context_indexing
    }

    pub fn take_context_preview_request(&mut self) -> Option<String> {
        self.context_preview_request.take()
    }

    pub fn apply_context_preview(&mut self, path: &str, text: &str, truncated: bool) -> bool {
        if self.mode != Mode::ContextPreview || self.context_preview_path.as_deref() != Some(path) {
            return false;
        }
        self.context_preview_lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.lines().map(ToOwned::to_owned).collect()
        };
        self.context_preview_cursor = 0;
        self.context_preview_anchor = None;
        self.context_preview_error = None;
        self.context_preview_truncated = truncated;
        true
    }

    pub fn fail_context_preview(&mut self, path: &str, message: String) -> bool {
        if self.mode != Mode::ContextPreview || self.context_preview_path.as_deref() != Some(path) {
            return false;
        }
        self.context_preview_lines.clear();
        self.context_preview_error = Some(message);
        self.context_preview_truncated = false;
        true
    }

    pub fn context_preview_path(&self) -> Option<&str> {
        (self.mode == Mode::ContextPreview)
            .then_some(self.context_preview_path.as_deref())
            .flatten()
    }

    pub fn context_preview_line_count(&self) -> usize {
        self.context_preview_lines.len()
    }

    pub fn context_preview_line(&self, index: usize) -> Option<&str> {
        self.context_preview_lines.get(index).map(String::as_str)
    }

    pub fn context_preview_cursor(&self) -> usize {
        self.context_preview_cursor
    }

    pub fn context_preview_selection(&self) -> Option<(usize, usize)> {
        (self.mode == Mode::ContextPreview && !self.context_preview_lines.is_empty()).then(|| {
            let anchor = self
                .context_preview_anchor
                .unwrap_or(self.context_preview_cursor);
            (
                min(anchor, self.context_preview_cursor),
                max(anchor, self.context_preview_cursor),
            )
        })
    }

    pub fn context_preview_error(&self) -> Option<&str> {
        self.context_preview_error.as_deref()
    }

    pub fn context_preview_truncated(&self) -> bool {
        self.context_preview_truncated
    }

    pub fn set_history(&mut self, workspace: Vec<String>, global: Vec<String>) {
        self.history_workspace = workspace;
        self.history_global = global;
        self.recompute_history_matches();
    }

    pub fn history_query(&self) -> Option<(&str, char)> {
        (self.mode == Mode::History).then_some((
            self.history_query.as_str(),
            match self.history_scope {
                HistoryScope::Workspace => 'w',
                HistoryScope::Global => 'a',
            },
        ))
    }

    pub fn history_match_count(&self) -> usize {
        self.history_matches.len()
    }

    pub fn history_selected(&self) -> usize {
        self.history_selected
    }

    pub fn history_item(&self, index: usize) -> Option<&str> {
        let candidate = *self.history_matches.get(index)?;
        self.active_history().get(candidate).map(String::as_str)
    }

    pub fn prompt(&self) -> Option<(char, &str)> {
        match self.mode {
            Mode::Command => Some((':', self.command.as_str())),
            Mode::SearchForward => Some(('/', self.command.as_str())),
            Mode::SearchBackward => Some(('?', self.command.as_str())),
            Mode::Normal
            | Mode::Insert
            | Mode::Visual
            | Mode::VisualLine
            | Mode::History
            | Mode::Context
            | Mode::ContextPreview => None,
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
            | Mode::SearchBackward
            | Mode::History
            | Mode::Context
            | Mode::ContextPreview => None,
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
        self.ctrl_c_armed = false;
        self.record_input(RepeatEvent {
            mode: self.mode,
            input: RepeatInput::Paste(text.to_owned()),
        });
        self.handle_paste_inner(text);
        self.finish_recording();
    }

    fn handle_paste_inner(&mut self, text: &str) {
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
            Mode::History => {
                self.history_query.push_str(text);
                self.recompute_history_matches();
            }
            Mode::Context => {
                self.context_query.push_str(text);
                self.recompute_context_matches();
                self.request_context_search();
            }
            Mode::ContextPreview => {}
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
                Mode::Normal
                    | Mode::Visual
                    | Mode::VisualLine
                    | Mode::Command
                    | Mode::ContextPreview
            ) || key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER));
        let key = if command_context {
            normalize_command_key(key)
        } else {
            key
        };

        if is_ctrl_c(key) {
            self.handle_ctrl_c();
            return;
        }
        self.ctrl_c_armed = false;
        self.record_input(RepeatEvent {
            mode: self.mode,
            input: RepeatInput::Key(key),
        });
        self.handle_key_inner(key);
        self.finish_recording();
    }

    fn handle_ctrl_c(&mut self) {
        if self.ctrl_c_armed {
            self.outcome = Some(Outcome::Cancel);
            return;
        }

        self.ctrl_c_armed = true;
        self.buffer.commit_group();
        if self.buffer.len_chars() > 0 {
            self.buffer.begin_group();
            self.buffer.replace(0..self.buffer.len_chars(), "");
            self.buffer.commit_group();
        }
        self.buffer.set_cursor(0);
        self.mode = Mode::Normal;
        self.pending = Pending::None;
        self.count = None;
        self.preferred_col = None;
        self.visual_anchor = None;
        self.command.clear();
        self.history_query.clear();
        self.history_selected = 0;
        self.context_query.clear();
        self.context_matches.clear();
        self.context_selected = 0;
        self.context_indexing = false;
        self.context_search_generation = self.context_search_generation.wrapping_add(1);
        self.context_search_request = None;
        self.context_last_search_query = None;
        self.clear_context_preview();
        self.repeat_prefix.clear();
        self.recording = None;
    }

    fn handle_key_inner(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('[') => {
                    if self.mode == Mode::ContextPreview {
                        self.close_context_preview();
                    } else {
                        self.enter_normal();
                    }
                    return;
                }
                KeyCode::Char('r') if self.mode == Mode::Normal => {
                    self.buffer.redo();
                    self.reset_command();
                    return;
                }
                KeyCode::Char('p') if self.mode == Mode::Normal => {
                    self.open_history();
                    return;
                }
                _ => {}
            }
        }

        if matches!(self.pending, Pending::Find { .. }) {
            self.handle_find_target(key);
            return;
        }
        if matches!(self.pending, Pending::TextObject { .. }) {
            self.handle_text_object_target(key);
            return;
        }

        match self.mode {
            Mode::Insert => self.handle_insert(key),
            Mode::Normal => self.handle_normal(key),
            Mode::Visual | Mode::VisualLine => self.handle_visual(key),
            Mode::Command => self.handle_command(key),
            Mode::SearchForward | Mode::SearchBackward => self.handle_search(key),
            Mode::History => self.handle_history(key),
            Mode::Context => self.handle_context(key),
            Mode::ContextPreview => self.handle_context_preview(key),
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

    fn handle_text_object_target(&mut self, key: KeyEvent) {
        let Pending::TextObject {
            around,
            operator,
            count,
        } = self.pending
        else {
            return;
        };
        self.pending = Pending::None;
        self.count = None;

        if key.code == KeyCode::Esc {
            self.enter_normal();
            return;
        }
        let KeyCode::Char(target) = key.code else {
            return;
        };
        let range = match target {
            'w' => self.word_text_object(around, count),
            '"' | '\'' | '`' => self.quote_text_object(target, around),
            '(' | ')' | 'b' => self.parenthesis_text_object(around, count),
            _ => None,
        };
        let Some(range) = range else {
            return;
        };

        if let Some(operator) = operator {
            self.apply_operator(operator, range);
        } else {
            self.select_range(range);
        }
    }

    fn handle_insert(&mut self, key: KeyEvent) {
        let command = key.modifiers.contains(KeyModifiers::SUPER);
        let option = key.modifiers.contains(KeyModifiers::ALT);
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.enter_normal(),
            KeyCode::Backspace if command => self.clear_buffer(),
            KeyCode::Backspace if option => self.delete_previous_word(),
            KeyCode::Delete if command => self.delete_to_line_end(),
            KeyCode::Delete if option => self.delete_next_word(),
            KeyCode::Left if command => self.move_line_start(false),
            KeyCode::Right if command => self.move_line_end(true),
            KeyCode::Up if command => self.buffer.set_cursor(0),
            KeyCode::Down if command => self.buffer.set_cursor(self.buffer.len_chars()),
            KeyCode::Left if option => self.move_to_previous_word(),
            KeyCode::Right if option => self.move_to_next_word(),
            KeyCode::Char('a') if control => self.move_line_start(false),
            KeyCode::Char('e') if control => self.move_line_end(true),
            KeyCode::Char('u') if control => self.clear_buffer(),
            KeyCode::Char('w') if control => self.delete_previous_word(),
            KeyCode::Char('k') if control => self.delete_to_line_end(),
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
            KeyCode::Char('@')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
                    && self.can_open_context() =>
            {
                self.open_context();
            }
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
                let command = self.command.trim().to_owned();
                match command.as_str() {
                    "q" | "q!" => self.outcome = Some(Outcome::Cancel),
                    "w" | "wq" | "x" => self.outcome = Some(Outcome::Accept),
                    "history" | "his" => self.open_history(),
                    _ => self.enter_normal(),
                }
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                self.command.clear();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.command.clear();
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
                delete_previous_word_from_string(&mut self.command);
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
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                self.command.clear();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.command.clear();
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
                delete_previous_word_from_string(&mut self.command);
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

    fn handle_history(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_history(),
            KeyCode::Enter => self.accept_history(),
            KeyCode::Tab => {
                self.history_scope = match self.history_scope {
                    HistoryScope::Workspace => HistoryScope::Global,
                    HistoryScope::Global => HistoryScope::Workspace,
                };
                self.recompute_history_matches();
            }
            KeyCode::Up => {
                self.history_selected = self.history_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                self.history_selected =
                    (self.history_selected + 1).min(self.history_matches.len().saturating_sub(1));
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                self.history_query.clear();
                self.recompute_history_matches();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.history_query.clear();
                self.recompute_history_matches();
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
                delete_previous_word_from_string(&mut self.history_query);
                self.recompute_history_matches();
            }
            KeyCode::Backspace => {
                self.history_query.pop();
                self.recompute_history_matches();
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.history_selected = self.history_selected.saturating_sub(1);
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.history_selected =
                    (self.history_selected + 1).min(self.history_matches.len().saturating_sub(1));
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.history_query.push(ch);
                self.recompute_history_matches();
            }
            _ => {}
        }
    }

    fn handle_context(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_context(true),
            KeyCode::Enter => self.accept_context(),
            KeyCode::Tab => self.attach_selected_context_file(),
            KeyCode::Up => {
                self.context_selected = self.context_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                self.context_selected =
                    (self.context_selected + 1).min(self.context_matches.len().saturating_sub(1));
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
                self.context_query.clear();
                self.recompute_context_matches();
                self.request_context_search();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.context_query.clear();
                self.recompute_context_matches();
                self.request_context_search();
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {
                delete_previous_word_from_string(&mut self.context_query);
                self.recompute_context_matches();
                self.request_context_search();
            }
            KeyCode::Backspace if self.context_query.is_empty() => self.close_context(false),
            KeyCode::Backspace => {
                self.context_query.pop();
                self.recompute_context_matches();
                self.request_context_search();
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.context_selected = self.context_selected.saturating_sub(1);
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.context_selected =
                    (self.context_selected + 1).min(self.context_matches.len().saturating_sub(1));
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
            {
                self.context_query.push(ch);
                self.recompute_context_matches();
                self.request_context_search();
            }
            _ => {}
        }
    }

    fn handle_context_preview(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => self.close_context_preview(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.context_preview_cursor = self.context_preview_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.context_preview_cursor = (self.context_preview_cursor + 1)
                    .min(self.context_preview_lines.len().saturating_sub(1));
            }
            KeyCode::Home | KeyCode::Char('g') => self.context_preview_cursor = 0,
            KeyCode::End | KeyCode::Char('G') => {
                self.context_preview_cursor = self.context_preview_lines.len().saturating_sub(1);
            }
            KeyCode::Char('v') => {
                if self.context_preview_lines.is_empty() {
                    return;
                }
                self.context_preview_anchor = if self.context_preview_anchor.is_some() {
                    None
                } else {
                    Some(self.context_preview_cursor)
                };
            }
            KeyCode::Enter => self.attach_context_preview_range(),
            KeyCode::Tab | KeyCode::Char('a') => self.attach_context_preview_whole(),
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
            KeyCode::Char('.') => self.repeat_last_change(count.unwrap_or(1)),
            _ => {}
        }
    }

    fn handle_visual(&mut self, key: KeyEvent) {
        if let KeyCode::Char(ch @ '1'..='9') = key.code {
            self.push_count(ch);
            return;
        }
        if key.code == KeyCode::Char('0') && self.count.is_some() {
            self.push_count('0');
            return;
        }

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
            KeyCode::Char('i') => self.start_text_object(false, None, count),
            KeyCode::Char('a') => self.start_text_object(true, None, count),
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
            KeyCode::Char('i') => {
                self.start_text_object(false, Some(operator), count);
                return false;
            }
            KeyCode::Char('a') => {
                self.start_text_object(true, Some(operator), count);
                return false;
            }
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
        if start == end {
            if operator == Operator::Change {
                self.buffer.set_cursor(start);
                self.enter_insert();
            }
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

    fn clear_buffer(&mut self) {
        if self.buffer.len_chars() > 0 {
            self.buffer.replace(0..self.buffer.len_chars(), "");
        }
        self.buffer.set_cursor(0);
    }

    fn delete_to_line_end(&mut self) {
        let cursor = self.buffer.cursor();
        let end = self.buffer.line_end(self.buffer.current_line());
        if cursor < end {
            self.buffer.delete(cursor..end);
        }
    }

    fn delete_previous_word(&mut self) {
        let cursor = self.buffer.cursor();
        let start = self.buffer.prev_word_start(cursor, 1);
        if start < cursor {
            self.buffer.delete(start..cursor);
        }
    }

    fn delete_next_word(&mut self) {
        let cursor = self.buffer.cursor();
        let mut end = cursor;
        while end < self.buffer.len_chars()
            && self.buffer.char_at(end).is_some_and(char::is_whitespace)
        {
            end += 1;
        }
        if let Some(ch) = self.buffer.char_at(end) {
            let class = text_object_class(ch);
            while end < self.buffer.len_chars()
                && self.buffer.char_at(end).map(text_object_class) == Some(class)
            {
                end += 1;
            }
        }
        if cursor < end {
            self.buffer.delete(cursor..end);
        }
    }

    fn move_to_previous_word(&mut self) {
        let cursor = self.buffer.prev_word_start(self.buffer.cursor(), 1);
        self.buffer.set_cursor(cursor);
        self.preferred_col = None;
    }

    fn move_to_next_word(&mut self) {
        let cursor = self.buffer.next_word_start(self.buffer.cursor(), 1);
        self.buffer.set_cursor(cursor);
        self.preferred_col = None;
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

    fn can_open_context(&self) -> bool {
        let cursor = self.buffer.cursor();
        cursor == 0
            || self
                .buffer
                .char_at(cursor - 1)
                .is_some_and(char::is_whitespace)
    }

    fn open_context(&mut self) {
        if let Some(recording) = &mut self.recording
            && recording.events.last().is_some_and(|event| {
                event.mode == Mode::Insert
                    && matches!(
                        &event.input,
                        RepeatInput::Key(KeyEvent {
                            code: KeyCode::Char('@'),
                            ..
                        })
                    )
            })
        {
            recording.events.pop();
        }
        self.mode = Mode::Context;
        self.context_query.clear();
        self.context_last_search_query = None;
        if self.context_search_enabled {
            self.context_files.clear();
            self.context_matches.clear();
            self.request_context_search();
        } else {
            self.recompute_context_matches();
        }
    }

    fn close_context(&mut self, keep_literal: bool) {
        let literal = keep_literal.then(|| format!("@{}", self.context_query));
        self.mode = Mode::Insert;
        self.context_query.clear();
        self.context_selected = 0;
        self.clear_context_preview();
        if let Some(literal) = literal {
            self.buffer.insert(&literal);
            self.record_context_insertion(literal);
        }
    }

    fn accept_context(&mut self) {
        let range = parse_context_query(&self.context_query).range;
        if matches!(range, ContextRange::Incomplete | ContextRange::Invalid) {
            return;
        }
        let Some(path) = self
            .context_item(self.context_selected)
            .map(ToOwned::to_owned)
        else {
            if !self.context_indexing {
                self.close_context(true);
            }
            return;
        };
        if path.ends_with('/') {
            self.attach_context_reference(&path, None);
            return;
        }
        if let ContextRange::Valid { start, end } = range {
            self.attach_context_reference(&path, Some((start, end)));
            return;
        }
        self.open_context_preview(path);
    }

    fn attach_selected_context_file(&mut self) {
        if let Some(path) = self
            .context_item(self.context_selected)
            .map(ToOwned::to_owned)
        {
            self.attach_context_reference(&path, None);
        }
    }

    fn open_context_preview(&mut self, path: String) {
        self.mode = Mode::ContextPreview;
        self.context_preview_path = Some(path.clone());
        self.context_preview_lines.clear();
        self.context_preview_cursor = 0;
        self.context_preview_anchor = None;
        self.context_preview_request = Some(path);
        self.context_preview_error = None;
        self.context_preview_truncated = false;
    }

    fn close_context_preview(&mut self) {
        self.clear_context_preview();
        self.mode = Mode::Context;
    }

    fn clear_context_preview(&mut self) {
        self.context_preview_path = None;
        self.context_preview_lines.clear();
        self.context_preview_cursor = 0;
        self.context_preview_anchor = None;
        self.context_preview_request = None;
        self.context_preview_error = None;
        self.context_preview_truncated = false;
    }

    fn attach_context_preview_range(&mut self) {
        let Some(path) = self.context_preview_path.clone() else {
            return;
        };
        let Some((start, end)) = self.context_preview_selection() else {
            return;
        };
        self.attach_context_reference(&path, Some((start + 1, end + 1)));
    }

    fn attach_context_preview_whole(&mut self) {
        let Some(path) = self.context_preview_path.clone() else {
            return;
        };
        self.attach_context_reference(&path, None);
    }

    fn attach_context_reference(&mut self, path: &str, range: Option<(usize, usize)>) {
        let reference = context_reference(path, range);
        self.mode = Mode::Insert;
        self.context_query.clear();
        self.context_selected = 0;
        self.clear_context_preview();
        self.buffer.insert(&reference);
        self.record_context_insertion(reference);
    }

    fn record_context_insertion(&mut self, text: String) {
        if let Some(recording) = &mut self.recording {
            recording.events.push(RepeatEvent {
                mode: Mode::Insert,
                input: RepeatInput::Paste(text),
            });
        }
    }

    fn request_context_search(&mut self) {
        if !self.context_search_enabled {
            return;
        }
        let query = parse_context_query(&self.context_query).path.to_owned();
        if self.context_last_search_query.as_deref() == Some(query.as_str()) {
            return;
        }
        self.context_last_search_query = Some(query.clone());
        self.context_search_generation = self.context_search_generation.wrapping_add(1);
        self.context_search_request = Some((self.context_search_generation, query));
        self.context_indexing = true;
    }

    fn recompute_context_matches(&mut self) {
        let query = parse_context_query(&self.context_query).path;
        if query.is_empty() {
            self.context_matches = (0..self.context_files.len()).collect();
            self.context_selected = 0;
            return;
        }
        let query = query.strip_prefix("./").unwrap_or(query);
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buffer = Vec::new();
        let matcher = &mut self.context_matcher;
        let mut matches: Vec<(usize, u32)> = self
            .context_files
            .iter()
            .enumerate()
            .filter_map(|(index, path)| {
                pattern
                    .score(Utf32Str::new(path, &mut buffer), matcher)
                    .map(|score| (index, score))
            })
            .collect();
        matches.sort_unstable_by(|(left_index, left_score), (right_index, right_score)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_index.cmp(right_index))
        });
        self.context_matches = matches.into_iter().map(|(index, _)| index).collect();
        self.context_selected = 0;
    }

    fn open_history(&mut self) {
        self.buffer.commit_group();
        self.history_return_mode = Mode::Normal;
        self.mode = Mode::History;
        self.history_scope = HistoryScope::Workspace;
        self.history_query.clear();
        self.recompute_history_matches();
        self.reset_command();
    }

    fn close_history(&mut self) {
        self.mode = self.history_return_mode;
        self.history_query.clear();
        self.history_selected = 0;
        self.reset_command();
        self.clamp_normal_cursor();
    }

    fn accept_history(&mut self) {
        let Some(text) = self
            .history_item(self.history_selected)
            .map(ToOwned::to_owned)
        else {
            return;
        };
        self.buffer.begin_group();
        self.buffer.replace(0..self.buffer.len_chars(), &text);
        self.buffer.set_cursor(self.buffer.len_chars());
        self.buffer.commit_group();
        self.close_history();
    }

    fn active_history(&self) -> &[String] {
        match self.history_scope {
            HistoryScope::Workspace => &self.history_workspace,
            HistoryScope::Global => &self.history_global,
        }
    }

    fn recompute_history_matches(&mut self) {
        let mut matches: Vec<(usize, i64)> = self
            .active_history()
            .iter()
            .enumerate()
            .filter_map(|(index, text)| {
                fuzzy_score(text, &self.history_query).map(|score| (index, score))
            })
            .collect();
        matches.sort_unstable_by(|(left_index, left_score), (right_index, right_score)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_index.cmp(right_index))
        });
        self.history_matches = matches.into_iter().map(|(index, _)| index).collect();
        self.history_selected = 0;
    }

    fn record_input(&mut self, event: RepeatEvent) {
        if self.replaying || matches!(event.mode, Mode::Context | Mode::ContextPreview) {
            return;
        }
        if let Some(recording) = &mut self.recording {
            recording.events.push(event);
            return;
        }
        if event.mode != Mode::Normal || self.pending != Pending::None {
            self.repeat_prefix.clear();
            return;
        }

        match &event.input {
            RepeatInput::Key(key) if self.is_repeat_count_key(*key) => {
                self.repeat_prefix.push(event);
            }
            RepeatInput::Key(key) if is_change_start(*key) => {
                let mut events = std::mem::take(&mut self.repeat_prefix);
                events.push(event);
                self.recording = Some(ChangeRecording {
                    events,
                    revision_before: self.buffer.revision(),
                });
            }
            RepeatInput::Paste(_) => {
                self.repeat_prefix.clear();
                self.recording = Some(ChangeRecording {
                    events: vec![event],
                    revision_before: self.buffer.revision(),
                });
            }
            RepeatInput::Key(_) => self.repeat_prefix.clear(),
        }
    }

    fn is_repeat_count_key(&self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return false;
        }
        match key.code {
            KeyCode::Char('1'..='9') => true,
            KeyCode::Char('0') => self.count.is_some() || !self.repeat_prefix.is_empty(),
            _ => false,
        }
    }

    fn finish_recording(&mut self) {
        if self.replaying
            || self.recording.is_none()
            || self.mode != Mode::Normal
            || self.pending != Pending::None
        {
            return;
        }
        let recording = self.recording.take().expect("recording is present");
        if self.buffer.revision() != recording.revision_before {
            self.last_change = recording.events;
        }
    }

    fn repeat_last_change(&mut self, count: usize) {
        if self.last_change.is_empty() || self.replaying {
            return;
        }
        let events = self.last_change.clone();
        self.replaying = true;
        'repeat: for _ in 0..count.max(1) {
            for event in &events {
                if self.mode != event.mode {
                    break 'repeat;
                }
                match &event.input {
                    RepeatInput::Key(key) => self.handle_key_inner(*key),
                    RepeatInput::Paste(text) => self.handle_paste_inner(text),
                }
            }
        }
        self.replaying = false;
        self.clamp_normal_cursor();
    }

    fn start_text_object(&mut self, around: bool, operator: Option<Operator>, count: usize) {
        self.pending = Pending::TextObject {
            around,
            operator,
            count: count.max(1),
        };
    }

    fn select_range(&mut self, range: Range<usize>) {
        let start = range.start.min(self.buffer.len_chars());
        let end = range.end.min(self.buffer.len_chars()).max(start);
        if start == end {
            return;
        }
        self.buffer.commit_group();
        self.mode = Mode::Visual;
        self.visual_anchor = Some(start);
        self.buffer.set_cursor(self.buffer.prev_grapheme(end));
        self.preferred_col = None;
        self.reset_command();
    }

    fn word_text_object(&self, around: bool, count: usize) -> Option<Range<usize>> {
        let len = self.buffer.len_chars();
        if len == 0 {
            return None;
        }

        let cursor = self.buffer.cursor().min(len - 1);
        let class = self.buffer.char_at(cursor).map(text_object_class)?;
        let mut start = cursor;
        while start > 0 && self.buffer.char_at(start - 1).map(text_object_class) == Some(class) {
            start -= 1;
        }
        let mut end = cursor + 1;
        while end < len && self.buffer.char_at(end).map(text_object_class) == Some(class) {
            end += 1;
        }

        if class == 0 {
            let additions = if around {
                count.max(1)
            } else {
                count.saturating_sub(1)
            };
            let mut added = 0;
            for _ in 0..additions {
                let mut next = end;
                while next < len && self.buffer.char_at(next).is_some_and(char::is_whitespace) {
                    next += 1;
                }
                if next >= len {
                    break;
                }
                let next_class = self.buffer.char_at(next).map(text_object_class)?;
                end = next + 1;
                while end < len
                    && self.buffer.char_at(end).map(text_object_class) == Some(next_class)
                {
                    end += 1;
                }
                added += 1;
            }
            if around && added == 0 && start > 0 {
                let previous_class = self.buffer.char_at(start - 1).map(text_object_class)?;
                while start > 0
                    && self.buffer.char_at(start - 1).map(text_object_class) == Some(previous_class)
                {
                    start -= 1;
                }
            }
            return Some(start..end);
        }

        for _ in 1..count.max(1) {
            let mut next = end;
            while next < len && self.buffer.char_at(next).is_some_and(char::is_whitespace) {
                next += 1;
            }
            if next >= len {
                break;
            }
            let next_class = self.buffer.char_at(next).map(text_object_class)?;
            end = next + 1;
            while end < len && self.buffer.char_at(end).map(text_object_class) == Some(next_class) {
                end += 1;
            }
        }

        if around {
            let mut trailing = end;
            while trailing < len
                && self
                    .buffer
                    .char_at(trailing)
                    .is_some_and(char::is_whitespace)
            {
                trailing += 1;
            }
            if trailing > end {
                end = trailing;
            } else {
                while start > 0
                    && self
                        .buffer
                        .char_at(start - 1)
                        .is_some_and(char::is_whitespace)
                {
                    start -= 1;
                }
            }
        }

        Some(start..end)
    }

    fn quote_text_object(&self, quote: char, around: bool) -> Option<Range<usize>> {
        let cursor = self.buffer.cursor();
        let line = self.buffer.current_line();
        let start = self.buffer.line_start(line);
        let end = self.buffer.line_end(line);
        let positions: Vec<usize> = (start..end)
            .filter(|index| self.buffer.char_at(*index) == Some(quote) && !self.is_escaped(*index))
            .collect();
        let pair = positions
            .chunks_exact(2)
            .find(|pair| pair[0] <= cursor && cursor <= pair[1])?;

        if around {
            Some(pair[0]..self.buffer.next_grapheme(pair[1]))
        } else {
            Some(self.buffer.next_grapheme(pair[0])..pair[1])
        }
    }

    fn is_escaped(&self, index: usize) -> bool {
        let mut backslashes = 0;
        let mut cursor = index;
        while cursor > 0 && self.buffer.char_at(cursor - 1) == Some('\\') {
            backslashes += 1;
            cursor -= 1;
        }
        backslashes % 2 == 1
    }

    fn parenthesis_text_object(&self, around: bool, count: usize) -> Option<Range<usize>> {
        let cursor = self.buffer.cursor();
        let mut stack = Vec::new();
        let mut containing = Vec::new();
        for index in 0..self.buffer.len_chars() {
            match self.buffer.char_at(index) {
                Some('(') => stack.push(index),
                Some(')') => {
                    if let Some(open) = stack.pop()
                        && open <= cursor
                        && cursor <= index
                    {
                        containing.push((open, index));
                    }
                }
                _ => {}
            }
        }
        containing.sort_unstable_by_key(|(open, close)| close - open);
        let (open, close) = containing.get(count.saturating_sub(1)).copied()?;

        if around {
            Some(open..self.buffer.next_grapheme(close))
        } else {
            Some(self.buffer.next_grapheme(open)..close)
        }
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
        let was_insert = matches!(self.mode, Mode::Insert | Mode::Context);
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

fn is_ctrl_c(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn parse_context_query(query: &str) -> ParsedContextQuery<'_> {
    let Some((path, suffix)) = query.rsplit_once(':') else {
        return ParsedContextQuery {
            path: query,
            range: ContextRange::None,
        };
    };
    if path.is_empty() || !suffix.chars().all(|ch| ch.is_ascii_digit() || ch == '-') {
        return ParsedContextQuery {
            path: query,
            range: ContextRange::None,
        };
    }
    if suffix.is_empty() {
        return ParsedContextQuery {
            path,
            range: ContextRange::Incomplete,
        };
    }

    let range = match suffix.split_once('-') {
        None => parse_line_number(suffix)
            .map(|line| ContextRange::Valid {
                start: line,
                end: line,
            })
            .unwrap_or(ContextRange::Invalid),
        Some((start, end)) if !start.is_empty() && end.is_empty() => {
            if parse_line_number(start).is_some() {
                ContextRange::Incomplete
            } else {
                ContextRange::Invalid
            }
        }
        Some((start, end)) if !start.is_empty() && !end.is_empty() && !end.contains('-') => {
            match (parse_line_number(start), parse_line_number(end)) {
                (Some(start), Some(end)) if start <= end => ContextRange::Valid { start, end },
                _ => ContextRange::Invalid,
            }
        }
        Some(_) => ContextRange::Invalid,
    };
    ParsedContextQuery { path, range }
}

fn parse_line_number(value: &str) -> Option<usize> {
    value.parse().ok().filter(|line| *line > 0)
}

fn context_reference(path: &str, range: Option<(usize, usize)>) -> String {
    let path = if path.contains(' ') {
        format!("\"{path}\"")
    } else {
        path.to_owned()
    };
    let range = match range {
        Some((start, end)) if start == end => format!(":{start}"),
        Some((start, end)) => format!(":{start}-{end}"),
        None => String::new(),
    };
    format!("@{path}{range} ")
}

fn delete_previous_word_from_string(input: &mut String) {
    while input.chars().next_back().is_some_and(char::is_whitespace) {
        input.pop();
    }
    let Some(class) = input.chars().next_back().map(text_object_class) else {
        return;
    };
    while input.chars().next_back().map(text_object_class) == Some(class) {
        input.pop();
    }
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    let candidate: Vec<char> = candidate.to_lowercase().chars().collect();
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() {
        return Some(0);
    }

    let mut search_from = 0;
    let mut previous = None;
    let mut score = 0_i64;
    for query_char in query {
        let relative = candidate[search_from..]
            .iter()
            .position(|candidate_char| *candidate_char == query_char)?;
        let index = search_from + relative;
        score += 100;
        if previous.is_some_and(|previous| previous + 1 == index) {
            score += 60;
        }
        if index == 0 || !candidate[index - 1].is_alphanumeric() {
            score += 30;
        }
        score -= relative as i64;
        previous = Some(index);
        search_from = index + 1;
    }
    score -= candidate.len().min(i64::MAX as usize) as i64 / 16;
    Some(score)
}

fn is_change_start(key: KeyEvent) -> bool {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return false;
    }
    matches!(
        key.code,
        KeyCode::Char('i' | 'a' | 'I' | 'A' | 'o' | 'O' | 'd' | 'c' | 'x' | 'p' | 'P')
            | KeyCode::Delete
    )
}

fn text_object_class(ch: char) -> u8 {
    if ch.is_whitespace() {
        0
    } else if ch.is_alphanumeric() || ch == '_' {
        1
    } else {
        2
    }
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
    fn mac_command_backspace_clears_the_buffer_and_can_be_undone() {
        let mut editor = Editor::new("first\nsecond");
        editor.buffer.set_cursor(editor.buffer.len_chars());
        editor.handle_key(key('i'));
        editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER));
        assert_eq!(editor.buffer.as_string(), "");
        assert_eq!(editor.buffer.cursor(), 0);

        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        editor.handle_key(key('u'));
        assert_eq!(editor.buffer.as_string(), "first\nsecond");
    }

    #[test]
    fn option_and_shell_shortcuts_edit_words_and_lines() {
        let mut editor = Editor::new("alpha beta gamma");
        editor.buffer.set_cursor(editor.buffer.len_chars());
        editor.handle_key(key('i'));
        editor.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT));
        assert_eq!(editor.buffer.as_string(), "alpha beta ");
        editor.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(editor.buffer.as_string(), "alpha ");
        editor.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(editor.buffer.as_string(), "");

        let mut forward = Editor::new("one two");
        forward.handle_key(key('i'));
        forward.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::ALT));
        assert_eq!(forward.buffer.as_string(), " two");

        let mut multiline = Editor::new("one\ntwo");
        multiline.buffer.set_cursor(multiline.buffer.len_chars());
        multiline.handle_key(key('i'));
        multiline.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(multiline.buffer.as_string(), "");
    }

    #[test]
    fn command_and_option_arrows_use_mac_navigation() {
        let mut editor = Editor::new("one two\nthree four");
        editor.buffer.set_cursor(editor.buffer.len_chars());
        editor.handle_key(key('i'));
        editor.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SUPER));
        assert_eq!(editor.buffer.cursor(), 0);
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(editor.buffer.cursor(), 4);
        editor.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SUPER));
        assert_eq!(editor.buffer.cursor(), 7);
        editor.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SUPER));
        assert_eq!(editor.buffer.cursor(), editor.buffer.len_chars());
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
    fn first_ctrl_c_clears_and_second_consecutive_ctrl_c_cancels() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut editor = Editor::new("draft");

        editor.handle_key(ctrl_c);
        assert_eq!(editor.buffer.as_string(), "");
        assert_eq!(editor.mode(), Mode::Normal);
        assert_eq!(editor.outcome(), None);

        editor.handle_key(ctrl_c);
        assert_eq!(editor.outcome(), Some(Outcome::Cancel));
    }

    #[test]
    fn other_input_disarms_double_ctrl_c_exit() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut editor = Editor::new("draft");

        editor.handle_key(ctrl_c);
        editor.handle_key(key('u'));
        assert_eq!(editor.buffer.as_string(), "draft");
        editor.handle_key(ctrl_c);

        assert_eq!(editor.buffer.as_string(), "");
        assert_eq!(editor.outcome(), None);
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
    fn context_search_is_requested_only_when_the_picker_opens() {
        let mut editor = Editor::new("");
        editor.enable_context_search();
        assert_eq!(editor.take_context_search_request(), None);
        editor.handle_key(key('i'));
        assert_eq!(editor.take_context_search_request(), None);
        editor.handle_key(key('@'));
        assert_eq!(
            editor.take_context_search_request(),
            Some((1, String::new()))
        );
        assert_eq!(editor.take_context_search_request(), None);
        assert!(editor.context_indexing());
    }

    #[test]
    fn context_picker_does_not_accept_before_search_finishes() {
        let mut editor = Editor::new("");
        editor.enable_context_search();
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        editor.handle_key(key('m'));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.mode(), Mode::Context);
        assert_eq!(editor.buffer.as_string(), "");
    }

    #[test]
    fn context_picker_can_accept_a_directory_from_a_partial_result() {
        let mut editor = Editor::new("");
        editor.enable_context_search();
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        editor.take_context_search_request();
        for ch in "src".chars() {
            editor.handle_key(key(ch));
        }
        let (generation, _) = editor.take_context_search_request().unwrap();
        assert!(editor.apply_context_search_result(generation, vec!["src/".to_owned()], false));
        assert!(editor.context_indexing());
        assert_eq!(editor.context_item(0), Some("src/"));

        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.mode(), Mode::Insert);
        assert_eq!(editor.buffer.as_string(), "@src/ ");
    }

    #[test]
    fn context_search_ignores_stale_results() {
        let mut editor = Editor::new("");
        editor.enable_context_search();
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        let (first_generation, _) = editor.take_context_search_request().unwrap();
        editor.handle_key(key('m'));
        let (current_generation, query) = editor.take_context_search_request().unwrap();
        assert_eq!(query, "m");

        assert!(!editor.apply_context_search_result(
            first_generation,
            vec!["stale.rs".to_owned()],
            true
        ));
        assert!(editor.context_indexing());
        assert!(editor.apply_context_search_result(
            current_generation,
            vec!["main.rs".to_owned()],
            true
        ));
        assert_eq!(editor.context_item(0), Some("main.rs"));
        assert!(!editor.context_indexing());
    }

    #[test]
    fn context_picker_fuzzy_selects_workspace_files() {
        let mut editor = Editor::new("");
        editor.set_context_files(vec![
            "README.md".to_owned(),
            "src/buffer.rs".to_owned(),
            "src/main.rs".to_owned(),
        ]);
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        assert_eq!(editor.mode(), Mode::Context);
        editor.handle_key(key('m'));
        editor.handle_key(key('r'));
        assert_eq!(editor.context_item(0), Some("src/main.rs"));
        editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(editor.mode(), Mode::Insert);
        assert_eq!(editor.buffer.as_string(), "@src/main.rs ");
    }

    #[test]
    fn context_preview_selects_an_inclusive_line_range() {
        let mut editor = Editor::new("");
        editor.set_context_files(vec!["src/main.rs".to_owned()]);
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.mode(), Mode::ContextPreview);
        assert_eq!(
            editor.take_context_preview_request(),
            Some("src/main.rs".to_owned())
        );
        assert!(editor.apply_context_preview("src/main.rs", "one\ntwo\nthree\nfour\n", false));

        editor.handle_key(key('j'));
        editor.handle_key(key('v'));
        editor.handle_key(key('j'));
        editor.handle_key(key('j'));
        assert_eq!(editor.context_preview_selection(), Some((1, 3)));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert_eq!(editor.mode(), Mode::Insert);
        assert_eq!(editor.buffer.as_string(), "@src/main.rs:2-4 ");
    }

    #[test]
    fn context_preview_can_return_to_files_or_attach_the_whole_file() {
        let mut editor = Editor::new("");
        editor.set_context_files(vec!["src/main.rs".to_owned()]);
        editor.handle_key(key('i'));
        for ch in "@main".chars() {
            editor.handle_key(key(ch));
        }
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(editor.mode(), Mode::Context);
        assert_eq!(editor.context_query(), Some("main"));

        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        editor.handle_key(key('a'));
        assert_eq!(editor.mode(), Mode::Insert);
        assert_eq!(editor.buffer.as_string(), "@src/main.rs ");
    }

    #[test]
    fn context_picker_inserts_single_lines_and_line_ranges() {
        let mut single = Editor::new("");
        single.set_context_files(vec!["src/main.rs".to_owned()]);
        single.handle_key(key('i'));
        for ch in "@src/main.rs:10".chars() {
            single.handle_key(key(ch));
        }
        single.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(single.buffer.as_string(), "@src/main.rs:10 ");

        let mut range = Editor::new("");
        range.set_context_files(vec!["docs/my file.md".to_owned()]);
        range.handle_key(key('i'));
        for ch in "@docs/my file.md:10-40".chars() {
            range.handle_key(key(ch));
        }
        range.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(range.buffer.as_string(), "@\"docs/my file.md\":10-40 ");
    }

    #[test]
    fn incomplete_or_invalid_line_ranges_are_not_accepted() {
        for query in ["@src/main.rs:", "@src/main.rs:10-", "@src/main.rs:40-10"] {
            let mut editor = Editor::new("");
            editor.set_context_files(vec!["src/main.rs".to_owned()]);
            editor.handle_key(key('i'));
            for ch in query.chars() {
                editor.handle_key(key(ch));
            }
            editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert_eq!(editor.mode(), Mode::Context);
            assert_eq!(editor.buffer.as_string(), "");
        }
    }

    #[test]
    fn line_range_edits_reuse_the_existing_file_search() {
        let mut editor = Editor::new("");
        editor.enable_context_search();
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        editor.take_context_search_request();
        for ch in "src/main.rs".chars() {
            editor.handle_key(key(ch));
        }
        let (_, query) = editor.take_context_search_request().unwrap();
        assert_eq!(query, "src/main.rs");

        for ch in ":10-40".chars() {
            editor.handle_key(key(ch));
        }
        assert_eq!(editor.take_context_search_request(), None);
    }

    #[test]
    fn context_picker_quotes_spaces_and_preserves_cancelled_literals() {
        let mut selected = Editor::new("");
        selected.set_context_files(vec!["docs/my file.md".to_owned()]);
        selected.handle_key(key('i'));
        selected.handle_key(key('@'));
        selected.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(selected.buffer.as_string(), "@\"docs/my file.md\" ");

        let mut cancelled = Editor::new("");
        cancelled.handle_key(key('i'));
        cancelled.handle_key(key('@'));
        cancelled.handle_key(key('u'));
        cancelled.handle_key(key('s'));
        cancelled.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(cancelled.mode(), Mode::Insert);
        assert_eq!(cancelled.buffer.as_string(), "@us");
    }

    #[test]
    fn context_trigger_requires_a_token_boundary() {
        let mut editor = Editor::new("mail");
        editor.buffer.set_cursor(editor.buffer.len_chars());
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        assert_eq!(editor.mode(), Mode::Insert);
        assert_eq!(editor.buffer.as_string(), "mail@");
    }

    #[test]
    fn dot_repeats_the_selected_context_reference_without_reopening_picker() {
        let mut editor = Editor::new("one\ntwo");
        editor.set_context_files(vec!["src/main.rs".to_owned()]);
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        let second_line = editor.buffer.line_start(1);
        editor.buffer.set_cursor(second_line);
        editor.handle_key(key('.'));
        assert_eq!(
            editor.buffer.as_string(),
            "@src/main.rs one\n@src/main.rs two"
        );
        assert_eq!(editor.mode(), Mode::Normal);
    }

    #[test]
    fn history_picker_fuzzy_filters_and_replaces_the_buffer() {
        let mut editor = Editor::new("current draft");
        editor.set_history(
            vec![
                "recent prompt".to_owned(),
                "fix login bug".to_owned(),
                "older prompt".to_owned(),
            ],
            vec!["global prompt".to_owned()],
        );

        editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(editor.mode(), Mode::History);
        for ch in "flb".chars() {
            editor.handle_key(key(ch));
        }
        assert_eq!(editor.history_match_count(), 1);
        assert_eq!(editor.history_item(0), Some("fix login bug"));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(editor.mode(), Mode::Normal);
        assert_eq!(editor.buffer.as_string(), "fix login bug");

        editor.handle_key(key('u'));
        assert_eq!(editor.buffer.as_string(), "current draft");
    }

    #[test]
    fn history_picker_toggles_global_scope_and_can_cancel() {
        let mut editor = Editor::new("draft");
        editor.set_history(
            vec!["workspace prompt".to_owned()],
            vec!["global prompt".to_owned()],
        );
        editor.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(editor.history_item(0), Some("workspace prompt"));
        editor.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(editor.history_item(0), Some("global prompt"));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(editor.buffer.as_string(), "draft");
        assert_eq!(editor.mode(), Mode::Normal);
    }

    #[test]
    fn dot_repeats_insert_sessions_at_the_current_cursor() {
        let mut editor = Editor::new("a b");
        editor.handle_key(key('i'));
        editor.handle_key(key('X'));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        editor.handle_key(key('w'));
        editor.handle_key(key('.'));
        assert_eq!(editor.buffer.as_string(), "Xa Xb");
    }

    #[test]
    fn dot_repeats_operators_text_objects_and_counts() {
        let mut words = Editor::new("one two three");
        words.handle_key(key('d'));
        words.handle_key(key('a'));
        words.handle_key(key('w'));
        words.handle_key(key('.'));
        assert_eq!(words.buffer.as_string(), "three");

        let mut chars = Editor::new("abcdef");
        chars.handle_key(key('2'));
        chars.handle_key(key('x'));
        chars.handle_key(key('.'));
        assert_eq!(chars.buffer.as_string(), "ef");
    }

    #[test]
    fn dot_count_repeats_the_last_change_multiple_times() {
        let mut editor = Editor::new("abcdef");
        editor.handle_key(key('x'));
        editor.handle_key(key('2'));
        editor.handle_key(key('.'));
        assert_eq!(editor.buffer.as_string(), "def");
    }

    #[test]
    fn failed_dot_motion_does_not_leak_insert_keys_into_normal_mode() {
        let mut editor = Editor::new("\"one\"\nplain");
        editor.buffer.set_cursor(1);
        editor.handle_key(key('c'));
        editor.handle_key(key('i'));
        editor.handle_key(key('"'));
        editor.handle_key(key('d'));
        editor.handle_key(key('d'));
        editor.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(editor.buffer.as_string(), "\"dd\"\nplain");

        editor.buffer.set_cursor(5);
        editor.handle_key(key('.'));
        assert_eq!(editor.buffer.as_string(), "\"dd\"\nplain");
    }

    #[test]
    fn normal_mode_paste_can_be_repeated() {
        let mut editor = Editor::new("a");
        editor.handle_paste("X");
        editor.handle_key(key('.'));
        assert_eq!(editor.buffer.as_string(), "XXa");
    }

    #[test]
    fn word_text_objects_support_inner_around_counts_and_visual_mode() {
        let mut inner = Editor::new("one two");
        inner.handle_key(key('d'));
        inner.handle_key(key('i'));
        inner.handle_key(key('w'));
        assert_eq!(inner.buffer.as_string(), " two");

        let mut around = Editor::new("one two");
        around.handle_key(key('d'));
        around.handle_key(key('a'));
        around.handle_key(key('w'));
        assert_eq!(around.buffer.as_string(), "two");

        let mut counted = Editor::new("one two three");
        counted.handle_key(key('2'));
        counted.handle_key(key('d'));
        counted.handle_key(key('a'));
        counted.handle_key(key('w'));
        assert_eq!(counted.buffer.as_string(), "three");

        let mut whitespace = Editor::new("one   two three");
        whitespace.buffer.set_cursor(3);
        whitespace.handle_key(key('d'));
        whitespace.handle_key(key('i'));
        whitespace.handle_key(key('w'));
        assert_eq!(whitespace.buffer.as_string(), "onetwo three");

        let mut around_whitespace = Editor::new("one   two three");
        around_whitespace.buffer.set_cursor(3);
        around_whitespace.handle_key(key('d'));
        around_whitespace.handle_key(key('a'));
        around_whitespace.handle_key(key('w'));
        assert_eq!(around_whitespace.buffer.as_string(), "one three");

        let mut visual = Editor::new("one two");
        visual.handle_key(key('v'));
        visual.handle_key(key('i'));
        visual.handle_key(key('w'));
        visual.handle_key(key('d'));
        assert_eq!(visual.buffer.as_string(), " two");
    }

    #[test]
    fn quote_text_objects_exclude_or_include_delimiters() {
        let mut inner = Editor::new("say \"hello world\" now");
        inner.buffer.set_cursor(7);
        inner.handle_key(key('d'));
        inner.handle_key(key('i'));
        inner.handle_key(key('"'));
        assert_eq!(inner.buffer.as_string(), "say \"\" now");

        let mut around = Editor::new("say \"hello world\" now");
        around.buffer.set_cursor(7);
        around.handle_key(key('d'));
        around.handle_key(key('a'));
        around.handle_key(key('"'));
        assert_eq!(around.buffer.as_string(), "say  now");

        let mut escaped = Editor::new("say \"a \\\"quoted\\\" value\" now");
        escaped.buffer.set_cursor(9);
        escaped.handle_key(key('d'));
        escaped.handle_key(key('i'));
        escaped.handle_key(key('"'));
        assert_eq!(escaped.buffer.as_string(), "say \"\" now");

        let mut empty = Editor::new("\"\"");
        empty.handle_key(key('c'));
        empty.handle_key(key('i'));
        empty.handle_key(key('"'));
        assert_eq!(empty.mode(), Mode::Insert);
        empty.handle_key(key('x'));
        empty.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(empty.buffer.as_string(), "\"x\"");
    }

    #[test]
    fn parenthesis_text_objects_select_nested_pairs() {
        let mut inner = Editor::new("a (b (c) d) e");
        inner.buffer.set_cursor(6);
        inner.handle_key(key('d'));
        inner.handle_key(key('i'));
        inner.handle_key(key('('));
        assert_eq!(inner.buffer.as_string(), "a (b () d) e");

        let mut around = Editor::new("a (b (c) d) e");
        around.buffer.set_cursor(6);
        around.handle_key(key('d'));
        around.handle_key(key('a'));
        around.handle_key(key('('));
        assert_eq!(around.buffer.as_string(), "a (b  d) e");

        let mut outer = Editor::new("a (b (c) d) e");
        outer.buffer.set_cursor(6);
        outer.handle_key(key('2'));
        outer.handle_key(key('d'));
        outer.handle_key(key('i'));
        outer.handle_key(key('('));
        assert_eq!(outer.buffer.as_string(), "a () e");
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
