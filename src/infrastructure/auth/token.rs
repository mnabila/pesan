use anyhow::Result;
use keyring::{Entry, Error as KeyringError};

use crate::application::ports::TokenStore;
use crate::infrastructure::database as db;
use crate::infrastructure::database::Db;

/// Keychain service name; the per-account `keychain_ref` is the "user" field.
const SERVICE: &str = "pesan";

/// Store (or replace) the login password for a password-auth account. Same
/// keyring-first, DB-fallback path as the refresh token; no access-token
/// sibling is involved.
pub async fn store_password(pool: &Db, keychain_ref: &str, password: &str) -> Result<()> {
    match keyring_set(keychain_ref, password) {
        Ok(()) => {
            let _ = db::secrets::delete(pool, keychain_ref).await;
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); storing account password in the app database \
                 (pesan.db, plaintext). Install a Secret Service (e.g. gnome-keyring) for \
                 encrypted storage."
            );
            db::secrets::set(pool, keychain_ref, password).await
        }
    }
}

/// Load the login password for a password-auth account, if stored. Keychain
/// first, then the DB fallback.
pub async fn load_password(pool: &Db, keychain_ref: &str) -> Result<Option<String>> {
    match keyring_get(keychain_ref) {
        Ok(Some(secret)) => Ok(Some(secret)),
        Ok(None) => db::secrets::get(pool, keychain_ref).await,
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); reading account password from the app database"
            );
            db::secrets::get(pool, keychain_ref).await
        }
    }
}

/// Store (or replace) the refresh token for an account. Tries the OS keychain
/// first and falls back to the DB secret store if the keychain is unavailable.
pub async fn store_refresh_token(pool: &Db, keychain_ref: &str, refresh_token: &str) -> Result<()> {
    match keyring_set(keychain_ref, refresh_token) {
        Ok(()) => {
            // Keychain is the source of truth now; drop any stale DB entry.
            let _ = db::secrets::delete(pool, keychain_ref).await;
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); storing refresh token in the app database \
                 (pesan.db, plaintext). Install a Secret Service (e.g. gnome-keyring) for \
                 encrypted storage."
            );
            db::secrets::set(pool, keychain_ref, refresh_token).await
        }
    }
}

/// Load the refresh token for an account, if one has been stored. Checks the OS
/// keychain first, then the DB secret store.
pub async fn load_refresh_token(pool: &Db, keychain_ref: &str) -> Result<Option<String>> {
    match keyring_get(keychain_ref) {
        Ok(Some(secret)) => Ok(Some(secret)),
        // Not in the keychain - it may have been written to the DB fallback.
        Ok(None) => db::secrets::get(pool, keychain_ref).await,
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); reading refresh token from the app database"
            );
            db::secrets::get(pool, keychain_ref).await
        }
    }
}

/// Remove an account's refresh token from both the keychain and the DB store.
/// A missing entry is treated as success. Also clears any cached access token.
pub async fn delete_refresh_token(pool: &Db, keychain_ref: &str) -> Result<()> {
    // Best-effort keychain delete; also clear the DB fallback.
    let _ = keyring_delete(keychain_ref);
    let access = access_ref(keychain_ref);
    let _ = keyring_delete(&access);
    let _ = db::secrets::delete(pool, &access).await;
    db::secrets::delete(pool, keychain_ref).await
}

/// The sibling reference under which an account's cached access token lives
/// (e.g. `pesan/work/refresh` -> `pesan/work/access`).
fn access_ref(keychain_ref: &str) -> String {
    match keychain_ref.strip_suffix("/refresh") {
        Some(base) => format!("{base}/access"),
        None => format!("{keychain_ref}/access"),
    }
}

/// Cache an access token and its absolute expiry (unix seconds), keyed off the
/// account's `keychain_ref`. Stored as `"<expiry>\n<token>"`. Best effort:
/// keychain first, DB fallback otherwise.
pub async fn store_access_token(
    pool: &Db,
    keychain_ref: &str,
    access_token: &str,
    expires_at: u64,
) -> Result<()> {
    let key = access_ref(keychain_ref);
    let value = format!("{expires_at}\n{access_token}");
    match keyring_set(&key, &value) {
        Ok(()) => {
            let _ = db::secrets::delete(pool, &key).await;
            Ok(())
        }
        Err(_) => db::secrets::set(pool, &key, &value).await,
    }
}

/// Load a cached access token and its expiry (unix seconds), if present and
/// parseable. Checks the keychain first, then the DB fallback.
pub async fn load_access_token(pool: &Db, keychain_ref: &str) -> Result<Option<(String, u64)>> {
    let key = access_ref(keychain_ref);
    let raw = match keyring_get(&key) {
        Ok(Some(v)) => Some(v),
        Ok(None) => db::secrets::get(pool, &key).await?,
        Err(_) => db::secrets::get(pool, &key).await?,
    };
    let Some(raw) = raw else { return Ok(None) };
    let Some((exp, token)) = raw.split_once('\n') else {
        return Ok(None);
    };
    let Ok(exp) = exp.parse::<u64>() else {
        return Ok(None);
    };
    Ok(Some((token.to_string(), exp)))
}

fn keyring_set(keychain_ref: &str, refresh_token: &str) -> Result<(), KeyringError> {
    Entry::new(SERVICE, keychain_ref)?.set_password(refresh_token)
}

