//! TUI-side IPC adapters. `IpcBackend`/`IpcMailSource` route mail operations to
//! the daemon over the unix socket; `IpcWatchFactory` subscribes to the daemon's
//! arrival pushes instead of opening its own IMAP IDLE. Strict client: no direct
//! IMAP - if the daemon is unreachable the online path errors and the app shows
//! the offline cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

use super::{
    PROTOCOL_VERSION, Payload, PushEvent, Request, RpcOp, ServerMsg, read_frame, write_frame,
};
use crate::application::MailSource;
use crate::application::account::connect_params::ConnectParams;
use crate::application::mail::imap_cmd::{Cmd, ImapHandle};
use crate::application::oauth::{ResolvedOAuth, TokenSet};
use crate::application::ports::{MailBackend, NewMailWatch, WatchHandle};
use crate::domain::{Draft, Folder, MailUpdate, NewMail};
use crate::infrastructure::database::Db;
use crate::infrastructure::mail::backend::ImapBackend;

/// True if the daemon socket accepts a connection right now. A cheap synchronous
/// probe, used where the decision must be made without `await` (watcher setup,
/// and the `client_services` daemon-backed flag).
pub(crate) fn daemon_reachable(sock: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(sock).is_ok()
}

// ===================================================================== backend

/// `MailBackend` that prefers the daemon and falls back to a direct IMAP
/// connection when the socket is unreachable.
pub struct IpcBackend {
    inner: ImapBackend,
    sock: PathBuf,
}

impl IpcBackend {
    pub fn new(pool: Db, sock: PathBuf) -> Self {
        Self { inner: ImapBackend::new(pool), sock }
    }
}

#[async_trait]
impl MailBackend for IpcBackend {
    async fn refresh_access_token(
        &self,
        oauth: ResolvedOAuth,
        refresh_token: String,
    ) -> Result<TokenSet> {
        // Token refresh is rare and usually served from the shared access-token
        // cache the daemon warms; keep it local either way.
        self.inner.refresh_access_token(oauth, refresh_token).await
    }

    async fn connect(
        &self,
        params: ConnectParams,
        on_lost: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<Box<dyn MailSource>> {
        // Strict client: the daemon is the only thing that talks to IMAP. If the
        // socket is unreachable we error (the app then shows the offline cache)
        // rather than falling back to a direct IMAP connection.
        let src = connect_ipc(&self.sock, &params.label, on_lost)
            .await
            .map_err(|e| anyhow!("pesan daemon unavailable: {e:#}"))?;
        tracing::info!("'{}' routed through daemon", params.label);
        Ok(Box::new(src))
    }

    async fn offline_source(&self, account_id: Option<i64>) -> Box<dyn MailSource> {
        // Cache/empty source only - no network, so this is fine in a strict client.
        self.inner.offline_source(account_id).await
    }
}

// ================================================================ mail source

/// A `MailSource` whose operations are shipped to the daemon via a client shim
/// [`ImapHandle`]. Mirrors the live `ImapSource`, tracking the viewed folder so
/// bare-UID trait calls know which mailbox to target.
struct IpcMailSource {
    handle: ImapHandle,
    current_folder: Mutex<String>,
}

#[async_trait]
impl MailSource for IpcMailSource {
    async fn list_folders(&self) -> Result<Vec<Folder>> {
        self.handle.list_folders().await
    }

    async fn list_messages(&self, folder: &str) -> Result<Vec<crate::domain::Envelope>> {
        *self.current_folder.lock().unwrap() = folder.to_string();
        self.handle.list_messages(folder).await
    }

    async fn fetch_message(&self, uid: u64) -> Result<crate::domain::Message> {
        let folder = self.current_folder.lock().unwrap().clone();
        self.handle.fetch_message(&folder, uid).await
    }

    async fn send(&mut self, draft: &Draft) -> Result<()> {
        self.handle.send(draft.clone()).await
    }

    async fn set_seen(&mut self, uid: u64, seen: bool) {
        let folder = self.current_folder.lock().unwrap().clone();
        if let Err(e) = self.handle.set_seen(&folder, uid, seen).await {
            tracing::warn!("set_seen({uid}) failed: {e}");
        }
    }

    async fn set_flagged(&mut self, uid: u64, flagged: bool) {
        let folder = self.current_folder.lock().unwrap().clone();
        if let Err(e) = self.handle.set_flagged(&folder, uid, flagged).await {
            tracing::warn!("set_flagged({uid}) failed: {e}");
        }
    }

    async fn delete(&mut self, uid: u64) -> Result<()> {
        let folder = self.current_folder.lock().unwrap().clone();
        self.handle.delete(&folder, uid).await
    }

