use super::*;

impl App {
    pub(crate) fn request_delete_account(&mut self, idx: usize) {
        let Some(state) = &self.settings else { return };
        if let Some(account) = state.accounts.get(idx) {
            let name = account.name.clone();
            self.confirm = Some(ConfirmState {
                prompt: format!("Delete account \"{name}\"?"),
                action: ConfirmAction::DeleteAccount(idx),
            });
        }
    }

    pub(crate) async fn toggle_default_account(&mut self, idx: usize) {
        let Some(state) = &mut self.settings else {
            return;
        };
        if let Some(account) = state.accounts.get(idx).cloned()
            && let Some(account_id) = account.id
        {
            let _ = self.services.accounts.set_default(account_id).await;
            self.reload_accounts().await;
            self.set_toast(
                format!("{} is now the default account", account.name),
                ToastKind::Success,
            );
        }
    }

    /// Queue the real OAuth flow for the account being edited. The event loop
    /// runs the (blocking) browser consent and calls [`Self::apply_oauth_result`].
    /// Validates the form and resolves provider credentials up front so config
    /// mistakes surface immediately rather than after a browser round-trip.
    pub(crate) fn request_authorize(&mut self) {
        let Some(state) = &self.settings else { return };
        let Some(form) = &state.form else { return };
        // Name/email are optional: OAuth fills the email from the provider and the
        // name defaults to it (both editable later). Only the provider is required.
        let name = form.name.text().trim().to_string();
        let email = form.email.text().trim().to_string();
        let provider_key = state
            .providers
            .get(form.provider_idx)
            .cloned()
            .unwrap_or_else(|| "gmail".to_string());
        let Some(provider) = self.config.providers.get(&provider_key) else {
            self.set_toast(
                format!("Provider '{provider_key}' is not defined in config.yaml"),
                ToastKind::Error,
            );
            return;
        };
        let Some(oauth_cfg) = provider.oauth.as_ref() else {
            self.set_toast(
                format!("Provider '{provider_key}' has no OAuth; add it with a password instead"),
                ToastKind::Warning,
            );
            return;
        };
        let oauth = match ResolvedOAuth::from_config(oauth_cfg) {
            Ok(o) => o,
            Err(e) => {
                self.set_toast(format!("OAuth config: {e}"), ToastKind::Error);
                return;
            }
        };

        // Keep an existing account's keychain ref; a new account's ref is minted
        // in `apply_oauth_result` once the email (hence the name) is known.
        let existing = if state.is_new {
            None
        } else {
            state.accounts.get(state.selected).cloned()
        };
        let keychain_ref = existing
            .as_ref()
            .map(|a| a.keychain_ref.clone())
            .filter(|r| !r.trim().is_empty())
            .unwrap_or_default();

        let account = Account {
            id: existing.as_ref().and_then(|a| a.id),
            name,
            email,
            provider: provider_key,
            keychain_ref,
            is_default: form.is_default,
            created_at: existing.map(|a| a.created_at).unwrap_or_else(now_ts),
        };
        self.pending_oauth = Some(PendingOAuth {
            oauth,
            set_default: form.is_default,
            account,
        });
        self.oauth_in_progress = true;
        self.set_toast("Opening browser for authorization...", ToastKind::Info);
    }

    pub fn take_pending_oauth(&mut self) -> Option<PendingOAuth> {
        self.pending_oauth.take()
    }

    /// Queue a *background* re-authorization for an existing account: when a
    /// live connect fails because the stored credential was rejected (expired/
    /// revoked refresh token), or at startup for an account that was saved but
    /// never authorized. The event loop runs the browser consent flow and
    /// [`Self::apply_oauth_result`] stores the fresh refresh token and
    /// reconnects - no trip through the account manager needed. Once per
    /// account per session; never while another OAuth flow is queued.
    pub(crate) async fn queue_auto_reauth(&mut self, account: &Account) {
        if self.pending_oauth.is_some() || self.oauth_in_progress {
            return;
        }
        // A failed/cancelled consent must not loop browser prompts this session.
        if !self.auto_reauth_attempted.insert(account.name.clone()) {
            return;
        }
        let Some(provider) = self.config.providers.get(&account.provider) else {
            return;
        };
        let Some(oauth_cfg) = provider.oauth.as_ref() else {
            return; // password account: nothing to re-consent
        };
        let Ok(oauth) = ResolvedOAuth::from_config(oauth_cfg) else {
            return;
        };
        tracing::info!("auto re-authorization for {}", account.name);
        self.pending_oauth = Some(PendingOAuth {
            oauth,
            set_default: false,
            account: account.clone(),
        });
        self.oauth_in_progress = true;
        self.set_toast(
            format!("Session expired - re-authorizing {}...", account.name),
            ToastKind::Info,
        );
    }

