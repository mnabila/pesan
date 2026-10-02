use super::*;

use crate::ui::keymap;

impl App {
    /// Compose is an inline form: header/attach fields are edited with their
    /// `TextInput`, the body opens in `$EDITOR`. App-level keys (send/draft/
    /// discard/editor) are reserved before keystrokes reach a field.
    pub(crate) async fn compose_key(&mut self, key: &Key) {
        let Some((focus, editing)) = self.compose.as_ref().map(|c| (c.focus, c.editing)) else {
            return;
        };

        // Send / save-draft and field-cycling (focus next/prev) work from
        // anywhere, honoring user remaps; cycling also leaves edit mode.
        // Scope to the compose keymap only (not Global): otherwise the global
        // pane-switch keys H/L would leak in as focus prev/next.
        if let Some(a) = keymap::resolve_in_ctx(
            keymap::Ctx::Compose,
            &[*key],
            &self.keymap_table,
        ) {
            match a {
                Action::Send | Action::SaveDraft => {
                    self.action(a).await;
                    return;
                }
                Action::FocusNext => {
                    if let Some(c) = &mut self.compose {
                        c.editing = false;
                        c.focus = c.focus.next();
                    }
                    return;
                }
                Action::FocusPrev => {
                    if let Some(c) = &mut self.compose {
                        c.editing = false;
                        c.focus = c.focus.prev();
                    }
                    return;
                }
                _ => {}
            }
        }

        if editing {
            self.compose_edit_key(key, focus).await;
        } else {
            // Nav mode: honor the rest of the compose keymap (external editor,
            // discard) and the global overlay toggles, then fall back to the
            // built-in field navigation.
            if let Some(a) = keymap::resolve(
                keymap::Ctx::Compose,
                &[*key],
                &self.keymap_table,
            ) {
                match a {
                    Action::ExternalEditor | Action::DiscardDraft => {
                        self.action(a).await;
                        return;
                    }
                    _ => {}
                }
            }
            if let Some(Action::Help) =
                keymap::resolve(keymap::Ctx::Global, &[*key], &self.keymap_table)
            {
                self.action(Action::Help).await;
                return;
            }
            self.compose_nav_key(key, focus).await;
        }
    }