    async fn move_to(&mut self, uid: u64, folder: &str) -> Result<()> {
        let src = self.current_folder.lock().unwrap().clone();
        self.handle.move_to(&src, uid, folder).await
    }

    fn imap_handle(&self) -> Option<ImapHandle> {
        Some(self.handle.clone())
    }
}

// ======================================================================= shim

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Payload, String>>>>>;

/// Clonable request issuer over the shim connection: correlates each request id
/// to a oneshot the reader task fulfills.
#[derive(Clone)]
struct Caller {
    account: String,
    out_tx: mpsc::UnboundedSender<Request>,
    pending: Pending,
    next_id: Arc<AtomicU64>,
}

impl Caller {
    async fn call(&self, op: RpcOp) -> Result<Payload, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let req = Request { id, account: self.account.clone(), op };
        if self.out_tx.send(req).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err("daemon connection closed".to_string());
        }
        match rx.await {
            Ok(result) => result,
            Err(_) => Err("daemon connection dropped".to_string()),
        }
    }
}

/// Dial the daemon, perform the `Hello` handshake, and wire up the shim tasks.
/// Returns a source whose `imap_handle()` transparently routes to the daemon.
async fn connect_ipc(
    sock: &Path,
    account: &str,
    on_lost: Option<mpsc::UnboundedSender<String>>,
) -> Result<IpcMailSource> {
    let stream = UnixStream::connect(sock)
        .await
        .map_err(|e| anyhow!("dial daemon socket {}: {e}", sock.display()))?;
    let (mut rd, mut wr) = stream.into_split();

    // Version handshake: a mismatch (or any protocol error) bails so the caller
    // falls back to a direct connection.
    write_frame(
        &mut wr,
        &Request {
            id: 0,
            account: account.to_string(),
            op: RpcOp::Hello { version: PROTOCOL_VERSION },
        },
    )
    .await?;
    match read_frame::<ServerMsg, _>(&mut rd).await? {
        ServerMsg::Response(r) => match r.result {
            Ok(Payload::Hello { version }) if version == PROTOCOL_VERSION => {}
            Ok(Payload::Hello { version }) => {
                bail!("daemon protocol {version} != client {PROTOCOL_VERSION}")
            }
            other => bail!("unexpected hello reply: {other:?}"),
        },
        ServerMsg::Push(_) => bail!("unexpected push before hello"),
    }

    let (handle, hi_rx, lo_rx) = ImapHandle::channels();
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Request>();

    tokio::spawn(writer_loop(wr, out_rx));
    tokio::spawn(reader_loop(rd, pending.clone(), on_lost, account.to_string()));
    let caller = Caller {
        account: account.to_string(),
        out_tx,
        pending,
        next_id: Arc::new(AtomicU64::new(1)),
    };
    tokio::spawn(dispatch_loop(hi_rx, lo_rx, caller));

    Ok(IpcMailSource {
        handle,
        current_folder: Mutex::new("INBOX".to_string()),
    })
}

/// Drain outbound requests to the socket. Ends when the shim is dropped (senders
/// gone) or the socket errors.
async fn writer_loop(
    mut wr: tokio::net::unix::OwnedWriteHalf,
    mut out_rx: mpsc::UnboundedReceiver<Request>,
) {
    while let Some(req) = out_rx.recv().await {
        if write_frame(&mut wr, &req).await.is_err() {
            break;
        }
    }
}

/// Read responses and fulfill their pending oneshots. On EOF/error, fail all
/// in-flight calls and notify `on_lost` so the app reconnects (which re-dials
/// the socket, or falls back to direct IMAP).
async fn reader_loop(
    mut rd: tokio::net::unix::OwnedReadHalf,
    pending: Pending,
    on_lost: Option<mpsc::UnboundedSender<String>>,
    account: String,
) {
    loop {
        match read_frame::<ServerMsg, _>(&mut rd).await {
            Ok(ServerMsg::Response(resp)) => {
                if let Some(tx) = pending.lock().unwrap().remove(&resp.id) {
                    let _ = tx.send(resp.result);
                }
            }
            Ok(ServerMsg::Push(_)) => {} // command connection never subscribes
            Err(_) => break,
        }
    }
    pending.lock().unwrap().clear(); // drop senders -> in-flight callers get Err
    if let Some(tx) = on_lost {
        let _ = tx.send(account);
    }
}

/// Drain the handle's command queues (hi before lo) and run each as an IPC
/// round-trip on its own task, preserving concurrency and priority.
async fn dispatch_loop(
    mut hi_rx: mpsc::UnboundedReceiver<Cmd>,
    mut lo_rx: mpsc::UnboundedReceiver<Cmd>,
    caller: Caller,
) {
    loop {
        let cmd = tokio::select! {
            biased;
            Some(cmd) = hi_rx.recv() => cmd,
            Some(cmd) = lo_rx.recv() => cmd,
            else => break,
        };
        let caller = caller.clone();
        tokio::spawn(async move { handle_cmd(caller, cmd).await });
    }
}

