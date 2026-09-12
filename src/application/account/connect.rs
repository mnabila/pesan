use anyhow::Result;
use tokio::sync::mpsc::UnboundedSender;

use crate::application::Services;
use crate::application::account::connect_params::{ConnectParams, ImapAuth};
use crate::application::mail::fetch::MailSource;
use crate::domain::{Envelope, Folder};

/// Refresh below this many seconds of remaining validity, so a token doesn't
/// expire mid-connect.
const ACCESS_TOKEN_MARGIN_SECS: i64 = 60;

pub(crate) fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Why a live-connect failed. `Auth` means the stored credential itself was
/// rejected (expired/revoked refresh token, or the server refused the login);
/// the shell can recover by re-running the authorization flow in the
/// background. Anything else (DNS, TLS, timeout, server error) is `Other` and
/// only worth a warning toast - re-consenting would not help.
#[derive(Debug, Clone)]
pub enum ConnectError {
    Auth(String),
    Other(String),
}

impl ConnectError {
    pub fn is_auth(&self) -> bool {
        matches!(self, Self::Auth(_))
    }
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(m) | Self::Other(m) => f.write_str(m),
        }
    }
}

/// True when an error chain indicates the *credential* was rejected rather
/// than the network/server being unavailable. Matches the markers raised by
/// the IMAP auth handshake (`XOAUTH2 authentication failed`, `IMAP LOGIN
/// failed`) and by the OAuth refresh grant (`token endpoint rejected (...)`,
/// carrying `invalid_grant` and friends from the provider).
fn is_auth_rejection(err: &dyn std::fmt::Display) -> bool {
    let m = err.to_string().to_ascii_lowercase();
    m.contains("authentication failed")
        || m.contains("login failed")
        || m.contains("token endpoint rejected")
        || m.contains("invalid_grant")
        || m.contains("invalid_client")
        || m.contains("unauthorized_client")
}

fn classify(context: &str, err: anyhow::Error) -> ConnectError {
    let msg = format!("{context}: {err:#}");
    if is_auth_rejection(&msg) || err.chain().any(|c| is_auth_rejection(&c)) {
        ConnectError::Auth(msg)
    } else {
        ConnectError::Other(msg)
    }
}

/// The live source and the mail already fetched for it on the background task,
/// so the shell can swap it in with no further network. Delivered to
/// `App::on_connected` via `Event::Connected`.
pub struct LiveData {
    pub source: Box<dyn MailSource>,
    pub folders: Vec<Folder>,
    /// Name of the folder whose messages were loaded (usually INBOX).
    pub folder: String,
    pub envelopes: Vec<Envelope>,
}

/// Return a usable access token for the account: the cached one when still
/// valid (the fast path, no network), otherwise a fresh refresh which is cached
/// for next time. Returns `(token, expires_at_unix, from_cache)`; `None` for
/// password accounts (no OAuth token dance).
async fn ensure_access_token(
    svc: &Services,
    params: &ConnectParams,
    force_refresh: bool,
) -> Result<Option<(String, i64, bool)>> {
    let ImapAuth::OAuth {
        oauth,
        refresh_token,
        ..
    } = &params.auth
    else {
        return Ok(None);
    };
    // Daemon-backed (TUI client): the daemon owns the live IMAP session and
    // refreshes its own access token, and `IpcBackend::connect` ignores the
    // token in `params` entirely - so the client must not perform the OAuth
    // refresh itself. Hand back nothing; the connect is served over IPC.
    if svc.daemon_backed {
        return Ok(None);
    }
    if !force_refresh
        && let Some((token, exp)) = svc.tokens.load_access_token(&params.keychain_ref).await?
        && (exp as i64) > now_ts() + ACCESS_TOKEN_MARGIN_SECS
    {
        return Ok(Some((token, exp as i64, true)));
    }
    let tokens = svc
        .backend
        .refresh_access_token(oauth.clone(), refresh_token.clone())
        .await?;
    let ttl = tokens
        .expires_in
        .map(|d| d.as_secs() as i64)
        .unwrap_or(3600);
    let expires_at = (now_ts() + ttl).max(0);
    let _ = svc
        .tokens
        .store_access_token(
            &params.keychain_ref,
            &tokens.access_token,
            expires_at as u64,
        )
        .await;
    Ok(Some((tokens.access_token, expires_at, false)))
}

/// Build an offline source for instant display before/without a live
/// connection. See [`MailBackend::offline_source`].
pub(crate) async fn offline_source(svc: &Services, account_id: Option<i64>) -> Box<dyn MailSource> {
    svc.backend.offline_source(account_id).await
}