    /// Editing mode: keystrokes flow into the focused text field; Enter/Esc
    /// leave edit mode.
    async fn compose_edit_key(&mut self, key: &Key, _focus: ComposeFocus) {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => {
                if let Some(c) = &mut self.compose {
                    c.editing = false;
                }
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

    /// Navigation mode, fully table-driven against the `Compose` keymap: move
    /// field focus, scroll the body when it is focused, or start editing the
    /// focused field / open $EDITOR on the body. Send / draft / discard / editor
    /// / help are consumed earlier in [`Self::compose_key`].
    async fn compose_nav_key(&mut self, key: &Key, focus: ComposeFocus) {
        let Some(action) = keymap::resolve_in_ctx(
            keymap::Ctx::Compose,
            &[*key],
            &self.keymap_table,
        ) else {
            return;
        };
        match action {
            Action::FocusDown | Action::FocusNext => self.compose_focus_step('j'),
            Action::FocusUp | Action::FocusPrev => self.compose_focus_step('k'),
            Action::FocusLeft => self.compose_focus_step('h'),
            Action::FocusRight => self.compose_focus_step('l'),
            // On the body row, edit means "open $EDITOR"; on a header/attach row
            // it drops into the inline field editor.
            Action::EditField => {
                if focus == ComposeFocus::Body {
                    self.action(Action::ExternalEditor).await;
                } else if let Some(c) = &mut self.compose {
                    c.editing = true;
                    if let Some(input) = c.focused_input_mut() {
                        input.focus(true);
                    }
                }
            }
            Action::FilePicker => self.action(Action::FilePicker).await,
            Action::RemoveAttachment => self.remove_last_attachment(),
            // Body scrolling: only meaningful while the body row is focused.
            Action::ScrollDown if focus == ComposeFocus::Body => self.scroll_compose_body(1),
            Action::ScrollUp if focus == ComposeFocus::Body => self.scroll_compose_body(-1),
            Action::PageDown if focus == ComposeFocus::Body => self.scroll_compose_body(10),
            Action::PageUp if focus == ComposeFocus::Body => self.scroll_compose_body(-10),
            Action::ScrollStart if focus == ComposeFocus::Body => {
                if let Some(c) = &mut self.compose {
                    c.body_scroll = 0;
                }
            }
            Action::ScrollEnd if focus == ComposeFocus::Body => {
                if let Some(c) = &mut self.compose {
                    c.body_scroll = c.body.lines().count().saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    /// Move compose field focus one grid step (`h`/`j`/`k`/`l`).
    fn compose_focus_step(&mut self, dir: char) {
        if let Some(c) = &mut self.compose {
            c.focus = c.focus.grid_step(dir);
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

        // Provider chooser (new-account step 1) is its own mode: intercept before
        // the form/edit dispatch and the save/help app keys below.
        if self.settings.as_ref().is_some_and(|s| s.choosing_provider) {
            self.settings_choose_provider_key(key).await;
            return;
        }

        // Shift+W saves, except while actively typing into a form text field,
        // where the character must reach the input instead.
        let field_editing = self.settings.as_ref().is_some_and(|s| s.field_editing);

        // Honor user remaps for the settings keymap (save / close). While a form
        // text field is being typed into, these keys must still reach the input,
        // so the save guard respects `field_editing` and close respects `editing`.
        if !field_editing {
            if let Some(a) = keymap::resolve(
                keymap::Ctx::Settings,
                &[*key],
                &self.keymap_table,
            ) {
                match a {
                    Action::SaveSettings => {
                        self.action(a).await;
                        return;
                    }
                    Action::CloseSettings if !editing => {
                        self.action(a).await;
                        return;
                    }
                    _ => {}
                }
            }
            // `?` opens the help overlay (global toggle) unless typing into a field.
            if let Some(Action::Help) = keymap::resolve(
                keymap::Ctx::Global,
                &[*key],
                &self.keymap_table,
            ) {
                self.action(Action::Help).await;
                return;
            }
        }
        // `q` closes the account manager even if the user has not remapped it.
        if key.code == KeyCode::Char('q') && !editing && !key.ctrl {
            self.action(Action::CloseSettings).await;
            return;
        }

        if editing {
            self.settings_edit_key(key, focus).await;
        } else {
            self.settings_nav_key(key, focus).await;
        }
    }

    /// Provider chooser (new-account step 1): `j`/`k` move the selection,
    /// `Enter`/`Space` confirms and advances to the identity/auth form, `Esc`
    /// cancels back to the account list. Its own mode, so it intercepts before
    /// the form/edit dispatch above.
    async fn settings_choose_provider_key(&mut self, key: &Key) {
        let max = self
            .settings
            .as_ref()
            .map(|s| s.providers.len().saturating_sub(1))
            .unwrap_or(0);
        match key.code {
            KeyCode::Esc => {
                if let Some(s) = &mut self.settings {
                    s.choosing_provider = false;
                    if s.is_new {
                        // Cancelling the new-account wizard discards the form.
                        s.editing = false;
                        s.form = None;
                        s.focus = SettingsFocus::Accounts;
                    } else {
                        // Cancelling a provider switch while editing returns to
                        // the identity/auth form, keeping its fields.
                        s.focus = SettingsFocus::Provider;
                    }
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(s) = &mut self.settings {
                    s.choose_idx = (s.choose_idx + 1).min(max);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(s) = &mut self.settings {
                    s.choose_idx = s.choose_idx.saturating_sub(1);
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.choose_provider(),
            _ => {}
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

        // Field navigation is table-driven against the `SettingsForm` keymap, so
        // every key here is user-rebindable (see `keymap/table.rs`).
        let Some(action) = keymap::resolve_in_ctx(
            keymap::Ctx::SettingsForm,
            &[*key],
            &self.keymap_table,
        ) else {
            return;
        };
        match action {
            // Esc / close leaves the form back to the account list.
            Action::CloseSettings => {
                if let Some(s) = &mut self.settings {
                    s.editing = false;
                    s.focus = SettingsFocus::Accounts;
                }
            }
            Action::FocusDown | Action::FocusNext => self.cycle_settings_focus(1),
            Action::FocusUp | Action::FocusPrev => self.cycle_settings_focus(-1),
            Action::FocusRight => self.settings_focus_horizontal(1),
            Action::FocusLeft => self.settings_focus_horizontal(-1),
            Action::EditField => self.settings_activate_field(focus, text_field).await,
            // A remap that puts save in the form section resolves here; the plain
            // `W` is already handled by `settings_key` before this point.
            Action::SaveSettings => self.save_settings().await,
            _ => {}
        }
    }

    /// Activate the focused account-form field: text fields drop into the inline
    /// editor; the provider row opens the chooser; the default toggle flips; the
    /// authorize / save buttons run their action.
    async fn settings_activate_field(&mut self, focus: Option<SettingsFocus>, text_field: bool) {
        match focus {
            Some(SettingsFocus::Name | SettingsFocus::Email | SettingsFocus::Password) => {
                if text_field
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
                // Editing an existing account: switch providers through the same
                // chooser the new-account wizard uses (clearer than a tiny inline
                // dropdown).
                if let Some(s) = &mut self.settings
                    && let Some(form) = s.form.as_mut()
                {
                    s.choose_idx = form.provider_idx;
                    s.choosing_provider = true;
                    s.field_editing = false;
                }
            }
            Some(SettingsFocus::IsDefault) => {
                if let Some(s) = &mut self.settings
                    && let Some(form) = s.form.as_mut()
                {
                    form.is_default = !form.is_default;
                }
            }
            Some(SettingsFocus::Authorize) => self.request_authorize(),
            Some(SettingsFocus::Save) => self.save_password_account().await,
            _ => {}
        }
    }

    /// Account-list navigation, table-driven against the `Settings` keymap: move
    /// the selection or act on the highlighted account (open / add / delete / set
    /// default / filter). Save / close / help are consumed earlier in
    /// [`Self::settings_key`].
    async fn settings_nav_key(&mut self, key: &Key, _focus: Option<SettingsFocus>) {
        let Some(action) = keymap::resolve_in_ctx(
            keymap::Ctx::Settings,
            &[*key],
            &self.keymap_table,
        ) else {
            return;
        };
        match action {
            Action::MoveUp => self.settings_move(-1),
            Action::MoveDown => self.settings_move(1),
            Action::OpenAccount => self.begin_edit_account(false),
            Action::AddAccount => self.begin_add_account(),
            Action::FilterAccounts => {
                if let Some(s) = &mut self.settings {
                    s.filtering = true;
                }
            }
            Action::DeleteAccount => {
                let idx = self.settings.as_ref().map(|s| s.selected).unwrap_or(0);
                self.request_delete_account(idx);
            }
            Action::SetDefaultAccount => {
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

    /// Move focus horizontally (left/right) within the 2-column form grid. The
    /// focus order is row-major, so the horizontal neighbour of item `i` is `i`
    /// with its low bit toggled - the other column in the same row, when it
    /// exists.
    fn settings_focus_horizontal(&mut self, dir: isize) {
        let order = self.form_focus_order();
        let Some(state) = &mut self.settings else {
            return;
        };
        let pos = order.iter().position(|f| *f == state.focus).unwrap_or(0) as isize;
        let len = order.len() as isize;
        let target = if dir > 0 {
            // Move right: from a left-column (even) item to the right one.
            if pos % 2 == 0 { pos + 1 } else { pos }
        } else {
            // Move left: from a right-column (odd) item to the left one.
            if pos % 2 == 1 { pos - 1 } else { pos }
        };
        if target >= 0 && target < len {
            state.focus = order[target as usize];
        }
    }
}