/// Translate one `Cmd` into an `RpcOp`, await the daemon's reply, and fulfill the
/// command's typed oneshot from the returned payload.
async fn handle_cmd(caller: Caller, cmd: Cmd) {
    match cmd {
        Cmd::ListFolders(reply) => {
            let r = caller.call(RpcOp::ListFolders).await;
            let _ = reply.send(as_folders(r));
        }
        Cmd::FolderCounts(names, reply) => {
            let r = caller.call(RpcOp::FolderCounts(names)).await;
            let _ = reply.send(as_counts(r));
        }
        Cmd::ListMessages(folder, reply) => {
            let r = caller.call(RpcOp::ListMessages(folder)).await;
            let _ = reply.send(as_envelopes(r));
        }
        Cmd::ListMessagesWindow(folder, offset, limit, reply) => {
            let r = caller
                .call(RpcOp::ListMessagesWindow { folder, offset, limit })
                .await;
            let _ = reply.send(as_window(r));
        }
        Cmd::FetchMessage(folder, uid, reply) => {
            let r = caller.call(RpcOp::FetchMessage { folder, uid }).await;
            let _ = reply.send(as_message(r));
        }
        Cmd::SetSeen(folder, uid, seen, reply) => {
            let r = caller.call(RpcOp::SetSeen { folder, uid, seen }).await;
            let _ = reply.send(as_unit(r));
        }
        Cmd::SetFlagged(folder, uid, flagged, reply) => {
            let r = caller.call(RpcOp::SetFlagged { folder, uid, flagged }).await;
            let _ = reply.send(as_unit(r));
        }
        Cmd::Send(draft, reply) => {
            let r = caller.call(RpcOp::Send(draft)).await;
            let _ = reply.send(as_unit(r));
        }
        Cmd::Delete(folder, uid, reply) => {
            let r = caller.call(RpcOp::Delete { folder, uid }).await;
            let _ = reply.send(as_unit(r));
        }
        Cmd::MoveTo(folder, uid, dest, reply) => {
            let r = caller.call(RpcOp::MoveTo { folder, uid, dest }).await;
            let _ = reply.send(as_unit(r));
        }
    }
}

fn payload_err(p: &Payload) -> anyhow::Error {
    anyhow!("daemon returned unexpected payload: {p:?}")
}

fn as_unit(r: Result<Payload, String>) -> Result<()> {
    match r {
        Ok(Payload::Unit) => Ok(()),
        Ok(p) => Err(payload_err(&p)),
        Err(e) => Err(anyhow!(e)),
    }
}

fn as_folders(r: Result<Payload, String>) -> Result<Vec<Folder>> {
    match r {
        Ok(Payload::Folders(f)) => Ok(f),
        Ok(p) => Err(payload_err(&p)),
        Err(e) => Err(anyhow!(e)),
    }
}

fn as_envelopes(r: Result<Payload, String>) -> Result<Vec<crate::domain::Envelope>> {
    match r {
        Ok(Payload::Envelopes(e)) => Ok(e),
        Ok(p) => Err(payload_err(&p)),
        Err(e) => Err(anyhow!(e)),
    }
}

fn as_window(r: Result<Payload, String>) -> Result<crate::domain::MessageWindow> {
    match r {
        Ok(Payload::Window(w)) => Ok(w),
        Ok(p) => Err(payload_err(&p)),
        Err(e) => Err(anyhow!(e)),
    }
}

fn as_message(r: Result<Payload, String>) -> Result<crate::domain::Message> {
    match r {
        Ok(Payload::Message(m)) => Ok(*m),
        Ok(p) => Err(payload_err(&p)),
        Err(e) => Err(anyhow!(e)),
    }
}

/// Folder counts use a bare (non-`Result`) reply channel; an error yields an
/// empty list, exactly as the live worker does on failure.
fn as_counts(r: Result<Payload, String>) -> Vec<(String, usize, usize)> {
    match r {
        Ok(Payload::Counts(c)) => c,
        _ => Vec::new(),
    }
}

// ===================================================================== watcher

/// `NewMailWatch` that subscribes to the daemon's arrival stream. Strict client:
/// it never opens its own IMAP IDLE - if the daemon is unreachable the watcher is
/// inert (in practice watchers are only started after a successful connect).
pub struct IpcWatchFactory {
    sock: PathBuf,
}

impl IpcWatchFactory {
    pub fn new(sock: PathBuf) -> Self {
        Self { sock }
    }
}

/// Keeps the IPC subscribe relay alive; dropping the stop-signal ends it.
struct IpcWatchHandle {
    _stop: Option<oneshot::Sender<()>>,
}
impl WatchHandle for IpcWatchHandle {}

