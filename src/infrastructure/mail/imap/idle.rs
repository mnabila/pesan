use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_imap::extensions::idle::IdleResponse;
use futures::StreamExt;
use tokio::sync::{Notify, mpsc::UnboundedSender};

use crate::domain::{Envelope, MailUpdate, NewMail};
use crate::infrastructure::auth::oauth::refresh_access_token;
use crate::infrastructure::mail::imap::client::{
    ConnectParams, ENVELOPE_QUERY, ImapAuth, ImapSession, connect_and_auth, envelope_from_fetch,
};

/// Re-issue IDLE at least this often; RFC 2177 recommends under 29 minutes.
const IDLE_RENEW: Duration = Duration::from_secs(29 * 60);
/// Pause before reconnecting after a dropped/failed session.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(15);

/// Shared stop signal: the flag covers the gaps between runtimes, the notify
/// interrupts an in-flight IDLE wait promptly.
struct Shutdown {
    flag: AtomicBool,
    notify: Notify,
}

impl Shutdown {
    fn trigger(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Handle to a running watcher. Dropping it asks the worker to stop.
pub struct IdleWatcher {
    shutdown: Arc<Shutdown>,
    _handle: JoinHandle<()>,
}

impl Drop for IdleWatcher {
    fn drop(&mut self) {
        self.shutdown.trigger();
    }
}

impl crate::application::ports::WatchHandle for IdleWatcher {}

/// The production [`NewMailWatch`]: IMAP IDLE on a second connection, falling
/// back to polling when the server does not advertise IDLE.
pub struct IdleWatchFactory;

#[async_trait::async_trait]
impl crate::application::ports::NewMailWatch for IdleWatchFactory {
    fn watch(
        &self,
        params: ConnectParams,
        mailbox: String,
        account: String,
        poll_interval: Duration,
        events: UnboundedSender<MailUpdate>,
    ) -> Box<dyn crate::application::ports::WatchHandle> {
        Box::new(IdleWatcher::spawn(
            params,
            mailbox,
            account,
            poll_interval,
            events,
        ))
    }
}

impl IdleWatcher {
    /// Spawn a watcher for `mailbox` (usually INBOX). Failures are non-fatal:
    /// the worker logs and retries; the UI is never blocked.
    pub fn spawn(
        params: ConnectParams,
        mailbox: String,
        account: String,
        poll_interval: Duration,
        events: UnboundedSender<MailUpdate>,
    ) -> Self {
        let shutdown = Arc::new(Shutdown {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
        });
        let worker_shutdown = shutdown.clone();
        let handle = std::thread::Builder::new()
            .name(format!("idle-{account}"))
            .spawn(move || {
                watcher_main(
                    params,
                    mailbox,
                    account,
                    poll_interval,
                    events,
                    worker_shutdown,
                );
            })
            .expect("spawn idle watcher thread");
        Self {
            shutdown,
            _handle: handle,
        }
    }
}

fn watcher_main(
    params: ConnectParams,
    mailbox: String,
    account: String,
    poll_interval: Duration,
    events: UnboundedSender<MailUpdate>,
    shutdown: Arc<Shutdown>,
) {
    while !shutdown.is_set() {
        // Refresh the access token off any runtime (blocking reqwest). Password
        // accounts have no token; LOGIN uses the stored password directly.
        let access_token = match &params.auth {
            ImapAuth::Password { .. } => String::new(),
            ImapAuth::OAuth {
                oauth,
                refresh_token,
                ..
            } => match refresh_access_token(oauth, refresh_token) {
                Ok(tokens) => tokens.access_token,
                Err(e) => {
                    tracing::warn!("idle[{account}]: token refresh failed: {e:#}");
                    if backoff(&shutdown) {
                        break;
                    }
                    continue;
                }
            },
        };

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!("idle[{account}]: build runtime failed: {e}");
                break;
            }
        };

        let outcome = runtime.block_on(run_session(
            &params,
            &access_token,
            &mailbox,
            &account,
            poll_interval,
            &events,
            &shutdown,
        ));
        drop(runtime);

        match outcome {
            Ok(()) => break, // graceful shutdown
            Err(e) => {
                tracing::warn!("idle[{account}]: session ended ({e:#}); will reconnect");
                if backoff(&shutdown) {
                    break;
                }
            }
        }
    }
    tracing::info!("idle[{account}]: watcher stopped");
}

