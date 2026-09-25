use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::bootstrap::config::KeyBindings;
use crate::tui::app::event::Action;

/// Input context; the active table row is chosen by context + keymap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctx {
    Global,
    Sidebar,
    List,
    Reader,
    Compose,
    /// Account manager list (selecting/adding/deleting accounts).
    Settings,
    /// Account edit form (field navigation within an open account).
    SettingsForm,
    Search,
    Confirm,
}

impl Ctx {
    /// Config section name for this context (the `keybinding` table keys).
    pub fn from_section(name: &str) -> Option<Self> {
        Some(match name {
            "global" => Self::Global,
            "folders" | "sidebar" => Self::Sidebar,
            "list" => Self::List,
            "reader" => Self::Reader,
            "compose" => Self::Compose,
            "settings" => Self::Settings,
            "settings_form" | "account_form" => Self::SettingsForm,
            "search" => Self::Search,
            "confirm" => Self::Confirm,
            _ => return None,
        })
    }
}

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

pub const MAX_SEQ: usize = 2;

#[derive(Debug, Clone, Copy)]
pub struct Binding {
    pub ctx: Ctx,
    /// Key sequence; up to `MAX_SEQ` keys (for vim chords like `gg`, `dd`).
    pub seq: &'static [Key],
    pub action: Action,
    /// Short key label for the help page.
    pub label: &'static str,
    /// One-line description for the help page.
    pub desc: &'static str,
}

mod parse;
mod table;

use parse::compact_label;
pub use parse::parse_key_seq;
use table::bindings;

/// A binding with owned data, ready to have user overrides applied.
#[derive(Debug, Clone)]
pub struct OwnedBinding {
    pub ctx: Ctx,
    pub seq: Vec<Key>,
    pub action: Action,
    pub label: String,
    pub desc: String,
}

/// Build the effective binding table, applying the user's `keybinding`
/// overrides. The outer map is keyed by config section (`global`, `list`,
/// `compose`, ... - see [`Ctx::from_section`]); the inner map is action name ->
/// key sequence(s). An override replaces that action's built-in sequences
/// *within that section's context only*. Unknown sections, unknown action
/// names, actions not bound in the section's context, and unparseable keys are
/// ignored.
pub fn build_table(overrides: &KeyBindings) -> Vec<OwnedBinding> {
    let mut table: Vec<OwnedBinding> = bindings()
        .iter()
        .map(|b| OwnedBinding {
            ctx: b.ctx,
            seq: b.seq.to_vec(),
            action: b.action,
            label: b.label.to_string(),
            desc: b.desc.to_string(),
        })
        .collect();

    for (section, actions) in overrides {
        let Some(ctx) = Ctx::from_section(section) else {
            continue;
        };
        for (name, specs) in actions {
            let Some(action) = action_from_name(name) else {
                continue;
            };
            let parsed: Vec<Vec<Key>> = specs
                .sequences()
                .iter()
                .filter_map(|s| parse_key_seq(s))
                .collect();
            if parsed.is_empty() {
                continue;
            }

            // Only an action already bound in this context can be remapped;
            // the first matching row donates its help-page description.
            let Some(template) = table.iter().find(|b| b.ctx == ctx && b.action == action) else {
                continue;
            };
            let mut template = template.clone();
            table.retain(|b| !(b.ctx == ctx && b.action == action));
            for seq in &parsed {
                template.seq = seq.clone();
                template.label = compact_label(seq);
                table.push(template.clone());
            }
        }
    }
    table
}

/// Resolve the longest key sequence in `window` (most recent key last) that
/// matches a binding for `ctx`. Global bindings always apply; the
/// context-specific ones layer on top and win ties by running second.
pub fn resolve(ctx: Ctx, window: &[Key], table: &[OwnedBinding]) -> Option<Action> {
    let mut best: Option<(usize, Action)> = None;

    let try_ctx = |bctx: Ctx, best: &mut Option<(usize, Action)>| {
        for b in table {
            if b.ctx != bctx || b.seq.len() > window.len() {
                continue;
            }
            let start = window.len() - b.seq.len();
            if &window[start..] == b.seq.as_slice() {
                let take = best.as_ref().is_none_or(|(len, _)| b.seq.len() > *len);
                if take {
                    *best = Some((b.seq.len(), b.action));
                }
            }
        }
    };

    try_ctx(Ctx::Global, &mut best);
    try_ctx(ctx, &mut best);
    best.map(|(_, a)| a)
}

