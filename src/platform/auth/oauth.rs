use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
#[cfg(test)]
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oauth2::basic::BasicClient;
use oauth2::reqwest;
use oauth2::{
    AuthUrl, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope, TokenUrl,
};
use serde::Deserialize;
use url::Url;

use crate::platform::browser;

pub use crate::platform::oauth::{AuthCodeFlow, ResolvedOAuth, TokenSet};

/// The redirect URI advertised to the provider. The app binds **no** listener;
/// after consent the provider redirects the browser here (nothing serves it) and
/// the user pastes that URL back into the TUI. A fixed loopback value works with
/// Google/Microsoft "Desktop app" (installed/public) registrations, which accept
/// loopback redirects. The same string is echoed in the token exchange, so it
/// must be identical in both requests - hence a single built-in constant rather
/// than deriving it from the (possibly differently-normalized) pasted URL.
const REDIRECT_URI: &str = "http://localhost";

/// Raw JSON body of a token endpoint response. Parsed manually (rather than via
/// the `oauth2` typed exchange) so the OpenID `id_token` is captured alongside
/// the tokens.
#[derive(Debug, Deserialize)]
struct RawTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    id_token: Option<String>,
}

/// Decode the JSON payload (claims) of an OpenID `id_token` (a JWT). The
/// signature is not verified - the token came straight from the provider over
/// TLS, and the claims are only used to pre-fill form fields.
fn id_token_claims(id_token: &str) -> Option<serde_json::Value> {
    let payload_b64 = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload_b64.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The first non-empty string claim among `keys`, if any.
fn claim_str(claims: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        claims
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

fn http_client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::ClientBuilder::new()
        // Never follow redirects on the token endpoint (SSRF hardening).
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("build OAuth HTTP client")
}

/// Begin the authorization-code + PKCE flow: build the consent URL and open it
/// in a browser. Does **no** network and binds **no** listener; the caller then
/// prompts the user to paste the redirect URL and passes it to
/// [`exchange_pasted_redirect`]. Browser is opened via configured command or
/// system default. Must run off any async runtime (see the blocking-work rule).
pub fn begin_auth_code_flow(oauth: &ResolvedOAuth, browser_cmd: Option<&str>) -> Result<AuthCodeFlow> {
    let redirect = REDIRECT_URI;
    let client = BasicClient::new(ClientId::new(oauth.client_id.clone()))
        .set_client_secret(ClientSecret::new(oauth.client_secret.clone()))
        .set_auth_uri(AuthUrl::new(oauth.auth_url.clone()).context("invalid auth_url")?)
        .set_token_uri(TokenUrl::new(oauth.token_url.clone()).context("invalid token_url")?)
        .set_redirect_uri(RedirectUrl::new(redirect.to_string()).context("invalid redirect_uri")?);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    // Request `openid`+`email` on top of the provider's mail scopes so the token
    // response carries an id_token we can decode the account's address from.
    // `profile` is required for the id_token to include the `name` claim (the
    // real display name) - without it, only email is returned and the account
    // name falls back to the email address.
    let mut scopes: Vec<String> = oauth.scopes.clone();
    for extra in ["openid", "email", "profile"] {
        if !scopes.iter().any(|s| s == extra) {
            scopes.push(extra.to_string());
        }
    }

    let mut auth_req = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge)
        // Google only returns a refresh token with offline access, and only
        // re-issues one when the user is re-prompted for consent.
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent");
    for scope in &scopes {
        auth_req = auth_req.add_scope(Scope::new(scope.clone()));
    }
    let (authorize_url, csrf) = auth_req.url();
    let authorize_url = authorize_url.to_string();

    // Best effort: pop a browser. If that fails, the URL is still logged so the
    // user can open it manually (the TUI also shows it).
    tracing::info!("opening OAuth consent URL: {authorize_url}");
    if let Err(e) = browser::open_in_browser(Path::new(&authorize_url), browser_cmd) {
        tracing::warn!("could not open browser automatically: {e}");
    }

    Ok(AuthCodeFlow {
        authorize_url,
        redirect_uri: redirect.to_string(),
        pkce_verifier: pkce_verifier.secret().clone(),
        csrf_state: csrf.secret().clone(),
    })
}

