use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{Event as CrosstermEvent, KeyCode, KeyEvent};

use tokio::sync::mpsc::UnboundedSender;

use crate::application::MailSource;
use crate::application::account::{Account, keychain_ref_for, keychain_ref_for_password};
use crate::application::oauth::{AuthCodeFlow, ResolvedOAuth, TokenSet};
use crate::bootstrap::config::{Config, ThemeSpec};
use crate::domain::{Attachment, Envelope, Folder, Message};
use crate::tui::app::event::Action;
use crate::tui::app::event::{Event, NewMail};
use crate::tui::keymap::{Ctx, Key};
use crate::tui::theme::{Glyphs, Theme, parse_border_type, parse_color};
use crate::tui::widgets::TextInput;

// Feature submodules: domain types + small pure helpers, grouped by screen.
// The core `App` state machine and its `impl` stay in this module.
mod compose;
pub mod event;
mod jobs;
mod settings;
pub use compose::*;
pub use jobs::*;
pub use settings::*;
mod accounts;
mod compose_actions;
mod connect;
mod keys;
mod keys_forms;
mod mail_actions;
mod reader;
mod sidebar;
mod task;
pub(crate) use crate::application::account::connect::{now_ts, offline_source};
use crate::application::outcome::Effect;

/// Below this terminal width the sidebar + list layout collapses to a single
/// focus-cycled pane (folders -> list).
pub const NARROW_WIDTH: u16 = 90;

/// Infinite-scroll prefetch distance: once the list cursor is within this many
/// rows of the bottom of the loaded envelopes, the next older page is fetched in
/// the background so scrolling never stalls at the edge. Half a page
/// ([`LIST_WINDOW`](crate::application::mail::imap_cmd::LIST_WINDOW)), so the next batch starts loading around
/// the middle of the current page - well before the user reaches the bottom.
const PREFETCH_ROWS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Main,
    /// Full-screen message reader, entered by opening a message from the list.
    Reader,
    Compose,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Folders,
    List,
}

/// An entry in the left sidebar tree: an account header, or (under the active
/// account) one of its folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarItem {
    Account(usize),
    Folder(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmAction {
    DeleteMessage,
    DiscardDraft,
    DeleteAccount(usize),
}

pub struct ConfirmState {
    pub prompt: String,
    pub action: ConfirmAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub expires: Instant,
}

impl Toast {
    pub fn new(text: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            text: text.into(),
            kind,
            expires: Instant::now() + Duration::from_secs(4),
        }
    }
}

pub struct SearchState {
    pub input: TextInput,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            input: TextInput::new(""),
        }
    }
}

/// Ready for the event loop to suspend the terminal and run `$EDITOR`.
pub struct PendingExternalEditor {
    pub current_text: String,
    pub editor_cmd: Option<String>,
}

/// A queued OAuth authorization. The event loop opens the (blocking) browser
/// consent, then - after the user pastes the redirect URL - calls
/// [`App::apply_oauth_result`] to persist the refresh token and upsert `account`.
pub struct PendingOAuth {
    pub oauth: ResolvedOAuth,
    pub account: Account,
    pub set_default: bool,
}

/// An OAuth consent that has been opened in the browser and is waiting for the
/// user to paste back the redirect URL (the app runs no HTTP listener). Holds
/// the started [`AuthCodeFlow`], the originating request, and the paste input.
/// The event loop drives the two blocking steps (open browser, exchange code);
/// this carries the flow between them.
pub struct OAuthPaste {
    pub req: PendingOAuth,
    pub flow: AuthCodeFlow,
    pub input: TextInput,
}

/// Application state and behavior. Data comes from the active `MailSource`
/// Snapshot of the active view taken when a live session drops, so an automatic
/// reconnect can restore what the user was looking at (folder, highlighted
/// message, and the open message in the reader) instead of resetting to the top.
struct ReconnectCtx {
    select_uid: Option<u64>,
    open_uid: Option<u64>,
    was_reading: bool,
}

/// Memoized reader render: the Markdown body laid out into styled lines at a
/// given wrap width. Reused across frames (scroll, spinner ticks) so a repaint
/// doesn't re-parse the whole body; rebuilt when the width changes or
/// `reader_dirty` is set (new body, theme swap).
struct ReaderCache {
    wrap_width: usize,
    lines: Vec<ratatui::text::Line<'static>>,
}

