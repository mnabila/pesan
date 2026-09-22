use anyhow::Result;
use async_trait::async_trait;

use crate::application::account::Account;

#[allow(dead_code)]
#[async_trait]
pub trait AccountRepo: Send + Sync {
    async fn list(&self) -> Result<Vec<Account>>;
    async fn get(&self, id: &str) -> Result<Option<Account>>;
    async fn get_by_name(&self, name: &str) -> Result<Option<Account>>;
    /// Insert or update; returns the account UUID. First account becomes the default.
    async fn upsert(&self, account: &Account) -> Result<String>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn set_default(&self, id: &str) -> Result<()>;
}
