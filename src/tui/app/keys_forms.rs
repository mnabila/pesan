use super::*;

impl App {
    /// Compose is an inline form: header/attach fields are edited with their
    /// `TextInput`, the body opens in `$EDITOR`. App-level keys (send/draft/
    /// discard/editor) are reserved before keystrokes reach a field.
    pub(crate) async fn compose_key(&mut self, key: &Key) {
        let Some((focus, editing)) = self.compose.as_ref().map(|c| (c.focus, c.editing)) else {
            return;
        };

        // Send/draft/editor work from anywhere, in either mode.
        match key.code {
            KeyCode::Char('s') if key.ctrl => return self.action(Action::Send).await,
            KeyCode::Char('d') if key.ctrl => return self.action(Action::SaveDraft).await,
            KeyCode::Char('e') if key.ctrl => return self.action(Action::ExternalEditor).await,
            // Tab/Shift-Tab always move between rows and leave editing mode.
            KeyCode::Tab => {
                if let Some(c) = &mut self.compose {
                    c.editing = false;
                    c.focus = c.focus.next();
                }
                return;
            }
            KeyCode::BackTab => {
                if let Some(c) = &mut self.compose {
                    c.editing = false;
                    c.focus = c.focus.prev();
                }
                return;
            }
            _ => {}
        }

        if editing {
            self.compose_edit_key(key, focus).await;
        } else {
            self.compose_nav_key(key, focus).await;
        }
    }

    /// Editing mode: keystrokes flow into the focused text field; Enter/Esc
    /// leave edit mode (Attach commits/removes attachment paths).
    async fn compose_edit_key(&mut self, key: &Key, focus: ComposeFocus) {
        match key.code {
            // Enter/Esc leave editing (Attach commits the path on Enter).
            KeyCode::Enter if focus == ComposeFocus::Attach => self.add_attachment(),
            KeyCode::Enter | KeyCode::Esc => {
                if let Some(c) = &mut self.compose {
                    c.editing = false;
                }
            }
            KeyCode::Char('x') if key.ctrl && focus == ComposeFocus::Attach => {
                self.remove_last_attachment()
            }
            _ => {
                if let Some(c) = &mut self.compose
                    && let Some(input) = c.focused_input_mut()
                {
                    input.handle(&KeyEvent::new(key.code, key.to_modifiers()));
                }
            }
        }
    }

