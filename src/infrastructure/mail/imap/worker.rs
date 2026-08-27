use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use anyhow::{Context, Result, anyhow};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::application::mail::imap_cmd::Cmd;
use crate::infrastructure::auth::oauth::{TokenSet, refresh_access_token, xoauth2_payload};

use crate::domain::Draft;

use super::client::{CONNECT_TIMEOUT, ConnectParams, ImapAuth, ImapSession, OP_TIMEOUT};
use super::proto::{
    delete_uid, fetch_message, folder_counts, list_folders, list_messages, list_messages_window,
    move_uid, select_folder, store_flag,
};

pub(super) fn worker_main(
    params: ConnectParams,
    mut hi_rx: UnboundedReceiver<Cmd>,
    mut lo_rx: UnboundedReceiver<Cmd>,
    ready: SyncSender<Result<()>>,
    on_lost: Option<UnboundedSender<String>>,
) {
    // 1. Obtain the access token BEFORE any Tokio runtime exists on this thread.
    //    A caller-supplied (cached, still-valid) token skips the refresh; that is
    //    the fast path. Otherwise refresh via the blocking reqwest client. The
    //    token + expiry are kept so a later send can refresh a lapsed token.
    //    Password accounts have no token: LOGIN uses the stored password directly.
    let (mut access_token, mut token_expires_at) = match &params.auth {
        ImapAuth::Password { .. } => (String::new(), i64::MAX),
        ImapAuth::OAuth {
            access_token: Some(token),
            access_expires_at,
            ..
        } => (token.clone(), access_expires_at.unwrap_or(i64::MAX)),
        ImapAuth::OAuth {
            oauth,
            refresh_token,
            access_token: None,
            ..
        } => match refresh_access_token(oauth, refresh_token) {
            Ok(tokens) => {
                let expiry = token_expiry(&tokens);
                (tokens.access_token, expiry)
            }
            Err(e) => {
                let _ = ready.send(Err(e.context("refresh access token for IMAP")));
                return;
            }
        },
    };

    // 2. Build a current-thread runtime and connect.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready.send(Err(anyhow!("build IMAP runtime: {e}")));
            return;
        }
    };

    runtime.block_on(async move {
        let mut session = match connect_and_auth(&params, &access_token).await {
            Ok(s) => {
                let _ = ready.send(Ok(()));
                s
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };

        // Serve commands until both channels close. `biased` polls the
        // high-priority queue first, so an interactive open/switch jumps ahead of
        // any queued background sweep (it waits at most for the one command
        // already in flight, never the whole sweep).
        let mut wedged = false;
        loop {
            let cmd = tokio::select! {
                biased;
                Some(cmd) = hi_rx.recv() => cmd,
                Some(cmd) = lo_rx.recv() => cmd,
                else => break,
            };
            // A timed-out command means the session is wedged (a cancelled
            // async-imap op leaves the stream desynced), so stop serving.
            if !handle_cmd(
                &mut session,
                &params,
                &mut access_token,
                &mut token_expires_at,
                cmd,
            )
            .await
            {
                tracing::warn!("IMAP command timed out; shutting down wedged session");
                wedged = true;
                break;
            }
        }

        // The session may be wedged, so bound the logout too - never hang here.
        let _ = tokio::time::timeout(OP_TIMEOUT, session.logout()).await;

        // Notify the app so it can auto-reconnect. Only on a wedge - a clean
        // shutdown (the source was dropped) closes the channels and exits via the
        // `else` arm above, which must not trigger a reconnect.
        if wedged && let Some(tx) = on_lost {
            let _ = tx.send(params.label.clone());
        }
    });
}

