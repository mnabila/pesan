pub mod auth;
pub mod database;
pub mod mail;

use crate::application::Services;
use crate::infrastructure::database::Db;

/// The production wiring: SQLite repos over the shared pool, keyring-backed
/// token store, desktop notifier. The only place port impls are chosen.
pub fn sqlite_services(pool: Db) -> Services {
    Services {
        accounts: std::sync::Arc::new(database::accounts::SqliteAccountRepo::new(pool.clone())),
        cache: std::sync::Arc::new(database::cache::SqliteMailCache::new(pool.clone())),
        tokens: std::sync::Arc::new(auth::token::KeyringTokenStore::new(pool.clone())),
        backend: std::sync::Arc::new(mail::backend::ImapBackend::new(pool)),
        notifier: std::sync::Arc::new(mail::notify::DesktopNotifier::new()),
        watcher: std::sync::Arc::new(mail::imap::idle::IdleWatchFactory),
    }
}
