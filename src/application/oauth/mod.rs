use anyhow::bail;

use crate::bootstrap::config::OAuth;

/// Provider OAuth settings with any `${ENV}` placeholders resolved. Built from
/// [`crate::bootstrap::config::OAuth`] at connect time.
#[derive(Debug, Clone)]
pub struct ResolvedOAuth {
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub client_id: String,
    pub client_secret: String,
}

impl ResolvedOAuth {
    /// Resolve a provider's OAuth block, expanding `${VAR}` in the client
    /// id/secret from the environment. Fails if the client id is empty (the
    /// user has not filled in real app credentials in `config.yaml`).
    pub fn from_config(oauth: &OAuth) -> anyhow::Result<Self> {
        let client_id = expand_env(&oauth.client_id);
        let client_secret = expand_env(&oauth.client_secret);
        if client_id.trim().is_empty() {
            bail!("provider client_id is empty - set it in config.yaml (providers.<name>.oauth)");
        }
        Ok(Self {
            auth_url: oauth.auth_url.clone(),
            token_url: oauth.token_url.clone(),
            scopes: oauth.scopes.clone(),
            client_id,
            client_secret,
        })
    }
}

/// A started authorization-code + PKCE flow, held between opening the browser
/// and the user pasting the redirect URL back (the app runs no HTTP listener).
/// Plain data: `infrastructure::auth::oauth` fills it in `begin_auth_code_flow`
/// and consumes it in `exchange_pasted_redirect`; the UI only carries it across
/// the two steps. Lives in `application` so the UI can hold it without reaching
/// into `infrastructure`.
#[derive(Debug, Clone)]
pub struct AuthCodeFlow {
    /// The consent URL, shown in the prompt so the user can open it manually if
    /// the browser did not pop up.
    pub authorize_url: String,
    /// The redirect URI advertised to the provider; echoed in the token request.
    pub redirect_uri: String,
    /// PKCE verifier secret, needed to complete the code exchange.
    pub pkce_verifier: String,
    /// CSRF state to match against the pasted redirect's `state` parameter.
    pub csrf_state: String,
}

/// The tokens returned by an authorization or refresh exchange.
#[derive(Debug, Clone)]
pub struct TokenSet {
    pub access_token: String,
    /// Present on the initial authorization; often absent on refresh (the old
    /// refresh token stays valid), so callers should keep the previous one.
    pub refresh_token: Option<String>,
    pub expires_in: Option<std::time::Duration>,
    /// The account's email address, decoded from the OpenID `id_token` returned
    /// by the initial authorization (best effort; `None` on refresh or when the
    /// provider returns no id_token). Used to auto-fill the account form.
    pub email: Option<String>,
    /// The account owner's display name, from the id_token's `name` claim (best
    /// effort). Used as the default account label instead of the raw email.
    pub name: Option<String>,
}

/// Expand `${VAR}` occurrences in `input` from the environment. An unset or
/// non-`${...}` string is returned as-is (trimmed of surrounding whitespace).
/// Only a whole-value `${VAR}` is expanded, matching the config convention
/// `client_secret: ${PESAN_GMAIL_SECRET}`.
fn expand_env(input: &str) -> String {
    let trimmed = input.trim();
    if let Some(var) = trimmed
        .strip_prefix("${")
        .and_then(|rest| rest.strip_suffix('}'))
    {
        return std::env::var(var).unwrap_or_default();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::expand_env;

    #[test]
    fn expand_env_reads_whole_value_placeholder() {
        // SAFETY: single-threaded test setting a scoped var.
        unsafe { std::env::set_var("PESAN_TEST_SECRET", "s3cr3t") };
        assert_eq!(expand_env("${PESAN_TEST_SECRET}"), "s3cr3t");
        assert_eq!(expand_env("  ${PESAN_TEST_SECRET}  "), "s3cr3t");
        assert_eq!(expand_env("literal-value"), "literal-value");
        assert_eq!(expand_env("${PESAN_UNSET_XYZ}"), "");
    }
}
