use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::editor::{Editor, Mode};

const TAB_WIDTH: usize = 4;

#[derive(Debug, Default)]
pub struct ViewState {
    scroll_line: usize,
    scroll_subrow: usize,
    preview_scroll_line: usize,
}

impl ViewState {
    pub fn render(&mut self, frame: &mut Frame<'_>, editor: &Editor) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return;
        }
        if editor.mode() == Mode::History {
            self.render_history(frame, area, editor);
            return;
        }
        if editor.mode() == Mode::Context {
            self.render_context(frame, area, editor);
            return;
        }
        if editor.mode() == Mode::ContextPreview {
            self.render_context_preview(frame, area, editor);
            return;
        }

        let prompt = editor.prompt();
        let command_height = u16::from(prompt.is_some());
        let text_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(command_height),
        );

        if text_area.height > 0 {
            self.render_text(frame, text_area, editor);
        }

        if let Some((prefix, input)) = prompt {
            let command_area = Rect::new(area.x, area.bottom() - 1, area.width, 1);
            let text = format!("{prefix}{input}");
            frame.render_widget(Paragraph::new(text.as_str()), command_area);
            let cursor_x = text
                .chars()
                .count()
                .min(area.width.saturating_sub(1) as usize) as u16;
            frame.set_cursor_position(Position::new(command_area.x + cursor_x, command_area.y));
        }
    }

    fn render_context(&self, frame: &mut Frame<'_>, area: Rect, editor: &Editor) {
        let Some(query) = editor.context_query() else {
            return;
        };
        let list_height = area.height.saturating_sub(1) as usize;
        let selected = editor.context_selected();
        let start = selected.saturating_sub(list_height.saturating_sub(1));
        let mut rows: Vec<Line<'static>> = (start..editor.context_match_count())
            .take(list_height)
            .filter_map(|index| {
                let item = editor.context_item(index)?;
                let style = if index == selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Some(Line::from(Span::styled(item.to_owned(), style)))
            })
            .collect();
        if rows.is_empty() && editor.context_indexing() && list_height > 0 {
            rows.push(Line::from("searching…"));
        }
        if list_height > 0 {
            frame.render_widget(
                Paragraph::new(rows),
                Rect::new(area.x, area.y, area.width, list_height as u16),
            );
        }

        let prompt_area = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        let prompt = format!("@{query}");
        frame.render_widget(Paragraph::new(prompt.as_str()), prompt_area);
        let cursor_x = prompt
            .chars()
            .count()
            .min(area.width.saturating_sub(1) as usize) as u16;
        frame.set_cursor_position(Position::new(prompt_area.x + cursor_x, prompt_area.y));
    }

    fn render_context_preview(&mut self, frame: &mut Frame<'_>, area: Rect, editor: &Editor) {
        let Some(path) = editor.context_preview_path() else {
            return;
        };
        let header = if editor.context_preview_truncated() {
            format!("@{path}  [partial]")
        } else {
            format!("@{path}")
        };
        frame.render_widget(
            Paragraph::new(header),
            Rect::new(area.x, area.y, area.width, 1),
        );
        let body_height = area.height.saturating_sub(1) as usize;
        if body_height == 0 {
            frame.set_cursor_position(Position::new(area.x, area.y));
            return;
        }
        let body = Rect::new(area.x, area.y + 1, area.width, body_height as u16);
        if let Some(error) = editor.context_preview_error() {
            frame.render_widget(Paragraph::new(error.to_owned()), body);
            frame.set_cursor_position(Position::new(body.x, body.y));
            return;
        }
        let line_count = editor.context_preview_line_count();
        if line_count == 0 {
            frame.render_widget(Paragraph::new("loading…"), body);
            frame.set_cursor_position(Position::new(body.x, body.y));
            return;
        }

        let cursor = editor.context_preview_cursor();
        let number_width = line_count.to_string().len().max(1);
        let gutter_width = (number_width + 1).min(area.width as usize);
        let content_width = (area.width as usize).saturating_sub(gutter_width).max(1);
        if cursor < self.preview_scroll_line {
            self.preview_scroll_line = cursor;
        } else {
            let rows_before_cursor = (self.preview_scroll_line..cursor)
                .filter_map(|line| editor.context_preview_line(line))
                .map(|line| wrapped_row_count(line, content_width))
                .sum::<usize>();
            if rows_before_cursor >= body_height {
                self.preview_scroll_line = cursor;
            }
        }
        let selection = editor.context_preview_selection();
        let mut rows = Vec::with_capacity(body_height);
        let mut cursor_y = None;
        let mut line_index = self.preview_scroll_line;
        while rows.len() < body_height && line_index < line_count {
            let text = editor.context_preview_line(line_index).unwrap_or_default();
            let wrapped = wrap_line(text, 0, None, content_width);
            let selected =
                selection.is_some_and(|(start, end)| (start..=end).contains(&line_index));
            for (subrow, row) in wrapped.into_iter().enumerate() {
                if rows.len() >= body_height {
                    break;
                }
                if line_index == cursor && subrow == 0 {
                    cursor_y = Some(rows.len());
                }
                let gutter = if subrow == 0 {
                    format!("{:>number_width$} ", line_index + 1)
                } else {
                    " ".repeat(gutter_width)
                };
                let mut spans = vec![Span::raw(gutter)];
                spans.extend(row.spans);
                let mut line = Line::from(spans);
                if selected {
                    line = line.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                rows.push(line);
            }
            line_index += 1;
        }
        rows.resize_with(body_height, Line::default);
        frame.render_widget(Paragraph::new(rows), body);
        let cursor_y = cursor_y
            .unwrap_or_default()
            .min(body_height.saturating_sub(1));
        frame.set_cursor_position(Position::new(
            body.x + gutter_width.min(area.width.saturating_sub(1) as usize) as u16,
            body.y + cursor_y as u16,
        ));
    }

    fn render_history(&self, frame: &mut Frame<'_>, area: Rect, editor: &Editor) {
        let Some((query, scope)) = editor.history_query() else {
            return;
        };
        let list_height = area.height.saturating_sub(1) as usize;
        let selected = editor.history_selected();
        let start = selected.saturating_sub(list_height.saturating_sub(1));
        let rows: Vec<Line<'static>> = (start..editor.history_match_count())
            .take(list_height)
            .filter_map(|index| {
                let item = editor.history_item(index)?;
                let style = if index == selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Some(Line::from(Span::styled(history_preview(item), style)))
            })
            .collect();
        if list_height > 0 {
            frame.render_widget(
                Paragraph::new(rows),
                Rect::new(area.x, area.y, area.width, list_height as u16),
            );
        }

        let prompt_area = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        let prompt = format!("{scope}/{query}");
        frame.render_widget(Paragraph::new(prompt.as_str()), prompt_area);
        let cursor_x = prompt
            .chars()
            .count()
            .min(area.width.saturating_sub(1) as usize) as u16;
        frame.set_cursor_position(Position::new(prompt_area.x + cursor_x, prompt_area.y));
    }

    fn render_text(&mut self, frame: &mut Frame<'_>, area: Rect, editor: &Editor) {
        let width = area.width as usize;
        let height = area.height as usize;
        let cursor_line = editor.buffer.current_line();
        let cursor_text = editor.buffer.line_text(cursor_line);
        let (cursor_subrow, cursor_x) = visual_position(
            &cursor_text,
            editor.buffer.char_column(editor.buffer.cursor()),
            width,
        );

        if cursor_line < self.scroll_line
            || (cursor_line == self.scroll_line && cursor_subrow < self.scroll_subrow)
        {
            self.scroll_line = cursor_line;
            self.scroll_subrow = cursor_subrow;
        }

        let cursor_distance = rows_between(
            editor,
            self.scroll_line,
            self.scroll_subrow,
            cursor_line,
            cursor_subrow,
            width,
        );
        if cursor_distance >= height {
            self.scroll_line = cursor_line;
            self.scroll_subrow = cursor_subrow.saturating_sub(height.saturating_sub(1));
        }

        let selection = editor.selection();
        let mut visible = Vec::with_capacity(height);
        let mut line_index = self.scroll_line;
        let mut first_subrow = self.scroll_subrow;
        while visible.len() < height && line_index < editor.buffer.len_lines() {
            let text = editor.buffer.line_text(line_index);
            let line_start = editor.buffer.line_start(line_index);
            let rows = wrap_line(&text, line_start, selection.as_ref(), width);
            visible.extend(
                rows.into_iter()
                    .skip(first_subrow)
                    .take(height - visible.len()),
            );
            first_subrow = 0;
            line_index += 1;
        }
        visible.resize_with(height, Line::default);
        frame.render_widget(Paragraph::new(visible), area);

        let cursor_y = rows_between(
            editor,
            self.scroll_line,
            self.scroll_subrow,
            cursor_line,
            cursor_subrow,
            width,
        )
        .min(height.saturating_sub(1));
        frame.set_cursor_position(Position::new(
            area.x + cursor_x.min(width.saturating_sub(1)) as u16,
            area.y + cursor_y as u16,
        ));
    }
}

