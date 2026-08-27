use super::*;

#[tokio::test]
async fn offline_first_shows_cache_and_fts_search() {
    use crate::domain::{Flags, FolderCategory};
    // A file-backed DB so the App's CacheSource (its own connection) sees the
    // same rows we seed here (:memory: connections are independent).
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
    let id = crate::infrastructure::database::accounts::upsert(&conn, &account)
        .await
        .unwrap();
    database::cache::upsert_folders(
        &conn,
        id,
        &[Folder {
            name: "INBOX".to_string(),
            total: 2,
            unread: 1,
            category: FolderCategory::Mailbox,
        }],
    )
    .await
    .unwrap();
    database::cache::upsert_envelopes(
        &conn,
        id,
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
    let accounts = crate::infrastructure::database::accounts::list(&conn)
        .await
        .unwrap();
    let conn2 = database::open(&path).await.unwrap();
    let app = App::new(
        Config::default(),
        conn2.clone(),
        accounts,
        crate::infrastructure::sqlite_services(conn2.clone()),
    )
    .await;

    // Offline-first: the cached inbox is shown from the DB.
    assert_eq!(app.selected_folder_name(), "INBOX");
    assert_eq!(app.envelopes.len(), 2);

    let mut app = app;
    app.run_search("roadmap").await;
    assert_eq!(app.display_envelopes.len(), 1);
    assert!(app.display_envelopes[0].subject.contains("roadmap"));

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn reader_shows_real_recipient_from_headers_not_account() {
    use crate::domain::{Flags, Message};
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await; // active account: you@gmail.com
    // A message this account SENT: From is us, To is someone else. The reader
    // must show the real recipient from the headers, not assume it is us.
    app.open_message = Some(Message {
        envelope: Envelope {
            uid: 1,
            flags: Flags {
                seen: true,
                flagged: false,
            },
            from: Address::new(None, "you@gmail.com"),
            subject: "hello world".to_string(),
            date: 0,
            has_attachment: false,
            snippet: None,
            message_id: None,
        },
        body: "hello world".to_string(),
        raw_headers: Some(
            "From: you@gmail.com\nTo: dest@other.com\nSubject: hello world".to_string(),
        ),
    });
    app.view = View::Reader;

    let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let text: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    assert!(
        text.contains("dest@other.com"),
        "reader shows the real recipient from the To header"
    );
}

#[tokio::test]
async fn help_overlay_shows_global_refresh_key() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    send_key!(app, Key::ch('?'));
    assert!(app.help_open);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let text: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    // The global column is visible alongside the list keys, including refresh.
    assert!(
        text.contains("refresh mailbox"),
        "help shows the refresh key"
    );
    assert!(text.contains("quit"), "help shows global quit");
    assert!(
        text.contains("open message"),
        "help shows the active context"
    );
}

#[tokio::test]
async fn marked_rows_use_a_background_band_not_a_glyph() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    // Tag uid 2 (display row 3) - a non-cursor row, since the cursor row's own
    // selection highlight would otherwise mask the band.
    app.marked.insert(2);
    app.set_viewport_width(90);
    let mut term = Terminal::new(TestBackend::new(90, 10)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();

    let band = app.theme.marked_style().bg;
    assert!(band.is_some(), "marked style defines a background");
    // A cell inside the tagged row carries the band background...
    assert_eq!(
        buf[(30u16, 3u16)].style().bg,
        band,
        "tagged row shows the background band"
    );
    // ...while an untagged row does not.
    assert_ne!(
        buf[(30u16, 4u16)].style().bg,
        band,
        "untagged row has no band"
    );
    // The old leading circle glyph is gone.
    let row3: String = (0..buf.area.width)
        .map(|x| buf[(x, 3u16)].symbol())
        .collect();
    assert!(!row3.contains('●'), "no circle glyph on tagged rows");
}

#[tokio::test]
async fn list_shows_star_column_for_flagged_messages() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    // Star exactly one message (uid 2), leave uid 1 unflagged.
    if let Some(e) = app.envelopes.iter_mut().find(|e| e.uid == 2) {
        e.flags.flagged = true;
    }
    app.refresh_display_list(None);
    app.set_viewport_width(90);
    let mut term = Terminal::new(TestBackend::new(90, 10)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let row_text =
        |y: u16| -> String { (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect() };

    // Rows are newest-first (lower uid = newer): uid 1 at row 2, uid 2 at row 3.
    assert!(row_text(2).contains("Q3 roadmap review"), "row 2 is uid 1");
    assert!(!row_text(2).contains('★'), "unflagged row has no star");
    assert!(
        row_text(3).contains("PR: fix refresh flow"),
        "row 3 is uid 2"
    );
    assert!(row_text(3).contains('★'), "flagged row shows the star");
    // The header labels the star column.
    assert!(row_text(1).contains('★'), "header marks the star column");
}

#[tokio::test]
async fn list_date_column_is_right_aligned() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    app.set_viewport_width(120);
    let mut term = Terminal::new(TestBackend::new(120, 12)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();

    // On the first message row, the date must sit flush against the list pane's
    // right border (only the border cell after it), not float with a wide gap.
    let row = 2u16;
    let border_x = (0..buf.area.width)
        .rev()
        .find(|&x| buf[(x, row)].symbol() == "│")
        .expect("list right border");
    // A one-column margin (space) sits between the date and the border, with the
    // date ('...Jan') right-aligned just before it.
    assert_eq!(
        buf[(border_x - 1, row)].symbol(),
        " ",
        "one-column margin before the border"
    );
    let date_tail: String = ((border_x - 4)..(border_x - 1))
        .map(|x| buf[(x, row)].symbol())
        .collect();
    assert_eq!(
        date_tail, "Jan",
        "date sits right-aligned before the margin"
    );
}

#[tokio::test]
async fn list_never_renders_blank_sender_or_subject() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    // Simulate messages whose From/Subject failed to parse (an odd/undecodable
    // header), which previously cached and rendered as fully blank rows.
    for e in app.envelopes.iter_mut() {
        e.from = Address {
            name: None,
            email: String::new(),
        };
        e.subject = String::new();
    }
    app.refresh_display_list(None);

    let mut term = Terminal::new(TestBackend::new(100, 16)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let text: String = (0..buf.area.height)
        .flat_map(|y| {
            (0..buf.area.width)
                .map(move |x| (x, y))
                .map(|(x, y)| buf[(x, y)].symbol().to_string())
        })
        .collect();
    // The placeholders stand in for the missing fields instead of blank cells.
    assert!(
        text.contains("(unknown)"),
        "blank sender should fall back to a placeholder"
    );
    assert!(
        text.contains("(no subject)"),
        "blank subject should fall back to a placeholder"
    );
}

#[tokio::test]
async fn statusbar_position_places_the_single_bar() {
    use ratatui::{Terminal, backend::TestBackend};

    // Read the full text of buffer row `y`.
    fn row_text(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
    }

    let mut app = test_app().await;
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();

    // Default: the single statusbar (its `<email>` identity) sits on the last row.
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    assert!(
        row_text(&term, 29).contains("you@gmail.com"),
        "bar should be at bottom"
    );
    assert!(
        !row_text(&term, 0).contains("you@gmail.com"),
        "top row should be body"
    );

    // Top position moves it to the first row instead.
    app.config.ui.statusbar_position = "top".to_string();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    assert!(
        row_text(&term, 0).contains("you@gmail.com"),
        "bar should be at top"
    );
    assert!(
        !row_text(&term, 29).contains("you@gmail.com"),
        "bottom row should be body"
    );

    // Every view lays out cleanly at the top, wide and cramped.
    let mut tiny = Terminal::new(TestBackend::new(20, 6)).unwrap();
    tiny.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    send_key!(app, Key::ch('c')); // compose
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    app.view = View::Settings;
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
}

#[tokio::test]
async fn statusbar_left_identity_and_right_status_slot() {
    use ratatui::{Terminal, backend::TestBackend};

    fn row_text(term: &Terminal<TestBackend>, y: u16) -> String {
        let buf = term.backend().buffer();
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
    }

    let mut app = test_app().await;
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();

    // Idle: left shows `<email>` identity + current mailbox; the right status
    // slot is empty.
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let bar = row_text(&term, 29);
    assert!(
        bar.contains("<you@gmail.com>"),
        "identity on the left: {bar:?}"
    );
    assert!(bar.contains("INBOX"), "mailbox on the left: {bar:?}");

    // A notification renders inline in the bar's right slot - not as a floating
    // toast box over the body (the body's last row is the bar itself).
    app.set_toast("Message sent", ToastKind::Success);
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    assert!(
        row_text(&term, 29).contains("Message sent"),
        "notification renders in the status bar"
    );

    // With no notification, an in-flight sync shows a determinate progress bar.
    app.toast = None;
    app.sync = Some(crate::tui::app::event::SyncProgress {
        account: app.active_account_name().to_string(),
        done: 3,
        total: 10,
    });
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    assert!(
        row_text(&term, 29).contains("Syncing 3/10"),
        "determinate sync bar renders in the status bar"
    );

    // Reaching the total clears the bar.
    app.on_sync_progress(crate::tui::app::event::SyncProgress {
        account: app.active_account_name().to_string(),
        done: 10,
        total: 10,
    });
    assert!(app.sync.is_none(), "completed sync clears the bar");
}

#[tokio::test]
async fn reader_render_is_memoized_across_scroll() {
    let mut app = test_app().await;
    // Open the message with a cached body (uid 1 is newest -> display index 0).
    app.selected_message = app
        .display_envelopes
        .iter()
        .position(|e| e.uid == 1)
        .unwrap();
    app.view = View::Reader;
    app.load_selected_preview().await;
    assert!(app.open_message.is_some(), "cached message opens");

    // Opening a message marks the render dirty; the first `prepare_reader` builds
    // the memoized lines and clears the flag.
    assert!(app.reader_dirty, "a new body marks the reader render dirty");
    app.prepare_reader(80);
    assert!(!app.reader_dirty, "prepare_reader clears the dirty flag");
    let body_text: String = app
        .reader_lines()
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(
        body_text.contains("roadmap sequencing"),
        "the cached body is rendered: {body_text:?}"
    );
    let cached_len = app.reader_lines().len();

    // Scrolling must not invalidate the cache: a repaint reuses the rendered
    // lines instead of re-parsing the whole body.
    app.reader_offset = 1;
    app.prepare_reader(80);
    assert!(!app.reader_dirty, "a scroll leaves the render cache intact");
    assert_eq!(
        app.reader_lines().len(),
        cached_len,
        "same memoized lines after scrolling"
    );

    // A width change rebuilds at the new wrap width (terminal width minus the
    // reader's two border columns).
    app.prepare_reader(40);
    assert_eq!(
        app.reader_cache.as_ref().unwrap().wrap_width,
        38,
        "wrap width follows the terminal width"
    );

    // Opening a different message re-marks the render dirty.
    app.set_open_message(None);
    assert!(
        app.reader_dirty,
        "swapping the open message re-dirties the render"
    );
}

#[test]
fn resolve_theme_uses_custom_colors() {
    let mut themes = HashMap::new();
    themes.insert("nord".to_string(), nord_spec());
    let (name, theme) = App::resolve_theme("nord", &themes);
    assert_eq!(name, "nord");
    assert_eq!(theme.bg, ratatui::style::Color::Rgb(0x2e, 0x34, 0x40));
    assert_eq!(theme.accent, ratatui::style::Color::Rgb(0x88, 0xc0, 0xd0));
}

#[test]
fn resolve_theme_reserves_builtin_names() {
    // A custom theme named "dark" must not override the built-in dark.
    let mut themes = HashMap::new();
    let mut evil = nord_spec();
    evil.bg = "#ffffff".into();
    themes.insert("dark".to_string(), evil);
    let (name, theme) = App::resolve_theme("dark", &themes);
    assert_eq!(name, "dark");
    assert_eq!(theme.bg, Theme::dark().bg);
}

#[test]
fn resolve_theme_unknown_falls_back_to_dark() {
    let themes = HashMap::new();
    let (name, theme) = App::resolve_theme("bogus", &themes);
    assert_eq!(name, "dark");
    assert_eq!(theme.bg, Theme::dark().bg);
}

#[test]
fn resolve_theme_bad_color_falls_back_per_role() {
    let mut themes = HashMap::new();
    let mut spec = nord_spec();
    spec.bg = "not-a-color".into();
    themes.insert("nord".to_string(), spec);
    let (_, theme) = App::resolve_theme("nord", &themes);
    // The bad bg falls back to the dark base; other roles still apply.
    assert_eq!(theme.bg, Theme::dark().bg);
    assert_eq!(theme.accent, ratatui::style::Color::Rgb(0x88, 0xc0, 0xd0));
}

#[tokio::test]
async fn app_new_applies_custom_theme_from_config() {
    let mut config = Config::default();
    config.ui.theme = "nord".to_string();
    config.themes.insert("nord".to_string(), nord_spec());
    let conn = database::open(Path::new(":memory:")).await.unwrap();
    let app = App::new(
        config,
        conn.clone(),
        vec![],
        crate::infrastructure::sqlite_services(conn.clone()),
    )
    .await;
    assert_eq!(app.theme_name(), "nord");
    assert_eq!(app.theme.bg, ratatui::style::Color::Rgb(0x2e, 0x34, 0x40));
}

#[tokio::test]
async fn border_type_config_changes_rendered_glyphs() {
    use ratatui::{Terminal, backend::TestBackend};

    // Render the same view with a plain vs a thick border and confirm the
    // heavy box-drawing glyph only appears with `border_type: thick`.
    async fn render_with(border_type: &str) -> String {
        let mut config = Config::default();
        config.ui.border_type = border_type.to_string();
        let conn = database::open(Path::new(":memory:")).await.unwrap();
        let app = App::new(
            config,
            conn.clone(),
            vec![],
            crate::infrastructure::sqlite_services(conn.clone()),
        )
        .await;
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    let plain = render_with("plain").await;
    let thick = render_with("thick").await;
    // '┏' (U+250F) is the heavy top-left corner used by BorderType::Thick.
    assert!(
        !plain.contains('┏'),
        "plain border must not use heavy glyphs"
    );
    assert!(thick.contains('┏'), "thick border must use heavy glyphs");
}

#[tokio::test]
async fn reader_word_wraps_long_body_lines() {
    use crate::domain::Message;
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    app.action(Action::OpenMessage).await;
    assert_eq!(app.view, View::Reader);

    // Replace the open message with one long body line (no newlines) that far
    // exceeds the pane width.
    let long = "lorem ipsum dolor sit amet ".repeat(12);
    let env = app.open_message.as_ref().unwrap().envelope.clone();
    app.open_message = Some(Message {
        envelope: env,
        body: long.clone(),
        raw_headers: None,
    });

    let w = 60u16;
    let mut term = Terminal::new(TestBackend::new(w, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();

    // No rendered row overflows the pane (inner width = w - 2 borders), and the
    // body is spread across multiple rows rather than truncated to one.
    let mut body_rows = 0usize;
    for y in 0..buf.area().height {
        let row: String = (0..buf.area().width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        if row.contains("lorem ipsum") {
            body_rows += 1;
        }
    }
    assert!(
        body_rows >= 3,
        "long body should wrap across several rows, got {body_rows}"
    );
}
