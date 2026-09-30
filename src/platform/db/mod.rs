use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::AssertSqlSafe;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

/// The shared handle to the on-disk (or in-memory) SQLite database. `sqlx`
/// pools are cheap to clone (an `Arc` inside), so the app and each cache-backed
/// `MailSource` hold their own clone.
pub type Db = SqlitePool;

/// One schema migration: the target `user_version` it advances the DB to, the
/// SQL that does it (embedded from `migrations/*.sql` at compile time), and a
/// short label for the log line.
struct Migration {
    version: i64,
    sql: &'static str,
    label: &'static str,
}

/// The ordered migration set, embedded into the binary from `migrations/`.
/// Append a new `.sql` file and a matching row here; keep `version` contiguous
/// and ascending. `user_version` is the on-disk marker of the last applied step.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../../../migrations/0001_accounts.sql"),
        label: "1",
    },
    Migration {
        version: 2,
        sql: include_str!("../../../migrations/0002_offline_cache.sql"),
        label: "2 (offline cache)",
    },
    Migration {
        version: 3,
        sql: include_str!("../../../migrations/0003_secret_fallback.sql"),
        label: "3 (secret fallback store)",
    },
    Migration {
        version: 4,
        sql: include_str!("../../../migrations/0004_cached_raw_headers.sql"),
        label: "4 (cached raw headers)",
    },
    Migration {
        version: 5,
        sql: include_str!("../../../migrations/0005_cached_raw_html.sql"),
        label: "5 (cached raw HTML)",
    },
    Migration {
        version: 6,
        sql: include_str!("../../../migrations/0006_maildir_body_store.sql"),
        label: "6 (maildir body store)",
    },
    Migration {
        version: 7,
        sql: include_str!("../../../migrations/0007_index_sender_subject.sql"),
        label: "7 (index sender+subject only)",
    },
    Migration {
        version: 8,
        sql: include_str!("../../../migrations/0008_uuid_account_ids.sql"),
        label: "8 (uuid account ids)",
    },
];

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

/// Run idempotent schema migrations. Applies every embedded step whose `version`
/// is above the DB's current `user_version`, then advances the marker. Add a new
/// step by dropping a `.sql` file in `migrations/` and a row in `MIGRATIONS`.
async fn migrate(pool: &Db) -> Result<()> {
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .context("read user_version")?;
    for m in MIGRATIONS {
        if version >= m.version {
            continue;
        }
        sqlx::raw_sql(m.sql)
            .execute(pool)
            .await
            .with_context(|| format!("migrate db to v{}", m.version))?;
        // `m.version` is a hardcoded i64 from `MIGRATIONS`, not user input, and
        // `PRAGMA user_version` takes no bind parameters.
        sqlx::raw_sql(AssertSqlSafe(format!("PRAGMA user_version = {};", m.version)))
            .execute(pool)
            .await
            .context("set user_version")?;
        tracing::info!("db migrated to version {}", m.label);
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
        assert_eq!(version, 8);
    }

    #[tokio::test]
    async fn fts5_index_is_available() {
        // Confirms the bundled SQLite has FTS5 and the trigger wiring works.
        let pool = open(Path::new(":memory:")).await.unwrap();
        sqlx::query(
            "INSERT INTO messages (account_id, folder, uid, from_email, subject, body) \
             VALUES ('a1', 'INBOX', 1, 'a@b.io', 'Roadmap review', 'lets discuss the roadmap')",
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