impl NewMailWatch for IpcWatchFactory {
    fn watch(
        &self,
        _params: ConnectParams,
        _mailbox: String,
        account: String,
        _poll_interval: Duration,
        events: mpsc::UnboundedSender<MailUpdate>,
    ) -> Box<dyn WatchHandle> {
        let stop = if daemon_reachable(&self.sock) {
            let (stop_tx, stop_rx) = oneshot::channel();
            tokio::spawn(subscribe_loop(self.sock.clone(), account, events, stop_rx));
            Some(stop_tx)
        } else {
            None // no daemon: inert watcher (strict client never opens IMAP IDLE)
        };
        Box::new(IpcWatchHandle { _stop: stop })
    }
}

/// Maintain a subscribe connection, relaying the account's pushes into the app
/// event stream (arrivals + folder re-syncs) until the socket drops or the
/// handle is dropped.
async fn subscribe_loop(
    sock: PathBuf,
    account: String,
    events: mpsc::UnboundedSender<MailUpdate>,
    stop_rx: oneshot::Receiver<()>,
) {
    let relay = async {
        let Ok(stream) = UnixStream::connect(&sock).await else {
            return;
        };
        let (mut rd, mut wr) = stream.into_split();
        let sub = Request { id: 0, account: account.clone(), op: RpcOp::Subscribe };
        if write_frame(&mut wr, &sub).await.is_err() {
            return;
        }
        loop {
            match read_frame::<ServerMsg, _>(&mut rd).await {
                Ok(ServerMsg::Push(ev)) if ev.account() == account => {
                    let update = match ev {
                        PushEvent::NewMail { account, folder, envelopes } => {
                            MailUpdate::Arrived(NewMail { account, folder, envelopes })
                        }
                        PushEvent::FolderSync { account, folder, envelopes } => {
                            MailUpdate::FolderSynced { account, folder, envelopes }
                        }
                    };
                    if events.send(update).is_err() {
                        break;
                    }
                }
                Ok(_) => {} // subscribe ack, or another account's push
                Err(_) => break,
            }
        }
    };
    tokio::select! {
        _ = relay => {}
        _ = stop_rx => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::MailSource;
    use crate::domain::{Folder, FolderCategory};
    use crate::infrastructure::mail::ipc::{Response, ServerMsg};
    use tokio::net::UnixListener;

    /// A minimal in-process daemon: answers `Hello`, `ListFolders`, and
    /// `ListMessages` so the client shim can be driven end-to-end over a real
    /// unix socket.
    async fn fake_server(listener: UnixListener) {
        let (stream, _) = listener.accept().await.unwrap();
        let (mut rd, mut wr) = stream.into_split();
        while let Ok(req) = read_frame::<Request, _>(&mut rd).await {
            let result = match req.op {
                RpcOp::Hello { .. } => Ok(Payload::Hello { version: PROTOCOL_VERSION }),
                RpcOp::ListFolders => Ok(Payload::Folders(vec![Folder {
                    name: "INBOX".to_string(),
                    total: 1,
                    unread: 0,
                    category: FolderCategory::Mailbox,
                }])),
                RpcOp::ListMessages(_) => Ok(Payload::Envelopes(vec![])),
                _ => Err("unsupported".to_string()),
            };
            let msg = ServerMsg::Response(Response { id: req.id, result });
            if write_frame(&mut wr, &msg).await.is_err() {
                break;
            }
        }
    }

    #[tokio::test]
    async fn client_shim_round_trips_through_socket() {
        let sock = std::env::temp_dir().join(format!("pesan-ipc-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(fake_server(listener));

        // connect_ipc performs the Hello handshake and wires up the shim.
        let src = connect_ipc(&sock, "acct", None).await.unwrap();
        // Drives Cmd::ListFolders -> RpcOp::ListFolders -> Payload::Folders.
        let folders = src.list_folders().await.unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].name, "INBOX");
        // Drives Cmd::ListMessages -> RpcOp::ListMessages -> Payload::Envelopes.
        let envs = src.list_messages("INBOX").await.unwrap();
        assert!(envs.is_empty());
        // And the handle exposed to background tasks routes the same way.
        let handle = src.imap_handle().expect("ipc source exposes a handle");
        assert_eq!(handle.list_folders().await.unwrap().len(), 1);

        let _ = std::fs::remove_file(&sock);
    }

    #[tokio::test]
    async fn connect_ipc_fails_when_socket_absent() {
        let sock = std::env::temp_dir().join(format!("pesan-ipc-missing-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        // No listener: dialing must error so the backend falls back to direct IMAP.
        assert!(connect_ipc(&sock, "acct", None).await.is_err());
    }
}