pub fn cursor_style(mode: Mode) -> crossterm::cursor::SetCursorStyle {
    use crossterm::cursor::SetCursorStyle;
    match mode {
        Mode::Insert
        | Mode::Command
        | Mode::SearchForward
        | Mode::SearchBackward
        | Mode::History
        | Mode::Context => SetCursorStyle::SteadyBar,
        Mode::ContextPreview | Mode::Normal => SetCursorStyle::SteadyBlock,
        Mode::Visual | Mode::VisualLine => SetCursorStyle::SteadyUnderScore,
    }
}

fn history_preview(text: &str) -> String {
    text.split_whitespace()
        .take(64)
        .collect::<Vec<_>>()
        .join(" ")
}

fn rows_between(
    editor: &Editor,
    start_line: usize,
    start_subrow: usize,
    target_line: usize,
    target_subrow: usize,
    width: usize,
) -> usize {
    if target_line <= start_line {
        return target_subrow.saturating_sub(start_subrow);
    }

    let mut rows =
        wrapped_row_count(&editor.buffer.line_text(start_line), width).saturating_sub(start_subrow);
    for line in start_line + 1..target_line {
        rows += wrapped_row_count(&editor.buffer.line_text(line), width);
    }
    rows + target_subrow
}

fn wrap_line(
    text: &str,
    global_start: usize,
    selection: Option<&Range<usize>>,
    width: usize,
) -> Vec<Line<'static>> {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return vec![Line::default()];
    }

    let entities = entity_ranges(text);
    let mut rows = Vec::new();
    let mut spans = Vec::new();
    let mut row_width = 0;
    let mut char_offset = 0;

    for grapheme in text.graphemes(true) {
        let char_len = grapheme.chars().count();
        let global_idx = global_start + char_offset;
        let rendered = if grapheme == "\t" {
            " ".repeat(TAB_WIDTH - (row_width % TAB_WIDTH))
        } else {
            grapheme.to_owned()
        };
        let grapheme_width = UnicodeWidthStr::width(rendered.as_str()).max(1);

        if row_width > 0 && row_width + grapheme_width > width {
            rows.push(Line::from(std::mem::take(&mut spans)));
            row_width = 0;
        }

        let selected = selection.is_some_and(|range| range.contains(&global_idx));
        let semantic = entities.iter().any(|range| range.contains(&char_offset));
        let mut style = Style::default();
        if selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        if semantic {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        spans.push(Span::styled(rendered, style));
        row_width += grapheme_width;
        char_offset += char_len;

        if row_width >= width {
            rows.push(Line::from(std::mem::take(&mut spans)));
            row_width = 0;
        }
    }

    if !spans.is_empty() {
        rows.push(Line::from(spans));
    }
    if rows.is_empty() {
        rows.push(Line::default());
    }
    rows
}