/// Resolve a key sequence against ONLY `ctx`, without layering the global
/// bindings. Text-entry overlays (search box, confirm prompt) use this so a
/// printable key that happens to be a global hotkey - `q`, `z`, `R`, `S`... -
/// reaches the input as text instead of firing quit/refresh/etc. while typing.
pub fn resolve_in_ctx(ctx: Ctx, window: &[Key], table: &[OwnedBinding]) -> Option<Action> {
    let mut best: Option<(usize, Action)> = None;
    for b in table {
        if b.ctx != ctx || b.seq.len() > window.len() {
            continue;
        }
        let start = window.len() - b.seq.len();
        if &window[start..] == b.seq.as_slice()
            && best.as_ref().is_none_or(|(len, _)| b.seq.len() > *len)
        {
            best = Some((b.seq.len(), b.action));
        }
    }
    best.map(|(_, a)| a)
}

/// True if `window` is a proper prefix of some binding and we should wait for
/// one more key before dispatching (vim chords such as `g` waiting for `gg`).
pub fn is_partial(ctx: Ctx, window: &[Key], table: &[OwnedBinding]) -> bool {
    if window.len() >= MAX_SEQ {
        return false;
    }
    table.iter().any(|b| {
        (b.ctx == Ctx::Global || b.ctx == ctx)
            && b.seq.len() > window.len()
            && b.seq.starts_with(window)
    })
}

