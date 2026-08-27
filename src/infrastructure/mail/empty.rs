use anyhow::{Result, bail};
use async_trait::async_trait;

use crate::application::mail::fetch::MailSource;
use crate::domain::{Envelope, Folder, Message};

/// An empty mail source: no folders, no messages, no writes.
pub struct EmptySource;

impl EmptySource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MailSource for EmptySource {
    async fn list_folders(&self) -> Result<Vec<Folder>> {
        Ok(Vec::new())
    }

    async fn list_messages(&self, _folder: &str) -> Result<Vec<Envelope>> {
        Ok(Vec::new())
    }

    async fn fetch_message(&self, uid: u64) -> Result<Message> {
        bail!("message {uid} is not available - connect an account to load mail")
    }

    async fn delete(&mut self, _uid: u64) -> Result<()> {
        bail!("nothing to delete - no mail is loaded")
    }
}
