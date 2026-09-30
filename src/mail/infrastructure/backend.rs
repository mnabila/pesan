use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use crate::platform::connect_params::ConnectParams;
use crate::mail::application::fetch::MailSource;
use crate::platform::oauth::{ResolvedOAuth, TokenSet};
use crate::platform::auth::oauth;
use crate::mail::application::ports::MailBackend;
use crate::platform::db::Db;
use crate::mail::infrastructure::maildir::MaildirStore;
use crate::mail::infrastructure::imap;
use crate::mail::infrastructure::sqlite_cache;
use crate::mail::infrastructure::cache;
use crate::mail::infrastructure::empty;

pub struct ImapBackend {
    pool: Db,
    maildir: MaildirStore,
}

impl ImapBackend {
    pub fn new(pool: Db, maildir: MaildirStore) -> Self {
        Self { pool, maildir }
    }
}

#[async_trait]
impl MailBackend for ImapBackend {
    async fn refresh_access_token(
        &self,
        oauth: ResolvedOAuth,
        refresh_token: String,
    ) -> Result<TokenSet> {
        // Blocking `reqwest`: run on a dedicated OS thread (no Tokio runtime, so
        // the blocking HTTP is legal) and await the one-shot result.
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(oauth::refresh_access_token(
                &oauth,
                &refresh_token,
            ));
        });
        match rx.await {
            Ok(res) => res,
            Err(_) => Err(anyhow!("token refresh thread died")),
        }
    }

    async fn connect(
        &self,
        params: ConnectParams,
        on_lost: Option<UnboundedSender<String>>,
    ) -> Result<Box<dyn MailSource>> {
        // The connect itself is blocking (TLS + auth); keep it off any runtime
        // thread via spawn_blocking.
        let src = tokio::task::spawn_blocking(move || {
            imap::ImapSource::connect(params, on_lost)
        })
        .await
        .map_err(|e| anyhow!("connect task panicked: {e}"))??;
        Ok(Box::new(src))
    }

    async fn offline_source(&self, account_id: Option<&str>) -> Box<dyn MailSource> {
        if let Some(id) = account_id
            && sqlite_cache::has_messages(&self.pool, id)
                .await
                .unwrap_or(false)
        {
            return Box::new(cache::CacheSource::new(
                self.pool.clone(),
                self.maildir.clone(),
                id.to_string(),
            ));
        }
        Box::new(empty::EmptySource::new())
    }
}
