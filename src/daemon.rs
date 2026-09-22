//! Headless daemon: the auto-fetch loop + local mail server behind `pesan daemon`.
//!
//! It re-assembles the background half of the TUI's `connect_all_accounts`
//! without a terminal: connect every authorized account, warm the offline cache
//! (folders + envelopes + today's bodies), then watch the configured folders and
//! raise a desktop notification on each arrival, prefetching the new bodies so a
//! later TUI open is instant.
//!
//! It also owns the live IMAP/SMTP sessions and serves them to the TUI over a
//! unix socket (see `infrastructure::mail::ipc`), so a running client routes its
//! operations here instead of opening its own IMAP connections. Runs until
//! SIGTERM / Ctrl-C.

use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::application::MailSource;
use crate::application::Services;
use crate::application::account::connect::connect_account;
use crate::application::account::connect_params::resolve_connect_params;
use crate::application::mail::imap_cmd::ImapHandle;
use crate::bootstrap::config::Config;
use crate::domain::{Envelope, MailUpdate, NewMail};
use crate::infrastructure::database::accounts::Account;
use crate::infrastructure::mail::ipc::PushEvent;
use crate::infrastructure::{self, database};
use crate::shared::is_today;

/// A new-mail batch tagged with the account + folder it arrived on, so the loop
/// can file it under the right cache key (the domain `NewMail` carries neither).
type Arrival = (String, String, NewMail);

/// Broadcast capacity for arrival pushes to subscribed clients. Small; clients
/// that lag just miss a push and re-sync from the cache.
const PUSH_CAP: usize = 256;

/// A request from the socket server to the session owner: hand back a command
/// handle for an account, connecting it lazily if needed.
enum HubMsg {
    Handle {
        account: String,
        reply: oneshot::Sender<Result<ImapHandle, String>>,
    },
}

/// Run the daemon until a shutdown signal (SIGTERM / Ctrl-C) arrives. The owner
/// task holds every IMAP session and is aborted on shutdown, dropping the
/// sessions/watchers; the socket server dies with the process.
pub async fn run(config: Config, pool: database::Db) -> Result<()> {
    let services = infrastructure::sqlite_services(pool.clone());
    let (req_tx, req_rx) = mpsc::unbounded_channel::<HubMsg>();
    let (pushes, _keepalive) = broadcast::channel::<PushEvent>(PUSH_CAP);

    start_ipc_server(req_tx, pushes.clone());

    let mut owner = tokio::spawn(owner_task(config, pool, services, req_rx, pushes));

    tokio::select! {
        _ = shutdown_signal() => tracing::info!("daemon: shutdown signal received; stopping"),
        r = &mut owner => {
            if let Err(e) = r {
                tracing::warn!("daemon: session owner ended: {e}");
            }
        }
    }
    owner.abort();
    tracing::info!("daemon: stopped");
    Ok(())
}

/// Bind the client socket and spawn the accept loop. Best-effort: a bind failure
/// is logged and the daemon still runs its watch/notify duties.
fn start_ipc_server(req_tx: mpsc::UnboundedSender<HubMsg>, pushes: broadcast::Sender<PushEvent>) {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    let sock = match crate::bootstrap::config::socket_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("daemon: no socket path ({e}); running without client server");
            return;
        }
    };
    // Enclose the socket in a private (0700) directory. `bind` creates the
    // socket under the process umask (often 0666) and we only chmod it to 0600
    // afterwards, leaving a brief window; a 0700 parent means no other local
    // user can traverse to the socket during (or after) that window - important
    // for the `data_dir` fallback, whose ancestors are world-traversable
    // (the XDG runtime dir is already 0700).
    if let Some(dir) = sock.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let _ = std::fs::remove_file(&sock); // clear a stale socket from a prior run
    let listener = match tokio::net::UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("daemon: cannot bind {} ({e}); running without client server", sock.display());
            return;
        }
    };
    let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600));
    tracing::info!("daemon: IPC server listening on {}", sock.display());

    let hub: Arc<dyn infrastructure::mail::ipc::server::Sessions> = Arc::new(Hub { req_tx, pushes });
    tokio::spawn(infrastructure::mail::ipc::server::serve(listener, hub));
}

