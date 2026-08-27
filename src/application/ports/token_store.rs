use anyhow::Result;
use async_trait::async_trait;

#[allow(dead_code)]
#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn store_password(&self, keychain_ref: &str, password: &str) -> Result<()>;
    async fn load_password(&self, keychain_ref: &str) -> Result<Option<String>>;
    async fn store_refresh_token(&self, keychain_ref: &str, refresh_token: &str) -> Result<()>;
    async fn load_refresh_token(&self, keychain_ref: &str) -> Result<Option<String>>;
    async fn delete_refresh_token(&self, keychain_ref: &str) -> Result<()>;
    async fn store_access_token(
        &self,
        keychain_ref: &str,
        token: &str,
        expires_at_unix: u64,
    ) -> Result<()>;
    async fn load_access_token(&self, keychain_ref: &str) -> Result<Option<(String, u64)>>;
}