/// Execute one worker command against the live session, each op bounded by
/// [`OP_TIMEOUT`]. Returns `false` if an op timed out - the session is then
/// considered wedged and the caller stops the worker loop.
async fn handle_cmd(
    session: &mut ImapSession,
    params: &ConnectParams,
    access_token: &mut String,
    token_expires_at: &mut i64,
    cmd: Cmd,
) -> bool {
    /// Await `$fut` under [`OP_TIMEOUT`]; on timeout send a timeout error on
    /// `$reply` and return `false` from `handle_cmd`.
    macro_rules! bounded {
        ($reply:expr, $fut:expr) => {
            match tokio::time::timeout(OP_TIMEOUT, $fut).await {
                Ok(result) => {
                    let _ = $reply.send(result);
                }
                Err(_) => {
                    let _ = $reply.send(Err(anyhow!(
                        "IMAP operation timed out after {}s",
                        OP_TIMEOUT.as_secs()
                    )));
                    return false;
                }
            }
        };
    }

    match cmd {
        Cmd::ListFolders(reply) => bounded!(reply, list_folders(session)),
        Cmd::FolderCounts(names, reply) => {
            // Counts have no `Result` reply; on timeout return empty and stop.
            match tokio::time::timeout(OP_TIMEOUT, folder_counts(session, &names)).await {
                Ok(counts) => {
                    let _ = reply.send(counts);
                }
                Err(_) => {
                    let _ = reply.send(Vec::new());
                    return false;
                }
            }
        }
        Cmd::ListMessages(folder, reply) => bounded!(reply, list_messages(session, &folder)),
        Cmd::ListMessagesWindow(folder, offset, limit, reply) => {
            bounded!(reply, list_messages_window(session, &folder, offset, limit))
        }
        Cmd::FetchMessage(folder, uid, reply) => bounded!(reply, async {
            select_folder(session, &folder).await?;
            fetch_message(session, uid).await
        }),
        Cmd::SetSeen(folder, uid, on, reply) => bounded!(reply, async {
            select_folder(session, &folder).await?;
            store_flag(session, uid, "\\Seen", on).await
        }),
        Cmd::SetFlagged(folder, uid, on, reply) => bounded!(reply, async {
            select_folder(session, &folder).await?;
            store_flag(session, uid, "\\Flagged", on).await
        }),
        Cmd::Send(draft, reply) => {
            // SMTP opens a fresh connection per send and authenticates with
            // XOAUTH2, so a lapsed access token would fail auth. Refresh first
            // if it is at/near expiry.
            if let Err(e) = refresh_token_if_stale(params, access_token, token_expires_at) {
                let _ = reply.send(Err(e));
                return true;
            }
            bounded!(
                reply,
                send_and_append(session, params, access_token, &draft)
            );
        }
        Cmd::Delete(folder, uid, reply) => bounded!(reply, async {
            select_folder(session, &folder).await?;
            delete_uid(session, uid).await
        }),
        Cmd::MoveTo(src, uid, dest, reply) => bounded!(reply, async {
            select_folder(session, &src).await?;
            move_uid(session, uid, &dest).await
        }),
    }
    true
}

/// Refresh below this many seconds of remaining validity, so a send doesn't
/// authenticate with an about-to-expire token.
const SEND_REFRESH_MARGIN_SECS: i64 = 60;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Absolute expiry (unix seconds) for a freshly minted token set, defaulting to
/// one hour when the provider omits `expires_in`.
fn token_expiry(tokens: &TokenSet) -> i64 {
    let ttl = tokens
        .expires_in
        .map(|d| d.as_secs() as i64)
        .unwrap_or(3600);
    unix_now() + ttl
}

/// If the access token is at/near expiry, refresh it in place. The refresh uses
/// a blocking HTTP client, which cannot run inside the worker's Tokio runtime, so
/// it runs on a scratch thread (no runtime) that we join. A no-op while the token
/// is still comfortably valid.
fn refresh_token_if_stale(
    params: &ConnectParams,
    access_token: &mut String,
    expires_at: &mut i64,
) -> Result<()> {
    // Password accounts have no token to refresh.
    let ImapAuth::OAuth {
        oauth,
        refresh_token,
        ..
    } = &params.auth
    else {
        return Ok(());
    };
    if unix_now() < *expires_at - SEND_REFRESH_MARGIN_SECS {
        return Ok(());
    }
    let joined = std::thread::scope(|s| {
        s.spawn(|| refresh_access_token(oauth, refresh_token))
            .join()
    });
    let tokens = joined
        .map_err(|_| anyhow!("token refresh thread panicked"))?
        .context("refresh access token for send")?;
    *expires_at = token_expiry(&tokens);
    *access_token = tokens.access_token;
    Ok(())
}