/// A finished connect attempt, sent from a per-account connect task back to the
/// owner to install. Carrying the (`Send`) source + watch handles back lets many
/// accounts connect concurrently while the owner stays the single mutator of the
/// session maps.
struct ConnectOutcome {
    id: String,
    name: String,
    result: Result<ConnectSpoils, String>,
}

struct ConnectSpoils {
    source: Box<dyn MailSource>,
    watchers: Vec<Box<dyn crate::application::ports::WatchHandle>>,
    handle: Option<ImapHandle>,
}

/// The session owner. Connects every account **in parallel** (each on its own
/// task that reports back via `done`), serves handle requests from the socket
/// server (waiting on an in-flight connect, or starting one), and processes
/// arrivals (cache + prefetch + notify + push). Owns the session maps; aborting
/// it on shutdown drops every session and watcher.
async fn owner_task(
    config: Config,
    pool: database::Db,
    services: Services,
    mut req_rx: mpsc::UnboundedReceiver<HubMsg>,
    pushes: broadcast::Sender<PushEvent>,
) {
    let folders = config.daemon.resolved_folders(&config.notifications);
    let cap = config.daemon.prefetch_cap();
    let poll = config.daemon.poll_interval();

    let (arrivals_tx, mut arrivals_rx) = mpsc::unbounded_channel::<Arrival>();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<ConnectOutcome>();
    let mut sources: HashMap<String, Box<dyn MailSource>> = HashMap::new();
    let mut names: HashMap<String, String> = HashMap::new();
    let mut watch_handles: Vec<Box<dyn crate::application::ports::WatchHandle>> = Vec::new();
    // Accounts with a connect task in flight, and the handle requests waiting on
    // each - so N clients asking for the same account share one connect.
    let mut connecting: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut waiters: HashMap<String, Vec<oneshot::Sender<Result<ImapHandle, String>>>> =
        HashMap::new();

    // Periodic folder re-sync (optional): re-list watched folders to catch
    // server-side changes IDLE doesn't report, pushing refreshed lists to clients.
    let mut resync_iv = config.daemon.resync_interval().map(tokio::time::interval);

    // Periodic reconnect sweep: any in-scope account not currently connected (a
    // failed initial connect, or a watcher that died) is retried, so a transient
    // startup failure self-heals instead of leaving that account silent until a
    // TUI client happens to ask for it.
    let mut retry_iv = tokio::time::interval(poll.max(std::time::Duration::from_secs(60)));

    // Eager startup: fire every in-scope account's connect at once (parallel).
    let accounts = database::accounts::list(&pool).await.unwrap_or_default();
    let want: Vec<Account> = accounts
        .into_iter()
        .filter(|a| a.id.is_some() && config.daemon.includes(&a.name))
        .collect();
    tracing::info!("daemon: connecting {} account(s) in parallel", want.len());
    for account in want {
        let id = account.id.clone().expect("filtered to Some");
        if connecting.insert(id.clone()) {
            spawn_connect(&config, &services, &folders, cap, poll, account, id, &arrivals_tx, &done_tx);
        }
    }

    loop {
        tokio::select! {
            Some(msg) = req_rx.recv() => match msg {
                HubMsg::Handle { account, reply } => {
                    // Fast path: already connected.
                    let hit = database::accounts::get_by_name(&pool, &account).await;
                    match hit {
                        Ok(Some(acct)) => {
                            let Some(id) = acct.id.clone() else {
                                let _ = reply.send(Err("account has no id".to_string()));
                                continue;
                            };
                            if let Some(h) = sources.get(&id).and_then(|s| s.imap_handle()) {
                                let _ = reply.send(Ok(h));
                                continue;
                            }
                            // Otherwise queue on the in-flight (or new) connect.
                            waiters.entry(id.clone()).or_default().push(reply);
                            if connecting.insert(id.clone()) {
                                spawn_connect(
                                    &config, &services, &folders, cap, poll, acct, id,
                                    &arrivals_tx, &done_tx,
                                );
                            }
                        }
                        Ok(None) => { let _ = reply.send(Err(format!("unknown account '{account}'"))); }
                        Err(e) => { let _ = reply.send(Err(e.to_string())); }
                    }
                }
            },
            Some(outcome) = done_rx.recv() => {
                let ConnectOutcome { id, name, result } = outcome;
                connecting.remove(&id);
                match result {
                    Ok(spoils) => {
                        names.insert(id.clone(), name);
                        watch_handles.extend(spoils.watchers);
                        sources.insert(id.clone(), spoils.source);
                        for w in waiters.remove(&id).unwrap_or_default() {
                            let h = spoils
                                .handle
                                .clone()
                                .ok_or_else(|| "account has no live handle".to_string());
                            let _ = w.send(h);
                        }
                    }
                    Err(e) => {
                        for w in waiters.remove(&id).unwrap_or_default() {
                            let _ = w.send(Err(e.clone()));
                        }
                        tracing::warn!("daemon: {e}");
                    }
                }
            }
            Some((id, folder, mail)) = arrivals_rx.recv() => {
                if mail.envelopes.is_empty() {
                    continue;
                }
                tracing::info!(
                    "daemon: new mail on {}/{}: {} message(s)",
                    mail.account, folder, mail.envelopes.len()
                );
                // File the arrivals into the cache (preserves any cached body),
                // then prefetch their bodies so a later open is instant.
                let _ = services.cache.upsert_envelopes(&id, &folder, &mail.envelopes).await;
                if let Some(handle) = sources.get(&id).and_then(|s| s.imap_handle()) {
                    let uids: Vec<u64> = mail.envelopes.iter().take(cap).map(|e| e.uid).collect();
                    prefetch_bodies(&services, &handle, &id, &folder, uids).await;
                }
                notify(&config, &services, &mail.account, &mail.envelopes);
                // Push to any subscribed clients (ignore error: no subscribers).
                let _ = pushes.send(PushEvent::NewMail {
                    account: mail.account,
                    folder,
                    envelopes: mail.envelopes,
                });
            }
            // Periodic re-sync tick: re-list every connected account's watched
            // folders off-owner (handles are Send+Clone) and push any changes.
            _ = async { match resync_iv.as_mut() {
                Some(iv) => { iv.tick().await; }
                None => std::future::pending::<()>().await,
            } } => {
                for (id, src) in sources.iter() {
                    let (Some(handle), Some(name)) = (src.imap_handle(), names.get(id).cloned())
                    else { continue };
                    for folder in &folders {
                        tokio::spawn(resync_folder(
                            services.clone(), handle.clone(), pushes.clone(),
                            id.clone(), name.clone(), folder.clone(),
                        ));
                    }
                }
            }
            _ = retry_iv.tick() => {
                let accts = database::accounts::list(&pool).await.unwrap_or_default();
                for account in accts
                    .into_iter()
                    .filter(|a| a.id.is_some() && config.daemon.includes(&a.name))
                {
                    let id = account.id.clone().expect("filtered to Some");
                    // Skip already-connected or in-flight accounts; `connecting`
                    // guards against a double connect with the eager startup.
                    if !sources.contains_key(&id) && connecting.insert(id.clone()) {
                        tracing::info!("daemon: retrying connect for account '{}'", account.name);
                        spawn_connect(
                            &config, &services, &folders, cap, poll, account, id,
                            &arrivals_tx, &done_tx,
                        );
                    }
                }
            }
            else => break,
        }
    }
}

