use crate::application::account::Account;
use crate::bootstrap::config::Config;
use crate::tui::widgets::TextInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsFocus {
    Accounts,
    Name,
    Email,
    Provider,
    Password,
    IsDefault,
    /// OAuth providers: the "Authorize with <provider>" button.
    Authorize,
    /// Password providers: the "Save account" button.
    Save,
}

pub struct AccountForm {
    pub name: TextInput,
    pub email: TextInput,
    /// Login password, used only for password-auth (non-OAuth) providers.
    pub password: TextInput,
    pub provider_idx: usize,
    pub is_default: bool,
}

pub struct SettingsState {
    pub accounts: Vec<Account>,
    pub providers: Vec<String>,
    pub selected: usize,
    pub editing: bool,
    pub is_new: bool,
    /// True while the new-account wizard is on its first step (provider chooser)
    /// rather than the identity/auth form. Editing an existing account skips
    /// this and goes straight to the form.
    pub choosing_provider: bool,
    /// Selection index within `providers` for the chooser screen.
    pub choose_idx: usize,
    pub form: Option<AccountForm>,
    /// Within the account form, `false` navigates fields and `true` types into
    /// the focused text field (entered with `e`, left with Esc/Enter).
    pub field_editing: bool,
    pub focus: SettingsFocus,
    /// Live substring filter over the account list (matches name/email/provider).
    pub account_filter: String,
    /// True while the user is typing into `account_filter`.
    pub filtering: bool,
}

impl SettingsState {
    pub fn new(accounts: Vec<Account>, _config: &Config, providers: Vec<String>) -> Self {
        Self {
            accounts,
            providers,
            selected: 0,
            editing: false,
            is_new: false,
            choosing_provider: false,
            choose_idx: 0,
            form: None,
            field_editing: false,
            focus: SettingsFocus::Accounts,
            account_filter: String::new(),
            filtering: false,
        }
    }

    /// Indices of accounts matching `account_filter` (case-insensitive substring
    /// over name/email/provider). An empty filter returns every account.
    pub fn filtered_accounts(&self) -> Vec<usize> {
        let q = self.account_filter.trim().to_lowercase();
        if q.is_empty() {
            return (0..self.accounts.len()).collect();
        }
        self.accounts
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                a.name.to_lowercase().contains(&q)
                    || a.email.to_lowercase().contains(&q)
                    || a.provider.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }
}
