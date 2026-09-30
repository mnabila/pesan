use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A minimal single-line input for header fields and forms. Handles typing,
/// Backspace/Delete, arrow/Home/End navigation, Ctrl-a/Ctrl-e, and word jumps.
/// The caret renders as an inverted cell and a real terminal cursor is placed
/// at the caret column when focused (see [`render_input`]).
#[derive(Clone)]
pub struct TextInput {
    text: String,
    cursor: usize, // byte index into self.text (kept on a char boundary)
    focused: bool,
}

impl TextInput {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cursor: usize::MAX,
            focused: false,
        }
    }

    pub fn focus(&mut self, focused: bool) {
        if self.focused != focused {
            self.focused = focused;
            if focused {
                self.cursor = self.text.len();
            }
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Byte offset of the caret within [`Self::text`]; only meaningful while
    /// focused (see `new`).
    pub fn cursor_byte(&self) -> usize {
        self.cursor
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Handle a key event; returns true if the widget consumed it.
    pub fn handle(&mut self, key: &KeyEvent) -> bool {
        // `new()` leaves the cursor at a `usize::MAX` "end" sentinel until the
        // field is focused. Clamp defensively so edits work even if the caller
        // routes keys to an unfocused input (otherwise Backspace/Delete index
        // out of bounds).
        if self.cursor > self.text.len() {
            self.cursor = self.text.len();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Home | KeyCode::Char('a') if ctrl => {
                self.cursor = 0;
                true
            }
            KeyCode::End | KeyCode::Char('e') if ctrl => {
                self.cursor = self.text.len();
                true
            }
            KeyCode::Char(c) => {
                if ctrl || alt {
                    return false;
                }
                if self.cursor >= self.text.len() {
                    self.text.push(c);
                    self.cursor = self.text.len();
                } else {
                    self.text.insert(self.cursor, c);
                    self.cursor += c.len_utf8();
                }
                true
            }
            KeyCode::Backspace => self.delete_before(),
            KeyCode::Delete => self.delete_at(),
            KeyCode::Left if ctrl => {
                self.cursor_to_prev_word();
                true
            }
            KeyCode::Right if ctrl => {
                self.cursor_to_next_word();
                true
            }
            KeyCode::Left => self.cursor_left(),
            KeyCode::Right => self.cursor_right(),
            KeyCode::Tab => {
                self.cursor = self.text.len();
                false
            }
            _ => false,
        }
    }

    fn cursor_left(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor = prev_boundary(&self.text, self.cursor);
        true
    }

    fn cursor_right(&mut self) -> bool {
        if self.cursor >= self.text.len() {
            return false;
        }
        self.cursor += self.text[self.cursor..]
            .chars()
            .next()
            .map_or(1, |c| c.len_utf8());
        true
    }

    fn cursor_to_prev_word(&mut self) {
        while self.cursor_left() {
            if self.cursor > 0 {
                let c = self.text[self.cursor..].chars().next().unwrap_or(' ');
                if c.is_whitespace() {
                    self.cursor_left();
                    break;
                }
            }
        }
    }

    fn cursor_to_next_word(&mut self) {
        while self.cursor_right() {
            let prev = prev_boundary(&self.text, self.cursor);
            let c = self.text[prev..self.cursor].chars().next().unwrap_or(' ');
            if c.is_whitespace() {
                break;
            }
        }
    }

    fn delete_before(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let start = prev_boundary(&self.text, self.cursor);
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        true
    }

    fn delete_at(&mut self) -> bool {
        if self.cursor >= self.text.len() {
            return false;
        }
        let end = self.cursor
            + self.text[self.cursor..]
                .chars()
                .next()
                .map_or(1, |c| c.len_utf8());
        self.text.replace_range(self.cursor..end, "");
        true
    }
}

fn prev_boundary(s: &str, byte: usize) -> usize {
    let mut i = byte.saturating_sub(1);
    while i > 0 && (s.as_bytes()[i] & 0xC0) == 0x80 {
        i -= 1;
    }
    i
}

//
// Drawing
//

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, layout::Rect};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::ui::theme::Theme;

/// Render the input as a single-line Paragraph; when focused the caret is
/// shown and the terminal cursor is placed at its column.
pub fn render_input(
    frame: &mut Frame<'_>,
    area: Rect,
    input: &TextInput,
    theme: &Theme,
    placeholder: Option<&str>,
) {
    let text = input.text();
    let focused = input.is_focused();
    let caret_byte = input.cursor_byte().min(text.len());
    let width = area.width as usize;

    // Ensure the caret stays in view by trimming the visible window left.
    let (start_byte, line_str) = if focused && width > 0 {
        let caret_col = text[..caret_byte].width();
        let over = caret_col.saturating_sub(width - 1);
        let start = byte_after_width(text, over);
        (start, text[start..].to_string())
    } else {
        (0, text.to_string())
    };

    let mut spans: Vec<Span> = Vec::new();
    if line_str.is_empty() && !focused {
        if let Some(p) = placeholder {
            spans.push(Span::styled(p, Style::new().fg(theme.dim)));
        }
    } else {
        let mut col = 0usize;
        for (b, ch) in line_str.char_indices() {
            let w = ch.width().unwrap_or(0);
            if col + w > width {
                spans.push(Span::raw("…"));
                break;
            }
            let abs_byte = start_byte + b;
            let is_caret = focused && abs_byte == caret_byte;
            let style = if is_caret {
                Style::new().fg(theme.bg).bg(theme.accent)
            } else {
                Style::default()
            };
            spans.push(Span::styled(ch.to_string(), style));
            col += w;
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    if focused {
        let caret_col = text[start_byte..caret_byte].width() as u16;
        let x = (area.x + caret_col).min(area.x.saturating_add(area.width).saturating_sub(1));
        frame.set_cursor_position((x, area.y));
    }
}

/// Byte index into `s` where the accumulated display width first exceeds
/// `max_width`. Returns `s.len()` if the whole string fits.
fn byte_after_width(s: &str, max_width: usize) -> usize {
    let mut acc = 0usize;
    for (i, ch) in s.char_indices() {
        let w = ch.width().unwrap_or(0);
        if acc + w > max_width {
            return i;
        }
        acc += w;
    }
    s.len()
}
