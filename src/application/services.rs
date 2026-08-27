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
}
