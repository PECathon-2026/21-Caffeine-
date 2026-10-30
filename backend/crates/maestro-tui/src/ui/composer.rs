//! Bottom composer: grows to 6 rows, history at edges, Enter submits,
//! Alt+Enter inserts a newline. Supports a masked mode for API-key entry.

use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub const MAX_ROWS: u16 = 6;
const HISTORY_CAP: usize = 100;

#[derive(Debug, Default)]
pub struct Composer {
    text: String,
    /// Byte offset of the cursor within `text`.
    cursor: usize,
    history: Vec<String>,
    hist_idx: Option<usize>,
    saved_draft: String,
    /// Render `•` per char (API key entry); history is disabled while set.
    pub masked: bool,
}

impl Composer {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.hist_idx = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.hist_idx = None;
    }

    pub fn newline(&mut self) {
        self.insert('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = prev_boundary(&self.text, self.cursor);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = next_boundary(&self.text, self.cursor);
        self.text.drain(self.cursor..next);
    }

    pub fn left(&mut self) {
        if self.cursor > 0 {
            self.cursor = prev_boundary(&self.text, self.cursor);
        }
    }

    pub fn right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = next_boundary(&self.text, self.cursor);
        }
    }

    pub fn home(&mut self) {
        let line_start = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.cursor = line_start;
    }

    pub fn end(&mut self) {
        let line_end = self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.text.len());
        self.cursor = line_end;
    }

    /// Cursor is on the first logical line?
    pub fn at_first_line(&self) -> bool {
        !self.text[..self.cursor].contains('\n')
    }

    /// Cursor is on the last logical line?
    pub fn at_last_line(&self) -> bool {
        !self.text[self.cursor..].contains('\n')
    }

    /// Move up a logical line; returns false when already at the top (edge —
    /// caller recalls history).
    pub fn cursor_up(&mut self) -> bool {
        if self.at_first_line() {
            return false;
        }
        let cur_start = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let col = self.cursor - cur_start;
        let prev_start = self.text[..cur_start - 1]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let prev_len = cur_start - 1 - prev_start;
        self.cursor = prev_start + col.min(prev_len);
        true
    }

    /// Move down a logical line; returns false at the bottom edge.
    pub fn cursor_down(&mut self) -> bool {
        if self.at_last_line() {
            return false;
        }
        let cur_start = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let col = self.cursor - cur_start;
        let next_start = self.text[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i + 1)
            .unwrap_or(self.text.len());
        let next_end = self.text[next_start..]
            .find('\n')
            .map(|i| next_start + i)
            .unwrap_or(self.text.len());
        self.cursor = (next_start + col).min(next_end);
        true
    }

    /// Take the submitted text, pushing it onto history (unless masked).
    pub fn take(&mut self) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.hist_idx = None;
        if !self.masked && !text.trim().is_empty() {
            if self.history.last().map(|h| h.as_str()) != Some(text.as_str()) {
                self.history.push(text.clone());
            }
            if self.history.len() > HISTORY_CAP {
                self.history.remove(0);
            }
        }
        text
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.hist_idx = None;
    }

    /// Ctrl+U: delete from the start of the current line to the cursor.
    pub fn kill_line_start(&mut self) {
        let line_start = self.text[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.text.drain(line_start..self.cursor);
        self.cursor = line_start;
        self.hist_idx = None;
    }

    /// Ctrl+W: delete the word (and trailing spaces) before the cursor.
    pub fn kill_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(' ');
        let word_start = trimmed.rfind([' ', '\n']).map(|i| i + 1).unwrap_or(0);
        self.text.drain(word_start..self.cursor);
        self.cursor = word_start;
        self.hist_idx = None;
    }

    pub fn history_prev(&mut self) {
        if self.masked || self.history.is_empty() {
            return;
        }
        let idx = match self.hist_idx {
            None => {
                self.saved_draft = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.hist_idx = Some(idx);
        self.text = self.history[idx].clone();
        self.cursor = self.text.len();
    }

    pub fn history_next(&mut self) {
        let Some(idx) = self.hist_idx else { return };
        if idx + 1 < self.history.len() {
            self.hist_idx = Some(idx + 1);
            self.text = self.history[idx + 1].clone();
        } else {
            self.hist_idx = None;
            self.text = std::mem::take(&mut self.saved_draft);
        }
        self.cursor = self.text.len();
    }

    /// Rows needed at `width` (1..=6), bordered box adds 2 outside this.
    pub fn rows(&self, width: u16) -> u16 {
        let inner = (width.saturating_sub(2)).max(1) as usize;
        let display = self.display_text();
        let n: usize = super::transcript::wrap(&display, inner).len().max(1);
        (n as u16).min(MAX_ROWS)
    }

    fn display_text(&self) -> String {
        if self.masked {
            self.text.chars().map(|_| '•').collect()
        } else {
            self.text.clone()
        }
    }

    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        placeholder: &str,
        focused: bool,
    ) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let display = self.display_text();
        if display.is_empty() {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("❯ ", Style::default().fg(theme.accent)),
                    Span::styled(placeholder.to_string(), Style::default().fg(theme.dim)),
                ])),
                inner,
            );
            if focused {
                frame.set_cursor_position((inner.x + 2, inner.y));
            }
            return;
        }
        let body_w = inner.width.saturating_sub(2).max(1) as usize;
        let wrapped = super::transcript::wrap(&display, body_w);
        let visible = wrapped.len().min(inner.height as usize);
        let start = wrapped.len() - visible;
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, text) in wrapped[start..].iter().enumerate() {
            let prefix = if start + i == 0 {
                Span::styled("❯ ", Style::default().fg(theme.accent))
            } else {
                Span::raw("  ")
            };
            lines.push(Line::from(vec![
                prefix,
                Span::styled(text.clone(), Style::default().fg(theme.text)),
            ]));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        if focused {
            // Cursor: recompute position of the cursor within the wrap.
            let upto: String = display
                .get(..self.masked_cursor())
                .unwrap_or(&display)
                .to_string();
            let cursor_lines = super::transcript::wrap(&upto, body_w);
            let row = cursor_lines.len().saturating_sub(1);
            let col = cursor_lines.last().map(|l| super::dwidth(l)).unwrap_or(0);
            let row_on_screen = row.saturating_sub(start);
            frame.set_cursor_position((
                inner.x + 2 + col.min(body_w) as u16,
                inner.y + (row_on_screen as u16).min(inner.height.saturating_sub(1)),
            ));
        }
    }

    fn masked_cursor(&self) -> usize {
        if self.masked {
            // '•' is 3 bytes; map char count.
            let chars = self.text[..self.cursor].chars().count();
            chars * '•'.len_utf8()
        } else {
            self.cursor
        }
    }
}

