//! Wire protocol shared by the daemon's socket server (`server.rs`) and the TUI
//! client (`client.rs`). Frames are length-prefixed (`u32` little-endian byte
//! count) JSON. Requests carry an `id` so many can be in flight on one
//! connection; the client correlates each `Response` back to its `id`. The
//! server may also push unsolicited `PushEvent`s to subscribed clients.

pub mod client;
pub mod server;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::mail::{Draft, Envelope, Folder, Message, MessageWindow};

/// Protocol version, exchanged in `Hello`. A client that sees a different server
/// version logs and falls back to a direct IMAP connection.
pub const PROTOCOL_VERSION: u32 = 1;

/// Upper bound on a single frame's JSON body, to reject a corrupt/hostile length
/// prefix before allocating. Message bodies are text (attachments are paths, not
/// inlined), so 64 MiB is comfortably generous.
const MAX_FRAME: usize = 64 << 20;

/// One client -> server message. Every client frame is a request; `Subscribe`'s
/// account is carried in `account` like any other op.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Request {
    pub id: u64,
    /// Account name (matches `accounts.name`); the server maps it to a session.
    pub account: String,
    pub op: RpcOp,
}

/// A mail operation. Mirrors the in-process `Cmd` (`imap_cmd.rs`) minus its reply
/// channel, plus the control ops `Hello`/`Subscribe`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub enum RpcOp {
    Hello { version: u32 },
    /// Subscribe this connection to new-mail pushes for `Request.account`.
    Subscribe,
    ListFolders,
    FolderCounts(Vec<String>),
    ListMessages(String),
    ListMessagesWindow { folder: String, offset: u32, limit: u32 },
    FetchMessage { folder: String, uid: u64 },
    SetSeen { folder: String, uid: u64, seen: bool },
    SetFlagged { folder: String, uid: u64, flagged: bool },
    Send(Draft),
    Delete { folder: String, uid: u64 },
    MoveTo { folder: String, uid: u64, dest: String },
}

/// One server -> client message: either a reply to a request or an unsolicited
/// arrival push.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub enum ServerMsg {
    Response(Response),
    Push(PushEvent),
}

/// A reply correlated to `Request.id`. `result` is the op's payload or a
/// stringified error (anyhow errors do not cross the socket structurally).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Response {
    pub id: u64,
    pub result: Result<Payload, String>,
}

/// The successful result of an [`RpcOp`], shaped per op.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub enum Payload {
    Unit,
    Hello { version: u32 },
    Folders(Vec<Folder>),
    Counts(Vec<(String, usize, usize)>),
    Envelopes(Vec<Envelope>),
    Window(MessageWindow),
    Message(Box<Message>),
}

/// An unsolicited update pushed to subscribed clients: either new arrivals to
/// merge, or a full newest-window snapshot of a folder to replace (from the
/// daemon's periodic re-sync - carries flag/removal changes, not just arrivals).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub enum PushEvent {
    NewMail { account: String, folder: String, envelopes: Vec<Envelope> },
    FolderSync { account: String, folder: String, envelopes: Vec<Envelope> },
}

impl PushEvent {
    /// The owning account name (used to route the push to the right subscriber).
    pub fn account(&self) -> &str {
        match self {
            PushEvent::NewMail { account, .. } | PushEvent::FolderSync { account, .. } => account,
        }
    }
}

/// Write one length-prefixed JSON frame and flush.
pub async fn write_frame<T, W>(w: &mut W, msg: &T) -> std::io::Result<()>
where
    T: Serialize,
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(body.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "frame too large"))?;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// Read one length-prefixed JSON frame. Returns `UnexpectedEof` when the peer
/// has closed the connection cleanly between frames.
pub async fn read_frame<T, R>(r: &mut R) -> std::io::Result<T>
where
    T: DeserializeOwned,
    R: AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds max {MAX_FRAME}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{Address, Flags};

    fn sample_envelope() -> Envelope {
        Envelope {
            uid: 42,
            flags: Flags { seen: true, flagged: false },
            from: Address::new(Some("Ada".to_string()), "ada@example.com"),
            subject: "hi".to_string(),
            date: 1_700_000_000,
            has_attachment: false,
            snippet: Some("preview".to_string()),
            message_id: Some("<abc@x>".to_string()),
        }
    }

    #[tokio::test]
    async fn request_frame_round_trips() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let req = Request {
            id: 7,
            account: "work".to_string(),
            op: RpcOp::ListMessages("INBOX".to_string()),
        };
        write_frame(&mut a, &req).await.unwrap();
        let got: Request = read_frame(&mut b).await.unwrap();
        assert_eq!(got.id, 7);
        assert_eq!(got.account, "work");
        assert!(matches!(got.op, RpcOp::ListMessages(f) if f == "INBOX"));
    }

    #[tokio::test]
    async fn server_msg_response_and_push_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(8192);
        let resp = ServerMsg::Response(Response {
            id: 3,
            result: Ok(Payload::Envelopes(vec![sample_envelope()])),
        });
        let push = ServerMsg::Push(PushEvent::NewMail {
            account: "work".to_string(),
            folder: "INBOX".to_string(),
            envelopes: vec![sample_envelope()],
        });
        write_frame(&mut a, &resp).await.unwrap();
        write_frame(&mut a, &push).await.unwrap();

        let r1: ServerMsg = read_frame(&mut b).await.unwrap();
        match r1 {
            ServerMsg::Response(r) => {
                assert_eq!(r.id, 3);
                match r.result {
                    Ok(Payload::Envelopes(es)) => assert_eq!(es[0].uid, 42),
                    other => panic!("unexpected payload {other:?}"),
                }
            }
            other => panic!("expected response, got {other:?}"),
        }
        let r2: ServerMsg = read_frame(&mut b).await.unwrap();
        assert!(matches!(
            r2,
            ServerMsg::Push(PushEvent::NewMail { envelopes, .. }) if envelopes[0].uid == 42
        ));
    }

    #[tokio::test]
    async fn eof_between_frames_is_reported() {
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        let got: std::io::Result<Request> = read_frame(&mut b).await;
        assert_eq!(got.unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