/// Sleep out the reconnect backoff, returning early (`true`) if asked to stop.
fn backoff(shutdown: &Shutdown) -> bool {
    let step = Duration::from_millis(500);
    let mut waited = Duration::ZERO;
    while waited < RECONNECT_BACKOFF {
        if shutdown.is_set() {
            return true;
        }
        std::thread::sleep(step);
        waited += step;
    }
    shutdown.is_set()
}

enum WokeBy {
    Data,
    Renew,
    Shutdown,
}

/// Run a single connected session until shutdown (`Ok`) or a network error
/// (`Err`, prompting a reconnect).
async fn run_session(
    params: &ConnectParams,
    access_token: &str,
    mailbox: &str,
    account: &str,
    poll_interval: Duration,
    events: &UnboundedSender<MailUpdate>,
    shutdown: &Shutdown,
) -> Result<()> {
    let mut session = connect_and_auth(params, access_token).await?;
    let status = session
        .select(mailbox)
        .await
        .with_context(|| format!("SELECT {mailbox} for idle"))?;
    // New arrivals get UID >= uid_next at connect time.
    let mut next_uid = status.uid_next.unwrap_or(1);

    let supports_idle = session
        .capabilities()
        .await
        .map(|c| c.has_str("IDLE"))
        .unwrap_or(false);
    tracing::info!(
        "idle[{account}]: watching {mailbox} (idle={supports_idle}, uid_next={next_uid})"
    );

    loop {
        if shutdown.is_set() {
            let _ = session.logout().await;
            return Ok(());
        }

        let woke = if supports_idle {
            let (returned, woke) = idle_wait(session, shutdown).await?;
            session = returned;
            woke
        } else {
            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => WokeBy::Data,
                _ = shutdown.notify.notified() => WokeBy::Shutdown,
            }
        };

        match woke {
            WokeBy::Shutdown => {
                let _ = session.logout().await;
                return Ok(());
            }
            WokeBy::Renew => continue,
            WokeBy::Data => {
                let (fresh, new_next) = fetch_since(&mut session, next_uid).await?;
                next_uid = new_next.max(next_uid);
                if !fresh.is_empty() {
                    let _ = events.send(MailUpdate::Arrived(NewMail {
                        account: account.to_string(),
                        folder: mailbox.to_string(),
                        envelopes: fresh,
                    }));
                }
            }
        }
    }
}

/// Issue one IDLE and wait for activity, a renewal timeout, or shutdown.
/// Consumes and returns the session (async-imap's IDLE handle owns it).
async fn idle_wait(session: ImapSession, shutdown: &Shutdown) -> Result<(ImapSession, WokeBy)> {
    let mut handle = session.idle();
    handle.init().await.map_err(|e| anyhow!("IDLE init: {e}"))?;

    let woke = {
        let (fut, _stop) = handle.wait_with_timeout(IDLE_RENEW);
        tokio::pin!(fut);
        tokio::select! {
            res = &mut fut => match res {
                Ok(IdleResponse::NewData(_)) => WokeBy::Data,
                Ok(IdleResponse::Timeout) => WokeBy::Renew,
                Ok(IdleResponse::ManualInterrupt) => WokeBy::Renew,
                Err(e) => return Err(anyhow!("IDLE wait: {e}")),
            },
            _ = shutdown.notify.notified() => WokeBy::Shutdown,
        }
    };

    let session = handle.done().await.map_err(|e| anyhow!("IDLE done: {e}"))?;
    Ok((session, woke))
}

/// Fetch messages with UID >= `next_uid` (newly arrived), returning them
/// newest-first plus the next UID threshold to watch from.
async fn fetch_since(session: &mut ImapSession, next_uid: u32) -> Result<(Vec<Envelope>, u32)> {
    let seq = format!("{next_uid}:*");
    let mut stream = session
        .uid_fetch(seq, ENVELOPE_QUERY)
        .await
        .context("UID FETCH new mail")?;

    let mut fresh = Vec::new();
    while let Some(item) = stream.next().await {
        let fetch = item.context("read new-mail FETCH")?;
        // "N:*" coerces to the highest existing UID when N is past the end, so
        // drop anything below the threshold we actually asked for.
        match fetch.uid {
            Some(uid) if u64::from(uid) >= u64::from(next_uid) => {
                if let Some(env) = envelope_from_fetch(&fetch) {
                    fresh.push(env);
                }
            }
            _ => {}
        }
    }
    drop(stream);

    let new_next = fresh
        .iter()
        .map(|e| e.uid)
        .max()
        .map(|m| (m as u32).saturating_add(1))
        .unwrap_or(next_uid);
    fresh.sort_by_key(|e| std::cmp::Reverse(e.date));
    Ok((fresh, new_next))
}
