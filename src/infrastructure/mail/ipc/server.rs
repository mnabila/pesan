//! Daemon-side socket server. Accepts client connections, maps each `Request`
//! to the owning account's live [`ImapHandle`], and streams `NewMail` pushes to
//! subscribers. The daemon supplies a [`Sessions`] provider that owns the actual
//! IMAP sessions (see `crate::daemon`).

use std::sync::Arc;

use async_trait::async_trait;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

use super::{PROTOCOL_VERSION, Payload, PushEvent, Request, Response, RpcOp, ServerMsg, read_frame, write_frame};
use crate::application::mail::imap_cmd::ImapHandle;

/// The daemon's session registry, seen by the server. Resolves an account name
/// to a command handle (connecting lazily) and hands out arrival subscriptions.
#[async_trait]
pub trait Sessions: Send + Sync + 'static {
    /// A command handle for `account`, connecting the session if needed.
    async fn handle(&self, account: &str) -> Result<ImapHandle, String>;
    /// Subscribe to new-mail pushes for every account (the client filters by name).
    fn subscribe(&self) -> broadcast::Receiver<PushEvent>;
}

/// Accept loop. Each connection is served on its own task and stops when the
/// client disconnects. Runs until the daemon process exits (which drops it).
pub async fn serve(listener: UnixListener, sessions: Arc<dyn Sessions>) {
    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                tokio::spawn(serve_conn(stream, sessions.clone()));
            }
            Err(e) => tracing::warn!("ipc: accept failed: {e}"),
        }
    }
}

/// Serve one client connection: a reader loop dispatching requests, and a single
/// writer task that serializes all responses/pushes back over the socket.
async fn serve_conn(stream: UnixStream, sessions: Arc<dyn Sessions>) {
    let (mut rd, wr) = stream.into_split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ServerMsg>();

    // Writer: the only task that touches the write half, so concurrent
    // request handlers never interleave bytes.
    let writer = tokio::spawn(async move {
        let mut wr = wr;
        while let Some(msg) = out_rx.recv().await {
            if write_frame(&mut wr, &msg).await.is_err() {
                break;
            }
        }
    });

    while let Ok(req) = read_frame::<Request, _>(&mut rd).await {
        dispatch(req, &sessions, &out_tx);
    }
    drop(out_tx); // let the writer drain and exit
    writer.abort();
}

/// Route one request. Control ops answer inline; mail ops and subscriptions run
/// on their own task so a slow fetch never blocks the reader loop.
fn dispatch(req: Request, sessions: &Arc<dyn Sessions>, out_tx: &mpsc::UnboundedSender<ServerMsg>) {
    let Request { id, account, op } = req;
    match op {
        RpcOp::Hello { .. } => {
            let _ = out_tx.send(ServerMsg::Response(Response {
                id,
                result: Ok(Payload::Hello { version: PROTOCOL_VERSION }),
            }));
        }
        RpcOp::Subscribe => {
            tracing::info!("{account}: subscribe");
            // Ack, then relay this account's pushes until the socket closes.
            let _ = out_tx.send(ServerMsg::Response(Response { id, result: Ok(Payload::Unit) }));
            let mut rx = sessions.subscribe();
            let out = out_tx.clone();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) if ev.account() == account => {
                            if out.send(ServerMsg::Push(ev)).is_err() {
                                break;
                            }
                        }
                        Ok(_) => {} // another account's push
                        Err(broadcast::error::RecvError::Lagged(_)) => {} // dropped some; keep going
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        op => {
            let sessions = sessions.clone();
            let out = out_tx.clone();
            tokio::spawn(async move {
                let desc = describe_op(&op);
                let result = match sessions.handle(&account).await {
                    Ok(h) => run_op(&h, op).await,
                    Err(e) => Err(e),
                };
                // Log every served request so the daemon's role as the server is
                // fully visible.
                match &result {
                    Ok(_) => tracing::info!("{account}: {desc}"),
                    Err(e) => tracing::warn!("{account}: {desc} failed: {e}"),
                }
                let _ = out.send(ServerMsg::Response(Response { id, result }));
            });
        }
    }
}

/// A short human description of an op for the daemon's request log.
fn describe_op(op: &RpcOp) -> String {
    match op {
        RpcOp::Hello { .. } => "hello".to_string(),
        RpcOp::Subscribe => "subscribe".to_string(),
        RpcOp::ListFolders => "list-folders".to_string(),
        RpcOp::FolderCounts(n) => format!("folder-counts ({} folders)", n.len()),
        RpcOp::ListMessages(f) => format!("list {f}"),
        RpcOp::ListMessagesWindow { folder, offset, limit } => {
            format!("list {folder} (older +{offset}..{})", offset + limit)
        }
        RpcOp::FetchMessage { folder, uid } => format!("fetch {folder}/{uid}"),
        RpcOp::SetSeen { folder, uid, seen } => format!("set-seen {folder}/{uid}={seen}"),
        RpcOp::SetFlagged { folder, uid, flagged } => format!("set-flagged {folder}/{uid}={flagged}"),
        RpcOp::Send(_) => "send".to_string(),
        RpcOp::Delete { folder, uid } => format!("delete {folder}/{uid}"),
        RpcOp::MoveTo { folder, uid, dest } => format!("move {folder}/{uid} -> {dest}"),
    }
}

/// Execute one mail op on a live handle, shaping the reply payload per op.
async fn run_op(h: &ImapHandle, op: RpcOp) -> Result<Payload, String> {
    let err = |e: anyhow::Error| e.to_string();
    match op {
        RpcOp::ListFolders => h.list_folders().await.map(Payload::Folders).map_err(err),
        RpcOp::FolderCounts(names) => Ok(Payload::Counts(h.folder_counts(names).await.map_err(err)?)),
        RpcOp::ListMessages(folder) => {
            h.list_messages(&folder).await.map(Payload::Envelopes).map_err(err)
        }
        RpcOp::ListMessagesWindow { folder, offset, limit } => h
            .list_messages_older(&folder, offset, limit)
            .await
            .map(Payload::Window)
            .map_err(err),
        RpcOp::FetchMessage { folder, uid } => h
            .fetch_message(&folder, uid)
            .await
            .map(|m| Payload::Message(Box::new(m)))
            .map_err(err),
        RpcOp::SetSeen { folder, uid, seen } => {
            h.set_seen(&folder, uid, seen).await.map(|_| Payload::Unit).map_err(err)
        }
        RpcOp::SetFlagged { folder, uid, flagged } => h
            .set_flagged(&folder, uid, flagged)
            .await
            .map(|_| Payload::Unit)
            .map_err(err),
        RpcOp::Send(draft) => h.send(draft).await.map(|_| Payload::Unit).map_err(err),
        RpcOp::Delete { folder, uid } => {
            h.delete(&folder, uid).await.map(|_| Payload::Unit).map_err(err)
        }
        RpcOp::MoveTo { folder, uid, dest } => {
            h.move_to(&folder, uid, &dest).await.map(|_| Payload::Unit).map_err(err)
        }
        // Control ops are handled in `dispatch`, never reach here.
        RpcOp::Hello { .. } | RpcOp::Subscribe => Ok(Payload::Unit),
    }
}
