use super::*;
use ratatui::{Terminal, backend::TestBackend};
use crate::mail::{Flags, FolderCategory};
use crate::account::infrastructure::accounts;
use crate::mail::infrastructure::sqlite_cache;
use crate::ui::app::event;
use crate::ui::{fmt, view};
use crate::wiring;

#[tokio::test]
async fn offline_first_shows_cache_and_fts_search() {
    let path = temp_db_path();
    let conn = database::open(&path).await.unwrap();
    let account = Account {
        id: None,
        name: "personal".to_string(),
        email: "you@gmail.com".to_string(),
        provider: "gmail".to_string(),
        keychain_ref: "pesan/personal/refresh".to_string(),
        is_default: true,
        created_at: 1,
    };
    let id = accounts::upsert(&conn, &account)
        .await
        .unwrap();
    sqlite_cache::upsert_folders(
        &conn,
        &id,
        &[Folder {
            name: "INBOX".to_string(),
            total: 2,
            unread: 1,
            category: FolderCategory::Mailbox,
        }],
    )
        .await
        .unwrap();
    sqlite_cache::upsert_envelopes(
        &conn,
        &id,
        "INBOX",
        &[
            Envelope {
                uid: 1,
                flags: Flags::default(),
                from: Address::new(Some("Jane".to_string()), "jane@acme.io"),
                subject: "Q3 roadmap review".to_string(),
                date: 100,
                has_attachment: false,
                snippet: None,
                message_id: None,
            },
            Envelope {
                uid: 2,
                flags: Flags::default(),
                from: Address::new(None, "bob@x.io"),
                subject: "lunch friday".to_string(),
                date: 90,
                has_attachment: false,
                snippet: None,
                message_id: None,
            },
        ],
    )
        .await
        .unwrap();
    let accounts = accounts::list(&conn)
        .await
        .unwrap();
    let conn2 = database::open(&path).await.unwrap();
    let mut app = App::new(
        Config::default(),
        accounts,
        wiring::sqlite_services(conn2.clone()),
    )
    .await;

    assert_eq!(app.selected_folder_name(), "INBOX");
    assert_eq!(app.envelopes.len(), 2);

    app.run_search("roadmap").await;
    assert_eq!(app.display_envelopes.len(), 1);
    assert!(app.display_envelopes[0].subject.contains("roadmap"));

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn reader_shows_real_recipient_from_headers() {
    use crate::mail::{Flags, Message};

    let mut app = test_app().await;
    app.open_message = Some(Message {
        envelope: Envelope {
            uid: 1,
            flags: Flags { seen: true, flagged: false },
            from: Address::new(None, "you@gmail.com"),
            subject: "hello world".to_string(),
            date: 0,
            has_attachment: false,
            snippet: None,
            message_id: None,
        },
        body: "hello world".to_string(),
        raw_html: None,
        raw_headers: Some("From: you@gmail.com\nTo: dest@other.com\nSubject: hello world".to_string()),
        raw: None,
    });
    app.view = View::Reader;

    let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let text: String = (0..term.backend().buffer().area.height)
        .flat_map(|y| (0..term.backend().buffer().area.width).map(move |x| (x, y)))
        .map(|(x, y)| term.backend().buffer()[(x, y)].symbol().to_string())
        .collect();
    assert!(text.contains("dest@other.com"));
}

#[tokio::test]
async fn help_overlay_shows_global_keys() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('?'));
    assert!(app.help_open);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let text: String = (0..term.backend().buffer().area.height)
        .flat_map(|y| (0..term.backend().buffer().area.width).map(move |x| (x, y)))
        .map(|(x, y)| term.backend().buffer()[(x, y)].symbol().to_string())
        .collect();
    assert!(text.contains("refresh mailbox"));
    assert!(text.contains("quit"));
    assert!(text.contains("open message"));
}

