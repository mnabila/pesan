use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::application::account::Account;
use crate::application::ports::{AccountRepo, MailCache, Notifier, SoundHint, TokenStore};
use crate::domain::{Envelope, Folder, Message};

/// In-memory [`AccountRepo`]: rows keyed by id, name-unique.
#[derive(Default)]
pub struct FakeAccountRepo {
    pub rows: Vec<Account>,
}

#[async_trait]
impl AccountRepo for FakeAccountRepo {
    async fn list(&self) -> Result<Vec<Account>> {
        Ok(self.rows.clone())
    }

    async fn get(&self, id: i64) -> Result<Option<Account>> {
        Ok(self.rows.iter().find(|a| a.id == Some(id)).cloned())
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<Account>> {
        Ok(self.rows.iter().find(|a| a.name == name).cloned())
    }

    async fn upsert(&self, _account: &Account) -> Result<i64> {
        Ok(1)
    }

    async fn delete(&self, _id: i64) -> Result<()> {
        Ok(())
    }

    async fn set_default(&self, _id: i64) -> Result<()> {
        Ok(())
    }
}

/// In-memory [`MailCache`].
#[derive(Default)]
pub struct FakeMailCache {
    /// Envelopes per `(account_id, folder)` as `load_envelopes` returns them.
    pub envelopes: HashMap<(i64, String), Vec<Envelope>>,
    /// Cached bodies per `(account_id, folder, uid)`.
    pub bodies: HashMap<(i64, String, u64), (String, Option<String>)>,
    /// Accounts that "have mail" (drives `has_messages`).
    pub populated: Vec<i64>,
}

impl FakeMailCache {
    pub fn cached(account_id: i64) -> Self {
        Self {
            populated: vec![account_id],
            ..Self::default()
        }
    }
}

#[async_trait]
impl MailCache for FakeMailCache {
    async fn upsert_folders(&self, _account_id: i64, _folders: &[Folder]) -> Result<()> {
        Ok(())
    }

    async fn load_folders(&self, _account_id: i64) -> Result<Vec<Folder>> {
        Ok(Vec::new())
    }

    async fn upsert_envelopes(
        &self,
        _account_id: i64,
        _folder: &str,
        _envelopes: &[Envelope],
    ) -> Result<()> {
        Ok(())
    }

    async fn load_envelopes(&self, account_id: i64, folder: &str) -> Result<Vec<Envelope>> {
        Ok(self
            .envelopes
            .get(&(account_id, folder.to_string()))
            .cloned()
            .unwrap_or_default())
    }

    async fn count_by_folder(&self, account_id: i64) -> Result<Vec<(String, usize, usize)>> {
        Ok(self
            .envelopes
            .iter()
            .filter(|((acc, _), _)| *acc == account_id)
            .map(|((_, folder), envs)| {
                let unread = envs.iter().filter(|e| !e.flags.seen).count();
                (folder.clone(), envs.len(), unread)
            })
            .collect())
    }

    async fn store_body(
        &self,
        _account_id: i64,
        _folder: &str,
        _uid: u64,
        _body: &str,
        _raw_headers: Option<&str>,
    ) -> Result<()> {
        Ok(())
    }

    async fn load_message(
        &self,
        account_id: i64,
        folder: &str,
        uid: u64,
    ) -> Result<Option<Message>> {
        let Some(env) = self
            .envelopes
            .get(&(account_id, folder.to_string()))
            .and_then(|v| v.iter().find(|e| e.uid == uid))
        else {
            return Ok(None);
        };
        let (body, raw_headers) = self
            .bodies
            .get(&(account_id, folder.to_string(), uid))
            .cloned()
            .unwrap_or_default();
        Ok(Some(Message {
            envelope: env.clone(),
            body,
            raw_headers,
        }))
    }

    async fn delete_message(&self, _account_id: i64, _folder: &str, _uid: u64) -> Result<()> {
        Ok(())
    }

    async fn move_message(
        &self,
        _account_id: i64,
        _from_folder: &str,
        _uid: u64,
        _to_folder: &str,
    ) -> Result<()> {
        Ok(())
    }

    async fn set_flags(
        &self,
        _account_id: i64,
        _folder: &str,
        _uid: u64,
        _seen: Option<bool>,
        _flagged: Option<bool>,
    ) -> Result<()> {
        Ok(())
    }

    async fn has_messages(&self, account_id: i64) -> Result<bool> {
        Ok(self.populated.contains(&account_id))
    }

    async fn search(&self, account_id: i64, _folder: &str, term: &str) -> Result<Vec<Envelope>> {
        // Naive substring search over subject, mirroring the FTS contract.
        Ok(self
            .envelopes
            .iter()
            .filter(|((id, _), _)| *id == account_id)
            .flat_map(|(_, v)| v)
            .filter(|e| e.subject.to_lowercase().contains(&term.to_lowercase()))
            .cloned()
            .collect())
    }
}

/// In-memory [`TokenStore`].
#[derive(Default)]
pub struct FakeTokenStore {
    pub refresh_tokens: std::sync::Mutex<HashMap<String, String>>,
}

#[async_trait]
impl TokenStore for FakeTokenStore {
    async fn store_password(&self, _keychain_ref: &str, _password: &str) -> Result<()> {
        Ok(())
    }

    async fn load_password(&self, _keychain_ref: &str) -> Result<Option<String>> {
        Ok(None)
    }

    async fn store_refresh_token(&self, keychain_ref: &str, refresh_token: &str) -> Result<()> {
        self.refresh_tokens
            .lock()
            .unwrap()
            .insert(keychain_ref.to_string(), refresh_token.to_string());
        Ok(())
    }

    async fn load_refresh_token(&self, keychain_ref: &str) -> Result<Option<String>> {
        Ok(self
            .refresh_tokens
            .lock()
            .unwrap()
            .get(keychain_ref)
            .cloned())
    }

    async fn delete_refresh_token(&self, keychain_ref: &str) -> Result<()> {
        self.refresh_tokens.lock().unwrap().remove(keychain_ref);
        Ok(())
    }

    async fn store_access_token(
        &self,
        _keychain_ref: &str,
        _token: &str,
        _expires_at_unix: u64,
    ) -> Result<()> {
        Ok(())
    }

    async fn load_access_token(&self, _keychain_ref: &str) -> Result<Option<(String, u64)>> {
        Ok(None)
    }
}

/// No-op [`Notifier`] recording nothing.
#[derive(Default)]
pub struct FakeNotifier;

impl Notifier for FakeNotifier {
    fn new_mail_batch(
        &self,
        _account: &str,
        _envelopes: &[Envelope],
        _show_sender: bool,
        _sound: Option<&SoundHint>,
    ) -> Result<()> {
        Ok(())
    }
}

/// [`NewMailWatch`] that panics when used; for tests whose use-case never
/// spawns a watcher, keeping the container honest about that.
pub struct PanicNewMailWatch;

#[async_trait]
impl crate::application::ports::NewMailWatch for PanicNewMailWatch {
    fn watch(
        &self,
        _params: crate::application::account::connect_params::ConnectParams,
        _mailbox: String,
        _account: String,
        _poll_interval: std::time::Duration,
        _events: tokio::sync::mpsc::UnboundedSender<crate::domain::MailUpdate>,
    ) -> Box<dyn crate::application::ports::WatchHandle> {
        panic!("watcher must not be used by this use-case")
    }
}