    /// Navigation mode: j/k move rows, Shift+J/K and arrows scroll the body,
    /// e/Enter edit the focused field or open $EDITOR on the body.
    async fn compose_nav_key(&mut self, key: &Key, focus: ComposeFocus) {
        match key.code {
            KeyCode::Esc => return self.action(Action::DiscardDraft).await,
            // `?` opens the keymap overlay (compose keys + global column). Only in
            // nav mode - while editing a field it is a literal character.
            KeyCode::Char('?') => return self.action(Action::Help).await,
            KeyCode::Char('j') => {
                if let Some(c) = &mut self.compose {
                    c.focus = c.focus.next();
                }
            }
            KeyCode::Char('k') => {
                if let Some(c) = &mut self.compose {
                    c.focus = c.focus.prev();
                }
            }
            // Body row: e/Enter open the editor; J/K (Shift) and arrows scroll.
            KeyCode::Char('e') | KeyCode::Enter if focus == ComposeFocus::Body => {
                self.action(Action::ExternalEditor).await
            }
            KeyCode::Char('J') | KeyCode::Down if focus == ComposeFocus::Body => {
                self.scroll_compose_body(1)
            }
            KeyCode::Char('K') | KeyCode::Up if focus == ComposeFocus::Body => {
                self.scroll_compose_body(-1)
            }
            KeyCode::PageDown if focus == ComposeFocus::Body => self.scroll_compose_body(10),
            KeyCode::PageUp if focus == ComposeFocus::Body => self.scroll_compose_body(-10),
            KeyCode::Char('g') | KeyCode::Home if focus == ComposeFocus::Body => {
                if let Some(c) = &mut self.compose {
                    c.body_scroll = 0;
                }
            }
            KeyCode::Char('G') | KeyCode::End if focus == ComposeFocus::Body => {
                if let Some(c) = &mut self.compose {
                    c.body_scroll = c.body.lines().count().saturating_sub(1);
                }
            }
            // Header/attach rows: arrows also move; e/Enter start editing.
            KeyCode::Down => {
                if let Some(c) = &mut self.compose {
                    c.focus = c.focus.next();
                }
            }
            KeyCode::Up => {
                if let Some(c) = &mut self.compose {
                    c.focus = c.focus.prev();
                }
            }
            KeyCode::Char('e') | KeyCode::Enter => {
                if let Some(c) = &mut self.compose {
                    c.editing = true;
                    if let Some(input) = c.focused_input_mut() {
                        input.focus(true);
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) async fn settings_key(&mut self, key: &Key) {
        let editing = self.settings.as_ref().is_some_and(|s| s.editing);
        let focus = self.settings.as_ref().map(|s| s.focus);
        let filtering = self.settings.as_ref().is_some_and(|s| s.filtering);

        // While typing an account filter, keystrokes edit the query (so every
        // printable key, including W, is treated as filter text). Esc clears the
        // filter; Enter/Down commits and drops back to list navigation.
        if filtering && !editing {
            match key.code {
                KeyCode::Esc => {
                    if let Some(s) = &mut self.settings {
                        s.account_filter.clear();
                        s.filtering = false;
                    }
                    self.settings_clamp_account_selection();
                }
                KeyCode::Enter | KeyCode::Down => {
                    if let Some(s) = &mut self.settings {
                        s.filtering = false;
                    }
                    self.settings_clamp_account_selection();
                }
                KeyCode::Backspace => {
                    if let Some(s) = &mut self.settings {
                        s.account_filter.pop();
                    }
                    self.settings_clamp_account_selection();
                }
                KeyCode::Char(c) if !key.ctrl => {
                    if let Some(s) = &mut self.settings {
                        s.account_filter.push(c);
                    }
                    self.settings_clamp_account_selection();
                }
                _ => {}
            }
            return;
        }

        // Shift+W saves, except while actively typing into a form text field,
        // where the character must reach the input instead.
        let field_editing = self.settings.as_ref().is_some_and(|s| s.field_editing);
        if key.code == KeyCode::Char('W') && !field_editing {
            self.action(Action::SaveSettings).await;
            return;
        }
        // `?` opens the account-manager help overlay (unless typing into a field).
        if key.code == KeyCode::Char('?') && !field_editing {
            self.help_open = true;
            return;
        }

        // App-level keys work in every settings mode. Field navigation is j/k
        // (Tab is intentionally not a selector in the account form).
        match key.code {
            KeyCode::Esc if !editing => {
                self.action(Action::CloseSettings).await;
                return;
            }
            KeyCode::Char('q') if !editing && !key.ctrl => {
                self.action(Action::CloseSettings).await;
                return;
            }
            _ => {}
        }

        if editing {
            self.settings_edit_key(key, focus).await;
        } else {
            self.settings_nav_key(key, focus).await;
        }
    }

    /// Account edit form. Text fields (Name/Email/Password) are modal: `e` or
    /// Enter starts typing, Esc/Enter stops. Esc from field-navigation exits to
    /// the list. Field order and the final action (Authorize vs Save) depend on
    /// whether the selected provider uses OAuth.
    async fn settings_edit_key(&mut self, key: &Key, focus: Option<SettingsFocus>) {
        let field_editing = self.settings.as_ref().is_some_and(|s| s.field_editing);
        let text_field = matches!(
            focus,
            Some(SettingsFocus::Name) | Some(SettingsFocus::Email) | Some(SettingsFocus::Password)
        );

        // Typing into a text field: Esc/Enter leave, everything else edits.
        if field_editing {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                if let Some(s) = &mut self.settings {
                    s.field_editing = false;
                    if let Some(form) = s.form.as_mut() {
                        form.name.focus(false);
                        form.email.focus(false);
                        form.password.focus(false);
                    }
                }
                return;
            }
            if let Some(field) = self.settings.as_mut().and_then(|s| {
                s.form.as_mut().map(|form| match focus {
                    Some(SettingsFocus::Name) => &mut form.name,
                    Some(SettingsFocus::Email) => &mut form.email,
                    _ => &mut form.password,
                })
            }) {
                field.handle(&KeyEvent::new(key.code, key.to_modifiers()));
            }
            return;
        }

        // Field navigation: Esc exits the form; j/k move; Enter edits/acts.
        if key.code == KeyCode::Esc {
            if let Some(s) = &mut self.settings {
                s.editing = false;
                s.focus = SettingsFocus::Accounts;
            }
            return;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => return self.cycle_settings_focus(1),
            KeyCode::Char('k') | KeyCode::Up => return self.cycle_settings_focus(-1),
            _ => {}
        }
        match focus {
            Some(SettingsFocus::Name | SettingsFocus::Email | SettingsFocus::Password) => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Char('e'))
                    && text_field
                    && let Some(s) = &mut self.settings
                {
                    s.field_editing = true;
                    // Focus the real input so its caret starts at the text end
                    // (without this its cursor stays at the `usize::MAX` sentinel
                    // and Backspace panics).
                    if let Some(form) = s.form.as_mut() {
                        let field = match focus {
                            Some(SettingsFocus::Name) => &mut form.name,
                            Some(SettingsFocus::Email) => &mut form.email,
                            _ => &mut form.password,
                        };
                        field.focus(true);
                    }
                }
            }
            Some(SettingsFocus::Provider) => {
                // Provider stays valid across an auth-kind switch (it appears in
                // both field orders), so no focus fix-up is needed here. h/l cycle
                // the provider (arrows still work as an alternate).
                let dec = matches!(key.code, KeyCode::Char('h') | KeyCode::Left);
                let inc = matches!(key.code, KeyCode::Char('l') | KeyCode::Right);
                if (dec || inc)
                    && let Some(s) = &mut self.settings
                    && let Some(form) = s.form.as_mut()
                {
                    let max = s.providers.len().saturating_sub(1);
                    if dec {
                        form.provider_idx = form.provider_idx.saturating_sub(1);
                    } else {
                        form.provider_idx = (form.provider_idx + 1).min(max);
                    }
                }
            }
            Some(SettingsFocus::IsDefault) => {
                if (key.code == KeyCode::Enter || key.code == KeyCode::Char(' '))
                    && let Some(s) = &mut self.settings
                    && let Some(form) = s.form.as_mut()
                {
                    form.is_default = !form.is_default;
                }
            }
            Some(SettingsFocus::Authorize) => {
                if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') {
                    self.request_authorize();
                }
            }
            Some(SettingsFocus::Save)
                if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') =>
            {
                self.save_password_account().await;
            }
            _ => {}
        }
    }

