use std::sync::Mutex;

use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::application::mail::fetch::MailSource;
use crate::domain::{Draft, Envelope, Folder, Message};
use crate::infrastructure::database::{Db, cache};
use crate::infrastructure::mail::maildir::MaildirStore;

/// Offline mail source backed by a clone of the shared cache-DB pool plus the
/// Maildir store (message content lives in files; the DB is the index).
pub struct CacheSource {
    db: Db,
    maildir: MaildirStore,
    account_id: String,
    /// Tracks the last-selected folder so `fetch_message`/flag ops (which only
    /// receive a UID) know which folder to read, mirroring the live session's
    /// implicit "selected mailbox" state. A `Mutex` (never held across `.await`)
    /// keeps the async trait futures `Send`.
    current_folder: Mutex<String>,
}

impl CacheSource {
    pub fn new(db: Db, maildir: MaildirStore, account_id: String) -> Self {
        Self {
            db,
            maildir,
            account_id,
            current_folder: Mutex::new("INBOX".to_string()),
        }
    }

    fn folder(&self) -> String {
        self.current_folder.lock().unwrap().clone()
    }
}

#[async_trait]
impl MailSource for CacheSource {
    async fn list_folders(&self) -> Result<Vec<Folder>> {
        cache::load_folders(&self.db, &self.account_id).await
    }

    async fn list_messages(&self, folder: &str) -> Result<Vec<Envelope>> {
        *self.current_folder.lock().unwrap() = folder.to_string();
        cache::load_envelopes(&self.db, &self.account_id, folder).await
    }

    async fn fetch_message(&self, uid: u64) -> Result<Message> {
        let folder = self.folder();
        match cache::load_message(&self.db, &self.maildir, &self.account_id, &folder, uid).await? {
            Some(m) if !m.body.is_empty() => Ok(m),
            Some(mut m) => {
                m.body = "(body not cached - reconnect to load the full message)".to_string();
                Ok(m)
            }
            None => bail!("message {uid} is not in the offline cache"),
        }
    }

    async fn send(&mut self, _draft: &Draft) -> Result<()> {
        bail!("offline: cannot send mail from the cache")
    }

    async fn set_seen(&mut self, uid: u64, seen: bool) {
        let folder = self.folder();
        let _ = cache::set_flags(&self.db, &self.maildir, &self.account_id, &folder, uid, Some(seen), None).await;
    }

    async fn set_flagged(&mut self, uid: u64, flagged: bool) {
        let folder = self.folder();
        let _ =
            cache::set_flags(&self.db, &self.maildir, &self.account_id, &folder, uid, None, Some(flagged)).await;
    }

    async fn delete(&mut self, uid: u64) -> Result<()> {
        let folder = self.folder();
        cache::delete_message(&self.db, &self.maildir, &self.account_id, &folder, uid).await
    }

    async fn move_to(&mut self, uid: u64, folder: &str) -> Result<()> {
        let from = self.folder();
        cache::move_message(&self.db, &self.maildir, &self.account_id, &from, uid, folder).await
    }
}
