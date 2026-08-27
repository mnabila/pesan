use crate::application::oauth::ResolvedOAuth;

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