/// Map a config action name to its `Action`. Later-milestone hotkeys are
/// intentionally excluded.
pub fn action_from_name(name: &str) -> Option<Action> {
    Some(match name {
        "quit" => Action::Quit,
        "move_up" => Action::MoveUp,
        "move_down" => Action::MoveDown,
        "page_up" => Action::PageUp,
        "page_down" => Action::PageDown,
        "move_first" => Action::MoveFirst,
        "move_last" => Action::MoveLast,
        "focus_next" => Action::FocusNext,
        "focus_prev" => Action::FocusPrev,
        "focus_up" => Action::FocusUp,
        "focus_down" => Action::FocusDown,
        "focus_left" => Action::FocusLeft,
        "focus_right" => Action::FocusRight,
        "edit_field" => Action::EditField,
        "open_account" => Action::OpenAccount,
        "add_account" => Action::AddAccount,
        "delete_account" => Action::DeleteAccount,
        "set_default_account" => Action::SetDefaultAccount,
        "expand_folder" => Action::ExpandFolder,
        "collapse_folder" => Action::CollapseFolder,
        "filter_accounts" => Action::FilterAccounts,
        "scroll_up" => Action::ScrollUp,
        "scroll_down" => Action::ScrollDown,
        "scroll_half_up" => Action::ScrollHalfUp,
        "scroll_half_down" => Action::ScrollHalfDown,
        "scroll_start" => Action::ScrollStart,
        "scroll_end" => Action::ScrollEnd,
        "toggle_headers" => Action::ToggleHeaders,
        "open_message" => Action::OpenMessage,
        "back" => Action::Back,
        "select_folder" => Action::SelectFolder,
        "toggle_sidebar" => Action::ToggleSidebar,
        "refresh" => Action::Refresh,
        "toggle_unread" => Action::ToggleUnread,
        "toggle_flag" => Action::ToggleFlag,
        "toggle_mark" => Action::ToggleMark,
        "delete" => Action::Delete,
        "archive" => Action::Archive,
        "compose" => Action::Compose,
        "reply" => Action::Reply,
        "forward" => Action::Forward,
        "send" => Action::Send,
        "save_draft" => Action::SaveDraft,
        "discard_draft" => Action::DiscardDraft,
        "external_editor" => Action::ExternalEditor,
        "open_attachment" => Action::OpenAttachment,
        "open_in_browser" => Action::OpenInBrowser,
        "settings" => Action::Settings,
        "close_settings" => Action::CloseSettings,
        "save_settings" => Action::SaveSettings,
        "help" => Action::Help,
        "jobs" => Action::Jobs,
        "search" => Action::Search,
        "commit_search" => Action::CommitSearch,
        "close_overlay" => Action::CloseOverlay,
        "search_next" => Action::SearchNext,
        "search_prev" => Action::SearchPrev,
        "confirm_yes" => Action::ConfirmYes,
        "confirm_no" => Action::ConfirmNo,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::bootstrap::config::{KeyBindings, KeySpecs};

    #[test]
    fn new_actions_are_bound_by_default() {
        let table = build_table(&HashMap::new());
        assert_eq!(resolve(Ctx::List, &[Key::page_up()], &table), Some(Action::PageUp));
        assert_eq!(resolve(Ctx::List, &[Key::page_down()], &table), Some(Action::PageDown));
        assert_eq!(
            resolve(Ctx::Reader, &[Key::cc('u')], &table),
            Some(Action::ScrollHalfUp)
        );
        assert_eq!(
            resolve(Ctx::Reader, &[Key::cc('d')], &table),
            Some(Action::ScrollHalfDown)
        );
    }

    #[test]
    fn open_in_browser_bound_to_uppercase_o() {
        let table = build_table(&HashMap::new());
        assert_eq!(
            resolve(Ctx::Reader, &[Key::ch('O')], &table),
            Some(Action::OpenInBrowser)
        );
        assert_eq!(
            resolve(Ctx::List, &[Key::ch('O')], &table),
            Some(Action::OpenInBrowser)
        );
        // Lowercase o keeps its existing meanings.
        assert_eq!(
            resolve(Ctx::Reader, &[Key::ch('o')], &table),
            Some(Action::OpenAttachment)
        );
        assert_eq!(
            resolve(Ctx::List, &[Key::ch('o')], &table),
            Some(Action::OpenMessage)
        );
        assert_eq!(action_from_name("open_in_browser"), Some(Action::OpenInBrowser));
    }

    #[test]
    fn overrides_apply_to_confirm_search_compose_settings() {
        let mut kb: KeyBindings = HashMap::new();
        let mut confirm = HashMap::new();
        confirm.insert("confirm_no".into(), KeySpecs::One("x".into()));
        kb.insert("confirm".into(), confirm);
        let mut search = HashMap::new();
        search.insert("commit_search".into(), KeySpecs::One("space".into()));
        kb.insert("search".into(), search);
        let mut compose = HashMap::new();
        compose.insert("send".into(), KeySpecs::One("ctrl+x".into()));
        kb.insert("compose".into(), compose);
        let mut settings = HashMap::new();
        settings.insert("save_settings".into(), KeySpecs::One("s".into()));
        kb.insert("settings".into(), settings);
        let table = build_table(&kb);

        // Confirm: user remap 'x' rejects, built-in 'y' still confirms.
        assert_eq!(resolve(Ctx::Confirm, &[Key::ch('x')], &table), Some(Action::ConfirmNo));
        assert_eq!(resolve(Ctx::Confirm, &[Key::ch('y')], &table), Some(Action::ConfirmYes));
        // Search: space now commits the search.
        assert_eq!(resolve(Ctx::Search, &[Key::ch(' ')], &table), Some(Action::CommitSearch));
        // Compose: ctrl+x now sends.
        assert_eq!(resolve(Ctx::Compose, &[Key::cc('x')], &table), Some(Action::Send));
        // Settings: 's' now saves.
        assert_eq!(resolve(Ctx::Settings, &[Key::ch('s')], &table), Some(Action::SaveSettings));
    }

    #[test]
    fn form_nav_and_edit_are_bound_by_default() {
        let table = build_table(&HashMap::new());
        // Compose field grid + edit.
        assert_eq!(resolve(Ctx::Compose, &[Key::ch('j')], &table), Some(Action::FocusDown));
        assert_eq!(resolve(Ctx::Compose, &[Key::ch('k')], &table), Some(Action::FocusUp));
        assert_eq!(resolve(Ctx::Compose, &[Key::ch('i')], &table), Some(Action::EditField));
        assert_eq!(resolve(Ctx::Compose, &[Key::enter()], &table), Some(Action::EditField));
        // Account edit form.
        assert_eq!(
            resolve(Ctx::SettingsForm, &[Key::ch('i')], &table),
            Some(Action::EditField)
        );
        assert_eq!(
            resolve(Ctx::SettingsForm, &[Key::ch('l')], &table),
            Some(Action::FocusRight)
        );
        // Account list.
        assert_eq!(resolve(Ctx::Settings, &[Key::ch('o')], &table), Some(Action::OpenAccount));
        assert_eq!(resolve(Ctx::Settings, &[Key::ch('a')], &table), Some(Action::AddAccount));
        assert_eq!(resolve(Ctx::Settings, &[Key::ch('d')], &table), Some(Action::DeleteAccount));
        assert_eq!(
            resolve(Ctx::Settings, &[Key::ch('x')], &table),
            Some(Action::SetDefaultAccount)
        );
    }

    #[test]
    fn form_edit_key_is_remappable() {
        // The whole point of the refactor: the form-edit key is configurable.
        let mut kb: KeyBindings = HashMap::new();
        let mut compose = HashMap::new();
        compose.insert("edit_field".into(), KeySpecs::One("e".into()));
        kb.insert("compose".into(), compose);
        let mut form = HashMap::new();
        form.insert("edit_field".into(), KeySpecs::One("a".into()));
        kb.insert("account_form".into(), form);
        let table = build_table(&kb);

        // Compose: 'e' now edits; the default 'i' is gone.
        assert_eq!(resolve(Ctx::Compose, &[Key::ch('e')], &table), Some(Action::EditField));
        assert_ne!(resolve(Ctx::Compose, &[Key::ch('i')], &table), Some(Action::EditField));
        // Account form (section alias `account_form`): 'a' now edits.
        assert_eq!(
            resolve(Ctx::SettingsForm, &[Key::ch('a')], &table),
            Some(Action::EditField)
        );
    }

    #[test]
    fn parses_bare_chars_and_shift() {
        assert_eq!(
            parse_key_seq("gg").unwrap(),
            vec![Key::ch('g'), Key::ch('g')]
        );
        assert_eq!(parse_key_seq("q").unwrap(), vec![Key::ch('q')]);
        // Shift is folded into the char: "S" is Char('S') with shift cleared, so
        // it matches the (also folded) key events from the terminal.
        let s = parse_key_seq("S").unwrap();
        assert_eq!(s, vec![Key::ch('S')]);
        assert!(!s[0].shift);
    }

    #[test]
    fn parses_named_and_modified_keys() {
        assert_eq!(parse_key_seq("enter").unwrap(), vec![Key::enter()]);
        assert_eq!(parse_key_seq("esc").unwrap(), vec![Key::esc()]);
        let ctrl_s = parse_key_seq("ctrl+s").unwrap();
        assert_eq!(ctrl_s.len(), 1);
        assert!(ctrl_s[0].ctrl);
        assert_eq!(ctrl_s[0].code, KeyCode::Char('s'));
        // shift+<char> folds to the char with shift cleared (the char carries it).
        let shift_help = parse_key_seq("shift+?").unwrap();
        assert_eq!(shift_help[0].code, KeyCode::Char('?'));
        assert!(!shift_help[0].shift);
        // shift+g folds to uppercase G.
        assert_eq!(parse_key_seq("shift+g").unwrap(), vec![Key::ch('G')]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_key_seq("").is_none());
        assert!(parse_key_seq("ctrl+garbagekey").is_none());
    }

    #[test]
    fn action_names_map() {
        assert_eq!(action_from_name("open_message"), Some(Action::OpenMessage));
        assert_eq!(
            action_from_name("toggle_sidebar"),
            Some(Action::ToggleSidebar)
        );
        assert_eq!(action_from_name("nonsense"), None);
    }

    #[test]
    fn multi_key_override_of_multi_binding_action_has_no_duplicates() {
        // open_message is bound to both Enter and o in the list. Overriding it
        // with two sequences must yield exactly two list bindings, not four.
        let overrides: KeyBindings = HashMap::from([(
            "list".to_string(),
            HashMap::from([(
                "open_message".to_string(),
                KeySpecs::Many(vec!["enter".to_string(), "o".to_string()]),
            )]),
        )]);
        let table = build_table(&overrides);
        let opens: Vec<_> = table
            .iter()
            .filter(|b| b.action == Action::OpenMessage)
            .collect();
        assert_eq!(opens.len(), 2, "no duplicate open_message rows");
        assert!(opens.iter().all(|b| b.ctx == Ctx::List));
    }

    #[test]
    fn overrides_are_scoped_to_their_section() {
        // move_up is bound in both folders and list; a `list` override must
        // leave the sidebar binding untouched.
        let overrides: KeyBindings = HashMap::from([(
            "list".to_string(),
            HashMap::from([("move_up".to_string(), KeySpecs::One("k".to_string()))]),
        )]);
        let table = build_table(&overrides);

        // The list binds 'k'; 'j' is gone there.
        assert_eq!(
            resolve(Ctx::List, &[Key::ch('k')], &table),
            Some(Action::MoveUp)
        );
        assert_ne!(
            resolve(Ctx::List, &[Key::ch('j')], &table),
            Some(Action::MoveUp)
        );
        // The sidebar keeps its built-in bindings ('k' up, 'j' down).
        assert_eq!(
            resolve(Ctx::Sidebar, &[Key::ch('k')], &table),
            Some(Action::MoveUp)
        );
        assert_eq!(
            resolve(Ctx::Sidebar, &[Key::ch('j')], &table),
            Some(Action::MoveDown)
        );

        // A global override with several alternatives all resolve.
        let overrides: KeyBindings = HashMap::from([(
            "global".to_string(),
            HashMap::from([(
                "quit".to_string(),
                KeySpecs::Many(vec!["x".to_string(), "q".to_string()]),
            )]),
        )]);
        let table = build_table(&overrides);
        assert_eq!(
            resolve(Ctx::Global, &[Key::ch('x')], &table),
            Some(Action::Quit)
        );
        assert_eq!(
            resolve(Ctx::Global, &[Key::ch('q')], &table),
            Some(Action::Quit)
        );
    }

    #[test]
    fn unknown_sections_and_unbound_actions_are_ignored() {
        let overrides: KeyBindings = HashMap::from([
            (
                "nonsense".to_string(),
                HashMap::from([("move_up".to_string(), KeySpecs::One("zzz".to_string()))]),
            ),
            (
                // move_up is bound in list but not in reader.
                "reader".to_string(),
                HashMap::from([("move_up".to_string(), KeySpecs::One("zzz".to_string()))]),
            ),
        ]);
        let table = build_table(&overrides);
        assert_eq!(
            resolve(Ctx::List, &[Key::ch('j')], &table),
            Some(Action::MoveDown),
            "built-ins survive bogus sections"
        );
        assert!(
            !table
                .iter()
                .any(|b| b.seq == vec![Key::ch('z'), Key::ch('z'), Key::ch('z')])
        );
    }

    #[test]
    fn partials_still_work_with_overrides() {
        let overrides: KeyBindings = HashMap::from([(
            "list".to_string(),
            HashMap::from([("move_first".to_string(), KeySpecs::One("zz".to_string()))]),
        )]);
        let table = build_table(&overrides);
        // Single 'z' is now a partial of the overridden 'zz' chord.
        assert!(is_partial(Ctx::List, &[Key::ch('z')], &table));
        // Resolving 'zz' fires the action.
        assert_eq!(
            resolve(Ctx::List, &[Key::ch('z'), Key::ch('z')], &table),
            Some(Action::MoveFirst)
        );
    }
}