fn visual_position(text: &str, char_column: usize, width: usize) -> (usize, usize) {
    let text = text.trim_end_matches(['\n', '\r']);
    let mut chars_seen = 0;
    let mut row = 0;
    let mut column = 0;
    for grapheme in text.graphemes(true) {
        if chars_seen >= char_column {
            break;
        }
        let grapheme_width = if grapheme == "\t" {
            TAB_WIDTH - (column % TAB_WIDTH)
        } else {
            UnicodeWidthStr::width(grapheme).max(1)
        };
        if column > 0 && column + grapheme_width > width {
            row += 1;
            column = 0;
        }
        column += grapheme_width;
        chars_seen += grapheme.chars().count();
        if column >= width {
            row += 1;
            column = 0;
        }
    }

    if column == 0 && row > 0 && chars_seen >= text.chars().count() {
        (row - 1, width.saturating_sub(1))
    } else {
        (row, column.min(width.saturating_sub(1)))
    }
}

fn wrapped_row_count(text: &str, width: usize) -> usize {
    let text = text.trim_end_matches(['\n', '\r']);
    if text.is_empty() {
        return 1;
    }
    let mut rows = 1;
    let mut column = 0;
    let mut graphemes = text.graphemes(true).peekable();
    while let Some(grapheme) = graphemes.next() {
        let grapheme_width = if grapheme == "\t" {
            TAB_WIDTH - (column % TAB_WIDTH)
        } else {
            UnicodeWidthStr::width(grapheme).max(1)
        };
        if column > 0 && column + grapheme_width > width {
            rows += 1;
            column = 0;
        }
        column += grapheme_width;
        if column >= width {
            column = 0;
            if graphemes.peek().is_some() {
                rows += 1;
            }
        }
    }
    rows
}

