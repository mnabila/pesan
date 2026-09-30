use crate::mail::application::outcome::{Effect, OpenOutcome};
use crate::mail::application::ports::MailCache;
use crate::mail::{Envelope, Message};

/// Resolve what opening `env` in `folder` should do. `live` reflects whether
/// the shell's current source has a live IMAP handle (offline opens fall back
/// to a direct source read, handled by the shell).
pub async fn open_message(
    cache: &dyn MailCache,
    account_id: Option<&str>,
    folder: &str,
    env: &Envelope,
    live: bool,
) -> OpenOutcome {
    // 1. Cache-first: a message opened (or synced) before shows instantly from
    //    SQLite - no network, no UI freeze.
    let mut cached = None;
    if let Some(id) = account_id
        && let Ok(Some(mut msg)) = cache.load_message(id, folder, env.uid).await
        && !msg.body.is_empty()
    {
        msg.envelope.flags = env.flags;
        cached = Some(msg);
    }
    let shown = cached.is_some();
    let had_headers = cached.as_ref().is_some_and(|m| m.raw_headers.is_some());

    if live {
        let immediate = cached.unwrap_or_else(|| Message {
            envelope: env.clone(),
            body: "Loading message...".to_string(),
            raw_html: None,
            raw_headers: None,
            raw: None,
        });
        let effect = if shown && had_headers {
            None
        } else {
            Some(Effect::FetchMessage {
                folder: folder.to_string(),
                env: env.clone(),
            })
        };
        return OpenOutcome {
            immediate: Some(immediate),
            effect,
        };
    }

    // Offline: the cached copy is all there is (the shell does a best-effort
    // direct source read when nothing is cached).
    OpenOutcome {
        immediate: cached,
        effect: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::application::testing::FakeMailCache;
    use crate::mail::{Address, Flags};

    fn env(uid: u64) -> Envelope {
        Envelope {
            uid,
            flags: Flags::default(),
            from: Address {
                name: Some("Ada".to_string()),
                email: "ada@example.com".into(),
            },
            subject: "hi".into(),
            date: 0,
            has_attachment: false,
            message_id: None,
            snippet: Some("body text".into()),
        }
    }

    fn cache_with_full_copy(account: &str) -> FakeMailCache {
        let mut c = FakeMailCache::cached(account);
        c.envelopes
            .insert((account.to_string(), "INBOX".to_string()), vec![env(1)]);
        c.bodies.insert(
            (account.to_string(), "INBOX".to_string(), 1),
            ("body".to_string(), None, Some("From: ada".to_string())),
        );
        c
    }

    #[tokio::test]
    async fn full_cache_hit_skips_revalidate() {
        let c = cache_with_full_copy("a7");
        let out = open_message(&c, Some("a7"), "INBOX", &env(1), true).await;
        assert_eq!(out.immediate.unwrap().body, "body");
        assert!(out.effect.is_none());
    }

    #[tokio::test]
    async fn partial_cache_revalidates() {
        let account = "a7";
        let mut c = cache_with_full_copy(account);
        // Same envelope cached but with no raw headers -> incomplete copy.
        c.bodies.insert(
            (account.to_string(), "INBOX".to_string(), 1),
            ("body".to_string(), None, None),
        );
        let out = open_message(&c, Some(account), "INBOX", &env(1), true).await;
        match out.effect {
            Some(Effect::FetchMessage { folder, env }) => {
                assert_eq!(folder, "INBOX");
                assert_eq!(env.uid, 1);
            }
            other => panic!("expected FetchMessage effect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_cache_shows_placeholder_when_live() {
        let c = FakeMailCache::cached("a7");
        let out = open_message(&c, Some("a7"), "INBOX", &env(2), true).await;
        let msg = out.immediate.unwrap();
        assert_eq!(msg.envelope.uid, 2);
        assert_eq!(msg.body, "Loading message...");
        assert!(out.effect.is_some());
    }

    #[tokio::test]
    async fn offline_without_cache_returns_nothing() {
        let c = FakeMailCache::cached("a7");
        let out = open_message(&c, Some("a7"), "INBOX", &env(3), false).await;
        assert!(out.immediate.is_none());
        assert!(out.effect.is_none());
    }
}
