use anyhow::{Context, Result};

use crate::platform::db::Db;

/// Store (or replace) a secret value under `key`.
pub async fn set(pool: &Db, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO secrets (ref, value) VALUES (?1, ?2)\n         ON CONFLICT(ref) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .context("write secret to db")?;
    Ok(())
}

/// Load a secret value by `key`, if present.
pub async fn get(pool: &Db, key: &str) -> Result<Option<String>> {
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM secrets WHERE ref = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .context("read secret from db")?;
    Ok(value)
}

/// Remove a secret by `key`. A missing entry is treated as success.
pub async fn delete(pool: &Db, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM secrets WHERE ref = ?1")
        .bind(key)
        .execute(pool)
        .await
        .context("delete secret from db")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::db;
    use std::path::Path;

    #[tokio::test]
    async fn set_get_delete_roundtrip() {
        let pool = db::open(Path::new(":memory:"))
            .await
            .unwrap();
        let key = "pesan/work/refresh";

        assert!(get(&pool, key).await.unwrap().is_none());

        set(&pool, key, "tok-123").await.unwrap();
        assert_eq!(get(&pool, key).await.unwrap().as_deref(), Some("tok-123"));

        // Upsert replaces the value in place.
        set(&pool, key, "tok-456").await.unwrap();
        assert_eq!(get(&pool, key).await.unwrap().as_deref(), Some("tok-456"));

        delete(&pool, key).await.unwrap();
        assert!(get(&pool, key).await.unwrap().is_none());
        // Deleting a missing key is fine.
        delete(&pool, key).await.unwrap();
    }
}
