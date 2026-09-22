use anyhow::Result;

use crate::application::Services;
use crate::application::account::Account;

/// Persist an OAuth outcome: store the refresh token in the keychain (with DB
/// fallback), then upsert the account row (and flip the default when asked).
/// Returns the saved account UUID.
pub async fn authorize_persist(
    svc: &Services,
    account: &Account,
    refresh_token: &str,
    set_default: bool,
) -> Result<String> {
    svc.tokens
        .store_refresh_token(&account.keychain_ref, refresh_token)
        .await?;
    let id = svc.accounts.upsert(account).await?;
    if set_default {
        let _ = svc.accounts.set_default(&id).await;
    }
    Ok(id)
}

/// Persist a password-login account: stash the password in the keyring, then
/// upsert the row (and flip the default when asked). Returns the saved UUID.
pub async fn save_password(
    svc: &Services,
    account: &Account,
    password: &str,
    set_default: bool,
) -> Result<String> {
    svc.tokens
        .store_password(&account.keychain_ref, password)
        .await?;
    let id = svc.accounts.upsert(account).await?;
    if set_default {
        let _ = svc.accounts.set_default(&id).await;
    }
    Ok(id)
}

/// Delete an account row and its stored secrets (refresh token + cached access
/// token, keychain first with the DB fallback cleared too).
pub async fn delete_account(svc: &Services, account_id: &str, keychain_ref: &str) -> Result<()> {
    svc.tokens.delete_refresh_token(keychain_ref).await?;
    svc.accounts.delete(account_id).await
}
