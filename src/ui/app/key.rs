use super::*;

use crate::ui::keymap;

impl App {
    /// Key handling while the job tracker window is open. The `jobs` toggle key
    /// (default backtick) closes it and honors remaps; `c` clears finished jobs;
    /// any other key also closes the window.
    async fn jobs_key(&mut self, k: &Key) {
        if let Some(action) = keymap::resolve(
            keymap::Ctx::Global,
            &[*k],
            &self.keymap_table,
        ) {
            self.action(action).await;
            return;
        }
        match k.code {
            KeyCode::Char('c') if !k.ctrl => self.jobs.clear_finished(),
            _ => self.jobs_open = false,
        }
    }

    /// Keys while the OAuth redirect-paste prompt is up: Esc cancels, Enter
    /// submits the pasted URL, everything else edits the paste input.
    fn oauth_paste_key(&mut self, k: &Key, raw: &KeyEvent) {
        match k.code {
            KeyCode::Esc => self.cancel_oauth(),
            KeyCode::Enter => self.submit_oauth_paste(),
            _ => {
                if let Some(p) = &mut self.oauth_paste {
                    p.input.handle(raw);
                }
            }
        }
    }

    pub async fn handle(&mut self, event: &CrosstermEvent) {
        if let CrosstermEvent::Key(k) = event {
            self.on_key(k).await;
        }
    }

    pub(crate) async fn on_key(&mut self, key: &KeyEvent) {
        let k = Key::from_event(key);
        // The OAuth redirect-paste prompt is a blocking modal: it owns all keys
        // until the user submits (Enter) or cancels (Esc).
        if self.oauth_paste.is_some() {
            self.oauth_paste_key(&k, key);
            return;
        }
        if self.jobs_open {
            self.jobs_key(&k).await;
            return;
        }
        if self.help_open {
            // The help key (e.g. `?`) toggles the overlay closed; Esc also closes.
            if let Some(Action::Help) =
                keymap::resolve(keymap::Ctx::Global, &[k], &self.keymap_table)
            {
                self.action(Action::Help).await;
            } else if k.code == KeyCode::Esc {
                self.help_open = false;
            }
            return;
        }
        if self.confirm.is_some() {
            self.confirm_key(&k).await;
            return;
        }
        if self.search.is_some() {
            self.search_key(&k).await;
            return;
        }
        if self.sidebar_filtering {
            self.sidebar_filter_key(&k).await;
            return;
        }
        match self.view {
            // The reader view reuses the keymap-driven dispatch of the main view;
            // `ctx_for` yields `Ctx::Reader` so reader bindings apply.
            View::Main | View::Reader => self.main_key(&k).await,
            View::Compose => self.compose_key(&k).await,
            View::Settings => self.settings_key(&k).await,
        }
    }

    fn ctx_for(&self) -> Ctx {
        if self.view == View::Reader {
            return Ctx::Reader;
        }
        match self.active_pane() {
            Pane::Folders => Ctx::Sidebar,
            Pane::List => Ctx::List,
        }
    }

    async fn main_key(&mut self, key: &Key) {
        let ctx = self.ctx_for();
        self.key_window.push(*key);
        if self.key_window.len() > keymap::MAX_SEQ {
            self.key_window.remove(0);
        }
        if let Some(action) = keymap::resolve(ctx, &self.key_window, &self.keymap_table)
        {
            self.key_window.clear();
            self.action(action).await;
        } else if !keymap::is_partial(ctx, &self.key_window, &self.keymap_table) {
            self.key_window.clear();
        }
    }