/// Complete a [`begin_auth_code_flow`]: parse `code`/`state` from the redirect
/// URL the user pasted back, verify the CSRF state, and exchange the code for
/// tokens (blocking network - run off any async runtime). The pasted value may
/// be the full redirect URL or just its `?query` part.
pub fn exchange_pasted_redirect(
    oauth: &ResolvedOAuth,
    flow: &AuthCodeFlow,
    pasted: &str,
) -> Result<TokenSet> {
    let (code, state) = parse_redirect(pasted, &flow.redirect_uri)?;
    if state != flow.csrf_state {
        bail!("OAuth state mismatch - possible CSRF, aborting");
    }

    // Exchange the code manually (rather than the typed `oauth2` exchange) so we
    // can read the OpenID `id_token` from the raw response and decode the email.
    let http = http_client()?;
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &flow.redirect_uri),
        ("client_id", &oauth.client_id),
        ("code_verifier", &flow.pkce_verifier),
    ];
    if !oauth.client_secret.is_empty() {
        form.push(("client_secret", &oauth.client_secret));
    }
    let resp = http
        .post(&oauth.token_url)
        .form(&form)
        .send()
        .context("token exchange request")?;
    let status = resp.status();
    let body = resp.text().context("read token exchange response")?;
    if !status.is_success() {
        bail!("token exchange failed: {status} {body}");
    }
    let raw: RawTokenResponse =
        serde_json::from_str(&body).context("parse token exchange response")?;

    let claims = raw.id_token.as_deref().and_then(id_token_claims);
    let (email, name) = match &claims {
        Some(c) => (
            claim_str(c, &["email", "preferred_username"]),
            claim_str(c, &["name", "given_name"]),
        ),
        None => (None, None),
    };
    Ok(TokenSet {
        email,
        name,
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        expires_in: raw.expires_in.map(Duration::from_secs),
    })
}

/// Exchange a stored refresh token for a fresh access token. Used to verify a
/// stored credential still works and before network calls that need a live
/// token. Implemented as a manual form post (like [`exchange_pasted_redirect`])
/// so a *rejected* grant surfaces a recognizable
/// `token endpoint rejected (...)` error the connect use-case can classify as
/// an authorization failure worth re-consenting, distinct from transport
/// errors.
pub fn refresh_access_token(oauth: &ResolvedOAuth, refresh_token: &str) -> Result<TokenSet> {
    let http = http_client()?;
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", &oauth.client_id),
    ];
    if !oauth.client_secret.is_empty() {
        form.push(("client_secret", &oauth.client_secret));
    }
    let resp = http
        .post(&oauth.token_url)
        .form(&form)
        .send()
        .context("token refresh request")?;
    let status = resp.status();
    let body = resp.text().context("read token refresh response")?;
    if !status.is_success() {
        bail!("token endpoint rejected ({status}): {body}");
    }
    let raw: RawTokenResponse =
        serde_json::from_str(&body).context("parse token refresh response")?;

    Ok(TokenSet {
        access_token: raw.access_token,
        refresh_token: raw.refresh_token,
        expires_in: raw.expires_in.map(Duration::from_secs),
        // Refresh grants don't return an id_token; email/name are already stored.
        email: None,
        name: None,
    })
}

/// Extract `(code, state)` from the redirect URL the user pasted back. Accepts
/// the full redirect URL (e.g. `http://localhost/?code=...&state=...`), or just
/// the `?query`/bare `code=...&state=...` fragment, so a user copying only the
/// address bar's query still works. Surfaces a provider `error=` as an error.
fn parse_redirect(pasted: &str, redirect_uri: &str) -> Result<(String, String)> {
    let pasted = pasted.trim();
    if pasted.is_empty() {
        bail!("paste the URL your browser was redirected to (it contains code=...)");
    }
    // Parse as an absolute URL when possible; otherwise treat the paste as a
    // query string and hang it off the configured redirect URI (falling back to
    // a dummy base) so relative pastes (`?code=...` or `code=...`) still parse.
    let url = Url::parse(pasted).or_else(|_| {
        let base = redirect_uri.trim_end_matches('/');
        let base = if base.is_empty() {
            "http://localhost"
        } else {
            base
        };
        let query = pasted.trim_start_matches('?');
        Url::parse(&format!("{base}/?{query}"))
    });
    let url = url.context("could not parse the pasted redirect URL")?;

    if let Some((_, err)) = url.query_pairs().find(|(k, _)| k == "error") {
        bail!("authorization denied: {err}");
    }
    let code = url
        .query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .context("the pasted URL has no authorization code (code=...)")?;
    let state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .context("the pasted URL has no state parameter")?;
    Ok((code, state))
}