fn keyring_get(keychain_ref: &str) -> Result<Option<String>, KeyringError> {
    match Entry::new(SERVICE, keychain_ref)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

fn keyring_delete(keychain_ref: &str) -> Result<(), KeyringError> {
    match Entry::new(SERVICE, keychain_ref)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => Err(e),
    }
}

// Port adapter ----------------------------------------------------------

/// [`TokenStore`] backed by the OS keychain with the DB `secrets` fallback; a
/// one-line delegation to the free functions above.
#[allow(dead_code)] // port consumed from Phase 2 onward
pub struct KeyringTokenStore {
    pool: Db,
}

impl KeyringTokenStore {
    pub fn new(pool: Db) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl TokenStore for KeyringTokenStore {
    async fn store_password(&self, keychain_ref: &str, password: &str) -> Result<()> {
        store_password(&self.pool, keychain_ref, password).await
    }

    async fn load_password(&self, keychain_ref: &str) -> Result<Option<String>> {
        load_password(&self.pool, keychain_ref).await
    }

    async fn store_refresh_token(&self, keychain_ref: &str, refresh_token: &str) -> Result<()> {
        store_refresh_token(&self.pool, keychain_ref, refresh_token).await
    }

    async fn load_refresh_token(&self, keychain_ref: &str) -> Result<Option<String>> {
        load_refresh_token(&self.pool, keychain_ref).await
    }

    async fn delete_refresh_token(&self, keychain_ref: &str) -> Result<()> {
        delete_refresh_token(&self.pool, keychain_ref).await
    }

    async fn store_access_token(
        &self,
        keychain_ref: &str,
        token: &str,
        expires_at_unix: u64,
    ) -> Result<()> {
        store_access_token(&self.pool, keychain_ref, token, expires_at_unix).await
    }

    async fn load_access_token(&self, keychain_ref: &str) -> Result<Option<(String, u64)>> {
        load_access_token(&self.pool, keychain_ref).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::account::{keychain_ref_for, keychain_ref_for_password};
    use std::path::Path;

    #[test]
    fn keychain_ref_follows_convention() {
        assert_eq!(keychain_ref_for("personal"), "pesan/personal/refresh");
        assert_eq!(keychain_ref_for("work"), "pesan/work/refresh");
    }

    #[test]
    fn password_ref_follows_convention() {
        assert_eq!(keychain_ref_for_password("work"), "pesan/work/password");
    }

    /// Password storage round-trips through the DB fallback (the keychain path
    /// needs a live OS keyring, absent on CI).
    #[tokio::test]
    async fn password_db_fallback_roundtrips() {
        let pool = db::open(Path::new(":memory:")).await.unwrap();
        let keychain_ref = keychain_ref_for_password("work");
        // Write via the DB fallback directly, then read through load_password.
        db::secrets::set(&pool, &keychain_ref, "hunter2")
            .await
            .unwrap();
        let got = load_password(&pool, &keychain_ref).await.unwrap();
        assert_eq!(got.as_deref(), Some("hunter2"));
        // delete_refresh_token also clears a password ref (best-effort).
        delete_refresh_token(&pool, &keychain_ref).await.unwrap();
        assert!(load_password(&pool, &keychain_ref).await.unwrap().is_none());
    }

    #[test]
    fn access_ref_is_refresh_sibling() {
        assert_eq!(access_ref("pesan/work/refresh"), "pesan/work/access");
        // A ref without the /refresh suffix just gets /access appended.
        assert_eq!(access_ref("custom-ref"), "custom-ref/access");
    }

    /// Access-token caching round-trips through the DB fallback (keychain path
    /// needs a live OS keyring, absent on CI).
    #[tokio::test]
    async fn access_token_db_fallback_roundtrips() {
        let pool = db::open(Path::new(":memory:")).await.unwrap();
        let keychain_ref = keychain_ref_for("work");
        // Write directly to the DB fallback under the derived access ref, then read.
        db::secrets::set(&pool, &access_ref(&keychain_ref), "1893456000\natok-xyz")
            .await
            .unwrap();
        let got = load_access_token(&pool, &keychain_ref).await.unwrap();
        assert_eq!(got, Some(("atok-xyz".to_string(), 1893456000)));
        // Deleting the refresh token also clears the cached access token.
        delete_refresh_token(&pool, &keychain_ref).await.unwrap();
        assert!(
            load_access_token(&pool, &keychain_ref)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The DB fallback path is exercised directly here (the keychain path depends
    /// on a live OS keyring, which CI/headless machines lack).
    #[tokio::test]
    async fn db_fallback_roundtrips() {
        let pool = db::open(Path::new(":memory:")).await.unwrap();
        let keychain_ref = keychain_ref_for("work");

        db::secrets::set(&pool, &keychain_ref, "tok-123")
            .await
            .unwrap();
        assert_eq!(
            db::secrets::get(&pool, &keychain_ref)
                .await
                .unwrap()
                .as_deref(),
            Some("tok-123"),
        );

        db::secrets::delete(&pool, &keychain_ref).await.unwrap();
        assert!(
            db::secrets::get(&pool, &keychain_ref)
                .await
                .unwrap()
                .is_none()
        );
    }
}