/// Spawn a connect for one account on its own task; the outcome is sent to the
/// owner via `done`. Runs concurrently with every other account's connect.
#[allow(clippy::too_many_arguments)]
fn spawn_connect(
    config: &Config,
    services: &Services,
    folders: &[String],
    cap: usize,
    poll: std::time::Duration,
    account: Account,
    id: String,
    arrivals_tx: &mpsc::UnboundedSender<Arrival>,
    done_tx: &mpsc::UnboundedSender<ConnectOutcome>,
) {
    let (config, services, folders) = (config.clone(), services.clone(), folders.to_vec());
    let arrivals_tx = arrivals_tx.clone();
    let done_tx = done_tx.clone();
    tokio::spawn(async move {
        let name = account.name.clone();
        let result =
            do_connect(&config, &services, &folders, cap, poll, &account, &id, &arrivals_tx).await;
        let _ = done_tx.send(ConnectOutcome { id, name, result });
    });
}

/// Connect one account (light: INBOX + folder list; other folders sync lazily
/// when a client lists them), prefetch today's watched-folder bodies, and build
/// its arrival watchers. Returns the spoils for the owner to install.
#[allow(clippy::too_many_arguments)]
async fn do_connect(
    config: &Config,
    services: &Services,
    folders: &[String],
    cap: usize,
    poll: std::time::Duration,
    account: &Account,
    id: &str,
    arrivals_tx: &mpsc::UnboundedSender<Arrival>,
) -> Result<ConnectSpoils, String> {
    let params = match resolve_connect_params(config, account, services.tokens.as_ref()).await {
        Ok(Some(p)) => p,
        Ok(None) => return Err(format!("account '{}' not authorized; skipping", account.name)),
        Err(e) => return Err(format!("account '{}' config error: {e:#}", account.name)),
    };
    let want = folders.first().cloned().unwrap_or_else(|| "INBOX".to_string());
    // sync_all=false: return a usable handle fast; other folders warm on demand.
    let live = connect_account(services, params.clone(), Some(id.to_string()), want, false, None)
        .await
        .map_err(|e| format!("connect '{}' failed: {e}", account.name))?;
    tracing::info!("daemon: connected '{}' ({} folders)", account.name, live.folders.len());

    let handle = live.source.imap_handle();
    if let Some(h) = &handle {
        for folder in folders {
            let envs = services.cache.load_envelopes(id, folder).await.unwrap_or_default();
            prefetch_bodies(services, h, id, folder, today_uids(&envs, cap)).await;
        }
    }

    // One watcher per configured folder; a relay re-tags each batch with
    // (account_id, folder) into the shared arrivals channel.
    let mut watchers = Vec::new();
    for folder in folders {
        let (w_tx, mut w_rx) = mpsc::unbounded_channel::<MailUpdate>();
        let tagged = arrivals_tx.clone();
        let (aid, f) = (id.to_string(), folder.clone());
        tokio::spawn(async move {
            while let Some(update) = w_rx.recv().await {
                // The daemon's own watcher is IMAP IDLE - arrivals only. Folder
                // re-syncs are produced by the daemon's periodic pass, not here.
                if let MailUpdate::Arrived(nm) = update
                    && tagged.send((aid.clone(), f.clone(), nm)).is_err()
                {
                    break;
                }
            }
        });
        watchers.push(
            services
                .watcher
                .watch(params.clone(), folder.clone(), account.name.clone(), poll, w_tx),
        );
    }

    Ok(ConnectSpoils { source: live.source, watchers, handle })
}

