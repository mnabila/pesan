use anyhow::Result;
use keyring::{Entry, Error as KeyringError};

use crate::account::application::ports::TokenStore;
use crate::platform::db::Db;
use crate::platform::secrets;

/// Keychain service name; the per-account `keychain_ref` is the "user" field.
const SERVICE: &str = "pesan";

/// Store (or replace) the login password for a password-auth account. Same
/// keyring-first, DB-fallback path as the refresh token; no access-token
/// sibling is involved.
pub async fn store_password(pool: &Db, keychain_ref: &str, password: &str) -> Result<()> {
    match keyring_set(keychain_ref, password).await {
        Ok(()) => {
            let _ = secrets::delete(pool, keychain_ref).await;
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); storing account password in the app database \
                 (pesan.db, plaintext). Install a Secret Service (e.g. gnome-keyring) for \
                 encrypted storage."
            );
            secrets::set(pool, keychain_ref, password).await
        }
    }
}

/// Load the login password for a password-auth account, if stored. Keychain
/// first, then the DB fallback.
pub async fn load_password(pool: &Db, keychain_ref: &str) -> Result<Option<String>> {
    match keyring_get(keychain_ref).await {
        Ok(Some(secret)) => Ok(Some(secret)),
        Ok(None) => secrets::get(pool, keychain_ref).await,
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); reading account password from the app database"
            );
            secrets::get(pool, keychain_ref).await
        }
    }
}

/// Store (or replace) the refresh token for an account. Tries the OS keychain
/// first and falls back to the DB secret store if the keychain is unavailable.
pub async fn store_refresh_token(pool: &Db, keychain_ref: &str, refresh_token: &str) -> Result<()> {
    match keyring_set(keychain_ref, refresh_token).await {
        Ok(()) => {
            // Keychain is the source of truth now; drop any stale DB entry.
            let _ = secrets::delete(pool, keychain_ref).await;
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); storing refresh token in the app database \
                 (pesan.db, plaintext). Install a Secret Service (e.g. gnome-keyring) for \
                 encrypted storage."
            );
            secrets::set(pool, keychain_ref, refresh_token).await
        }
    }
}

/// Load the refresh token for an account, if one has been stored. Checks the OS
/// keychain first, then the DB secret store.
pub async fn load_refresh_token(pool: &Db, keychain_ref: &str) -> Result<Option<String>> {
    match keyring_get(keychain_ref).await {
        Ok(Some(secret)) => Ok(Some(secret)),
        // Not in the keychain - it may have been written to the DB fallback.
        Ok(None) => secrets::get(pool, keychain_ref).await,
        Err(e) => {
            tracing::warn!(
                "OS keychain unavailable ({e}); reading refresh token from the app database"
            );
            secrets::get(pool, keychain_ref).await
        }
    }
}

/// Remove an account's refresh token from both the keychain and the DB store.
/// A missing entry is treated as success. Also clears any cached access token.
pub async fn delete_refresh_token(pool: &Db, keychain_ref: &str) -> Result<()> {
    // Best-effort keychain delete; also clear the DB fallback.
    let _ = keyring_delete(keychain_ref).await;
    let access = access_ref(keychain_ref);
    let _ = keyring_delete(&access).await;
    let _ = secrets::delete(pool, &access).await;
    secrets::delete(pool, keychain_ref).await
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
    match keyring_set(&key, &value).await {
        Ok(()) => {
            let _ = secrets::delete(pool, &key).await;
            Ok(())
        }
        Err(_) => secrets::set(pool, &key, &value).await,
    }
}

/// Load a cached access token and its expiry (unix seconds), if present and
/// parseable. Checks the keychain first, then the DB fallback.
pub async fn load_access_token(pool: &Db, keychain_ref: &str) -> Result<Option<(String, u64)>> {
    let key = access_ref(keychain_ref);
    let raw = match keyring_get(&key).await {
        Ok(Some(v)) => Some(v),
        Ok(None) => secrets::get(pool, &key).await?,
        Err(_) => secrets::get(pool, &key).await?,
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

// The `keyring` crate talks to the OS Secret Service over a *blocking* DBus
// call. Run each on `spawn_blocking` so a burst of concurrent token loads (e.g.
// the client connecting every account at once) never starves the async runtime
// - blocking a worker thread here previously froze the UI (stalled render ticks)
// until the keyring calls returned.
async fn keyring_set(keychain_ref: &str, secret: &str) -> Result<()> {
    let (key, val) = (keychain_ref.to_string(), secret.to_string());
    join_keyring(tokio::task::spawn_blocking(move || {
        Entry::new(SERVICE, &key)?.set_password(&val)
    }))
    .await
}

async fn keyring_get(keychain_ref: &str) -> Result<Option<String>> {
    let key = keychain_ref.to_string();
    join_keyring(tokio::task::spawn_blocking(move || {
        match Entry::new(SERVICE, &key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(e),
        }
    }))
    .await
}

async fn keyring_delete(keychain_ref: &str) -> Result<()> {
    let key = keychain_ref.to_string();
    join_keyring(tokio::task::spawn_blocking(move || {
        match Entry::new(SERVICE, &key)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(e),
        }
    }))
    .await
}

/// Flatten a `spawn_blocking` join of a keyring op into an `anyhow::Result`,
/// turning both a keyring error and a task-join failure into `Err` (callers then
/// fall back to the DB secret store).
async fn join_keyring<T>(
    handle: tokio::task::JoinHandle<Result<T, KeyringError>>,
) -> Result<T> {
    match handle.await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(anyhow::Error::new(e)),
        Err(join) => Err(anyhow::anyhow!("keyring task failed: {join}")),
    }
}

//
// Port adapter
//

/// [`TokenStore`] backed by the OS keychain with the DB `secrets` fallback; a
/// one-line delegation to the free functions above.
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
    use crate::account::{keychain_ref_for, keychain_ref_for_password};
    use crate::platform::db;
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
        secrets::set(&pool, &keychain_ref, "hunter2")
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
        secrets::set(&pool, &access_ref(&keychain_ref), "1893456000\natok-xyz")
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

        secrets::set(&pool, &keychain_ref, "tok-123")
            .await
            .unwrap();
        assert_eq!(
            secrets::get(&pool, &keychain_ref)
                .await
                .unwrap()
                .as_deref(),
            Some("tok-123"),
        );

        secrets::delete(&pool, &keychain_ref).await.unwrap();
        assert!(
            secrets::get(&pool, &keychain_ref)
                .await
                .unwrap()
                .is_none()
        );
    }
}
