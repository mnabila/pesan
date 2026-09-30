use anyhow::bail;

/// Provider OAuth settings with any `${ENV}` placeholders resolved.
///
/// Plain data owned by the core. Reading a provider's `oauth` block out of
/// `config.yaml` and expanding its placeholders is the config layer's job - see
/// [`crate::platform::config::OAuth::resolved`], the single place that builds
/// this from configuration. Whether the result is *usable* is a rule of the core,
/// so it is [`ResolvedOAuth::validate`] rather than part of the parsing.
#[derive(Debug, Clone)]
pub struct ResolvedOAuth {
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub client_id: String,
    pub client_secret: String,
}

impl ResolvedOAuth {
    /// Fails when the provider has no client id, i.e. the user has not filled in
    /// real app credentials in `config.yaml`. Every path that actually
    /// authenticates must pass through here.
    pub fn validate(self) -> anyhow::Result<Self> {
        if self.client_id.trim().is_empty() {
            bail!("provider client_id is empty - set it in config.yaml (providers.<name>.oauth)");
        }
        Ok(self)
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