fn prev_boundary(s: &str, from: usize) -> usize {
    (0..from)
        .rev()
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(0)
}

fn next_boundary(s: &str, from: usize) -> usize {
    (from + 1..=s.len())
        .find(|i| s.is_char_boundary(*i))
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_and_take() {
        let mut c = Composer::default();
        for ch in "hello".chars() {
            c.insert(ch);
        }
        c.backspace();
        assert_eq!(c.text(), "hell");
        assert_eq!(c.take(), "hell");
        assert!(c.is_empty());
    }

    #[test]
    fn history_at_edges() {
        let mut c = Composer::default();
        for text in ["first", "second"] {
            c.insert_str(text);
            c.take();
        }
        c.insert_str("draft");
        c.history_prev();
        assert_eq!(c.text(), "second");
        c.history_prev();
        assert_eq!(c.text(), "first");
        c.history_next();
        assert_eq!(c.text(), "second");
        c.history_next();
        assert_eq!(c.text(), "draft"); // draft restored
    }

    #[test]
    fn multiline_cursor_moves() {
        let mut c = Composer::default();
        c.insert_str("one\ntwo\nthree");
        assert!(c.at_last_line());
        assert!(c.cursor_up());
        assert!(c.cursor_up());
        assert!(c.at_first_line());
        assert!(!c.cursor_up()); // edge
        assert!(c.cursor_down());
        assert!(c.cursor_down());
        assert!(!c.cursor_down()); // edge
    }

    #[test]
    fn rows_grow_to_six() {
        let mut c = Composer::default();
        assert_eq!(c.rows(80), 1);
        c.insert_str("a\nb\nc\nd\ne\nf\ng\nh");
        assert_eq!(c.rows(80), MAX_ROWS);
    }

    #[test]
    fn masked_take_skips_history() {
        let mut c = Composer {
            masked: true,
            ..Default::default()
        };
        c.insert_str("sekrit");
        assert_eq!(c.take(), "sekrit");
        c.masked = false;
        c.history_prev();
        assert_eq!(c.text(), ""); // nothing recorded
    }
}

#[cfg(test)]
mod kill_tests {
    use super::*;

    #[test]
    fn ctrl_u_kills_to_line_start() {
        let mut c = Composer::default();
        c.insert_str("keep\nkill this");
        c.kill_line_start();
        assert_eq!(c.text(), "keep\n");
        c.kill_line_start(); // empty line: no-op
        assert_eq!(c.text(), "keep\n");
    }

    #[test]
    fn ctrl_w_kills_the_previous_word() {
        let mut c = Composer::default();
        c.insert_str("/steer fix the thing  ");
        c.kill_word();
        assert_eq!(c.text(), "/steer fix the ");
        c.kill_word();
        assert_eq!(c.text(), "/steer fix ");
        c.kill_word();
        c.kill_word();
        c.kill_word();
        assert_eq!(c.text(), "");
    }
}
