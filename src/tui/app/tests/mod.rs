use super::*;
use crate::bootstrap::config::Config;
use crate::domain::Address;
use crate::infrastructure::database;
use std::path::{Path, PathBuf};

mod account_manager;
mod compose;
mod delete;
mod jobs;
mod navigation;
mod paging;
mod rendering;
mod settings;

fn key_ev(k: &Key) -> crossterm::event::KeyEvent {
    let mut mods = crossterm::event::KeyModifiers::NONE;
    if k.ctrl {
        mods.insert(crossterm::event::KeyModifiers::CONTROL);
    }
    if k.shift {
        mods.insert(crossterm::event::KeyModifiers::SHIFT);
    }
    if k.alt {
        mods.insert(crossterm::event::KeyModifiers::ALT);
    }
    crossterm::event::KeyEvent::new(k.code, mods)
}

macro_rules! send_key {
    ($app:expr, $k:expr) => {
        $app.on_key(&key_ev(&$k)).await
    };
}
pub(crate) use send_key;

/// Seed a real offline cache (folders + envelopes + one body) for `account_id`,
/// so tests exercise the actual `CacheSource` path instead of any fixtures. The
/// pool is capped at one connection, so the same `:memory:` pool the App clones
/// into its `CacheSource` sees these rows.
async fn seed_fixture_cache(conn: &database::Db, account_id: i64) {
    use crate::domain::{Flags, FolderCategory};
    let mailbox = FolderCategory::Mailbox;
    let label = FolderCategory::Label;
    let f = |name: &str, total: usize, unread: usize, category| Folder {
        name: name.to_string(),
        total,
        unread,
        category,
    };
    // Order matters: the Work subtree is followed by Personal so the
    // collapse-and-skip navigation test has a sibling to land on.
    let folders = vec![
        f("INBOX", 5, 2, mailbox),
        f("Starred", 2, 0, mailbox),
        f("Sent", 2, 0, mailbox),
        f("Drafts", 1, 1, mailbox),
        f("Archive", 0, 0, mailbox),
        f("Spam", 0, 0, mailbox),
        f("Trash", 0, 0, mailbox),
        f("Work", 2, 0, label),
        f("Work/projectA", 1, 0, label),
        f("Work/projectB", 1, 0, label),
        f("Personal", 0, 0, label),
    ];
    database::cache::upsert_folders(conn, account_id, &folders)
        .await
        .unwrap();

    let env = |uid: u64, subject: &str, seen: bool| Envelope {
        uid,
        flags: Flags {
            seen,
            flagged: false,
        },
        from: Address::new(Some("Jane Cooper".to_string()), "jane@acme.io"),
        subject: subject.to_string(),
        // Newest-first ordering (load sorts by date DESC): lower uid = newer.
        date: 1000 - uid as i64,
        has_attachment: false,
        snippet: None,
        message_id: Some(format!("<{uid}@pesan.test>")),
    };
    let inbox = [
        env(1, "Q3 roadmap review", false),
        env(2, "PR: fix refresh flow", false),
        env(3, "Your receipt for August", true),
        env(4, "Dinner this weekend?", true),
        env(5, "5 new jobs for you", true),
    ];
    database::cache::upsert_envelopes(conn, account_id, "INBOX", &inbox)
        .await
        .unwrap();
    database::cache::store_body(
        conn,
        account_id,
        "INBOX",
        1,
        "Let us discuss the roadmap sequencing.",
        Some("Subject: Q3 roadmap review\r\nFrom: jane@acme.io\r\nDate: Mon, 1 Jan 2024 09:00:00 +0000"),
    )
    .await
    .unwrap();
}

async fn test_app() -> App {
    let conn = database::open(Path::new(":memory:")).await.unwrap();
    let account = Account {
        id: None,
        name: "personal".to_string(),
        email: "you@gmail.com".to_string(),
        provider: "gmail".to_string(),
        keychain_ref: String::new(),
        is_default: true,
        created_at: 1,
    };
    let id = crate::infrastructure::database::accounts::upsert(&conn, &account)
        .await
        .unwrap();
    seed_fixture_cache(&conn, id).await;
    let accounts = crate::infrastructure::database::accounts::list(&conn)
        .await
        .unwrap();
    App::new(
        Config::default(),
        conn.clone(),
        accounts,
        crate::infrastructure::sqlite_services(conn.clone()),
    )
    .await
}

fn temp_db_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut p = std::env::temp_dir();
    p.push(format!(
        "pesan-test-{}-{}.db",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    p
}

async fn focus_authorize(app: &mut App) {
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a')); // add account -> enters edit mode
    // The OAuth field order is Provider -> Name -> Default -> Authorize; step
    // down until the Authorize button is focused.
    for _ in 0..6 {
        if app.settings.as_ref().unwrap().focus == SettingsFocus::Authorize {
            break;
        }
        send_key!(app, Key::ch('j'));
    }
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Authorize
    );
}

fn nord_spec() -> crate::bootstrap::config::ThemeSpec {
    crate::bootstrap::config::ThemeSpec {
        bg: "#2e3440".into(),
        fg: "#d8dee9".into(),
        dim: "#4c566a".into(),
        accent: "#88c0d0".into(),
        unread: "#ebcb8b".into(),
        success: "#a3be8c".into(),
        warning: "#d08770".into(),
        error: "#bf616a".into(),
        border: "#3b4252".into(),
        border_focus: "#88c0d0".into(),
    }
}

/// App whose config also defines a password-login provider `fastmail` (no
/// `oauth` block), for exercising the manual-add and password-auth paths.
async fn test_app_with_password_provider() -> App {
    let conn = database::open(Path::new(":memory:")).await.unwrap();
    let account = Account {
        id: None,
        name: "personal".to_string(),
        email: "you@gmail.com".to_string(),
        provider: "gmail".to_string(),
        keychain_ref: String::new(),
        is_default: true,
        created_at: 1,
    };
    let id = crate::infrastructure::database::accounts::upsert(&conn, &account)
        .await
        .unwrap();
    seed_fixture_cache(&conn, id).await;
    let accounts = crate::infrastructure::database::accounts::list(&conn)
        .await
        .unwrap();
    let mut config = Config::default();
    config.providers.insert(
        "fastmail".to_string(),
        crate::bootstrap::config::Provider {
            imap: crate::bootstrap::config::Server {
                host: "imap.fastmail.com".into(),
                port: 993,
            },
            smtp: crate::bootstrap::config::Server {
                host: "smtp.fastmail.com".into(),
                port: 465,
            },
            oauth: None,
        },
    );
    App::new(
        config,
        conn.clone(),
        accounts,
        crate::infrastructure::sqlite_services(conn.clone()),
    )
    .await
}

/// Build an older-page envelope with a `uid`; `date` is derived so higher uids
/// sort older (matches the fixture's `date = 1000 - uid` convention).
fn older_env(uid: u64) -> Envelope {
    use crate::domain::Flags;
    Envelope {
        uid,
        flags: Flags {
            seen: true,
            flagged: false,
        },
        from: Address::new(Some("Older Sender".to_string()), "old@acme.io"),
        subject: format!("older message {uid}"),
        date: 1000 - uid as i64,
        has_attachment: false,
        snippet: None,
        message_id: Some(format!("<{uid}@pesan.test>")),
    }
}
