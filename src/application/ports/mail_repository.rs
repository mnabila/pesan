use anyhow::Result;
use async_trait::async_trait;

use crate::domain::{Envelope, Folder, Message};

#[allow(dead_code)]
#[async_trait]
pub trait MailCache: Send + Sync {
    async fn upsert_folders(&self, account_id: i64, folders: &[Folder]) -> Result<()>;
    async fn load_folders(&self, account_id: i64) -> Result<Vec<Folder>>;
    /// Upsert envelopes for a folder, preserving any already-cached body.
    async fn upsert_envelopes(
        &self,
        account_id: i64,
        folder: &str,
        envelopes: &[Envelope],
    ) -> Result<()>;
    async fn load_envelopes(&self, account_id: i64, folder: &str) -> Result<Vec<Envelope>>;
    /// Store a fetched body (and raw headers when available) for a cached message.
    async fn store_body(
        &self,
        account_id: i64,
        folder: &str,
        uid: u64,
        body: &str,
        raw_headers: Option<&str>,
    ) -> Result<()>;
    /// Load a cached message (envelope + body). `body` is empty if not fetched yet.
    async fn load_message(
        &self,
        account_id: i64,
        folder: &str,
        uid: u64,
    ) -> Result<Option<Message>>;
    async fn delete_message(&self, account_id: i64, folder: &str, uid: u64) -> Result<()>;
    /// Move a cached message to another folder (Archive). No-op if missing.
    async fn move_message(
        &self,
        account_id: i64,
        from_folder: &str,
        uid: u64,
        to_folder: &str,
    ) -> Result<()>;
    /// Update seen/flagged flags of a cached message (each optional).
    async fn set_flags(
        &self,
        account_id: i64,
        folder: &str,
        uid: u64,
        seen: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<()>;
    /// True if the account has any cached messages (offline-display decision).
    async fn has_messages(&self, account_id: i64) -> Result<bool>;
    /// Full-text search within one folder's cached mail (subject/sender/body).
    async fn search(&self, account_id: i64, folder: &str, query: &str) -> Result<Vec<Envelope>>;
}