/// The raw (pre-base64) XOAUTH2 SASL payload, identical for IMAP and SMTP.
/// Format: `user=<email>^Aauth=Bearer <token>^A^A` where `^A` is 0x01.
/// `async-imap`'s SASL handshake base64-encodes the authenticator's response
/// itself, so it consumes this raw form; base64-encode it where the on-the-wire
/// blob is needed directly.
pub fn xoauth2_payload(user: &str, access_token: &str) -> String {
    format!("user={user}\x01auth=Bearer {access_token}\x01\x01")
}

/// Base64-encoded XOAUTH2 initial-client-response (the on-the-wire form).
#[cfg(test)]
pub fn xoauth2(user: &str, access_token: &str) -> String {
    BASE64.encode(xoauth2_payload(user, access_token).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_email_and_name_from_id_token() {
        // A JWT is <header>.<payload>.<sig>; only the payload is read. Header/sig
        // are dummies here since the signature is never verified.
        let payload = URL_SAFE_NO_PAD
            .encode(br#"{"email":"user@example.com","name":"Ada Lovelace","sub":"1"}"#);
        let jwt = format!("aaa.{payload}.bbb");
        let claims = id_token_claims(&jwt).expect("claims");
        assert_eq!(
            claim_str(&claims, &["email", "preferred_username"]).as_deref(),
            Some("user@example.com")
        );
        assert_eq!(
            claim_str(&claims, &["name", "given_name"]).as_deref(),
            Some("Ada Lovelace")
        );
    }

    #[test]
    fn falls_back_to_preferred_username_and_given_name() {
        let payload = URL_SAFE_NO_PAD
            .encode(br#"{"preferred_username":"ms@example.com","given_name":"Grace"}"#);
        let jwt = format!("h.{payload}.s");
        let claims = id_token_claims(&jwt).expect("claims");
        assert_eq!(
            claim_str(&claims, &["email", "preferred_username"]).as_deref(),
            Some("ms@example.com")
        );
        assert_eq!(
            claim_str(&claims, &["name", "given_name"]).as_deref(),
            Some("Grace")
        );
    }

    #[test]
    fn bad_id_token_yields_none() {
        assert!(id_token_claims("not-a-jwt").is_none());
        assert!(id_token_claims("").is_none());
    }

    #[test]
    fn xoauth2_matches_google_spec() {
        // Example from Google's XOAUTH2 documentation shape.
        let s = xoauth2("test@example.com", "vF9dft4qmT");
        let decoded = BASE64.decode(s).unwrap();
        assert_eq!(
            decoded,
            b"user=test@example.com\x01auth=Bearer vF9dft4qmT\x01\x01",
        );
    }

    #[test]
    fn parse_redirect_accepts_full_url() {
        let (code, state) = parse_redirect(
            "http://localhost/?code=abc123&state=xyz789&scope=mail",
            "http://localhost",
        )
        .expect("full redirect URL parses");
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");
    }

    #[test]
    fn parse_redirect_accepts_bare_query() {
        // A user copying only the query part (or address bar leftovers).
        let (code, state) =
            parse_redirect("?code=aaa&state=bbb", "http://localhost").expect("bare query parses");
        assert_eq!(code, "aaa");
        assert_eq!(state, "bbb");
        let (code, state) =
            parse_redirect("code=ccc&state=ddd", "http://localhost").expect("no-? query parses");
        assert_eq!(code, "ccc");
        assert_eq!(state, "ddd");
    }

    #[test]
    fn parse_redirect_reports_provider_error_and_missing_code() {
        let err = parse_redirect("http://localhost/?error=access_denied", "http://localhost")
            .expect_err("provider error must surface");
        assert!(err.to_string().contains("access_denied"), "{err}");

        assert!(
            parse_redirect("http://localhost/?state=only", "http://localhost").is_err(),
            "missing code must error"
        );
        assert!(
            parse_redirect("   ", "http://localhost").is_err(),
            "empty paste must error"
        );
    }
}
