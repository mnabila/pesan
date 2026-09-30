use anyhow::Result;
use async_trait::async_trait;

use crate::mail::application::imap_cmd::ImapHandle;
use crate::mail::{Draft, Envelope, Folder, Message};

#[async_trait]
pub trait MailSource: Send {
    async fn list_folders(&self) -> Result<Vec<Folder>>;

    async fn list_messages(&self, folder: &str) -> Result<Vec<Envelope>>;

    async fn fetch_message(&self, uid: u64) -> Result<Message>;

    async fn send(&mut self, _draft: &Draft) -> Result<()> {
        Ok(())
    }

    /// Mark `\Seen` on a message.
    async fn set_seen(&mut self, _uid: u64, _seen: bool) {}

    /// Toggle `\Flagged`.
    async fn set_flagged(&mut self, _uid: u64, _flagged: bool) {}

    /// Permanently remove a message (IMAP: `\Deleted` + EXPUNGE).
    async fn delete(&mut self, uid: u64) -> Result<()>;

    /// Move a message to another mailbox. Sources that don't distinguish moves
    /// from deletes fall back to [`MailSource::delete`]; the UI uses this for
    /// Archive.
    async fn move_to(&mut self, uid: u64, _folder: &str) -> Result<()> {
        self.delete(uid).await
    }

    /// A cloneable handle for issuing reads on a background task, when this is a
    /// live IMAP source. `None` for the empty/cache sources (nothing to fetch).
    /// The app uses it to refresh folders/messages without blocking the UI loop.
    fn imap_handle(&self) -> Option<ImapHandle> {
        None
    }
}
