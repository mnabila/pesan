//! Composition root: the `Services` DI bundle (mixing both slices' ports) and
//! the functions that assemble the production adapters. Lives above the slices,
//! outside the architecture guard's scanned set, so it may depend on `account`,
//! `mail`, and `platform` at once without inverting any slice boundary.

use std::sync::Arc;

use crate::account::application::ports::{AccountRepo, TokenStore};
use crate::account::infrastructure::{accounts, token};
use crate::mail::application::ports::{MailBackend, MailCache, NewMailWatch, Notifier};
use crate::mail::infrastructure::imap::idle;
use crate::mail::infrastructure::ipc::client;
use crate::mail::infrastructure::{backend, maildir, notify, sqlite_cache};
use crate::platform::config;
use crate::platform::db::Db;

pub mod connect;

/// Cloneable bundle of port implementations.
#[derive(Clone)]
pub struct Services {
    pub accounts: Arc<dyn AccountRepo>,
    pub cache: Arc<dyn MailCache>,
    pub tokens: Arc<dyn TokenStore>,
    pub backend: Arc<dyn MailBackend>,
    pub notifier: Arc<dyn Notifier>,
    pub watcher: Arc<dyn NewMailWatch>,
    /// True when `backend`/`watcher` route through a running `pesan daemon`.
    /// The TUI uses this to skip redundant background warm-up (the daemon already
    /// keeps the shared cache warm and watches for arrivals).
    pub daemon_backed: bool,
}

/// The production wiring: SQLite repos over the shared pool, keyring-backed
/// token store, desktop notifier, and a **direct** IMAP backend/watcher. Used by
/// the daemon (it *is* the server) and as the fallback the client wraps.
pub fn sqlite_services(pool: Db) -> Services {
    // The Maildir tree that mirrors message bodies as interoperable files. On an
    // unresolvable data dir (headless/misconfigured), fall back to a temp dir so
    // the app still runs; the DB index remains the primary store.
    let maildir_root = config::maildir_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("pesan-maildir"));
    sqlite_services_with_maildir(
        pool,
        maildir::MaildirStore::new(maildir_root),
    )
}

/// Like [`sqlite_services`] but with an explicit Maildir root, so tests can point
/// the body store at an isolated temp directory instead of the user's data dir.
pub fn sqlite_services_with_maildir(
    pool: Db,
    maildir: maildir::MaildirStore,
) -> Services {
    Services {
        accounts: Arc::new(accounts::SqliteAccountRepo::new(
            pool.clone(),
        )),
        cache: Arc::new(sqlite_cache::SqliteMailCache::new(
            pool.clone(),
            maildir.clone(),
        )),
        tokens: Arc::new(token::KeyringTokenStore::new(
            pool.clone(),
        )),
        backend: Arc::new(backend::ImapBackend::new(
            pool, maildir,
        )),
        notifier: Arc::new(notify::DesktopNotifier::new()),
        watcher: Arc::new(idle::IdleWatchFactory),
        daemon_backed: false,
    }
}

/// The TUI wiring: same repos/tokens/notifier, but the backend and watcher route
/// through the `pesan daemon` over its unix socket when one is running, each
/// falling back to the direct IMAP implementation when it is not. This is why an
/// open client no longer opens its own IMAP sessions while a daemon is up.
#[cfg(unix)]
pub fn client_services(pool: Db) -> Services {
    let sock = config::socket_path().unwrap_or_default();
    let maildir_root = config::maildir_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("pesan-maildir"));
    let maildir = maildir::MaildirStore::new(maildir_root);
    let mut services = sqlite_services(pool.clone());
    // Whether a daemon is up right now: lets the TUI skip redundant warm-up.
    // The backend/watcher still fall back to direct IMAP per-call if it dies.
    services.daemon_backed = client::daemon_reachable(&sock);
    services.backend = Arc::new(client::IpcBackend::new(
        pool,
        maildir,
        sock.clone(),
    ));
    services.watcher = Arc::new(client::IpcWatchFactory::new(sock));
    services
}

/// On non-unix there is no socket; the client always connects directly.
#[cfg(not(unix))]
pub fn client_services(pool: Db) -> Services {
    sqlite_services(pool)
}
