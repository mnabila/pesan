use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;

use crate::application::account::connect_params::ConnectParams;
use crate::domain::NewMail;

/// A running watcher. Dropping it stops the background work.
pub trait WatchHandle: Send {}

/// Factory port: starts an arrival watcher for one account's mailbox.
#[async_trait]
pub trait NewMailWatch: Send + Sync {
    /// Spawn the watcher on its own thread/connection. It pushes
    /// [`NewMail`] batches to `events` until the handle is dropped.
    fn watch(
        &self,
        params: ConnectParams,
        mailbox: String,
        account: String,
        poll_interval: Duration,
        events: UnboundedSender<NewMail>,
    ) -> Box<dyn WatchHandle>;
}
