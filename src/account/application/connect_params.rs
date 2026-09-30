//! Resolving a saved account + its stored secret into live-connect parameters.
//! Produces the shared `platform::connect_params` boundary types.

use crate::account::Account;
use crate::account::application::ports::TokenStore;
use crate::platform::connect_params::{ConnectParams, ImapAuth, ProviderSpec};

/// Assemble IMAP/SMTP/OAuth connection parameters for an account from its
/// provider + stored secret, or `Ok(None)` when the account is not yet authorized
/// (no stored refresh token for OAuth, or no stored password for a login
/// provider).
///
/// The single source of truth for turning a saved account into live-connect
/// params: the TUI (`App::connect_params`) and the headless daemon both call it,
/// so they resolve credentials identically.
pub async fn resolve_connect_params(
    provider: &ProviderSpec,
    account: &Account,
    tokens: &dyn TokenStore,
) -> anyhow::Result<Option<ConnectParams>> {
    // A provider with an OAuth block authenticates via XOAUTH2; one without
    // uses a stored login password (IMAP/SMTP LOGIN).
    let auth = match &provider.oauth {
        Some(oauth) => {
            let Some(refresh_token) = tokens.load_refresh_token(&account.keychain_ref).await? else {
                return Ok(None);
            };
            ImapAuth::OAuth {
                // Only now that the account is known to hold a refresh token do
                // we insist the provider is actually usable, so a half-filled
                // `config.yaml` does not mask the "not authorized yet" state.
                oauth: oauth.clone().validate()?,
                refresh_token,
                // Filled in by the connect path (cached token or a fresh
                // refresh); the IDLE watcher leaves it `None`.
                access_token: None,
                access_expires_at: None,
            }
        }
        None => {
            let Some(password) = tokens.load_password(&account.keychain_ref).await? else {
                return Ok(None);
            };
            ImapAuth::Password { password }
        }
    };
    Ok(Some(ConnectParams {
        label: account.name.clone(),
        email: account.email.clone(),
        host: provider.imap_host.clone(),
        port: provider.imap_port,
        smtp_host: provider.smtp_host.clone(),
        smtp_port: provider.smtp_port,
        keychain_ref: account.keychain_ref.clone(),
        auth,
    }))
}
