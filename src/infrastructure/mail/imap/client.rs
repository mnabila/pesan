use std::sync::Mutex;
use std::sync::mpsc::sync_channel;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_imap::Session;
use async_trait::async_trait;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;
use tokio_rustls::client::TlsStream;

use crate::application::mail::fetch::MailSource;
use crate::application::mail::imap_cmd::{Cmd, ImapHandle, call_on};
use crate::domain::{Draft, Envelope, Folder, Message};

pub(crate) use super::proto::{ENVELOPE_QUERY, envelope_from_fetch};
pub(crate) use super::worker::connect_and_auth;
use super::worker::worker_main;

/// Pagination policy lives in the application layer; the transport honors it.
pub(crate) use crate::application::mail::imap_cmd::LIST_WINDOW;

/// TCP + TLS + IMAP greeting timeout budget for the initial connect.
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-command network budget once the session is live. A single IMAP op (fetch,
/// list, flag, delete, move) that stalls past this is treated as a wedged
/// connection: `async-imap` cancellation would desync the protocol stream, so
/// the worker returns a timeout error to the caller and shuts the session down
/// rather than reuse it. Generous enough for a large message on a slow link.
pub(super) const OP_TIMEOUT: Duration = Duration::from_secs(30);

pub use crate::application::account::connect_params::{ConnectParams, ImapAuth};

pub(crate) type ImapSession = Session<TlsStream<TcpStream>>;

/// A live IMAP account. Dropping it drops the command channel, which ends the
/// worker loop and logs the session out.
pub struct ImapSource {
    hi: UnboundedSender<Cmd>,
    lo: UnboundedSender<Cmd>,
    /// Folder the user is viewing, set by `list_messages`, so the trait
    /// `fetch_message` (which only gets a UID) tells the worker which mailbox to
    /// SELECT. Mirrors `CacheSource::current_folder`; never held across `.await`.
    current_folder: Mutex<String>,
    _worker: JoinHandle<()>,
}

impl ImapSource {
    /// Connect and authenticate, blocking until the session is ready or the
    /// attempt fails. All network happens on the spawned worker thread.
    /// `on_lost` (when set) receives the account label if the worker's session
    /// wedges (a command times out) and shuts down, so the app can
    /// auto-reconnect. A clean shutdown (source dropped) never notifies.
    pub fn connect(
        params: ConnectParams,
        on_lost: Option<UnboundedSender<String>>,
    ) -> Result<Self> {
        // Two command queues: `hi` for interactive requests (open a message,
        // switch folder, flags/send/delete) and `lo` for background sweeps
        // (folder counts, all-folders warm-up). The worker drains `hi` first, so
        // an open never waits behind a long background sync on the one session.
        let (hi_tx, hi_rx) = unbounded_channel::<Cmd>();
        let (lo_tx, lo_rx) = unbounded_channel::<Cmd>();
        let (ready_tx, ready_rx) = sync_channel::<Result<()>>(0);
        let label = params.label.clone();

        let worker = std::thread::Builder::new()
            .name(format!("imap-{label}"))
            .spawn(move || worker_main(params, hi_rx, lo_rx, ready_tx, on_lost))
            .context("spawn IMAP worker thread")?;

        // Propagate the connect outcome (blocks until the worker reports).
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                hi: hi_tx,
                lo: lo_tx,
                current_folder: Mutex::new("INBOX".to_string()),
                _worker: worker,
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow!("IMAP worker exited before reporting readiness")),
        }
    }

    /// Send an interactive command and await its reply.
    async fn call<T>(&self, make: impl FnOnce(oneshot::Sender<Result<T>>) -> Cmd) -> Result<T> {
        call_on(&self.hi, make).await
    }

    /// A cloneable, `Send` handle onto this session's command channels, so reads
    /// (list/fetch) can run on a background task off the UI event loop.
    pub fn handle(&self) -> ImapHandle {
        ImapHandle {
            hi: self.hi.clone(),
            lo: self.lo.clone(),
        }
    }
}

#[async_trait]
impl MailSource for ImapSource {
    async fn list_folders(&self) -> Result<Vec<Folder>> {
        self.call(Cmd::ListFolders).await
    }

    async fn list_messages(&self, folder: &str) -> Result<Vec<Envelope>> {
        // Remember the viewed folder so a later bare-UID `fetch_message` knows
        // which mailbox to SELECT.
        *self.current_folder.lock().unwrap() = folder.to_string();
        let folder = folder.to_string();
        self.call(|tx| Cmd::ListMessages(folder, tx)).await
    }

    async fn fetch_message(&self, uid: u64) -> Result<Message> {
        let folder = self.current_folder.lock().unwrap().clone();
        self.call(|tx| Cmd::FetchMessage(folder, uid, tx)).await
    }

    async fn send(&mut self, draft: &Draft) -> Result<()> {
        let draft = draft.clone();
        self.call(|tx| Cmd::Send(draft, tx)).await
    }

    async fn set_seen(&mut self, uid: u64, seen: bool) {
        let folder = self.current_folder.lock().unwrap().clone();
        if let Err(e) = self.call(|tx| Cmd::SetSeen(folder, uid, seen, tx)).await {
            tracing::warn!("set_seen({uid}) failed: {e}");
        }
    }

    async fn set_flagged(&mut self, uid: u64, flagged: bool) {
        let folder = self.current_folder.lock().unwrap().clone();
        if let Err(e) = self
            .call(|tx| Cmd::SetFlagged(folder, uid, flagged, tx))
            .await
        {
            tracing::warn!("set_flagged({uid}) failed: {e}");
        }
    }

    async fn delete(&mut self, uid: u64) -> Result<()> {
        let folder = self.current_folder.lock().unwrap().clone();
        self.call(|tx| Cmd::Delete(folder, uid, tx)).await
    }

    async fn move_to(&mut self, uid: u64, folder: &str) -> Result<()> {
        let src = self.current_folder.lock().unwrap().clone();
        let dest = folder.to_string();
        self.call(|tx| Cmd::MoveTo(src, uid, dest, tx)).await
    }

    fn imap_handle(&self) -> Option<ImapHandle> {
        Some(self.handle())
    }
}