fn entity_ranges(text: &str) -> Vec<Range<usize>> {
    let chars: Vec<char> = text.chars().collect();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let starts_at_path = chars[index] == '@'
            && (index == 0
                || chars[index - 1].is_whitespace()
                || "([{,:".contains(chars[index - 1]));
        if starts_at_path && index + 1 < chars.len() && !chars[index + 1].is_whitespace() {
            let start = index;
            index += 1;
            if chars.get(index) == Some(&'"') {
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    index += 1;
                }
                if index < chars.len() {
                    index += 1;
                }
            }
            while index < chars.len() && !chars[index].is_whitespace() {
                index += 1;
            }
            ranges.push(start..index);
            continue;
        }

        if chars[index..].starts_with(&['[', 'i', 'm', 'a', 'g', 'e', ':']) {
            let start = index;
            while index < chars.len() && chars[index] != ']' {
                index += 1;
            }
            if index < chars.len() {
                index += 1;
            }
            ranges.push(start..index);
            continue;
        }
        index += 1;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use pretty_assertions::assert_eq;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    use super::*;

    fn key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    fn rendered_line(buffer: &Buffer, y: u16, width: u16) -> String {
        (0..width)
            .filter_map(|x| buffer.cell((x, y)))
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn wraps_to_available_width() {
        let rows = wrap_line("abcdefgh", 0, None, 4);
        assert_eq!(rows.len(), 2);
        assert_eq!(wrapped_row_count("abcdefgh", 4), 2);
        assert_eq!(visual_position("abcdefgh", 5, 4), (1, 1));
    }

    #[test]
    fn display_position_respects_tabs_and_emoji() {
        assert_eq!(visual_position("a\tb", 2, 80), (0, 4));
        assert_eq!(visual_position("🙂x", 1, 80), (0, 2));
    }

    #[test]
    fn context_preview_renders_line_numbers_and_the_selected_range() {
        let mut editor = Editor::new("");
        editor.set_context_files(vec!["src/main.rs".to_owned()]);
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        editor.take_context_preview_request();
        editor.apply_context_preview("src/main.rs", "one\ntwo\nthree\nfour\n", false);
        editor.handle_key(key('j'));
        editor.handle_key(key('v'));
        editor.handle_key(key('j'));

        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        let mut view = ViewState::default();
        terminal.draw(|frame| view.render(frame, &editor)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            (0..5)
                .map(|y| rendered_line(buffer, y, 20))
                .collect::<Vec<_>>(),
            [
                "@src/main.rs        ",
                "1 one               ",
                "2 two               ",
                "3 three             ",
                "4 four              ",
            ]
        );
        assert!(
            !buffer
                .cell((0, 1))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            buffer
                .cell((0, 2))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            buffer
                .cell((0, 3))
                .unwrap()
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn context_preview_keeps_the_cursor_visible_after_a_wrapped_line() {
        let mut editor = Editor::new("");
        editor.set_context_files(vec!["long.txt".to_owned()]);
        editor.handle_key(key('i'));
        editor.handle_key(key('@'));
        editor.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        editor.take_context_preview_request();
        editor.apply_context_preview("long.txt", "abcdefghijklmnopqrstuvwxyz\nnext\n", false);
        editor.handle_key(key('j'));

        let mut terminal = Terminal::new(TestBackend::new(8, 3)).unwrap();
        let mut view = ViewState::default();
        terminal.draw(|frame| view.render(frame, &editor)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            (0..3)
                .map(|y| rendered_line(buffer, y, 8))
                .collect::<Vec<_>>(),
            ["@long.tx", "2 next  ", "        "]
        );
    }

    #[test]
    fn finds_only_semantic_entities() {
        assert_eq!(entity_ranges("use @src/main.rs now"), vec![4..16]);
        assert_eq!(
            entity_ranges("use @\"docs/my file.md\":10-40 now"),
            vec![4..28]
        );
        assert_eq!(
            entity_ranges("mail@example.com"),
            Vec::<Range<usize>>::new()
        );
        assert_eq!(entity_ranges("[image: shot.png]"), vec![0..17]);
    }
}
