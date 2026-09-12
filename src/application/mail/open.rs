use crate::application::Services;
use crate::application::outcome::{Effect, OpenOutcome};
use crate::domain::{Envelope, Message};

/// Resolve what opening `env` in `folder` should do. `live` reflects whether
/// the shell's current source has a live IMAP handle (offline opens fall back
/// to a direct source read, handled by the shell).
pub async fn open_message(
    svc: &Services,
    account_id: Option<i64>,
    folder: &str,
    env: &Envelope,
    live: bool,
) -> OpenOutcome {
    // 1. Cache-first: a message opened (or synced) before shows instantly from
    //    SQLite - no network, no UI freeze.
    let mut cached = None;
    if let Some(id) = account_id
        && let Ok(Some(mut msg)) = svc.cache.load_message(id, folder, env.uid).await
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
            raw_headers: None,
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
    use crate::application::MailSource;
    use crate::application::account::connect_params::ConnectParams;
    use crate::application::oauth::ResolvedOAuth;
    use crate::application::outcome::Effect;
    use crate::application::services::Services;
    use crate::application::testing::{FakeMailCache, FakeNotifier};
    use crate::domain::{Address, Envelope, Flags};

    fn svc(cache: FakeMailCache) -> Services {
        Services {
            accounts: std::sync::Arc::new(crate::application::testing::FakeAccountRepo::default()),
            cache: std::sync::Arc::new(cache),
            tokens: std::sync::Arc::new(crate::application::testing::FakeTokenStore::default()),
            backend: unreachable_backend(),
            notifier: std::sync::Arc::new(FakeNotifier),
            watcher: std::sync::Arc::new(crate::application::testing::PanicNewMailWatch),
            daemon_backed: false,
        }
    }

    // `open_message` only touches `cache`, so the backend port is never called;
    // a panicking stub keeps the container honest about that.
    fn unreachable_backend() -> std::sync::Arc<dyn crate::application::ports::MailBackend> {
        struct Panicky;
        #[async_trait::async_trait]
        impl crate::application::ports::MailBackend for Panicky {
            async fn refresh_access_token(
                &self,
                _: ResolvedOAuth,
                _: String,
            ) -> anyhow::Result<crate::application::oauth::TokenSet> {
                panic!("backend must not be used by open_message");
            }
            async fn connect(
                &self,
                _: ConnectParams,
                _: Option<tokio::sync::mpsc::UnboundedSender<String>>,
            ) -> anyhow::Result<Box<dyn MailSource>> {
                panic!("backend must not be used by open_message");
            }
            async fn offline_source(&self, _: Option<i64>) -> Box<dyn MailSource> {
                panic!("backend must not be used by open_message");
            }
        }
        std::sync::Arc::new(Panicky)
    }

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

    fn cache_with_full_copy(account: i64) -> FakeMailCache {
        let mut c = FakeMailCache::cached(account);
        c.envelopes
            .insert((account, "INBOX".to_string()), vec![env(1)]);
        c.bodies.insert(
            (account, "INBOX".to_string(), 1),
            ("body".to_string(), Some("From: ada".to_string())),
        );
        c
    }

    #[tokio::test]
    async fn full_cache_hit_skips_revalidate() {
        let s = svc(cache_with_full_copy(7));
        let out = open_message(&s, Some(7), "INBOX", &env(1), true).await;
        assert_eq!(out.immediate.unwrap().body, "body");
        assert!(out.effect.is_none());
    }

    #[tokio::test]
    async fn partial_cache_revalidates() {
        let account = 7i64;
        let mut c = cache_with_full_copy(account);
        // Same envelope cached but with no raw headers -> incomplete copy.
        c.bodies.insert(
            (account, "INBOX".to_string(), 1),
            ("body".to_string(), None),
        );
        let s = svc(c);
        let out = open_message(&s, Some(account), "INBOX", &env(1), true).await;
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
        let s = svc(FakeMailCache::cached(7));
        let out = open_message(&s, Some(7), "INBOX", &env(2), true).await;
        let msg = out.immediate.unwrap();
        assert_eq!(msg.envelope.uid, 2);
        assert_eq!(msg.body, "Loading message...");
        assert!(out.effect.is_some());
    }

    #[tokio::test]
    async fn offline_without_cache_returns_nothing() {
        let s = svc(FakeMailCache::cached(7));
        let out = open_message(&s, Some(7), "INBOX", &env(3), false).await;
        assert!(out.immediate.is_none());
        assert!(out.effect.is_none());
    }
}