/// (empty, offline cache, or live IMAP).
pub struct App {
    // Environment
    pub config: Config,
    pub theme: Theme,
    pub glyphs: Glyphs,
    /// Cached pane border style parsed from `ui.border_type`.
    border_type: ratatui::widgets::BorderType,
    pub should_quit: bool,
    /// Raw DB pool. Production code routes all queries through `services`;
    /// kept (and exercised) by the integration tests, which seed a real
    /// `:memory:` cache directly.
    #[allow(dead_code)]
    pub pool: sqlx::SqlitePool,
    /// Port implementations (accounts repo, mail cache, token store, notifier).
    // Consumed by the spawn seam / use-cases (Phase 1-2); direct pool access
    // remains until every call site routes through here.
    #[allow(dead_code)]
    pub services: crate::application::Services,
    pub accounts: Vec<Account>,
    pub active_account: usize,

    // Mailbox data
    pub source: Box<dyn MailSource>,
    /// True when `source` is a live IMAP backend (enables write-through cache).
    live: bool,
    pub folders: Vec<Folder>,
    pub folder_collapsed: Vec<bool>,
    pub selected_folder: usize,
    /// Highlighted item in the account/folder sidebar tree.
    pub sidebar_sel: SidebarItem,
    pub envelopes: Vec<Envelope>,
    pub display_envelopes: Vec<Envelope>,
    pub selected_message: usize,
    /// UIDs of messages tagged in the list (Space), for bulk delete/archive.
    /// Scoped to the current folder; cleared on folder/account switch.
    pub marked: HashSet<u64>,
    pub open_message: Option<Message>,
    pub reader_offset: usize,
    /// Memoized Markdown->`Line` render of `open_message.body`. The reader
    /// repaints on every keystroke and spinner tick, so re-parsing the whole
    /// body each frame is wasteful; this caches the rendered lines and is only
    /// recomputed when the body, wrap width, theme, or ascii toggle changes.
    reader_cache: Option<ReaderCache>,
    /// Set when an input to the reader render changed (new body, theme swap),
    /// forcing `prepare_reader` to rebuild `reader_cache` on the next frame.
    reader_dirty: bool,
    /// Reader toggle: show the full RFC822 headers instead of the compact ones.
    pub show_headers: bool,
    /// True while a "load older messages" page is in flight, to show a hint and
    /// prevent duplicate concurrent fetches. Reset on folder/account switch.
    pub loading_older: bool,
    /// True once paging reached the oldest message in the current folder, so the
    /// app stops auto-paging. Reset on folder/account switch and on refresh.
    pub older_exhausted: bool,

    // View state
    pub view: View,
    pub focus: Pane,
    pub narrow: bool,
    pub narrow_pane: Pane,
    pub sidebar_collapsed: bool,
    pub compose: Option<ComposeState>,
    pub settings: Option<SettingsState>,
    pub search: Option<SearchState>,
    /// Live substring filter over the sidebar account headers (matches
    /// name/email). Only populated while `sidebar_filtering` is true.
    pub sidebar_filter: String,
    /// True while the user is typing an account filter in the mailbox sidebar.
    pub sidebar_filtering: bool,
    pub help_open: bool,
    /// True while the background job tracker window is showing (toggle: `` ` ``).
    pub jobs_open: bool,
    /// Registry of tracked background jobs shown in the tracker window.
    pub jobs: JobRegistry,
    pub confirm: Option<ConfirmState>,
    /// Pending notifications, in arrival order. Drawn one at a time; each expires
    /// on its own so a burst (e.g. several accounts at once) queues instead of
    /// the newest overwriting the rest.
    pub toasts: Vec<Toast>,
    pub pending_external: Option<PendingExternalEditor>,
    pub pending_oauth: Option<PendingOAuth>,
    /// An opened consent waiting for the user to paste the redirect URL back.
    /// Set by the event loop after it opens the browser; drives the paste
    /// overlay and receives keystrokes while present.
    pub oauth_paste: Option<OAuthPaste>,
    /// Set when the user submits the pasted redirect URL (Enter); the event loop
    /// takes it and runs the (blocking) code exchange off the runtime.
    pub oauth_submit: Option<OAuthPaste>,
    /// True from when an OAuth flow is queued until it resolves; drives the
    /// full-screen "authorizing"/paste overlay.
    pub oauth_in_progress: bool,
    /// Status-bar busy label (e.g. "Connecting personal...") while an operation
    /// is in flight; paired with the spinner.
    busy: Option<String>,
    spinner_frame: usize,
    /// Determinate progress of the background all-folders sync, shown in the
    /// status bar's right section; `None` when no sync is running.
    pub sync: Option<crate::tui::app::event::SyncProgress>,
    /// Sender the IMAP IDLE watcher uses to push new-mail events to the loop.
    event_tx: Option<UnboundedSender<Event>>,
    /// Arrival watchers, one per connected account. Dropping an entry stops that
    /// account's background new-mail polling/IDLE. The app keeps one for every
    /// authorized account so new mail is pulled and notified even when the
    /// account is not the one currently in focus.
    idle_watchers: HashMap<String, Box<dyn crate::application::ports::WatchHandle>>,
    /// Set while an automatic reconnect (after a live session wedged) is in
    /// flight, so overlapping `ConnectionLost` events don't stack reconnects and
    /// `on_connected` can restore the pre-drop selection. `None` = not reconnecting.
    reconnect: Option<ReconnectCtx>,
    /// Accounts this session has already auto-prompted to re-authorize, so a
    /// failed/cancelled browser flow doesn't loop prompts. Manual Authorize
    /// from the account manager is never blocked by this.
    auto_reauth_attempted: HashSet<String>,