    async fn confirm_key(&mut self, key: &Key) {
        // Resolve the confirm keymap so y/Enter (yes) and n/Esc (no) honor user
        // remaps; fall back to the literal y/n for compatibility.
        if let Some(action) = keymap::resolve_in_ctx(
            keymap::Ctx::Confirm,
            &[*key],
            &self.keymap_table,
        ) {
            self.action(action).await;
            return;
        }
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.action(Action::ConfirmYes).await,
            KeyCode::Char('n') | KeyCode::Char('N') => self.action(Action::ConfirmNo).await,
            _ => {}
        }
    }

    async fn search_key(&mut self, key: &Key) {
        let Some(search) = &mut self.search else {
            return;
        };
        if let Some(action) = keymap::resolve_in_ctx(
            keymap::Ctx::Search,
            &[*key],
            &self.keymap_table,
        ) {
            self.action(action).await;
            return;
        }
        // Any unbound key is text for the query.
        let _ = search
            .input
            .handle(&KeyEvent::new(key.code, key.to_modifiers()));
    }

    pub async fn action(&mut self, a: Action) {
        if !matches!(
            self.view,
            View::Main | View::Reader | View::Settings | View::Compose
        ) {
            return;
        }
        match a {
            Action::Quit => self.should_quit = true,

            // Navigation / selection
            Action::MoveUp => match self.active_pane() {
                Pane::Folders => self.move_sidebar(-1).await,
                Pane::List => self.move_message(-1).await,
            },
            Action::MoveDown => match self.active_pane() {
                Pane::Folders => self.move_sidebar(1).await,
                Pane::List => self.move_message(1).await,
            },
            Action::PageUp => self.move_message(-10).await,
            Action::PageDown => self.move_message(10).await,
            Action::MoveFirst => self.move_folder_and_list_absolute(true).await,
            Action::MoveLast => self.move_folder_and_list_absolute(false).await,
            Action::FocusNext => self.focus_next().await,
            Action::FocusPrev => self.focus_prev().await,
            // In the sidebar, expand/collapse a folder; on an account header,
            // expand = switch to that account.
            Action::ExpandFolder => match self.sidebar_sel {
                SidebarItem::Account(i) if i != self.active_account => {
                    self.switch_to_account(i).await
                }
                SidebarItem::Folder(_) => self.expand_selected_folder().await,
                _ => {}
            },
            Action::CollapseFolder => {
                if let SidebarItem::Folder(_) = self.sidebar_sel {
                    self.collapse_selected_folder().await;
                }
            }
            Action::FilterAccounts => self.begin_sidebar_filter(),

            // Reader scrolling
            Action::ScrollUp => self.scroll_reader(-1),
            Action::ScrollDown => self.scroll_reader(1),
            Action::ScrollHalfUp => self.scroll_reader(-10),
            Action::ScrollHalfDown => self.scroll_reader(10),
            Action::ScrollStart => self.reader_offset = 0,
            Action::ScrollEnd => {
                self.reader_offset = self.selected_body_text().lines().count().saturating_sub(1);
            }
            Action::ToggleHeaders => {
                self.show_headers = !self.show_headers;
                // Headers are cached on first open, so they are usually already
                // present. Only when they are missing (e.g. a message cached
                // before headers were stored) do we refetch to fill them in.
                if self.show_headers
                    && self
                        .open_message
                        .as_ref()
                        .is_some_and(|m| m.raw_headers.is_none())
                {
                    self.reload_open_headers().await;
                }
                // Start the reader at the top so the newly shown/hidden headers
                // are visible rather than scrolled past.
                self.reader_offset = 0;
            }
            Action::ToggleSidebar => self.sidebar_collapsed = !self.sidebar_collapsed,
            Action::Refresh => self.refresh_current_folder().await,

            // Mail read path
            Action::OpenMessage => {
                if self.selected_env().is_some() {
                    // The body fetch happens here (once), not on every cursor move.
                    self.load_selected_preview().await;
                    self.reader_offset = 0;
                    self.view = View::Reader;
                    self.mark_selected_seen().await;
                }
            }
            Action::Back => {
                if self.view == View::Reader {
                    self.view = View::Main;
                } else {
                    self.back().await;
                }
            }
            Action::SelectFolder => self.activate_sidebar().await,

            // Mailbox actions
            Action::ToggleUnread => self.toggle_self_selected_flag(false).await,
            Action::ToggleFlag => self.toggle_self_selected_flag(true).await,
            Action::ToggleMark => self.toggle_mark_selected(),
            Action::Delete => {
                let n = self.delete_target_count();
                if n > 0 {
                    let prompt = if n == 1 {
                        "Delete this message?".to_string()
                    } else {
                        format!("Delete {n} marked messages?")
                    };
                    self.confirm = Some(ConfirmState {
                        prompt,
                        action: ConfirmAction::DeleteMessage,
                    });
                }
            }
            Action::Archive => self.archive_selected().await,

            // Compose
            Action::Compose => self.open_compose(ComposeMode::New),
            Action::Reply => self.open_compose(ComposeMode::Reply),
            Action::Forward => self.open_compose(ComposeMode::Forward),
            Action::Send => self.send_compose().await,
            Action::SaveDraft => self.save_draft(),
            Action::DiscardDraft => {
                if self.compose.is_some() {
                    self.confirm = Some(ConfirmState {
                        prompt: "Discard this draft?".into(),
                        action: ConfirmAction::DiscardDraft,
                    });
                }
            }
            Action::ExternalEditor => self.request_external_editor(),
            Action::OpenAttachment => {
                if let Some(env) = self.selected_env()
                    && env.has_attachment
                {
                    self.set_toast("Opening attachments is stubbed until M2", ToastKind::Info);
                }
            }
            Action::OpenInBrowser => self.open_in_browser().await,

            // Settings
            Action::Settings => self.open_settings(),
            Action::CloseSettings => self.close_settings(),
            Action::SaveSettings => self.save_settings().await,

            // Overlays
            Action::Help => self.help_open = !self.help_open,
            Action::Jobs => self.jobs_open = !self.jobs_open,
            Action::Search => {
                if self.view == View::Main {
                    self.search = Some(SearchState::new());
                }
            }
            Action::CommitSearch => {
                let Some(search) = self.search.as_ref() else {
                    return;
                };
                let term = search.input.text().trim().to_string();
                self.search = None;
                self.run_search(&term).await;
            }
            Action::CloseOverlay => {
                if self.search.is_some() {
                    self.search = None;
                    self.refresh_display_list(None);
                }
            }
            Action::SearchNext => {
                if self.display_envelopes.len() < self.envelopes.len() {
                    self.move_message(1).await;
                }
            }
            Action::SearchPrev => {
                if self.display_envelopes.len() < self.envelopes.len() {
                    self.move_message(-1).await;
                }
            }

            // Accounts

            // Form-local actions (compose fields, account-form field navigation,
            // account list). These are dispatched directly by the form key
            // handlers in `keys_forms.rs`, never routed through this global
            // `action()`, so they are no-ops here.
            Action::FocusUp
            | Action::FocusDown
            | Action::FocusLeft
            | Action::FocusRight
            | Action::EditField
            | Action::OpenAccount
            | Action::AddAccount
            | Action::DeleteAccount
            | Action::SetDefaultAccount => {}

            // Confirm dialog
            Action::ConfirmYes => self.confirm_yes().await,
            Action::ConfirmNo => self.confirm = None,
        }
    }

    async fn move_folder_and_list_absolute(&mut self, first: bool) {
        match self.active_pane() {
            Pane::Folders => {
                self.sidebar_first_last(first).await;
            }
            Pane::List => {
                self.select_message(if first {
                    0
                } else {
                    self.display_envelopes.len().saturating_sub(1)
                })
                .await;
                // Jumping to the bottom (G) should also pull the next page.
                if !first {
                    self.maybe_prefetch_older().await;
                }
            }
        }
    }

    async fn focus_next(&mut self) {
        let next = |p: Pane| match p {
            Pane::Folders => Pane::List,
            Pane::List => Pane::Folders,
        };
        if self.narrow {
            self.narrow_pane = next(self.narrow_pane);
        } else {
            self.focus = next(self.focus);
        }
    }

    async fn focus_prev(&mut self) {
        let prev = |p: Pane| match p {
            Pane::Folders => Pane::List,
            Pane::List => Pane::Folders,
        };
        if self.narrow {
            self.narrow_pane = prev(self.narrow_pane);
        } else {
            self.focus = prev(self.focus);
        }
    }

    async fn back(&mut self) {
        if self.narrow {
            self.narrow_pane = match self.narrow_pane {
                Pane::List => Pane::Folders,
                Pane::Folders => Pane::Folders,
            };
        } else {
            self.focus = match self.focus {
                Pane::List => Pane::Folders,
                Pane::Folders => Pane::Folders,
            };
        }
    }

    fn scroll_reader(&mut self, delta: isize) {
        let max = self.selected_body_text().lines().count().saturating_sub(1);
        let next = self.reader_offset as isize + delta;
        self.reader_offset = next.clamp(0, max as isize) as usize;
    }

    pub(crate) fn scroll_compose_body(&mut self, delta: isize) {
        if let Some(c) = &mut self.compose {
            let max = c.body.lines().count().saturating_sub(1);
            let next = c.body_scroll as isize + delta;
            c.body_scroll = next.clamp(0, max as isize) as usize;
        }
    }
}
