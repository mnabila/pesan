use crossterm::event::KeyCode;

use super::Key;

/// Parse a user-supplied key sequence such as "gg", "enter", "ctrl+s",
/// "shift+?", or "alt+pgup". Sequences are space-free strings of bare chars;
/// a "+" marks a single chord with modifiers.
pub fn parse_key_seq(spec: &str) -> Option<Vec<Key>> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains('+') {
        let tokens: Vec<&str> = s.split('+').map(str::trim).collect();
        let last = *tokens.last()?;
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        for t in &tokens[..tokens.len() - 1] {
            match t.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "c" => ctrl = true,
                "alt" | "meta" | "m" => alt = true,
                "shift" => shift = true,
                _ => return None,
            }
        }
        Some(vec![
            Key {
                code: token_key_code(last)?,
                ctrl,
                alt,
                shift,
            }
            .fold_char_shift(),
        ])
    } else if let Some(code) = named_key_code(s) {
        Some(vec![Key {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }])
    } else {
        let mut keys = Vec::new();
        for c in s.chars() {
            if c == ' ' || c == '\t' {
                return None;
            }
            // The char case already encodes shift (e.g. `S`), so never set the
            // shift flag for bare-character specs - matches folded key events.
            keys.push(Key {
                code: KeyCode::Char(c),
                ctrl: false,
                alt: false,
                shift: false,
            });
        }
        (!keys.is_empty()).then_some(keys)
    }
}

fn named_key_code(s: &str) -> Option<KeyCode> {
    Some(match s.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" | "btab" | "shift+tab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" | "pageup" => KeyCode::PageUp,
        "pgdn" | "pagedown" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "space" => KeyCode::Char(' '),
        _ => return None,
    })
}

fn token_key_code(t: &str) -> Option<KeyCode> {
    if let Some(code) = named_key_code(t) {
        return Some(code);
    }
    let mut chars = t.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(KeyCode::Char(c))
}

/// Short single-line label for a key sequence, used in the help page and
/// override help text.
pub(super) fn compact_label(seq: &[Key]) -> String {
    seq.iter()
        .map(|k| {
            use KeyCode as KC;
            let code = match k.code {
                KC::Char(c) => c.to_string(),
                KC::Enter => "Enter".to_string(),
                KC::Esc => "Esc".to_string(),
                KC::Tab => "Tab".to_string(),
                KC::Backspace => "Backspace".to_string(),
                KC::Delete => "Delete".to_string(),
                KC::Insert => "Insert".to_string(),
                KC::Home => "Home".to_string(),
                KC::End => "End".to_string(),
                KC::PageUp => "PgUp".to_string(),
                KC::PageDown => "PgDn".to_string(),
                KC::Up => "Up".to_string(),
                KC::Down => "Down".to_string(),
                KC::Left => "Left".to_string(),
                KC::Right => "Right".to_string(),
                _ => "?".to_string(),
            };
            let mut s = String::new();
            if k.ctrl {
                s.push_str("Ctrl-");
            }
            if k.alt {
                s.push_str("Alt-");
            }
            if k.shift && !(matches!(k.code, KC::Char(c) if c.is_ascii_uppercase())) {
                s.push_str("Shift-");
            }
            s.push_str(&code);
            s
        })
        .collect::<Vec<_>>()
        .join(" ")
}
