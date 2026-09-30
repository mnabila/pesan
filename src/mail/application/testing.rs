//! In-memory mail-slice test doubles.

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;

use crate::mail::application::ports::MailCache;
use crate::mail::{Envelope, Folder, Message};

/// In-memory [`MailCache`].
#[derive(Default)]
pub struct FakeMailCache {
    /// Envelopes per `(account_id, folder)` as `load_envelopes` returns them.
    pub envelopes: HashMap<(String, String), Vec<Envelope>>,
    /// Cached bodies per `(account_id, folder, uid)`: plain text plus the raw
    /// HTML source and raw headers when available.
    pub bodies: HashMap<(String, String, u64), (String, Option<String>, Option<String>)>,
    /// Accounts that "have mail" (drives `has_messages`).
    pub populated: Vec<String>,
}

impl FakeMailCache {
    pub fn cached(account_id: &str) -> Self {
        Self {
            populated: vec![account_id.to_string()],
            ..Self::default()
        }
    }
}

#[async_trait]
impl MailCache for FakeMailCache {
    async fn upsert_folders(&self, _account_id: &str, _folders: &[Folder]) -> Result<()> {
        Ok(())
    }

    async fn upsert_envelopes(
        &self,
        _account_id: &str,
        _folder: &str,
        _envelopes: &[Envelope],
    ) -> Result<()> {
        Ok(())
    }

    async fn load_envelopes(&self, account_id: &str, folder: &str) -> Result<Vec<Envelope>> {
        Ok(self
            .envelopes
            .get(&(account_id.to_string(), folder.to_string()))
            .cloned()
            .unwrap_or_default())
    }

    async fn count_by_folder(&self, account_id: &str) -> Result<Vec<(String, usize, usize)>> {
        Ok(self
            .envelopes
            .iter()
            .filter(|((acc, _), _)| acc.as_str() == account_id)
            .map(|((_, folder), envs)| {
                let unread = envs.iter().filter(|e| !e.flags.seen).count();
                (folder.clone(), envs.len(), unread)
            })
            .collect())
    }

    async fn store_body(
        &self,
        _account_id: &str,
        _folder: &str,
        _uid: u64,
        _raw: Option<&[u8]>,
    ) -> Result<()> {
        Ok(())
    }

    async fn load_message(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
    ) -> Result<Option<Message>> {
        let Some(env) = self
            .envelopes
            .get(&(account_id.to_string(), folder.to_string()))
            .and_then(|v| v.iter().find(|e| e.uid == uid))
        else {
            return Ok(None);
        };
        let (body, raw_html, raw_headers) = self
            .bodies
            .get(&(account_id.to_string(), folder.to_string(), uid))
            .cloned()
            .unwrap_or_default();
        Ok(Some(Message {
            envelope: env.clone(),
            body,
            raw_html,
            raw_headers,
            raw: None,
        }))
    }

    async fn delete_message(&self, _account_id: &str, _folder: &str, _uid: u64) -> Result<()> {
        Ok(())
    }

    async fn set_flags(
        &self,
        _account_id: &str,
        _folder: &str,
        _uid: u64,
        _seen: Option<bool>,
        _flagged: Option<bool>,
    ) -> Result<()> {
        Ok(())
    }

    async fn has_messages(&self, account_id: &str) -> Result<bool> {
        Ok(self.populated.iter().any(|a| a == account_id))
    }

    async fn search(&self, account_id: &str, _folder: &str, term: &str) -> Result<Vec<Envelope>> {
        // Naive substring search over subject, mirroring the FTS contract.
        Ok(self
            .envelopes
            .iter()
            .filter(|((id, _), _)| id.as_str() == account_id)
            .flat_map(|(_, v)| v)
            .filter(|e| e.subject.to_lowercase().contains(&term.to_lowercase()))
            .cloned()
            .collect())
    }
}