    /// Enter the redirect-paste step: the browser consent has been opened and we
    /// now wait for the user to paste back the URL the browser landed on. Called
    /// by the event loop once [`crate::infrastructure::auth::oauth::begin_auth_code_flow`]
    /// returns. The input is pre-focused so typing/pasting goes straight in.
    pub fn begin_oauth_paste(&mut self, req: PendingOAuth, flow: AuthCodeFlow) {
        let mut input = TextInput::new("");
        input.focus(true);
        self.oauth_paste = Some(OAuthPaste { req, flow, input });
        self.oauth_in_progress = true;
        self.set_toast(
            "Consent opened in your browser - paste the redirect URL, then Enter",
            ToastKind::Info,
        );
    }

    /// The user submitted the pasted redirect URL (Enter). Hand the flow to the
    /// event loop, which runs the (blocking) code exchange off the runtime. A
    /// blank paste is rejected so Enter doesn't kick off a doomed exchange.
    pub(crate) fn submit_oauth_paste(&mut self) {
        let blank = self
            .oauth_paste
            .as_ref()
            .is_none_or(|p| p.input.text().trim().is_empty());
        if blank {
            self.set_toast(
                "Paste the redirect URL first (or Esc to cancel)",
                ToastKind::Warning,
            );
            return;
        }
        self.oauth_submit = self.oauth_paste.take();
        self.set_toast("Exchanging authorization code...", ToastKind::Info);
    }

    /// Taken by the event loop to run the code exchange for a submitted paste.
    pub fn take_oauth_submit(&mut self) -> Option<OAuthPaste> {
        self.oauth_submit.take()
    }

    /// True when the redirect-paste prompt should render *inline* as a split
    /// panel under the Account Manager form - i.e. the user launched Authorize
    /// from the "New/Edit account" form. Otherwise (auto re-auth, startup
    /// consent) it floats as a centered overlay.
    pub fn oauth_paste_inline(&self) -> bool {
        self.oauth_paste.is_some()
            && self.view == View::Settings
            && self.settings.as_ref().is_some_and(|s| s.editing)
    }

    /// The user aborted a queued OAuth consent flow (Esc, typically after
    /// closing the browser tab or before pasting). Drop the paste overlay and
    /// note it, without treating it as a failure. The account is left as-is; a
    /// manual Authorize from Settings can retry, and (for auto re-auth) the
    /// once-per-session guard already recorded this account so we won't nag.
    pub fn cancel_oauth(&mut self) {
        self.oauth_paste = None;
        self.oauth_submit = None;
        self.oauth_in_progress = false;
        self.set_toast("Authorization cancelled", ToastKind::Info);
    }

