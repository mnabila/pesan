use anyhow::Result;
use async_trait::async_trait;

use crate::application::account::connect_params::ConnectParams;
use crate::application::mail::fetch::MailSource;
use crate::application::oauth::{ResolvedOAuth, TokenSet};

/// The mail transport port: OAuth token refresh plus live/offline source
/// construction. Implemented by `infrastructure::mail::backend::ImapBackend`.
#[async_trait]
pub trait MailBackend: Send + Sync {
    /// Refresh an OAuth access token (blocking `reqwest` on a dedicated thread
    /// inside the adapter).
    async fn refresh_access_token(
        &self,
        oauth: ResolvedOAuth,
        refresh_token: String,
    ) -> Result<TokenSet>;

    /// Establish a live IMAP session and return the ready source. The blocking
    /// TLS+auth handshake runs off-runtime inside the adapter. `on_lost`, when
    /// set, receives the account label if the session wedges later.
    async fn connect(
        &self,
        params: ConnectParams,
        on_lost: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    ) -> Result<Box<dyn MailSource>>;

    /// Build the offline display source for an account: cache-backed when the
    /// account has cached mail, else an empty source.
    async fn offline_source(&self, account_id: Option<&str>) -> Box<dyn MailSource>;
}