pub(crate) async fn connect_and_auth(
    params: &ConnectParams,
    access_token: &str,
) -> Result<ImapSession> {
    let connector = tls_connector()?;
    let server_name = ServerName::try_from(params.host.clone())
        .with_context(|| format!("invalid IMAP host name: {}", params.host))?;

    let connect = async {
        let tcp = TcpStream::connect((params.host.as_str(), params.port))
            .await
            .context("TCP connect to IMAP server")?;
        let tls = connector
            .connect(server_name, tcp)
            .await
            .context("TLS handshake with IMAP server")?;
        let mut client = async_imap::Client::new(tls);
        // Consume the server greeting before authenticating.
        client
            .read_response()
            .await
            .context("read IMAP greeting")?
            .ok_or_else(|| anyhow!("server closed before greeting"))?;
        Ok::<_, anyhow::Error>(client)
    };

    let client = tokio::time::timeout(CONNECT_TIMEOUT, connect)
        .await
        .map_err(|_| anyhow!("timed out connecting to IMAP server"))??;

    match &params.auth {
        ImapAuth::OAuth { .. } => {
            let authenticator = XOAuth2 {
                payload: xoauth2_payload(&params.email, access_token),
                sent: false,
            };
            client
                .authenticate("XOAUTH2", authenticator)
                .await
                .map_err(|(e, _client)| anyhow!("XOAUTH2 authentication failed: {e}"))
        }
        ImapAuth::Password { password } => client
            .login(&params.email, password)
            .await
            .map_err(|(e, _client)| anyhow!("IMAP LOGIN failed: {e}")),
    }
}

/// Pluggable SASL authenticator for XOAUTH2. The first server continuation gets
/// the credential payload; any subsequent continuation (an error challenge) is
/// answered with an empty string so the server can finish reporting the error.
struct XOAuth2 {
    payload: String,
    sent: bool,
}

impl async_imap::Authenticator for XOAuth2 {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> Self::Response {
        if self.sent {
            String::new()
        } else {
            self.sent = true;
            self.payload.clone()
        }
    }
}

fn tls_connector() -> Result<TlsConnector> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(TlsConnector::from(Arc::new(config)))
}

/// Send an outgoing message over SMTP, then save a copy to the Sent mailbox via
/// IMAP APPEND. Gmail auto-saves SMTP-sent mail, so the APPEND is skipped there
/// to avoid a duplicate; a failed APPEND is logged but does not fail the send.
async fn send_and_append(
    session: &mut ImapSession,
    params: &ConnectParams,
    access_token: &str,
    draft: &Draft,
) -> Result<()> {
    let message = crate::infrastructure::mail::smtp::build_message(&params.email, draft)?;
    let raw = message.formatted();
    // XOAUTH2 uses the (possibly just-refreshed) access token; a password account
    // authenticates SMTP with its stored password via AUTH LOGIN.
    let (secret, mechanism) = match &params.auth {
        ImapAuth::OAuth { .. } => (
            access_token,
            crate::infrastructure::mail::smtp::SmtpAuth::Xoauth2,
        ),
        ImapAuth::Password { password } => (
            password.as_str(),
            crate::infrastructure::mail::smtp::SmtpAuth::Login,
        ),
    };
    let smtp_params = crate::infrastructure::mail::smtp::SmtpParams {
        host: &params.smtp_host,
        port: params.smtp_port,
        email: &params.email,
        secret,
        auth: mechanism,
    };
    tracing::info!(
        account = %params.label,
        smtp = %format!("{}:{}", params.smtp_host, params.smtp_port),
        "sending outgoing message via SMTP"
    );
    crate::infrastructure::mail::smtp::send(&smtp_params, message).await?;
    tracing::info!(account = %params.label, "outgoing message sent");

    if !params.smtp_host.to_lowercase().ends_with("gmail.com")
        && let Err(e) = session.append("Sent", Some("(\\Seen)"), None, &raw).await
    {
        tracing::warn!("APPEND to Sent failed (message was still sent): {e}");
    }
    Ok(())
}
