use anyhow::Result;
use sqlx::AssertSqlSafe;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::mail::application::ports::MailCache;
use crate::mail::{Address, Envelope, Flags, Folder, FolderCategory, Message};
use crate::platform::db::Db;
use crate::mail::infrastructure::maildir::MaildirStore;

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
pub async fn upsert_folders(db: &Db, account_id: &str, folders: &[Folder]) -> Result<()> {
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

pub async fn load_folders(db: &Db, account_id: &str) -> Result<Vec<Folder>> {
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
    account_id: &str,
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

pub async fn load_envelopes(db: &Db, account_id: &str, folder: &str) -> Result<Vec<Envelope>> {
    let sql = format!(
        "SELECT {ENVELOPE_COLS} FROM messages \
         WHERE account_id = ? AND folder = ? ORDER BY date DESC"
    );
    let rows = sqlx::query(AssertSqlSafe(sql))
        .bind(account_id)
        .bind(folder)
        .fetch_all(db)
        .await?;
    rows.iter().map(row_to_envelope).collect()
}

/// Store a fetched message body: the complete `raw` RFC822 bytes go to the
/// interoperable Maildir file, the sole home for message content. The SQLite
/// index keeps only the envelope; nothing about the body is written to it. The
/// file's flags mirror the cached envelope so an external tool sees the right
/// \Seen/\Flagged state. A no-op when `raw` is `None`.
pub async fn store_body(
    db: &Db,
    maildir: &MaildirStore,
    account_id: &str,
    folder: &str,
    uid: u64,
    raw: Option<&[u8]>,
) -> Result<()> {
    if let Some(raw) = raw {
        let (seen, flagged) = flags_of(db, account_id, folder, uid).await;
        maildir.write(account_id, folder, uid, seen, flagged, raw)?;
    }
    Ok(())
}

/// The cached `(seen, flagged)` flags for a message, defaulting to `(false,
/// false)` when the row is missing.
async fn flags_of(db: &Db, account_id: &str, folder: &str, uid: u64) -> (bool, bool) {
    sqlx::query("SELECT seen, flagged FROM messages WHERE account_id = ? AND folder = ? AND uid = ?")
        .bind(account_id)
        .bind(folder)
        .bind(uid as i64)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .and_then(|r| {
            Some((
                r.try_get::<i64, _>(0).ok()? != 0,
                r.try_get::<i64, _>(1).ok()? != 0,
            ))
        })
        .unwrap_or((false, false))
}

/// Load a cached message: envelope + flags from the index, and the body / HTML
/// source / raw headers re-derived from the Maildir file when one exists. `body`
/// is the indexed plain text (empty if never fetched) when there is no file.
pub async fn load_message(
    db: &Db,
    maildir: &MaildirStore,
    account_id: &str,
    folder: &str,
    uid: u64,
) -> Result<Option<Message>> {
    let sql = format!(
        "SELECT {ENVELOPE_COLS} FROM messages \
         WHERE account_id = ? AND folder = ? AND uid = ?"
    );
    let row = sqlx::query(AssertSqlSafe(sql))
        .bind(account_id)
        .bind(folder)
        .bind(uid as i64)
        .fetch_optional(db)
        .await?;
    let Some(r) = row else { return Ok(None) };
    let envelope = row_to_envelope(&r)?;
    // The Maildir file is the sole source of body content; empty when not fetched.
    let (body, raw_html, raw_headers) = match maildir.read_parsed(account_id, folder, uid) {
        Some(p) => (p.text, p.raw_html, p.raw_headers),
        None => (String::new(), None, None),
    };
    Ok(Some(Message {
        envelope,
        body,
        raw_html,
        raw_headers,
        raw: None,
    }))
}

pub async fn delete_message(
    db: &Db,
    maildir: &MaildirStore,
    account_id: &str,
    folder: &str,
    uid: u64,
) -> Result<()> {
    sqlx::query("DELETE FROM messages WHERE account_id = ? AND folder = ? AND uid = ?")
        .bind(account_id)
        .bind(folder)
        .bind(uid as i64)
        .execute(db)
        .await?;
    maildir.delete(account_id, folder, uid);
    Ok(())
}

/// Move a cached message to another folder (used for Archive). A no-op if the
/// source row is missing.
pub async fn move_message(
    db: &Db,
    maildir: &MaildirStore,
    account_id: &str,
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
    maildir.move_to(account_id, from_folder, uid, to_folder);
    Ok(())
}

/// Update the seen/flagged flags of a cached message (each optional).
pub async fn set_flags(
    db: &Db,
    maildir: &MaildirStore,
    account_id: &str,
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
    // Mirror the resulting flags onto the Maildir filename (no-op if no file).
    let (final_seen, final_flagged) = flags_of(db, account_id, folder, uid).await;
    maildir.set_flags(account_id, folder, uid, final_seen, final_flagged);
    Ok(())
}

/// True if the account has any cached messages (used to decide offline display).
pub async fn has_messages(db: &Db, account_id: &str) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM messages WHERE account_id = ?")
        .bind(account_id)
        .fetch_one(db)
        .await?;
    Ok(n > 0)
}

/// Full-text search within one folder's cached mail (subject/sender/body).
pub async fn search(db: &Db, account_id: &str, folder: &str, query: &str) -> Result<Vec<Envelope>> {
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
    let rows = sqlx::query(AssertSqlSafe(sql))
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
pub async fn count_by_folder(db: &Db, account_id: &str) -> Result<Vec<(String, usize, usize)>> {
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

//
// Port adapter
//

/// [`MailCache`] backed by the shared SQLite index plus a [`MaildirStore`] for
/// message content; one-line delegations to the free functions above.
pub struct SqliteMailCache {
    db: Db,
    maildir: MaildirStore,
}

impl SqliteMailCache {
    pub fn new(db: Db, maildir: MaildirStore) -> Self {
        Self { db, maildir }
    }
}

#[async_trait::async_trait]
impl MailCache for SqliteMailCache {
    async fn upsert_folders(&self, account_id: &str, folders: &[Folder]) -> Result<()> {
        upsert_folders(&self.db, account_id, folders).await
    }

    async fn upsert_envelopes(
        &self,
        account_id: &str,
        folder: &str,
        envelopes: &[Envelope],
    ) -> Result<()> {
        upsert_envelopes(&self.db, account_id, folder, envelopes).await
    }

    async fn load_envelopes(&self, account_id: &str, folder: &str) -> Result<Vec<Envelope>> {
        load_envelopes(&self.db, account_id, folder).await
    }

    async fn count_by_folder(&self, account_id: &str) -> Result<Vec<(String, usize, usize)>> {
        count_by_folder(&self.db, account_id).await
    }

    async fn store_body(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
        raw: Option<&[u8]>,
    ) -> Result<()> {
        store_body(&self.db, &self.maildir, account_id, folder, uid, raw).await
    }

    async fn load_message(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
    ) -> Result<Option<Message>> {
        load_message(&self.db, &self.maildir, account_id, folder, uid).await
    }

    async fn delete_message(&self, account_id: &str, folder: &str, uid: u64) -> Result<()> {
        delete_message(&self.db, &self.maildir, account_id, folder, uid).await
    }

    async fn set_flags(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
        seen: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<()> {
        set_flags(&self.db, &self.maildir, account_id, folder, uid, seen, flagged).await
    }

    async fn has_messages(&self, account_id: &str) -> Result<bool> {
        has_messages(&self.db, account_id).await
    }

    async fn search(&self, account_id: &str, folder: &str, query: &str) -> Result<Vec<Envelope>> {
        search(&self.db, account_id, folder, query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::db as db;
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

    /// A scratch Maildir root that cleans up on drop, for the cache tests.
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn mdir() -> (MaildirStore, Scratch) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("pesan-cache-md-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        (MaildirStore::new(p.clone()), Scratch(p))
    }

    /// A raw text/html message whose derived plain text contains `roadmap`.
    const RAW: &[u8] = b"From: Jane <jane@acme.io>\r\nSubject: Q3 roadmap review\r\n\
Content-Type: text/html\r\n\r\n<p>let us discuss the roadmap sequencing</p>\r\n";

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
        upsert_folders(&c, "a1", &folders).await.unwrap();
        let got = load_folders(&c, "a1").await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "INBOX");
        assert_eq!(got[1].category, FolderCategory::Label);
    }

    #[tokio::test]
    async fn envelopes_upsert_preserves_body_and_search_works() {
        let c = conn().await;
        let (md, _s) = mdir();
        upsert_envelopes(
            &c,
            "a1",
            "INBOX",
            &[env(10, "Q3 roadmap review", "jane@acme.io", false)],
        )
        .await
        .unwrap();
        // The raw message goes to the Maildir file; nothing body-related to SQLite.
        store_body(&c, &md, "a1", "INBOX", 10, Some(RAW)).await.unwrap();

        // Re-upsert (e.g. flags changed) must not remove the stored file.
        upsert_envelopes(
            &c,
            "a1",
            "INBOX",
            &[env(10, "Q3 roadmap review", "jane@acme.io", true)],
        )
        .await
        .unwrap();
        let msg = load_message(&c, &md, "a1", "INBOX", 10).await.unwrap().unwrap();
        assert!(msg.envelope.flags.seen);
        // Body + HTML source + raw headers are re-derived from the Maildir file.
        assert!(msg.body.contains("roadmap sequencing"));
        assert!(msg.raw_html.as_deref().unwrap().contains("<p>"));
        assert!(
            msg.raw_headers
                .as_deref()
                .unwrap()
                .contains("Subject: Q3 roadmap review")
        );

        // FTS indexes subject + sender only: subject/sender terms match, but a
        // body-only word does not (body lives on disk, not in the index).
        assert_eq!(search(&c, "a1", "INBOX", "roadmap").await.unwrap().len(), 1);
        assert_eq!(search(&c, "a1", "INBOX", "jane").await.unwrap().len(), 1);
        assert_eq!(search(&c, "a1", "INBOX", "sequenc").await.unwrap().len(), 0);
        assert_eq!(search(&c, "a1", "INBOX", "nonsense").await.unwrap().len(), 0);
        // Other account/folder does not leak.
        assert_eq!(search(&c, "a2", "INBOX", "roadmap").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn store_body_writes_maildir_file() {
        let c = conn().await;
        let (md, _s) = mdir();
        upsert_envelopes(&c, "a1", "INBOX", &[env(10, "s", "a@b.io", true)])
            .await
            .unwrap();
        store_body(&c, &md, "a1", "INBOX", 10, Some(RAW)).await.unwrap();
        // The interoperable file holds the verbatim raw message.
        assert_eq!(md.read("a1", "INBOX", 10).as_deref(), Some(RAW));
    }

    #[tokio::test]
    async fn delete_removes_from_cache_fts_and_file() {
        let c = conn().await;
        let (md, _s) = mdir();
        upsert_envelopes(&c, "a1", "INBOX", &[env(10, "hello world", "a@b.io", false)])
            .await
            .unwrap();
        store_body(&c, &md, "a1", "INBOX", 10, Some(RAW)).await.unwrap();
        assert!(has_messages(&c, "a1").await.unwrap());
        assert!(md.read("a1", "INBOX", 10).is_some());
        delete_message(&c, &md, "a1", "INBOX", 10).await.unwrap();
        assert!(!has_messages(&c, "a1").await.unwrap());
        assert_eq!(search(&c, "a1", "INBOX", "hello").await.unwrap().len(), 0);
        assert!(md.read("a1", "INBOX", 10).is_none());
    }

    #[tokio::test]
    async fn set_flags_updates_columns() {
        let c = conn().await;
        let (md, _s) = mdir();
        upsert_envelopes(&c, "a1", "INBOX", &[env(10, "s", "a@b.io", false)])
            .await
            .unwrap();
        set_flags(&c, &md, "a1", "INBOX", 10, Some(true), Some(true))
            .await
            .unwrap();
        let m = load_message(&c, &md, "a1", "INBOX", 10).await.unwrap().unwrap();
        assert!(m.envelope.flags.seen && m.envelope.flags.flagged);
    }
}
