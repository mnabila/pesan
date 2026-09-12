use anyhow::Result;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::application::ports::MailCache;
use crate::domain::{Address, Envelope, Flags, Folder, FolderCategory, Message};
use crate::infrastructure::database::Db;

const ENVELOPE_COLS: &str =
    "uid, seen, flagged, from_name, from_email, subject, date, has_attachment, message_id, snippet";

fn row_to_envelope(r: &SqliteRow) -> Result<Envelope> {
    Ok(Envelope {
        uid: r.try_get::<i64, _>(0)? as u64,
        flags: Flags {
            seen: r.try_get::<i64, _>(1)? != 0,
            flagged: r.try_get::<i64, _>(2)? != 0,
        },
        from: Address {
            name: r.try_get(3)?,
            email: r.try_get(4)?,
        },
        // A row cached with no subject shows a placeholder rather than blank.
        subject: {
            let s: String = r.try_get(5)?;
            if s.is_empty() {
                "(no subject)".to_string()
            } else {
                s
            }
        },
        date: r.try_get(6)?,
        has_attachment: r.try_get::<i64, _>(7)? != 0,
        message_id: r.try_get(8)?,
        snippet: r.try_get(9)?,
    })
}

/// Replace the cached folder list for an account (folders change wholesale).
pub async fn upsert_folders(db: &Db, account_id: i64, folders: &[Folder]) -> Result<()> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM folders WHERE account_id = ?")
        .bind(account_id)
        .execute(&mut *tx)
        .await?;
    for (i, f) in folders.iter().enumerate() {
        let category = match f.category {
            FolderCategory::Mailbox => "mailbox",
            FolderCategory::Label => "label",
        };
        sqlx::query(
            "INSERT INTO folders (account_id, name, total, unread, category, position) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(account_id)
        .bind(&f.name)
        .bind(f.total as i64)
        .bind(f.unread as i64)
        .bind(category)
        .bind(i as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn load_folders(db: &Db, account_id: i64) -> Result<Vec<Folder>> {
    let rows = sqlx::query(
        "SELECT name, total, unread, category FROM folders \
         WHERE account_id = ? ORDER BY position",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let category: String = r.try_get(3)?;
        out.push(Folder {
            name: r.try_get(0)?,
            total: r.try_get::<i64, _>(1)? as usize,
            unread: r.try_get::<i64, _>(2)? as usize,
            category: if category == "label" {
                FolderCategory::Label
            } else {
                FolderCategory::Mailbox
            },
        });
    }
    Ok(out)
}

/// Upsert envelopes for a folder, preserving any already-cached `body`.
pub async fn upsert_envelopes(
    db: &Db,
    account_id: i64,
    folder: &str,
    envelopes: &[Envelope],
) -> Result<()> {
    let mut tx = db.begin().await?;
    for e in envelopes {
        sqlx::query(
            "INSERT INTO messages \
               (account_id, folder, uid, seen, flagged, from_name, from_email, subject, date, has_attachment, message_id, snippet) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(account_id, folder, uid) DO UPDATE SET \
               seen=excluded.seen, flagged=excluded.flagged, from_name=excluded.from_name, \
               from_email=excluded.from_email, subject=excluded.subject, date=excluded.date, \
               has_attachment=excluded.has_attachment, message_id=excluded.message_id, \
               snippet=excluded.snippet",
        )
        .bind(account_id)
        .bind(folder)
        .bind(e.uid as i64)
        .bind(e.flags.seen as i64)
        .bind(e.flags.flagged as i64)
        .bind(&e.from.name)
        .bind(&e.from.email)
        .bind(&e.subject)
        .bind(e.date)
        .bind(e.has_attachment as i64)
        .bind(&e.message_id)
        .bind(&e.snippet)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn load_envelopes(db: &Db, account_id: i64, folder: &str) -> Result<Vec<Envelope>> {
    let sql = format!(
        "SELECT {ENVELOPE_COLS} FROM messages \
         WHERE account_id = ? AND folder = ? ORDER BY date DESC"
    );
    let rows = sqlx::query(&sql)
        .bind(account_id)
        .bind(folder)
        .fetch_all(db)
        .await?;
    rows.iter().map(row_to_envelope).collect()
}

/// Store a fetched body (and, when available, the raw RFC822 header block) for a
/// cached message, so a later open - even offline - can show the full headers.
pub async fn store_body(
    db: &Db,
    account_id: i64,
    folder: &str,
    uid: u64,
    body: &str,
    raw_headers: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE messages SET body = ?, raw_headers = ? \
         WHERE account_id = ? AND folder = ? AND uid = ?",
    )
    .bind(body)
    .bind(raw_headers)
    .bind(account_id)
    .bind(folder)
    .bind(uid as i64)
    .execute(db)
    .await?;
    Ok(())
}

/// Load a cached message (envelope + body). `body` is empty if not yet fetched.
pub async fn load_message(
    db: &Db,
    account_id: i64,
    folder: &str,
    uid: u64,
) -> Result<Option<Message>> {
    let sql = format!(
        "SELECT {ENVELOPE_COLS}, body, raw_headers FROM messages \
         WHERE account_id = ? AND folder = ? AND uid = ?"
    );
    let row = sqlx::query(&sql)
        .bind(account_id)
        .bind(folder)
        .bind(uid as i64)
        .fetch_optional(db)
        .await?;
    let Some(r) = row else { return Ok(None) };
    let envelope = row_to_envelope(&r)?;
    let body: Option<String> = r.try_get(10)?;
    let raw_headers: Option<String> = r.try_get(11)?;
    Ok(Some(Message {
        envelope,
        body: body.unwrap_or_default(),
        raw_headers,
    }))
}

pub async fn delete_message(db: &Db, account_id: i64, folder: &str, uid: u64) -> Result<()> {
    sqlx::query("DELETE FROM messages WHERE account_id = ? AND folder = ? AND uid = ?")
        .bind(account_id)
        .bind(folder)
        .bind(uid as i64)
        .execute(db)
        .await?;
    Ok(())
}

/// Move a cached message to another folder (used for Archive). A no-op if the
/// source row is missing.
pub async fn move_message(
    db: &Db,
    account_id: i64,
    from_folder: &str,
    uid: u64,
    to_folder: &str,
) -> Result<()> {
    sqlx::query("UPDATE messages SET folder = ? WHERE account_id = ? AND folder = ? AND uid = ?")
        .bind(to_folder)
        .bind(account_id)
        .bind(from_folder)
        .bind(uid as i64)
        .execute(db)
        .await?;
    Ok(())
}

/// Update the seen/flagged flags of a cached message (each optional).
pub async fn set_flags(
    db: &Db,
    account_id: i64,
    folder: &str,
    uid: u64,
    seen: Option<bool>,
    flagged: Option<bool>,
) -> Result<()> {
    if let Some(s) = seen {
        sqlx::query("UPDATE messages SET seen = ? WHERE account_id = ? AND folder = ? AND uid = ?")
            .bind(s as i64)
            .bind(account_id)
            .bind(folder)
            .bind(uid as i64)
            .execute(db)
            .await?;
    }
    if let Some(f) = flagged {
        sqlx::query(
            "UPDATE messages SET flagged = ? WHERE account_id = ? AND folder = ? AND uid = ?",
        )
        .bind(f as i64)
        .bind(account_id)
        .bind(folder)
        .bind(uid as i64)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// True if the account has any cached messages (used to decide offline display).
pub async fn has_messages(db: &Db, account_id: i64) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM messages WHERE account_id = ?")
        .bind(account_id)
        .fetch_one(db)
        .await?;
    Ok(n > 0)
}

/// Full-text search within one folder's cached mail (subject/sender/body).
pub async fn search(db: &Db, account_id: i64, folder: &str, query: &str) -> Result<Vec<Envelope>> {
    let fts = build_fts_query(query);
    if fts.is_empty() {
        return load_envelopes(db, account_id, folder).await;
    }
    let sql = format!(
        "SELECT {ENVELOPE_COLS} FROM messages \
         WHERE account_id = ? AND folder = ? \
           AND rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?) \
         ORDER BY date DESC LIMIT 500"
    );
    let rows = sqlx::query(&sql)
        .bind(account_id)
        .bind(folder)
        .bind(fts)
        .fetch_all(db)
        .await?;
    rows.iter().map(row_to_envelope).collect()
}

/// Turn free-text into a safe FTS5 MATCH expression: keep alphanumeric tokens,
/// make each a prefix match, AND them together. Non-alphanumerics are dropped so
/// user input can never produce an FTS syntax error.
fn build_fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|t| {
            t.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        })
        .filter(|t| !t.is_empty())
        .map(|t| format!("{t}*"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Per-folder cached message counts `(folder, total, unread)` from a single
/// grouped query. Used for an instant sidebar estimate before the live STATUS
/// sweep lands. Counts only what is cached (the synced window), so `total` can
/// undercount a large folder - the sweep replaces it with the server truth.
pub async fn count_by_folder(db: &Db, account_id: i64) -> Result<Vec<(String, usize, usize)>> {
    let rows = sqlx::query(
        "SELECT folder, COUNT(*), SUM(CASE WHEN seen = 0 THEN 1 ELSE 0 END) \
         FROM messages WHERE account_id = ? GROUP BY folder",
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push((
            r.try_get::<String, _>(0)?,
            r.try_get::<i64, _>(1)? as usize,
            r.try_get::<i64, _>(2)? as usize,
        ));
    }
    Ok(out)
}

// Port adapter ----------------------------------------------------------

/// [`MailCache`] backed by the shared SQLite pool; one-line delegations to the
/// free functions above.
#[allow(dead_code)] // port consumed from Phase 2 onward
pub struct SqliteMailCache {
    db: Db,
}

impl SqliteMailCache {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl MailCache for SqliteMailCache {
    async fn upsert_folders(&self, account_id: i64, folders: &[Folder]) -> Result<()> {
        upsert_folders(&self.db, account_id, folders).await
    }

    async fn load_folders(&self, account_id: i64) -> Result<Vec<Folder>> {
        load_folders(&self.db, account_id).await
    }

    async fn upsert_envelopes(
        &self,
        account_id: i64,
        folder: &str,
        envelopes: &[Envelope],
    ) -> Result<()> {
        upsert_envelopes(&self.db, account_id, folder, envelopes).await
    }

    async fn load_envelopes(&self, account_id: i64, folder: &str) -> Result<Vec<Envelope>> {
        load_envelopes(&self.db, account_id, folder).await
    }

    async fn count_by_folder(&self, account_id: i64) -> Result<Vec<(String, usize, usize)>> {
        count_by_folder(&self.db, account_id).await
    }

    async fn store_body(
        &self,
        account_id: i64,
        folder: &str,
        uid: u64,
        body: &str,
        raw_headers: Option<&str>,
    ) -> Result<()> {
        store_body(&self.db, account_id, folder, uid, body, raw_headers).await
    }

    async fn load_message(
        &self,
        account_id: i64,
        folder: &str,
        uid: u64,
    ) -> Result<Option<Message>> {
        load_message(&self.db, account_id, folder, uid).await
    }

    async fn delete_message(&self, account_id: i64, folder: &str, uid: u64) -> Result<()> {
        delete_message(&self.db, account_id, folder, uid).await
    }

    async fn move_message(
        &self,
        account_id: i64,
        from_folder: &str,
        uid: u64,
        to_folder: &str,
    ) -> Result<()> {
        move_message(&self.db, account_id, from_folder, uid, to_folder).await
    }

    async fn set_flags(
        &self,
        account_id: i64,
        folder: &str,
        uid: u64,
        seen: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<()> {
        set_flags(&self.db, account_id, folder, uid, seen, flagged).await
    }

    async fn has_messages(&self, account_id: i64) -> Result<bool> {
        has_messages(&self.db, account_id).await
    }

    async fn search(&self, account_id: i64, folder: &str, query: &str) -> Result<Vec<Envelope>> {
        search(&self.db, account_id, folder, query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::database as db;
    use std::path::Path;

    fn env(uid: u64, subject: &str, from: &str, body_seen: bool) -> Envelope {
        Envelope {
            uid,
            flags: Flags {
                seen: body_seen,
                flagged: false,
            },
            from: Address::new(Some("Jane".to_string()), from),
            subject: subject.to_string(),
            date: uid as i64,
            has_attachment: false,
            snippet: None,
            message_id: Some(format!("<{uid}@x>")),
        }
    }

    async fn conn() -> Db {
        db::open(Path::new(":memory:")).await.unwrap()
    }

    #[tokio::test]
    async fn folders_round_trip() {
        let c = conn().await;
        let folders = vec![
            Folder {
                name: "INBOX".into(),
                total: 5,
                unread: 2,
                category: FolderCategory::Mailbox,
            },
            Folder {
                name: "Work".into(),
                total: 3,
                unread: 0,
                category: FolderCategory::Label,
            },
        ];
        upsert_folders(&c, 1, &folders).await.unwrap();
        let got = load_folders(&c, 1).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "INBOX");
        assert_eq!(got[1].category, FolderCategory::Label);
    }

    #[tokio::test]
    async fn envelopes_upsert_preserves_body_and_search_works() {
        let c = conn().await;
        upsert_envelopes(
            &c,
            1,
            "INBOX",
            &[env(10, "Q3 roadmap review", "jane@acme.io", false)],
        )
        .await
        .unwrap();
        store_body(
            &c,
            1,
            "INBOX",
            10,
            "let us discuss the roadmap sequencing",
            Some("Subject: Q3 roadmap review\r\nFrom: jane@acme.io"),
        )
        .await
        .unwrap();

        // Re-upsert (e.g. flags changed) must not wipe the cached body.
        upsert_envelopes(
            &c,
            1,
            "INBOX",
            &[env(10, "Q3 roadmap review", "jane@acme.io", true)],
        )
        .await
        .unwrap();
        let msg = load_message(&c, 1, "INBOX", 10).await.unwrap().unwrap();
        assert!(msg.envelope.flags.seen);
        assert_eq!(msg.body, "let us discuss the roadmap sequencing");
        // Raw headers persist through the cache (and survive the re-upsert) so
        // the reader can show full headers offline after one open.
        assert_eq!(
            msg.raw_headers.as_deref(),
            Some("Subject: Q3 roadmap review\r\nFrom: jane@acme.io")
        );

        // FTS matches subject and body (prefix), scoped to the folder + account.
        assert_eq!(search(&c, 1, "INBOX", "roadmap").await.unwrap().len(), 1);
        assert_eq!(search(&c, 1, "INBOX", "sequenc").await.unwrap().len(), 1);
        assert_eq!(search(&c, 1, "INBOX", "nonsense").await.unwrap().len(), 0);
        // Other account/folder does not leak.
        assert_eq!(search(&c, 2, "INBOX", "roadmap").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn delete_removes_from_cache_and_fts() {
        let c = conn().await;
        upsert_envelopes(&c, 1, "INBOX", &[env(10, "hello world", "a@b.io", false)])
            .await
            .unwrap();
        assert!(has_messages(&c, 1).await.unwrap());
        delete_message(&c, 1, "INBOX", 10).await.unwrap();
        assert!(!has_messages(&c, 1).await.unwrap());
        assert_eq!(search(&c, 1, "INBOX", "hello").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn set_flags_updates_columns() {
        let c = conn().await;
        upsert_envelopes(&c, 1, "INBOX", &[env(10, "s", "a@b.io", false)])
            .await
            .unwrap();
        set_flags(&c, 1, "INBOX", 10, Some(true), Some(true))
            .await
            .unwrap();
        let m = load_message(&c, 1, "INBOX", 10).await.unwrap().unwrap();
        assert!(m.envelope.flags.seen && m.envelope.flags.flagged);
    }
}
