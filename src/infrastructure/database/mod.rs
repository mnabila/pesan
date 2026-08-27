pub mod accounts;
pub mod cache;
pub mod secrets;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

/// The shared handle to the on-disk (or in-memory) SQLite database. `sqlx`
/// pools are cheap to clone (an `Arc` inside), so the app and each cache-backed
/// `MailSource` hold their own clone.
pub type Db = SqlitePool;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS accounts (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,
  email         TEXT NOT NULL,
  provider      TEXT NOT NULL,
  keychain_ref  TEXT NOT NULL,
  is_default    INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);
"#;

/// M6 offline cache: folders, messages (+ FTS5 full-text index), and per-folder
/// sync state, all keyed by `account_id`. The FTS index is external-content over
/// `messages`, kept in sync by triggers, so cache writes stay simple upserts.
const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS folders (
  account_id INTEGER NOT NULL,
  name       TEXT NOT NULL,
  total      INTEGER NOT NULL DEFAULT 0,
  unread     INTEGER NOT NULL DEFAULT 0,
  category   TEXT NOT NULL DEFAULT 'mailbox',
  position   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, name)
);

CREATE TABLE IF NOT EXISTS messages (
  account_id     INTEGER NOT NULL,
  folder         TEXT NOT NULL,
  uid            INTEGER NOT NULL,
  seen           INTEGER NOT NULL DEFAULT 0,
  flagged        INTEGER NOT NULL DEFAULT 0,
  from_name      TEXT,
  from_email     TEXT NOT NULL DEFAULT '',
  subject        TEXT NOT NULL DEFAULT '',
  date           INTEGER NOT NULL DEFAULT 0,
  has_attachment INTEGER NOT NULL DEFAULT 0,
  message_id     TEXT,
  snippet        TEXT,
  body           TEXT,
  PRIMARY KEY (account_id, folder, uid)
);
CREATE INDEX IF NOT EXISTS messages_by_date
  ON messages (account_id, folder, date DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
  subject, from_name, from_email, body,
  content='messages', content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_name, from_email, body)
  VALUES (new.rowid, new.subject, new.from_name, new.from_email, new.body);
END;
CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_email, body)
  VALUES ('delete', old.rowid, old.subject, old.from_name, old.from_email, old.body);
END;
CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_name, from_email, body)
  VALUES ('delete', old.rowid, old.subject, old.from_name, old.from_email, old.body);
  INSERT INTO messages_fts(rowid, subject, from_name, from_email, body)
  VALUES (new.rowid, new.subject, new.from_name, new.from_email, new.body);
END;

CREATE TABLE IF NOT EXISTS sync_state (
  account_id   INTEGER NOT NULL,
  folder       TEXT NOT NULL,
  uid_validity INTEGER,
  last_uid     INTEGER,
  updated_at   INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, folder)
);
"#;

/// Fallback secret store: refresh tokens keyed by an account's `keychain_ref`,
/// used only when the OS keychain is unavailable. Plaintext on disk (same as the
/// rest of the DB), so it is a downgrade from the encrypted OS keychain.
const SCHEMA_V3: &str = r#"
CREATE TABLE IF NOT EXISTS secrets (
  ref   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

/// Persist the raw RFC822 header block alongside the cached body, so the reader
/// can show full headers offline after a message has been opened once.
const SCHEMA_V4: &str = r#"
ALTER TABLE messages ADD COLUMN raw_headers TEXT;
"#;

/// Open (creating if needed) the per-user database at `path` and run migrations.
/// Passing `":memory:"` gives an in-memory DB (with schema) for tests.
///
/// A file-backed DB uses **WAL** mode and a multi-connection pool so the UI's
/// cache reads (opening a message, switching folders) run concurrently with the
/// background sync's writes instead of serializing on one connection - otherwise
/// a burst of background writes can stall an interactive read and freeze the UI.
/// An in-memory DB keeps a single connection, since each `:memory:` connection
/// is a separate database.
pub async fn open(path: &Path) -> Result<Db> {
    let is_memory = path == Path::new(":memory:");
    let opts = if is_memory {
        SqliteConnectOptions::from_str("sqlite::memory:").context("in-memory sqlite options")?
    } else {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).context("create data dir")?;
        }
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // WAL lets readers proceed while a writer is active; NORMAL sync is
            // safe under WAL and much faster. A busy timeout avoids spurious
            // "database is locked" errors when a write is briefly in progress.
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
    };
    let pool = SqlitePoolOptions::new()
        .max_connections(if is_memory { 1 } else { 8 })
        .connect_with(opts)
        .await
        .with_context(|| format!("open db {}", path.display()))?;
    migrate(&pool).await?;
    Ok(pool)
}

/// Run idempotent schema migrations. New migrations append versioned batches
/// here; keep `user_version` as the marker.
async fn migrate(pool: &Db) -> Result<()> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .context("read user_version")?;
    if version < 1 {
        sqlx::raw_sql(SCHEMA)
            .execute(pool)
            .await
            .context("migrate db to v1")?;
        sqlx::raw_sql("PRAGMA user_version = 1;")
            .execute(pool)
            .await
            .context("set user_version")?;
        tracing::info!("db migrated to version 1");
    }
    if version < 2 {
        sqlx::raw_sql(SCHEMA_V2)
            .execute(pool)
            .await
            .context("migrate db to v2")?;
        sqlx::raw_sql("PRAGMA user_version = 2;")
            .execute(pool)
            .await
            .context("set user_version")?;
        tracing::info!("db migrated to version 2 (offline cache)");
    }
    if version < 3 {
        sqlx::raw_sql(SCHEMA_V3)
            .execute(pool)
            .await
            .context("migrate db to v3")?;
        sqlx::raw_sql("PRAGMA user_version = 3;")
            .execute(pool)
            .await
            .context("set user_version")?;
        tracing::info!("db migrated to version 3 (secret fallback store)");
    }
    if version < 4 {
        sqlx::raw_sql(SCHEMA_V4)
            .execute(pool)
            .await
            .context("migrate db to v4")?;
        sqlx::raw_sql("PRAGMA user_version = 4;")
            .execute(pool)
            .await
            .context("set user_version")?;
        tracing::info!("db migrated to version 4 (cached raw headers)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_and_migrates_in_memory() {
        let pool = open(Path::new(":memory:")).await.unwrap();
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(version, 4);
    }

    #[tokio::test]
    async fn fts5_index_is_available() {
        // Confirms the bundled SQLite has FTS5 and the trigger wiring works.
        let pool = open(Path::new(":memory:")).await.unwrap();
        sqlx::query(
            "INSERT INTO messages (account_id, folder, uid, from_email, subject, body) \
             VALUES (1, 'INBOX', 1, 'a@b.io', 'Roadmap review', 'lets discuss the roadmap')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let hits: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'roadmap'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(hits, 1);
    }
}
