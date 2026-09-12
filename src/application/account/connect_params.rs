use anyhow::Context;

use crate::application::account::Account;
use crate::application::oauth::ResolvedOAuth;
use crate::application::ports::TokenStore;
use crate::bootstrap::config::Config;

/// Everything the worker needs to establish a live session (IMAP) and send
/// outgoing mail (SMTP) for one account. Also reused by the IDLE watcher.
#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub label: String,
    pub email: String,
    pub host: String,
    pub port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// Account's secret-store key, used to cache/read the access token.
    pub keychain_ref: String,
    /// How this account authenticates to IMAP/SMTP.
    pub auth: ImapAuth,
}

/// The credential model for an account: OAuth2 (XOAUTH2 access tokens, refreshed
/// as needed) or a static login password (IMAP/SMTP LOGIN).
#[derive(Debug, Clone)]
pub enum ImapAuth {
    OAuth {
        oauth: ResolvedOAuth,
        refresh_token: String,
        /// A pre-acquired, still-valid access token. When set, the worker skips
        /// the (blocking) OAuth refresh and uses this directly - the
        /// warm-reconnect fast path. `None` refreshes on connect.
        access_token: Option<String>,
        /// Absolute expiry (unix seconds) of `access_token`, so the worker can
        /// tell when to refresh before sending. `None` = unknown (assumed valid).
        access_expires_at: Option<i64>,
    },
    Password {
        password: String,
    },
}

/// Assemble IMAP/SMTP/OAuth connection parameters for an account from config +
/// its stored secret, or `Ok(None)` when the account is not yet authorized (no
/// stored refresh token for OAuth, or no stored password for a login provider).
///
/// The single source of truth for turning a saved account into live-connect
/// params: the TUI (`App::connect_params`) and the headless daemon both call it,
/// so they resolve credentials identically.
pub async fn resolve_connect_params(
    config: &Config,
    account: &Account,
    tokens: &dyn TokenStore,
) -> anyhow::Result<Option<ConnectParams>> {
    let provider = config
        .providers
        .get(&account.provider)
        .with_context(|| {
            format!(
                "provider '{}' is not defined in config.yaml",
                account.provider
            )
        })?;
    // A provider with an `oauth` block authenticates via XOAUTH2; one without
    // uses a stored login password (IMAP/SMTP LOGIN).
    let auth = match &provider.oauth {
        Some(oauth_cfg) => {
            let Some(refresh_token) = tokens.load_refresh_token(&account.keychain_ref).await? else {
                return Ok(None);
            };
            let oauth = ResolvedOAuth::from_config(oauth_cfg)?;
            ImapAuth::OAuth {
                oauth,
                refresh_token,
                // Filled in by the connect path (cached token or a fresh
                // refresh); the IDLE watcher leaves it `None`.
                access_token: None,
                access_expires_at: None,
            }
        }
        None => {
            let Some(password) = tokens.load_password(&account.keychain_ref).await? else {
                return Ok(None);
            };
            ImapAuth::Password { password }
        }
    };
    Ok(Some(ConnectParams {
        label: account.name.clone(),
        email: account.email.clone(),
        host: provider.imap.host.clone(),
        port: provider.imap.port,
        smtp_host: provider.smtp.host.clone(),
        smtp_port: provider.smtp.port,
        keychain_ref: account.keychain_ref.clone(),
        auth,
    }))
}

impl ConnectParams {
    /// Store a freshly obtained access token + expiry into the OAuth auth. A
    /// no-op for password accounts (which carry no token).
    pub fn set_access_token(&mut self, token: String, expires_at: i64) {
        if let ImapAuth::OAuth {
            access_token,
            access_expires_at,
            ..
        } = &mut self.auth
        {
            *access_token = Some(token);
            *access_expires_at = Some(expires_at);
        }
    }
}
