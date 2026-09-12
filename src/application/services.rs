use std::sync::Arc;

use crate::application::ports::{
    AccountRepo, MailBackend, MailCache, NewMailWatch, Notifier, TokenStore,
};

/// Cloneable bundle of port implementations.
// `accounts`/`notifier` are consumed by Phase 3 (onboarding) and the new-mail
// notification path respectively.
#[allow(dead_code)]
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