#[tokio::test]
async fn marked_rows_use_background_band() {
    let mut app = test_app().await;
    app.marked.insert(2);
    app.set_viewport_width(90);
    let mut term = Terminal::new(TestBackend::new(90, 10)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();

    let band = app.theme.marked_style().bg;
    assert!(band.is_some());
    assert_eq!(buf[(30u16, 3u16)].style().bg, band);
    assert_ne!(buf[(30u16, 4u16)].style().bg, band);
    let row3: String = (0..buf.area.width)
        .map(|x| buf[(x, 3u16)].symbol())
        .collect();
    assert!(!row3.contains('●'));
}

#[tokio::test]
async fn list_shows_star_for_flagged_messages() {
    let mut app = test_app().await;
    if let Some(e) = app.envelopes.iter_mut().find(|e| e.uid == 2) {
        e.flags.flagged = true;
    }
    app.refresh_display_list(None);
    app.set_viewport_width(90);
    let mut term = Terminal::new(TestBackend::new(90, 10)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let row_text = |y: u16| -> String { (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect() };

    assert!(!row_text(2).contains('★'));
    assert!(row_text(3).contains("PR: fix refresh flow"));
    assert!(row_text(3).contains('★'));
    assert!(row_text(1).contains('★'));
}

#[tokio::test]
async fn list_date_column_right_aligned() {
    let mut app = test_app().await;
    app.set_viewport_width(120);
    let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();

    let row = 2u16;
    let border_x = (0..buf.area.width)
        .rev()
        .find(|&x| buf[(x, row)].symbol() == "│")
        .expect("list right border");
    assert_eq!(buf[(border_x - 1, row)].symbol(), " ");
    let first_date = 999;
    let expected = fmt::relative_date(first_date);
    let needle = expected.split_once(' ').map(|(_, rest)| rest).unwrap_or(&expected);
    let cells: Vec<String> = (0..buf.area.width)
        .map(|x| buf[(x as u16, row)].symbol().to_string())
        .collect();
    let needle_cells: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    let idx = (0..=cells.len().saturating_sub(needle_cells.len()))
        .find(|&s| cells[s..s + needle_cells.len()] == *needle_cells)
        .expect("date rendered");
    let gap = &cells[idx + needle_cells.len()..(border_x as usize - 1)];
    assert!(gap.iter().all(|c| c == " "));
}

#[tokio::test]
async fn list_never_renders_blank_sender_or_subject() {
    let mut app = test_app().await;
    for e in app.envelopes.iter_mut() {
        e.from = Address { name: None, email: String::new() };
        e.subject = String::new();
    }
    app.refresh_display_list(None);

    let mut term = Terminal::new(TestBackend::new(100, 16)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let text: String = (0..term.backend().buffer().area.height)
        .flat_map(|y| (0..term.backend().buffer().area.width).map(move |x| (x, y)))
        .map(|(x, y)| term.backend().buffer()[(x, y)].symbol().to_string())
        .collect();
    assert!(text.contains("(unknown)"));
    assert!(text.contains("(no subject)"));
}

#[tokio::test]
async fn statusbar_position_and_content() {
    fn row_text(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    let mut app = test_app().await;
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();

    // Default: bottom
    term.draw(|f| view::draw(f, &app)).unwrap();
    assert!(row_text(&term, 29).contains("you@gmail.com"));
    assert!(!row_text(&term, 0).contains("you@gmail.com"));

    // Top position
    app.config.ui.statusbar_position = "top".to_string();
    term.draw(|f| view::draw(f, &app)).unwrap();
    assert!(row_text(&term, 0).contains("you@gmail.com"));
    assert!(!row_text(&term, 29).contains("you@gmail.com"));

    // Tiny terminal lays out cleanly
    let mut tiny = Terminal::new(TestBackend::new(20, 6)).unwrap();
    tiny.draw(|f| view::draw(f, &app)).unwrap();
    send_key!(app, Key::ch('c'));
    term.draw(|f| view::draw(f, &app)).unwrap();
    app.view = View::Settings;
    term.draw(|f| view::draw(f, &app)).unwrap();
}

#[tokio::test]
async fn statusbar_identity_notification_sync() {
    fn row_text(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    let mut app = test_app().await;
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();

    // Idle: identity + mailbox
    term.draw(|f| view::draw(f, &app)).unwrap();
    let bar = row_text(&term, 29);
    assert!(bar.contains("<you@gmail.com>"));
    assert!(bar.contains("INBOX"));

    // Notification renders in bar
    app.set_toast("Message sent", ToastKind::Success);
    term.draw(|f| view::draw(f, &app)).unwrap();
    assert!(row_text(&term, 29).contains("Message sent"));

    // Sync progress
    app.toasts.clear();
    app.sync = Some(event::SyncProgress {
        account: app.active_account_name().to_string(),
        done: 3,
        total: 10,
    });
    term.draw(|f| view::draw(f, &app)).unwrap();
    assert!(row_text(&term, 29).contains("3/10"));

    // Complete clears
    app.on_sync_progress(event::SyncProgress {
        account: app.active_account_name().to_string(),
        done: 10,
        total: 10,
    });
    assert!(app.sync.is_none());
}

#[tokio::test]
async fn reader_render_memoized() {
    let mut app = test_app().await;
    app.selected_message = app
        .display_envelopes
        .iter()
        .position(|e| e.uid == 1)
        .unwrap();
    app.view = View::Reader;
    app.load_selected_preview().await;
    assert!(app.open_message.is_some());

    assert!(app.reader_dirty);
    app.prepare_reader(80);
    assert!(!app.reader_dirty);
    let body_text: String = app
        .reader_lines()
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(body_text.contains("roadmap sequencing"));
    let cached_len = app.reader_lines().len();

    // Scroll preserves cache
    app.reader_offset = 1;
    app.prepare_reader(80);
    assert!(!app.reader_dirty);
    assert_eq!(app.reader_lines().len(), cached_len);

    // Width change rebuilds
    app.prepare_reader(40);
    assert_eq!(app.reader_cache.as_ref().unwrap().wrap_width, 38);

    // Different message dirties
    app.set_open_message(None);
    assert!(app.reader_dirty);
}

#[test]
fn theme_resolution() {
    let mut themes = HashMap::new();
    themes.insert("nord".to_string(), nord_spec());

    let (name, theme) = App::resolve_theme("nord", &themes);
    assert_eq!(name, "nord");
    assert_eq!(theme.bg, ratatui::style::Color::Rgb(0x2e, 0x34, 0x40));
    assert_eq!(theme.accent, ratatui::style::Color::Rgb(0x88, 0xc0, 0xd0));

    // Custom "dark" doesn't override builtin
    let mut evil = nord_spec();
    evil.bg = "#ffffff".into();
    themes.insert("dark".to_string(), evil);
    let (name, theme) = App::resolve_theme("dark", &themes);
    assert_eq!(name, "dark");
    assert_eq!(theme.bg, Theme::dark().bg);

    // Unknown falls back
    let themes = HashMap::new();
    let (name, theme) = App::resolve_theme("bogus", &themes);
    assert_eq!(name, "dark");
    assert_eq!(theme.bg, Theme::dark().bg);

    // Bad color falls back per role
    let mut themes = HashMap::new();
    let mut spec = nord_spec();
    spec.bg = "not-a-color".into();
    themes.insert("nord".to_string(), spec);
    let (_, theme) = App::resolve_theme("nord", &themes);
    assert_eq!(theme.bg, Theme::dark().bg);
    assert_eq!(theme.accent, ratatui::style::Color::Rgb(0x88, 0xc0, 0xd0));
}

#[tokio::test]
async fn app_applies_custom_theme() {
    let mut config = Config::default();
    config.ui.theme = "nord".to_string();
    config.themes.insert("nord".to_string(), nord_spec());
    let conn = database::open(Path::new(":memory:")).await.unwrap();
    let app = App::new(
        config,
        vec![],
        wiring::sqlite_services(conn.clone()),
    )
    .await;
    assert_eq!(app.theme_name(), "nord");
    assert_eq!(app.theme.bg, ratatui::style::Color::Rgb(0x2e, 0x34, 0x40));
}

#[tokio::test]
async fn border_type_changes_glyphs() {
    async fn render_with(border_type: &str) -> String {
        let mut config = Config::default();
        config.ui.border_type = border_type.to_string();
        let conn = database::open(Path::new(":memory:")).await.unwrap();
        let app = App::new(
            config,
            vec![],
            wiring::sqlite_services(conn.clone()),
        )
        .await;
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| view::draw(f, &app)).unwrap();
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    let plain = render_with("plain").await;
    let thick = render_with("thick").await;
    assert!(!plain.contains('┏'));
    assert!(thick.contains('┏'));
}

#[tokio::test]
async fn reader_wraps_long_lines() {
    use crate::mail::Message;

    let mut app = test_app().await;
    app.action(Action::OpenMessage).await;
    assert_eq!(app.view, View::Reader);

    let long = "lorem ipsum dolor sit amet ".repeat(12);
    let env = app.open_message.as_ref().unwrap().envelope.clone();
    app.open_message = Some(Message {
        envelope: env,
        body: long,
        raw_html: None,
        raw_headers: None,
        raw: None,
    });

    let w = 60u16;
    let mut term = Terminal::new(TestBackend::new(w, 30)).unwrap();
    term.draw(|f| view::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();

    let mut body_rows = 0usize;
    for y in 0..buf.area().height {
        let row: String = (0..buf.area().width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        if row.contains("lorem ipsum") {
            body_rows += 1;
        }
    }
    assert!(body_rows >= 3);
}