/// Re-list a watched folder off the owner task; if its newest window changed
/// (new mail, flag updates, or a removal) update the cache and push the fresh
/// list so subscribed clients replace their view. No-op (no push) when unchanged.
async fn resync_folder(
    services: Services,
    handle: ImapHandle,
    pushes: broadcast::Sender<PushEvent>,
    account_id: String,
    account: String,
    folder: String,
) {
    let fresh = match handle.list_messages_bg(&folder).await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("daemon: resync {folder} list failed: {e}");
            return;
        }
    };
    let cached = services
        .cache
        .load_envelopes(&account_id, &folder)
        .await
        .unwrap_or_default();
    if !window_changed(&cached, &fresh) {
        return; // nothing new/changed - don't spam clients
    }
    let _ = services.cache.upsert_envelopes(&account_id, &folder, &fresh).await;
    tracing::info!("daemon: resync {account}/{folder} changed ({} msgs); pushed", fresh.len());
    let _ = pushes.send(PushEvent::FolderSync { account, folder, envelopes: fresh });
}

/// Whether the newest window changed vs the cache: any new UID, a seen/flagged
/// change on an existing one, or a removal within the fresh window's range.
fn window_changed(cached: &[Envelope], fresh: &[Envelope]) -> bool {
    use std::collections::{HashMap, HashSet};
    let cached_flags: HashMap<u64, (bool, bool)> = cached
        .iter()
        .map(|e| (e.uid, (e.flags.seen, e.flags.flagged)))
        .collect();
    for e in fresh {
        match cached_flags.get(&e.uid) {
            Some(&(seen, flagged)) if seen == e.flags.seen && flagged == e.flags.flagged => {}
            _ => return true, // new UID or flag change
        }
    }
    // A removal: a cached message in the newest window is gone from the fresh list.
    let fresh_uids: HashSet<u64> = fresh.iter().map(|e| e.uid).collect();
    cached
        .iter()
        .take(fresh.len())
        .any(|e| !fresh_uids.contains(&e.uid))
}

