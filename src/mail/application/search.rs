use anyhow::Result;

use crate::mail::Envelope;
use crate::mail::application::ports::MailCache;

/// Run a full-text cache search. Returns `Ok(None)` when there is nothing
/// cached to search (no account row or an empty cache), letting the caller use
/// its in-memory fallback.
pub async fn search_cache(
    cache: &dyn MailCache,
    account_id: Option<&str>,
    folder: &str,
    term: &str,
) -> Result<Option<Vec<Envelope>>> {
    let Some(id) = account_id else {
        return Ok(None);
    };
    if !cache.has_messages(id).await.unwrap_or(false) {
        return Ok(None);
    }
    Ok(Some(cache.search(id, folder, term).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::application::testing::FakeMailCache;
    use crate::mail::{Address, Envelope};

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
        let c = FakeMailCache::default();
        let out = search_cache(&c, Some("a7"), "INBOX", "hello").await.unwrap();
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn cached_mail_searches_the_whole_folder() {
        let mut c = FakeMailCache::cached("a7");
        c.envelopes.insert(
            ("a7".to_string(), "INBOX".to_string()),
            vec![env("Quarterly report"), env("lunch plans")],
        );
        let out = search_cache(&c, Some("a7"), "INBOX", "quarterly")
            .await
            .unwrap()
            .expect("cache search should run");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subject, "Quarterly report");
    }

    #[tokio::test]
    async fn no_account_row_falls_back_to_in_memory() {
        let c = FakeMailCache::cached("a7");
        let out = search_cache(&c, None, "INBOX", "hello").await.unwrap();
        assert!(out.is_none());
    }
}