    // Key dispatch
    theme_name: String,
    pub keymap_table: Vec<crate::tui::keymap::OwnedBinding>,
    key_window: Vec<Key>,
}

impl App {
    /// Build the app over injected port implementations. The composition root
    /// (`main`) chooses the production adapters via
    /// `infrastructure::sqlite_services`; tests may inject their own.
    pub async fn new(
        config: Config,
        pool: sqlx::SqlitePool,
        accounts: Vec<Account>,
        services: crate::application::Services,
    ) -> Self {
        let (theme_name, theme) = Self::resolve_theme(&config.ui.theme, &config.themes);
        let glyphs = Glyphs::new(config.ui.ascii);
        let border_type = parse_border_type(&config.ui.border_type);
        // A single built-in keymap; users customize individual keys via the
        // `keybinding` config section rather than selecting a preset.
        let active = accounts.iter().position(|a| a.is_default).unwrap_or(0);
        let keymap_table = crate::tui::keymap::build_table(&config.keybinding);

        // Offline-first: show cached mail immediately if the default account has
        // any; otherwise nothing until the live connect lands.
        let account_id = accounts.get(active).and_then(|a| a.id);
        let source = offline_source(&services, account_id).await;
        let folders = source.list_folders().await.unwrap_or_default();
        let folder_collapsed = vec![false; folders.len()];
        let current_folder = folders
            .first()
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "INBOX".to_string());
        let envelopes = source
            .list_messages(&current_folder)
            .await
            .unwrap_or_default();

