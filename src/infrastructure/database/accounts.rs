use anyhow::Result;

pub use crate::application::account::Account;
use crate::application::ports::AccountRepo;
use crate::infrastructure::database::Db;

const COLS: &str = "id, name, email, provider, keychain_ref, is_default, created_at";

pub async fn list(db: &Db) -> Result<Vec<Account>> {
    let sql = format!("SELECT {COLS} FROM accounts ORDER BY is_default DESC, name");
    let accounts = sqlx::query_as::<_, Account>(&sql).fetch_all(db).await?;
    Ok(accounts)
}

#[allow(dead_code)] // test-only
pub async fn get(db: &Db, id: i64) -> Result<Option<Account>> {
    let sql = format!("SELECT {COLS} FROM accounts WHERE id = ?");
    let account = sqlx::query_as::<_, Account>(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(account)
}

pub async fn get_by_name(db: &Db, name: &str) -> Result<Option<Account>> {
    let sql = format!("SELECT {COLS} FROM accounts WHERE name = ?");
    let account = sqlx::query_as::<_, Account>(&sql)
        .bind(name)
        .fetch_optional(db)
        .await?;
    Ok(account)
}

/// Insert a new account, or update an existing one (by id if set, else by name).
/// Returns the row id of the saved account. First account becomes the default.
pub async fn upsert(db: &Db, account: &Account) -> Result<i64> {
    if let Some(id) = account.id {
        sqlx::query(
            "UPDATE accounts SET name = ?, email = ?, provider = ?, \
             keychain_ref = ?, is_default = ? WHERE id = ?",
        )
        .bind(&account.name)
        .bind(&account.email)
        .bind(&account.provider)
        .bind(&account.keychain_ref)
        .bind(account.is_default)
        .bind(id)
        .execute(db)
        .await?;
        return Ok(id);
    }

    if let Some(existing) = get_by_name(db, &account.name).await? {
        let id = existing.id.unwrap_or_default();
        sqlx::query(
            "UPDATE accounts SET email = ?, provider = ?, keychain_ref = ?, \
             is_default = ? WHERE id = ?",
        )
        .bind(&account.email)
        .bind(&account.provider)
        .bind(&account.keychain_ref)
        .bind(account.is_default)
        .bind(id)
        .execute(db)
        .await?;
        return Ok(id);
    }

    let now = account.created_at.max(0);
    let res = sqlx::query(
        "INSERT INTO accounts (name, email, provider, keychain_ref, is_default, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&account.name)
    .bind(&account.email)
    .bind(&account.provider)
    .bind(&account.keychain_ref)
    .bind(account.is_default)
    .bind(now)
    .execute(db)
    .await?;
    let id = res.last_insert_rowid();

    // First account becomes the default so there is always one active.
    if list(db).await?.len() == 1 {
        set_default(db, id).await?;
    }
    Ok(id)
}

pub async fn delete(db: &Db, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM accounts WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    Ok(())
}

/// Promote one account to default, clearing the flag on all others.
pub async fn set_default(db: &Db, id: i64) -> Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("UPDATE accounts SET is_default = 0")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE accounts SET is_default = 1 WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

// Port adapter ----------------------------------------------------------

/// [`AccountRepo`] backed by the shared SQLite pool; a one-line delegation to
/// the free functions above.
#[allow(dead_code)] // port consumed from Phase 2 onward
pub struct SqliteAccountRepo {
    db: Db,
}

impl SqliteAccountRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl AccountRepo for SqliteAccountRepo {
    async fn list(&self) -> Result<Vec<Account>> {
        list(&self.db).await
    }

    async fn get(&self, id: i64) -> Result<Option<Account>> {
        get(&self.db, id).await
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<Account>> {
        get_by_name(&self.db, name).await
    }

    async fn upsert(&self, account: &Account) -> Result<i64> {
        upsert(&self.db, account).await
    }

    async fn delete(&self, id: i64) -> Result<()> {
        delete(&self.db, id).await
    }

    async fn set_default(&self, id: i64) -> Result<()> {
        set_default(&self.db, id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::database as db;

    async fn connect() -> Db {
        db::open(std::path::Path::new(":memory:")).await.unwrap()
    }

    fn account(name: &str) -> Account {
        Account {
            id: None,
            name: name.to_string(),
            email: format!("{name}@example.com"),
            provider: "gmail".to_string(),
            keychain_ref: "pesan/refresh/1".to_string(),
            is_default: false,
            created_at: 1,
        }
    }

    #[tokio::test]
    async fn first_account_becomes_default() {
        let db = connect().await;
        let id = upsert(&db, &account("personal")).await.unwrap();
        let got = get(&db, id).await.unwrap().unwrap();
        assert!(got.is_default);
        assert_eq!(list(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn upsert_updates_by_name() {
        let db = connect().await;
        let id = upsert(&db, &account("personal")).await.unwrap();
        let mut next = account("personal");
        next.email = "changed@example.com".to_string();
        let id2 = upsert(&db, &next).await.unwrap();
        assert_eq!(id, id2);
        let got = get(&db, id).await.unwrap().unwrap();
        assert_eq!(got.email, "changed@example.com");
        assert_eq!(list(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn set_default_flips_exactly_one() {
        let db = connect().await;
        let a = upsert(&db, &account("one")).await.unwrap();
        let b = upsert(&db, &account("two")).await.unwrap();
        set_default(&db, b).await.unwrap();
        let (o, t, w) = (
            get(&db, a).await.unwrap().unwrap(),
            get(&db, b).await.unwrap().unwrap(),
            list(&db).await.unwrap(),
        );
        assert!(!o.is_default);
        assert!(t.is_default);
        assert_eq!(w.iter().filter(|c| c.is_default).count(), 1);
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let db = connect().await;
        let id = upsert(&db, &account("personal")).await.unwrap();
        delete(&db, id).await.unwrap();
        assert!(get(&db, id).await.unwrap().is_none());
        assert!(list(&db).await.unwrap().is_empty());
    }
}