    /// Account-list navigation: j/k move the selection, e/a/d/x/`/` act on the
    /// highlighted account. UI preferences now live in `config.yaml`.
    async fn settings_nav_key(&mut self, key: &Key, _focus: Option<SettingsFocus>) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.settings_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.settings_move(1),
            // `o` opens the selected account (its form doubles as the detail view);
            // Esc exits. `a` adds a new account.
            KeyCode::Char('o') => self.begin_edit_account(false),
            KeyCode::Char('a') => self.begin_edit_account(true),
            KeyCode::Char('/') => {
                if let Some(s) = &mut self.settings {
                    s.filtering = true;
                }
            }
            KeyCode::Char('d') if !key.ctrl => {
                let idx = self.settings.as_ref().map(|s| s.selected).unwrap_or(0);
                self.request_delete_account(idx);
            }
            KeyCode::Char('x') => {
                let idx = self.settings.as_ref().map(|s| s.selected).unwrap_or(0);
                self.toggle_default_account(idx).await;
            }
            _ => {}
        }
    }

    /// Keep `selected` pointing at a visible (filtered) account. Called after the
    /// filter text changes so the highlighted row stays in the result set.
    fn settings_clamp_account_selection(&mut self) {
        let filtered = match &self.settings {
            Some(s) if s.focus == SettingsFocus::Accounts => s.filtered_accounts(),
            _ => return,
        };
        if filtered.is_empty() {
            return;
        }
        if let Some(s) = &mut self.settings
            && !filtered.contains(&s.selected)
        {
            s.selected = filtered[0];
        }
    }

    /// Move the account-list selection up/down within the filtered set.
    fn settings_move(&mut self, delta: isize) {
        let Some(s) = &self.settings else { return };
        let filtered = s.filtered_accounts();
        if filtered.is_empty() {
            return;
        }
        let cur = filtered.iter().position(|&i| i == s.selected).unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, filtered.len() as isize - 1) as usize;
        if let Some(s) = &mut self.settings {
            s.selected = filtered[next];
        }
    }

    /// Cycle focus through the account-edit form fields (order depends on the
    /// selected provider's auth kind).
    fn cycle_settings_focus(&mut self, dir: isize) {
        let order = self.form_focus_order();
        let Some(state) = &mut self.settings else {
            return;
        };
        let pos = order.iter().position(|f| *f == state.focus).unwrap_or(0) as isize;
        let len = order.len() as isize;
        state.focus = order[(pos + dir).rem_euclid(len) as usize];
    }
}
