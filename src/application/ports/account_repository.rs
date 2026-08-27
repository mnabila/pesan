use anyhow::Result;
use async_trait::async_trait;

use crate::application::account::Account;

#[allow(dead_code)]
#[async_trait]
pub trait AccountRepo: Send + Sync {
    async fn list(&self) -> Result<Vec<Account>>;
    async fn get(&self, id: i64) -> Result<Option<Account>>;
    async fn get_by_name(&self, name: &str) -> Result<Option<Account>>;
    /// Insert or update; returns the row id. First account becomes the default.
    async fn upsert(&self, account: &Account) -> Result<i64>;
    async fn delete(&self, id: i64) -> Result<()>;
    async fn set_default(&self, id: i64) -> Result<()>;
}