/// Run the full live-connect entirely off the UI thread: the blocking TLS/OAuth
/// handshake (inside the backend), then the initial folder list and the target
/// folder's messages, caching each as it goes.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn connect_account(
    svc: &Services,
    mut params: ConnectParams,
    account_id: Option<i64>,
    want_folder: String,
    sync_all_folders: bool,
    on_lost: Option<UnboundedSender<String>>,
) -> std::result::Result<LiveData, ConnectError> {
    // Fast path: reuse a cached, still-valid access token so the connect skips
    // the (blocking, network) OAuth refresh entirely. Falls back to a refresh
    // (off any Tokio runtime) and caches the fresh token for next time.
    let from_cache = match ensure_access_token(svc, &params, false)
        .await
        .map_err(|e| classify("obtain access token", e))?
    {
        Some((token, expires_at, from_cache)) => {
            params.set_access_token(token, expires_at);
            from_cache
        }
        // Password account: no token dance; connect straight away.
        None => false,
    };

    let source: Box<dyn MailSource> =
        match svc.backend.connect(params.clone(), on_lost.clone()).await {
            Ok(src) => src,
            // A cached token can be stale/revoked before it "expires" locally. Force
            // a refresh once and retry so the user isn't stuck offline until it lapses.
            Err(e) if from_cache => {
                tracing::warn!("connect with cached access token failed ({e:#}); refreshing");
                if let Some((token, expires_at, _)) = ensure_access_token(svc, &params, true)
                    .await
                    .map_err(|e| classify("refresh access token", e))?
                {
                    params.set_access_token(token, expires_at);
                }
                svc.backend
                    .connect(params, on_lost)
                    .await
                    .map_err(|e| classify("connect", e))?
            }
            Err(e) => return Err(classify("connect", e)),
        };

    let folders = source
        .list_folders()
        .await
        .map_err(|e| classify("list folders", e))?;
    if let Some(id) = account_id {
        let _ = svc.cache.upsert_folders(id, &folders).await;
    }

    // Load the folder the user is viewing if it still exists, else the first.
    let folder = folders
        .iter()
        .find(|f| f.name == want_folder)
        .or_else(|| folders.first())
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "INBOX".to_string());

    let envelopes = source.list_messages(&folder).await.unwrap_or_default();
    if let Some(id) = account_id {
        let _ = svc.cache.upsert_envelopes(id, &folder, &envelopes).await;
    }

    // Background account warm-up: sync every *other* folder's envelopes too, so
    // switching accounts (and folders) shows fresh cached mail instantly. Only
    // done when the caller asks (the discarded background connect), since the
    // active account fills its remaining folders after connect via a retained
    // handle (`spawn_folder_sync_all`).
    if sync_all_folders && let Some(id) = account_id {
        for f in &folders {
            if f.name == folder {
                continue;
            }
            if let Ok(envs) = source.list_messages(&f.name).await {
                let _ = svc.cache.upsert_envelopes(id, &f.name, &envs).await;
            }
        }
    }

    // Message bodies are fetched only when the user opens a message, so nothing
    // is pre-fetched here - the connect stays a folders + envelopes sync.
    Ok(LiveData {
        source,
        folders,
        folder,
        envelopes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imap_auth_failures_classify_as_auth() {
        let e = classify(
            "connect",
            anyhow::anyhow!("XOAUTH2 authentication failed: AUTHENTICATIONFAILED"),
        );
        assert!(e.is_auth(), "xoauth2 rejection: {e}");
        let e = classify(
            "connect",
            anyhow::anyhow!("IMAP LOGIN failed: bad credentials"),
        );
        assert!(e.is_auth(), "login rejection: {e}");
    }

    #[test]
    fn token_endpoint_rejections_classify_as_auth() {
        // What `refresh_access_token` raises when the grant is refused.
        let body = r#"token endpoint rejected (400 Bad Request): {"error":"invalid_grant"}"#;
        let e = classify("refresh access token", anyhow::anyhow!("{body}"));
        assert!(e.is_auth(), "rejected refresh grant: {e}");
    }

    #[test]
    fn transport_and_server_errors_are_not_auth() {
        for msg in [
            "timed out connecting to IMAP server",
            "TCP connect to IMAP server: connection refused",
            "TLS handshake with IMAP server: certificate expired",
            "list folders: the session wedged",
        ] {
            let e = classify("connect", anyhow::anyhow!("{msg}"));
            assert!(!e.is_auth(), "{msg} must not be auth");
        }
    }
}
