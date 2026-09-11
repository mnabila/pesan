use anyhow::{Result, anyhow};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::domain::{Draft, Envelope, Folder, Message, MessageWindow};

/// Newest-first message window fetched per folder, and the page size for
/// "load older messages" pagination. Bounds each round-trip on large mailboxes;
/// the app pages further back on demand (see `App::load_more_older`). This is
/// the application's pagination policy; the transport just honors it.
pub const LIST_WINDOW: u32 = 100;

/// A request to the IMAP worker; each carries a one-shot reply channel that the
/// async trait method awaits.
pub(crate) enum Cmd {
    ListFolders(oneshot::Sender<Result<Vec<Folder>>>),
    FolderCounts(Vec<String>, oneshot::Sender<Vec<(String, usize, usize)>>),
    ListMessages(String, oneshot::Sender<Result<Vec<Envelope>>>),
    /// Paginated list: (folder, offset, limit). `offset` newest messages are
    /// skipped, then up to `limit` older envelopes are fetched. Used to page
    /// back through large mailboxes on demand.
    ListMessagesWindow(String, u32, u32, oneshot::Sender<Result<MessageWindow>>),
    /// Fetch one message body. Carries the folder so the worker `SELECT`s the
    /// right mailbox first - the shared session's selected folder is otherwise
    /// mutated by background sweeps, which made an open fetch the wrong mailbox
    /// ("message N not found").
    FetchMessage(String, u64, oneshot::Sender<Result<Message>>),
    // Flag/delete/move all carry the source folder so the worker SELECTs the
    // right mailbox first (a background sweep may have moved the session).
    SetSeen(String, u64, bool, oneshot::Sender<Result<()>>),
    SetFlagged(String, u64, bool, oneshot::Sender<Result<()>>),
    Send(Draft, oneshot::Sender<Result<()>>),
    Delete(String, u64, oneshot::Sender<Result<()>>),
    MoveTo(String, u64, String, oneshot::Sender<Result<()>>),
}

/// Send a command on `chan` and await its one-shot reply.
pub(crate) async fn call_on<T>(
    chan: &UnboundedSender<Cmd>,
    make: impl FnOnce(oneshot::Sender<Result<T>>) -> Cmd,
) -> Result<T> {
    let (tx, rx) = oneshot::channel::<Result<T>>();
    chan.send(make(tx))
        .map_err(|_| anyhow!("IMAP worker is gone"))?;
    rx.await
        .map_err(|_| anyhow!("IMAP worker dropped the reply"))?
}

/// A lightweight, cloneable handle to a live IMAP worker's command channels.
/// Interactive reads go on the high-priority queue; background sweeps
/// (`*_bg`, `folder_counts`) go on the low-priority queue.
#[derive(Clone)]
pub struct ImapHandle {
    pub(crate) hi: UnboundedSender<Cmd>,
    pub(crate) lo: UnboundedSender<Cmd>,
}

impl ImapHandle {
    /// Interactive: list a folder the user just navigated to.
    pub async fn list_messages(&self, folder: &str) -> Result<Vec<Envelope>> {
        let folder = folder.to_string();
        call_on(&self.hi, |tx| Cmd::ListMessages(folder, tx)).await
    }

    /// Background: list a folder for cache warm-up (low priority).
    pub async fn list_messages_bg(&self, folder: &str) -> Result<Vec<Envelope>> {
        let folder = folder.to_string();
        call_on(&self.lo, |tx| Cmd::ListMessages(folder, tx)).await
    }

    /// Interactive: fetch an older page of a folder (pagination). `offset` is the
    /// number of newest messages already loaded (skipped); `limit` the page size.
    /// Runs on the high-priority queue since the user is waiting on it.
    pub async fn list_messages_older(
        &self,
        folder: &str,
        offset: u32,
        limit: u32,
    ) -> Result<MessageWindow> {
        let folder = folder.to_string();
        call_on(&self.hi, |tx| {
            Cmd::ListMessagesWindow(folder, offset, limit, tx)
        })
        .await
    }

    /// Interactive: fetch a message the user just opened. `folder` is sent so the
    /// worker SELECTs the right mailbox first, regardless of what a background
    /// sweep left the shared session selected on.
    pub async fn fetch_message(&self, folder: &str, uid: u64) -> Result<Message> {
        let folder = folder.to_string();
        call_on(&self.hi, |tx| Cmd::FetchMessage(folder, uid, tx)).await
    }

    /// Background: fetch a message body to warm the cache (low priority), so a
    /// later open of a recent message is an instant cache hit. Runs on the `lo`
    /// queue behind any interactive open.
    pub async fn fetch_message_bg(&self, folder: &str, uid: u64) -> Result<Message> {
        let folder = folder.to_string();
        call_on(&self.lo, |tx| Cmd::FetchMessage(folder, uid, tx)).await
    }

    pub async fn set_seen(&self, folder: &str, uid: u64, seen: bool) -> Result<()> {
        let folder = folder.to_string();
        call_on(&self.hi, |tx| Cmd::SetSeen(folder, uid, seen, tx)).await
    }

    pub async fn set_flagged(&self, folder: &str, uid: u64, flagged: bool) -> Result<()> {
        let folder = folder.to_string();
        call_on(&self.hi, |tx| Cmd::SetFlagged(folder, uid, flagged, tx)).await
    }

    /// Background: delete a message from `folder` on the server (low priority -
    /// the UI has already removed it optimistically).
    pub async fn delete(&self, folder: &str, uid: u64) -> Result<()> {
        let folder = folder.to_string();
        call_on(&self.lo, |tx| Cmd::Delete(folder, uid, tx)).await
    }

    /// Background: move a message from `src` to `dest` mailbox (used for Archive).
    pub async fn move_to(&self, src: &str, uid: u64, dest: &str) -> Result<()> {
        let (src, dest) = (src.to_string(), dest.to_string());
        call_on(&self.lo, |tx| Cmd::MoveTo(src, uid, dest, tx)).await
    }

    /// Background: fetch (MESSAGES, UNSEEN) counts for `names` to fill the
    /// sidebar after a fast connect (low priority).
    pub async fn folder_counts(&self, names: Vec<String>) -> Result<Vec<(String, usize, usize)>> {
        let (tx, rx) = oneshot::channel();
        self.lo
            .send(Cmd::FolderCounts(names, tx))
            .map_err(|_| anyhow!("IMAP worker is gone"))?;
        rx.await
            .map_err(|_| anyhow!("IMAP worker dropped the reply"))
    }
}
