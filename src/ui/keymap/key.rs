use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A single key with modifier state, comparable for table matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Key {
    pub const fn ch(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn cc(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            ctrl: true,
            alt: false,
            shift: false,
        }
    }
    pub const fn tab() -> Self {
        Self {
            code: KeyCode::Tab,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn back_tab() -> Self {
        Self {
            code: KeyCode::BackTab,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn esc() -> Self {
        Self {
            code: KeyCode::Esc,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn enter() -> Self {
        Self {
            code: KeyCode::Enter,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn left() -> Self {
        Self {
            code: KeyCode::Left,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn up() -> Self {
        Self {
            code: KeyCode::Up,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn down() -> Self {
        Self {
            code: KeyCode::Down,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn right() -> Self {
        Self {
            code: KeyCode::Right,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn home() -> Self {
        Self {
            code: KeyCode::Home,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }
    pub const fn end() -> Self {
        Self {
            code: KeyCode::End,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn page_up() -> Self {
        Self {
            code: KeyCode::PageUp,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn page_down() -> Self {
        Self {
            code: KeyCode::PageDown,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// Derive from a crossterm `KeyEvent`.
    pub fn from_event(e: &KeyEvent) -> Self {
        let has = |m: KeyModifiers| e.modifiers.contains(m);
        Self {
            code: e.code,
            ctrl: has(KeyModifiers::CONTROL),
            alt: has(KeyModifiers::ALT),
            shift: has(KeyModifiers::SHIFT),
        }
        .fold_char_shift()
    }

    /// Normalize printable character keys so matching is terminal-independent.
    /// A shifted character already carries its shift in the char value itself
    /// (`S`, `#`), yet terminals report the SHIFT modifier inconsistently for
    /// them - so for `Char` keys we fold shift into the (upper-cased) char and
    /// clear the flag. Non-character keys (Shift+Tab, Shift+Up) keep `shift`.
    pub fn fold_char_shift(mut self) -> Self {
        if let KeyCode::Char(c) = self.code
            && self.shift
        {
            self.code = KeyCode::Char(c.to_ascii_uppercase());
            self.shift = false;
        }
        self
    }

    /// Rebuild the crossterm modifiers for feeding downstream widgets
    /// (TextInput, edtui) that still work on `KeyEvent`.
    pub fn to_modifiers(self) -> KeyModifiers {
        let mut m = KeyModifiers::NONE;
        if self.ctrl {
            m |= KeyModifiers::CONTROL;
        }
        if self.alt {
            m |= KeyModifiers::ALT;
        }
        if self.shift {
            m |= KeyModifiers::SHIFT;
        }
        m
    }
}
