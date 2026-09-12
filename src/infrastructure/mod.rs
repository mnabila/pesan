pub mod auth;
pub mod database;
pub mod mail;

use crate::application::Services;
use crate::infrastructure::database::Db;

/// The production wiring: SQLite repos over the shared pool, keyring-backed
/// token store, desktop notifier, and a **direct** IMAP backend/watcher. Used by
/// the daemon (it *is* the server) and as the fallback the client wraps.
pub fn sqlite_services(pool: Db) -> Services {
    Services {
        accounts: std::sync::Arc::new(database::accounts::SqliteAccountRepo::new(pool.clone())),
        cache: std::sync::Arc::new(database::cache::SqliteMailCache::new(pool.clone())),
        tokens: std::sync::Arc::new(auth::token::KeyringTokenStore::new(pool.clone())),
        backend: std::sync::Arc::new(mail::backend::ImapBackend::new(pool)),
        notifier: std::sync::Arc::new(mail::notify::DesktopNotifier::new()),
        watcher: std::sync::Arc::new(mail::imap::idle::IdleWatchFactory),
        daemon_backed: false,
    }
}

/// The TUI wiring: same repos/tokens/notifier, but the backend and watcher route
/// through the `pesan daemon` over its unix socket when one is running, each
/// falling back to the direct IMAP implementation when it is not. This is why an
/// open client no longer opens its own IMAP sessions while a daemon is up.
#[cfg(unix)]
pub fn client_services(pool: Db) -> Services {
    let sock = crate::bootstrap::config::socket_path().unwrap_or_default();
    let mut services = sqlite_services(pool.clone());
    // Whether a daemon is up right now: lets the TUI skip redundant warm-up.
    // The backend/watcher still fall back to direct IMAP per-call if it dies.
    services.daemon_backed = mail::ipc::client::daemon_reachable(&sock);
    services.backend = std::sync::Arc::new(mail::ipc::client::IpcBackend::new(pool, sock.clone()));
    services.watcher = std::sync::Arc::new(mail::ipc::client::IpcWatchFactory::new(sock));
    services
}

/// On non-unix there is no socket; the client always connects directly.
#[cfg(not(unix))]
pub fn client_services(pool: Db) -> Services {
    sqlite_services(pool)
}
