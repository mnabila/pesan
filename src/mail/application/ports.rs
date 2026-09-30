//! Mail-slice boundary contracts: transport, cache, arrival watching, and
//! desktop notification. Adapters in `mail::adapters` implement these; the UI
//! reaches them only as `dyn` behind these traits.

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use crate::mail::{Envelope, Folder, MailUpdate, Message, MailSource};
use crate::platform::connect_params::ConnectParams;
use crate::platform::oauth::{ResolvedOAuth, TokenSet};
use crate::platform::sound::SoundHint;

/// The mail transport port: OAuth token refresh plus live/offline source
/// construction. Implemented by `mail::infrastructure::backend::ImapBackend`.
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
        on_lost: Option<UnboundedSender<String>>,
    ) -> Result<Box<dyn MailSource>>;

    /// Build the offline display source for an account: cache-backed when the
    /// account has cached mail, else an empty source.
    async fn offline_source(&self, account_id: Option<&str>) -> Box<dyn MailSource>;
}

#[async_trait]
pub trait MailCache: Send + Sync {
    async fn upsert_folders(&self, account_id: &str, folders: &[Folder]) -> Result<()>;
    /// Upsert envelopes for a folder, preserving any already-cached body.
    async fn upsert_envelopes(
        &self,
        account_id: &str,
        folder: &str,
        envelopes: &[Envelope],
    ) -> Result<()>;
    async fn load_envelopes(&self, account_id: &str, folder: &str) -> Result<Vec<Envelope>>;
    /// Cached message counts `(folder, total, unread)` per folder, for an instant
    /// sidebar estimate before the live count sweep. Counts only cached messages.
    async fn count_by_folder(&self, account_id: &str) -> Result<Vec<(String, usize, usize)>>;
    /// Store a fetched message body: the complete `raw` RFC822 bytes are written
    /// verbatim to the interoperable Maildir file, the sole on-disk home for
    /// message content. The SQLite index keeps only the envelope (sender/subject/
    /// flags); the body, HTML source and raw headers are re-derived from the file
    /// on a later open. A no-op when `raw` is `None` (nothing fetched).
    async fn store_body(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
        raw: Option<&[u8]>,
    ) -> Result<()>;
    /// Load a cached message (envelope + body). `body` is empty if not fetched yet.
    async fn load_message(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
    ) -> Result<Option<Message>>;
    async fn delete_message(&self, account_id: &str, folder: &str, uid: u64) -> Result<()>;
    /// Update seen/flagged flags of a cached message (each optional).
    async fn set_flags(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
        seen: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<()>;
    /// True if the account has any cached messages (offline-display decision).
    async fn has_messages(&self, account_id: &str) -> Result<bool>;
    /// Full-text search within one folder's cached mail (subject/sender/body).
    async fn search(&self, account_id: &str, folder: &str, query: &str) -> Result<Vec<Envelope>>;
}

/// A running watcher. Dropping it stops the background work.
pub trait WatchHandle: Send {}

/// Factory port: starts an arrival watcher for one account's mailbox.
#[async_trait]
pub trait NewMailWatch: Send + Sync {
    /// Spawn the watcher on its own thread/connection. It pushes [`MailUpdate`]s
    /// (new arrivals, and - for the daemon-backed watcher - full folder
    /// re-syncs) to `events` until the handle is dropped.
    fn watch(
        &self,
        params: ConnectParams,
        mailbox: String,
        account: String,
        poll_interval: Duration,
        events: UnboundedSender<MailUpdate>,
    ) -> Box<dyn WatchHandle>;
}

pub trait Notifier: Send + Sync {
    fn new_mail_batch(
        &self,
        account: &str,
        envelopes: &[Envelope],
        show_sender: bool,
        sound: Option<&SoundHint>,
    ) -> Result<()>;
}
