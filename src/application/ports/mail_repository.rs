use anyhow::Result;
use async_trait::async_trait;

use crate::domain::{Envelope, Folder, Message};

#[allow(dead_code)]
#[async_trait]
pub trait MailCache: Send + Sync {
    async fn upsert_folders(&self, account_id: &str, folders: &[Folder]) -> Result<()>;
    async fn load_folders(&self, account_id: &str) -> Result<Vec<Folder>>;
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
    /// Move a cached message to another folder (Archive). No-op if missing.
    async fn move_message(
        &self,
        account_id: &str,
        from_folder: &str,
        uid: u64,
        to_folder: &str,
    ) -> Result<()>;
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