    /// Persist the outcome of an OAuth run: on success store the refresh token
    /// in the keychain and upsert the account row; on failure surface the error.
    pub async fn apply_oauth_result(
        &mut self,
        mut req: PendingOAuth,
        result: anyhow::Result<TokenSet>,
    ) {
        // The browser flow is over; drop the progress overlay.
        self.oauth_in_progress = false;
        let tokens = match result {
            Ok(t) => t,
            Err(e) => {
                self.set_toast(format!("Authorization failed: {e}"), ToastKind::Error);
                return;
            }
        };
        // Auto-fill from the provider's id_token when the user didn't type these:
        // the email address, and the local name label from the real display name
        // (`name` claim), falling back to the email only if no name is returned.
        // Both stay editable afterward via `o`.
        if let Some(email) = tokens.email.as_deref().filter(|e| !e.is_empty())
            && req.account.email.trim().is_empty()
        {
            req.account.email = email.to_string();
        }
        if req.account.name.trim().is_empty() {
            req.account.name = tokens
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| req.account.email.clone());
        }
        if req.account.name.trim().is_empty() {
            self.set_toast(
                "Provider returned no email - open the account and set name/email",
                ToastKind::Warning,
            );
            return;
        }
        // Mint a keychain ref now that the name is settled (new accounts only).
        if req.account.keychain_ref.trim().is_empty() {
            req.account.keychain_ref = keychain_ref_for(&req.account.name);
        }
        let Some(refresh) = tokens.refresh_token else {
            self.set_toast(
                "No refresh token returned - re-consent with offline access",
                ToastKind::Error,
            );
            return;
        };
        let set_default = req.set_default;
        match crate::application::account::onboarding::authorize_persist(
            &self.services,
            &req.account,
            &refresh,
            set_default,
        )
        .await
        {
            Ok(_id) => {
                self.reload_accounts().await;
                // Leave the edit form now that the account is fully connected.
                if let Some(state) = &mut self.settings {
                    state.editing = false;
                    state.form = None;
                    state.is_new = false;
                    state.focus = SettingsFocus::Accounts;
                }
                self.set_toast(
                    format!(
                        "Authorized {} - token stored in keychain",
                        req.account.email
                    ),
                    ToastKind::Success,
                );
                // Connect the freshly authorized account live so its real mail
                // is ready when the user leaves the settings screen.
                if let Some(i) = self
                    .accounts
                    .iter()
                    .position(|a| a.name == req.account.name)
                {
                    // Connect in the background so the settings screen stays
                    // responsive (the user can close it while mail loads).
                    self.active_account = i;
                    self.selected_folder = 0;
                    self.spawn_connect(i).await;
                }
            }
            Err(e) => self.set_toast(format!("Account save failed: {e}"), ToastKind::Error),
        }
    }

    /// Save a password-login (non-OAuth) account: validate the form, stash the
    /// password in the keyring, upsert the row, then fall back to the list and
    /// connect live. Mirrors [`Self::apply_oauth_result`] for the password path.
    pub(crate) async fn save_password_account(&mut self) {
        let Some(state) = &self.settings else { return };
        let Some(form) = &state.form else { return };
        let name = form.name.text().trim().to_string();
        let email = form.email.text().trim().to_string();
        let password = form.password.text().to_string();
        if name.is_empty() || email.is_empty() {
            self.set_toast("Name and email are required", ToastKind::Warning);
            return;
        }
        if password.is_empty() {
            self.set_toast("Enter a password for this account", ToastKind::Warning);
            return;
        }
        let provider_key = state
            .providers
            .get(form.provider_idx)
            .cloned()
            .unwrap_or_default();

        // Keep an existing account's keychain ref; mint a password ref otherwise.
        let existing = if state.is_new {
            None
        } else {
            state.accounts.get(state.selected).cloned()
        };
        let keychain_ref = existing
            .as_ref()
            .map(|a| a.keychain_ref.clone())
            .filter(|r| !r.trim().is_empty())
            .unwrap_or_else(|| keychain_ref_for_password(&name));
        let is_default = form.is_default;
        let account = Account {
            id: existing.as_ref().and_then(|a| a.id),
            name,
            email,
            provider: provider_key,
            keychain_ref: keychain_ref.clone(),
            is_default,
            created_at: existing.map(|a| a.created_at).unwrap_or_else(now_ts),
        };

        if let Err(e) = crate::application::account::onboarding::save_password(
            &self.services,
            &account,
            &password,
            is_default,
        )
        .await
        {
            self.set_toast(format!("Account save failed: {e}"), ToastKind::Error);
            return;
        }
        self.reload_accounts().await;
        if let Some(state) = &mut self.settings {
            state.editing = false;
            state.form = None;
            state.is_new = false;
            state.focus = SettingsFocus::Accounts;
        }
        self.set_toast(
            format!("Saved {} - password stored", account.email),
            ToastKind::Success,
        );
        if let Some(i) = self.accounts.iter().position(|a| a.name == account.name) {
            self.active_account = i;
            self.selected_folder = 0;
            self.spawn_connect(i).await;
        }
    }

    pub(crate) fn begin_edit_account(&mut self, is_new: bool) {
        let Some(state) = &mut self.settings else {
            return;
        };
        let (name, email, provider_idx, is_default) = if is_new {
            (String::new(), String::new(), 0, self.accounts.is_empty())
        } else if let Some(account) = state.accounts.get(state.selected).cloned() {
            let pidx = state
                .providers
                .iter()
                .position(|p| *p == account.provider)
                .unwrap_or(0);
            (
                account.name.clone(),
                account.email.clone(),
                pidx,
                account.is_default,
            )
        } else {
            (String::new(), String::new(), 0, false)
        };
        state.editing = true;
        state.is_new = is_new;
        state.field_editing = false;
        state.form = Some(AccountForm {
            name: TextInput::new(name),
            email: TextInput::new(email),
            password: TextInput::new(String::new()),
            provider_idx,
            is_default,
        });
        // Start on the first field of the provider-appropriate field order.
        self.settings_reset_focus_to_first();
    }

    /// Point form focus at the first field for the current provider's auth kind.
    fn settings_reset_focus_to_first(&mut self) {
        let first = self
            .form_focus_order()
            .first()
            .copied()
            .unwrap_or(SettingsFocus::Provider);
        if let Some(state) = &mut self.settings {
            state.focus = first;
        }
    }

    /// True when the account form's currently selected provider authenticates via
    /// OAuth2 (has an `oauth` block). Defaults to `true` when unknown.
    pub(crate) fn form_provider_is_oauth(&self) -> bool {
        let Some(state) = &self.settings else {
            return true;
        };
        let Some(form) = &state.form else {
            return true;
        };
        state
            .providers
            .get(form.provider_idx)
            .and_then(|p| self.config.providers.get(p))
            .map(|p| p.is_oauth())
            .unwrap_or(true)
    }

    /// The account-form fields, in navigation order, for the selected provider.
    /// OAuth providers auto-fill the email (so it is not a field) and end on an
    /// Authorize button; password providers take a password and end on Save.
    pub(crate) fn form_focus_order(&self) -> Vec<SettingsFocus> {
        if self.form_provider_is_oauth() {
            vec![
                SettingsFocus::Provider,
                SettingsFocus::Name,
                SettingsFocus::IsDefault,
                SettingsFocus::Authorize,
            ]
        } else {
            vec![
                SettingsFocus::Name,
                SettingsFocus::Email,
                SettingsFocus::Provider,
                SettingsFocus::Password,
                SettingsFocus::IsDefault,
                SettingsFocus::Save,
            ]
        }
    }

    pub(crate) fn open_settings(&mut self) {
        let providers = self.config.providers.keys().cloned().collect::<Vec<_>>();
        let state = SettingsState::new(self.accounts.clone(), &self.config, providers);
        self.settings = Some(state);
        self.view = View::Settings;
    }

    pub(crate) async fn reload_accounts(&mut self) {
        self.accounts = self.services.accounts.list().await.unwrap_or_default();
        if self.active_account >= self.accounts.len() {
            self.active_account = self.accounts.len().saturating_sub(1);
        }
        let fresh = self.source.list_folders().await.unwrap_or_default();
        if fresh.len() != self.folders.len() {
            self.folder_collapsed = vec![false; fresh.len()];
        } else {
            self.folder_collapsed.resize(fresh.len(), false);
        }
        self.folders = fresh;
        if self.selected_folder >= self.folders.len() {
            self.selected_folder = self.folders.len().saturating_sub(1);
        }
        if let Some(state) = &mut self.settings {
            let mut selector = state.selected.min(state.accounts.len().saturating_sub(1));
            // Re-copy the (possibly re-ordered) account list into the settings pane.
            let accounts = self.services.accounts.list().await.unwrap_or_default();
            let prev_selected_name = state.accounts.get(state.selected).map(|a| a.name.clone());
            state.accounts = accounts;
            selector = prev_selected_name
                .as_ref()
                .and_then(|n| state.accounts.iter().position(|a| a.name == *n))
                .unwrap_or_else(|| selector.min(state.accounts.len().saturating_sub(1)));
            state.selected = selector;
        }
    }

    pub(crate) fn close_settings(&mut self) {
        self.settings = None;
        self.view = View::Main;
        self.rebuild_theme_glyphs_from_config();
    }

    pub(crate) async fn save_settings(&mut self) {
        // Saving a password-provider form must persist the password too, so route
        // it through the dedicated path rather than the OAuth-agnostic upsert.
        let editing = self.settings.as_ref().is_some_and(|s| s.editing);
        if editing && !self.form_provider_is_oauth() {
            self.save_password_account().await;
            return;
        }
        if let Some(state) = &mut self.settings {
            if state.editing
                && let Some(form) = &state.form
            {
                let name = form.name.text().trim().to_string();
                let email = form.email.text().trim().to_string();
                if name.is_empty() || email.is_empty() {
                    self.set_toast("Name and email are required", ToastKind::Warning);
                    return;
                }
                let provider = state
                    .providers
                    .get(form.provider_idx)
                    .cloned()
                    .unwrap_or_else(|| "gmail".to_string());
                let (id, current) = if state.is_new {
                    (None, None)
                } else {
                    (
                        state.accounts.get(state.selected).and_then(|a| a.id),
                        state.accounts.get(state.selected).cloned(),
                    )
                };
                let is_default = form.is_default;
                let account = Account {
                    id,
                    name,
                    email,
                    provider,
                    keychain_ref: current.map(|a| a.keychain_ref).unwrap_or_default(),
                    is_default,
                    created_at: chrono::Utc::now().timestamp(),
                };
                if let Ok(saved_id) = self.services.accounts.upsert(&account).await
                    && is_default
                {
                    let _ = self.services.accounts.set_default(saved_id).await;
                }
            }

            // Exit account-edit mode now that the form is persisted. UI
            // preferences are edited in config.yaml, so nothing else is written.
            if state.editing {
                state.editing = false;
                state.form = None;
                state.is_new = false;
                state.focus = SettingsFocus::Accounts;
            }
        }
        self.reload_accounts().await;
        self.set_toast("Settings saved", ToastKind::Success);
    }

    fn rebuild_theme_glyphs_from_config(&mut self) {
        let (name, theme) = Self::resolve_theme(&self.config.ui.theme, &self.config.themes);
        self.theme_name = name;
        self.theme = theme;
        self.keymap_table = crate::tui::keymap::build_table(&self.config.keybinding);
        self.glyphs = Glyphs::new(self.config.ui.ascii);
        self.border_type = parse_border_type(&self.config.ui.border_type);
        // Theme/ascii feed the reader render; force a rebuild on the next frame.
        self.reader_dirty = true;
    }
}