        let mut app = Self {
            config,
            theme,
            glyphs,
            border_type,
            live: false,
            should_quit: false,
            services,
            pool,
            accounts,
            active_account: active,
            source,
            folders,
            folder_collapsed,
            selected_folder: 0,
            sidebar_sel: SidebarItem::Folder(0),
            envelopes,
            display_envelopes: Vec::new(),
            selected_message: 0,
            marked: HashSet::new(),
            open_message: None,
            reader_offset: 0,
            reader_cache: None,
            reader_dirty: true,
            show_headers: false,
            loading_older: false,
            older_exhausted: false,
            view: View::Main,
            focus: Pane::List,
            narrow: false,
            narrow_pane: Pane::List,
            sidebar_collapsed: false,
            compose: None,
            settings: None,
            search: None,
            sidebar_filter: String::new(),
            sidebar_filtering: false,
            help_open: false,
            jobs_open: false,
            jobs: JobRegistry::new(),
            confirm: None,
            toasts: Vec::new(),
            pending_external: None,
            pending_oauth: None,
            oauth_paste: None,
            oauth_submit: None,
            oauth_in_progress: false,
            busy: None,
            spinner_frame: 0,
            sync: None,
            event_tx: None,
            idle_watchers: HashMap::new(),
            reconnect: None,
            auto_reauth_attempted: HashSet::new(),
            theme_name,
            keymap_table,
            key_window: Vec::new(),
        };
        app.refresh_display_list(None);
        app
    }

    #[allow(dead_code)] // test-only
    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    /// Pane border style parsed from `ui.border_type`, passed to every bordered
    /// `Block`.
    pub fn border_type(&self) -> ratatui::widgets::BorderType {
        self.border_type
    }

    /// Resolve a theme name to its colors. Built-in names (dark/light/mono) are
    /// reserved and win over any custom theme sharing the name; a custom name
    /// falls back to the dark base for any unparseable color role; an unknown
    /// name falls back to dark entirely. Returns the resolved name + `Theme`.
    fn resolve_theme(name: &str, themes: &HashMap<String, ThemeSpec>) -> (String, Theme) {
        if let Some(theme) = Theme::builtin(name) {
            return (name.to_string(), theme);
        }
        if let Some(spec) = themes.get(name) {
            let base = Theme::dark();
            let theme = Theme {
                bg: parse_color(&spec.bg, base.bg),
                fg: parse_color(&spec.fg, base.fg),
                dim: parse_color(&spec.dim, base.dim),
                accent: parse_color(&spec.accent, base.accent),
                unread: parse_color(&spec.unread, base.unread),
                success: parse_color(&spec.success, base.success),
                warning: parse_color(&spec.warning, base.warning),
                error: parse_color(&spec.error, base.error),
                border: parse_color(&spec.border, base.border),
                border_focus: parse_color(&spec.border_focus, base.border_focus),
            };
            return (name.to_string(), theme);
        }
        tracing::warn!("unknown theme {name:?}, falling back to dark");
        ("dark".to_string(), Theme::dark())
    }

    /// Recompute the responsive layout from the current terminal width. Called
    /// once per frame from the event loop. On the wide->narrow transition the
    /// single visible pane inherits the wide focus; on narrow->wide the wide
    /// focus inherits whichever pane was last visible so nothing jumps.
    pub fn set_viewport_width(&mut self, width: u16) {
        let narrow = width < NARROW_WIDTH;
        if narrow && !self.narrow {
            self.narrow_pane = self.focus;
        } else if !narrow && self.narrow {
            self.focus = self.narrow_pane;
        }
        self.narrow = narrow;
    }

    /// Replace the message shown in the reader and invalidate the memoized
    /// render, so `prepare_reader` rebuilds the styled lines for the new body.
    /// All writes to `open_message` route through here to keep the cache honest.
    fn set_open_message(&mut self, msg: Option<Message>) {
        self.open_message = msg;
        self.reader_dirty = true;
    }

    /// The Markdown wrap width for the reader: the full terminal width minus the
    /// reader pane's left/right border columns. Kept as one helper so the render
    /// (`prepare_reader`) and the draw code agree on the width.
    pub fn reader_wrap_width(total_width: u16) -> usize {
        (total_width as usize).saturating_sub(2).max(1)
    }

    /// Rebuild the memoized reader render if stale. Called once per frame from
    /// the event loop (before `tui::draw`, which must not mutate). The expensive
    /// Markdown parse + layout runs only when the body/theme/ascii changed
    /// (`reader_dirty`) or the wrap width changed - not on every scroll or tick.
    pub fn prepare_reader(&mut self, total_width: u16) {
        if self.view != View::Reader {
            return;
        }
        let Some(msg) = &self.open_message else {
            self.reader_cache = None;
            return;
        };
        let wrap_width = Self::reader_wrap_width(total_width);
        let fresh = self
            .reader_cache
            .as_ref()
            .is_some_and(|c| c.wrap_width == wrap_width);
        if fresh && !self.reader_dirty {
            return;
        }
        // Render into a local so the immutable borrow of `self` ends before the
        // `reader_cache` write below.
        let lines =
            crate::tui::markdown::render(&msg.body, wrap_width, &self.theme, self.config.ui.ascii);
        self.reader_cache = Some(ReaderCache { wrap_width, lines });
        self.reader_dirty = false;
    }

    /// Borrow the memoized reader lines (empty if not yet rendered). The draw
    /// path reads these instead of re-parsing the body each frame.
    pub fn reader_lines(&self) -> &[ratatui::text::Line<'static>] {
        self.reader_cache
            .as_ref()
            .map(|c| c.lines.as_slice())
            .unwrap_or(&[])
    }

    pub fn active_pane(&self) -> Pane {
        if self.narrow {
            self.narrow_pane
        } else {
            self.focus
        }
    }

    pub fn active_account_name(&self) -> &str {
        self.accounts
            .get(self.active_account)
            .map(|a| a.name.as_str())
            .unwrap_or("none")
    }

    pub fn active_account_email(&self) -> &str {
        self.accounts
            .get(self.active_account)
            .map(|a| a.email.as_str())
            .unwrap_or("")
    }

    pub fn account_connected(&self, i: usize) -> bool {
        self.accounts
            .get(i)
            .is_some_and(|a| !a.keychain_ref.trim().is_empty())
    }

    pub fn selected_folder_name(&self) -> &str {
        static EMPTY: &str = "";
        self.folders
            .get(self.selected_folder)
            .map(|f| f.name.as_str())
            .unwrap_or(EMPTY)
    }

    pub fn selected_env(&self) -> Option<&Envelope> {
        self.display_envelopes.get(self.selected_message)
    }

    pub fn selected_body_text(&self) -> &str {
        self.open_message
            .as_ref()
            .map(|m| m.body.as_str())
            .unwrap_or("")
    }

    pub fn set_toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        self.toasts.push(Toast::new(text, kind));
        // Keep the backlog bounded so a flood of accounts can't grow forever.
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// Wire the loop's event sender so background watchers can push new mail.
    pub fn set_event_sender(&mut self, tx: UnboundedSender<Event>) {
        self.event_tx = Some(tx);
    }

    fn active_account_id(&self) -> Option<i64> {
        self.accounts.get(self.active_account).and_then(|a| a.id)
    }
}

#[cfg(test)]
mod tests;
