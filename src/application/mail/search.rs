use anyhow::Result;

use crate::application::Services;
use crate::domain::Envelope;

/// Run a full-text cache search. Returns `Ok(None)` when there is nothing
/// cached to search (no account row or an empty cache), letting the caller use
/// its in-memory fallback.
pub async fn search_cache(
    svc: &Services,
    account_id: Option<i64>,
    folder: &str,
    term: &str,
) -> Result<Option<Vec<Envelope>>> {
    let Some(id) = account_id else {
        return Ok(None);
    };
    if !svc.cache.has_messages(id).await.unwrap_or(false) {
        return Ok(None);
    }
    Ok(Some(svc.cache.search(id, folder, term).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::services::Services;
    use crate::application::testing::{
        FakeAccountRepo, FakeMailCache, FakeNotifier, FakeTokenStore,
    };
    use crate::domain::{Address, Envelope};

    fn svc(cache: FakeMailCache) -> Services {
        Services {
            accounts: std::sync::Arc::new(FakeAccountRepo::default()),
            cache: std::sync::Arc::new(cache),
            tokens: std::sync::Arc::new(FakeTokenStore::default()),
            // Never called by search_cache.
            backend: panic_backend(),
            notifier: std::sync::Arc::new(FakeNotifier),
            watcher: std::sync::Arc::new(crate::application::testing::PanicNewMailWatch),
        }
    }

    fn panic_backend() -> std::sync::Arc<dyn crate::application::ports::MailBackend> {
        struct Panicky;
        #[async_trait::async_trait]
        impl crate::application::ports::MailBackend for Panicky {
            async fn refresh_access_token(
                &self,
                _: crate::application::oauth::ResolvedOAuth,
                _: String,
            ) -> anyhow::Result<crate::application::oauth::TokenSet> {
                panic!()
            }
            async fn connect(
                &self,
                _: crate::application::account::connect_params::ConnectParams,
                _: Option<tokio::sync::mpsc::UnboundedSender<String>>,
            ) -> anyhow::Result<Box<dyn crate::application::MailSource>> {
                panic!()
            }
            async fn offline_source(
                &self,
                _: Option<i64>,
            ) -> Box<dyn crate::application::MailSource> {
                panic!()
            }
        }
        std::sync::Arc::new(Panicky)
    }

    fn env(subject: &str) -> Envelope {
        Envelope {
            uid: 1,
            flags: Default::default(),
            from: Address {
                name: None,
                email: "a@b.c".into(),
            },
            subject: subject.into(),
            date: 0,
            has_attachment: false,
            message_id: None,
            snippet: None,
        }
    }

    #[tokio::test]
    async fn no_cache_falls_back_to_in_memory() {
        let s = svc(FakeMailCache::default());
        let out = search_cache(&s, Some(7), "INBOX", "hello").await.unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn cached_mail_searches_the_whole_folder() {
        let mut c = FakeMailCache::cached(7);
        c.envelopes.insert(
            (7, "INBOX".to_string()),
            vec![env("Quarterly report"), env("lunch plans")],
        );
        let s = svc(c);
        let out = search_cache(&s, Some(7), "INBOX", "quarterly")
            .await
            .unwrap()
            .expect("cache search should run");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subject, "Quarterly report");
    }

    #[tokio::test]
    async fn no_account_row_falls_back_to_in_memory() {
        let s = svc(FakeMailCache::cached(7));
        let out = search_cache(&s, None, "INBOX", "hello").await.unwrap();
        assert!(out.is_none());
    }
}