/// The session registry seen by the socket server. Forwards handle requests to
/// the owner task and hands out arrival subscriptions.
struct Hub {
    req_tx: mpsc::UnboundedSender<HubMsg>,
    pushes: broadcast::Sender<PushEvent>,
}

#[async_trait::async_trait]
impl infrastructure::mail::ipc::server::Sessions for Hub {
    async fn handle(&self, account: &str) -> Result<ImapHandle, String> {
        let (tx, rx) = oneshot::channel();
        self.req_tx
            .send(HubMsg::Handle { account: account.to_string(), reply: tx })
            .map_err(|_| "daemon session owner gone".to_string())?;
        rx.await.map_err(|_| "daemon session owner dropped".to_string())?
    }

    fn subscribe(&self) -> broadcast::Receiver<PushEvent> {
        self.pushes.subscribe()
    }
}

/// UIDs of today's messages (newest-first as stored), capped. Empty when the cap
/// is zero (prefetch disabled).
fn today_uids(envelopes: &[Envelope], cap: usize) -> Vec<u64> {
    if cap == 0 {
        return Vec::new();
    }
    envelopes
        .iter()
        .filter(|e| is_today(e.date))
        .take(cap)
        .map(|e| e.uid)
        .collect()
}

/// Warm the body cache for `uids` in `folder`. Skips anything already fully
/// cached (body + headers); best-effort, errors are logged at debug and ignored.
/// Mirrors the TUI's `Effect::PrefetchBodies` handler.
async fn prefetch_bodies(
    services: &crate::application::Services,
    handle: &ImapHandle,
    account_id: &str,
    folder: &str,
    uids: Vec<u64>,
) {
    for uid in uids {
        if let Ok(Some(msg)) = services.cache.load_message(account_id, folder, uid).await
            && !msg.body.is_empty()
            && msg.raw_headers.is_some()
        {
            continue;
        }
        match handle.fetch_message_bg(folder, uid).await {
            Ok(msg) => {
                let _ = services
                    .cache
                    .store_body(account_id, folder, uid, msg.raw.as_deref())
                    .await;
            }
            Err(e) => tracing::debug!("daemon: prefetch {folder}/{uid} failed: {e}"),
        }
    }
}

/// Fire one coalesced desktop notification for a batch, honoring config. Gated
/// on `daemon.notify` and the master `notifications.enabled`; content follows
/// the `notifications` settings.
fn notify(
    config: &Config,
    services: &crate::application::Services,
    account: &str,
    envelopes: &[Envelope],
) {
    if !config.daemon.notify || !config.notifications.enabled || envelopes.is_empty() {
        return;
    }
    let sound = config.notifications.sound_hint();
    if let Err(e) = services.notifier.new_mail_batch(
        account,
        envelopes,
        config.notifications.show_sender,
        sound.as_ref(),
    ) {
        tracing::warn!("daemon: desktop notification failed: {e}");
    }
}

/// Resolve when either SIGTERM (service stop) or Ctrl-C (SIGINT) arrives. On
/// non-unix, only Ctrl-C is available.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("daemon: cannot install SIGTERM handler: {e}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::window_changed;
    use crate::domain::{Address, Envelope, Flags};

    fn env(uid: u64, seen: bool) -> Envelope {
        Envelope {
            uid,
            flags: Flags { seen, flagged: false },
            from: Address::new(None, "a@b.c"),
            subject: String::new(),
            date: 0,
            has_attachment: false,
            snippet: None,
            message_id: None,
        }
    }

    #[test]
    fn unchanged_window_is_not_flagged() {
        let cached = vec![env(3, true), env(2, false), env(1, true)];
        let fresh = vec![env(3, true), env(2, false), env(1, true)];
        assert!(!window_changed(&cached, &fresh));
    }

    #[test]
    fn new_uid_flag_change_and_removal_are_detected() {
        let cached = vec![env(2, false), env(1, false)];
        // new uid 3
        assert!(window_changed(&cached, &[env(3, false), env(2, false), env(1, false)]));
        // uid 2 now seen
        assert!(window_changed(&cached, &[env(2, true), env(1, false)]));
        // uid 2 removed from the window (only uid 1 remains)
        assert!(window_changed(&cached, &[env(1, false)]));
    }
}
