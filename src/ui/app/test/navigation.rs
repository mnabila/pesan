use super::*;
use crate::mail;
use crate::mail::infrastructure::empty;
use crate::ui::app::event;
use crate::wiring;

#[tokio::test]
async fn quit_and_refresh() {
    // q quits
    let mut app = test_app().await;
    send_key!(app, Key::ch('q'));
    assert!(app.should_quit);

    // R on offline account attempts reconnect
    let mut app2 = test_app().await;
    send_key!(app2, Key::ch('R'));
    let toast = app2.toasts.last().expect("refresh toast");
    let text = toast.text.to_lowercase();
    assert!(
        text.contains("reconnect") || text.contains("offline"),
        "unexpected: {}",
        toast.text
    );
}

#[tokio::test]
async fn no_account_shows_empty() {
    let conn = database::open(Path::new(":memory:")).await.unwrap();
    let app = App::new(
        Config::default(),
        vec![],
        wiring::sqlite_services(conn.clone()),
    )
    .await;
    assert!(app.folders.is_empty());
    assert!(app.envelopes.is_empty());
    assert!(app.open_message.is_none());
}

#[tokio::test]
async fn message_list_navigation() {
    let mut app = test_app().await;
    let first = app.selected_message;
    send_key!(app, Key::ch('j'));
    assert_eq!(app.selected_message, first + 1);
    send_key!(app, Key::ch('k'));
    assert_eq!(app.selected_message, first);
}

#[tokio::test]
async fn folder_collapse_expand() {
    let mut app = test_app().await;
    app.focus = Pane::Folders;
    let all = app.folders.len();
    assert_eq!(app.visible_folder_indices().len(), all);

    let work = app.folders.iter().position(|f| f.name == "Work").unwrap();
    app.load_folder(work).await;
    assert!(app.folder_has_children(work));

    // Collapse
    send_key!(app, Key::ch('h'));
    assert!(app.folder_collapsed[work]);
    assert_eq!(app.visible_folder_indices().len(), all - 2);

    // Move down skips hidden children
    send_key!(app, Key::ch('j'));
    assert_eq!(app.selected_folder_name(), "Personal");

    // Expand
    send_key!(app, Key::ch('k'));
    send_key!(app, Key::ch('l'));
    assert!(!app.folder_collapsed[work]);
    assert_eq!(app.visible_folder_indices().len(), all);
}

#[tokio::test]
async fn connection_lost_reconnect_restores_reader() {
    let mut app = test_app().await;
    app.live = true;
    app.selected_message = app
        .display_envelopes
        .iter()
        .position(|e| e.uid == 1)
        .unwrap();
    app.view = View::Reader;
    app.load_selected_preview().await;
    assert!(app.open_message.is_some());

    app.on_connection_lost("personal".to_string()).await;
    assert!(!app.live);
    let ctx = app.reconnect.as_ref().expect("reconnect context");
    assert_eq!(ctx.select_uid, Some(1));
    assert_eq!(ctx.open_uid, Some(1));
    assert!(ctx.was_reading);

    let data = event::LiveData {
        source: Box::new(empty::EmptySource::new()),
        folders: app.folders.clone(),
        folder: "INBOX".to_string(),
        envelopes: app.envelopes.clone(),
    };
    app.on_connected(event::Connected {
        account: "personal".to_string(),
        result: Ok(data),
    })
    .await;

    assert!(app.live);
    assert!(app.reconnect.is_none());
    assert_eq!(app.display_envelopes[app.selected_message].uid, 1);
    assert_eq!(app.view, View::Reader);
    assert_eq!(app.open_message.as_ref().map(|m| m.envelope.uid), Some(1));
}

#[tokio::test]
async fn connection_lost_ignored_when_offline_or_wrong_account() {
    let mut app = test_app().await;
    app.on_connection_lost("personal".to_string()).await;
    assert!(app.reconnect.is_none());

    app.live = true;
    app.on_connection_lost("someone-else".to_string()).await;
    assert!(app.reconnect.is_none());
    assert!(app.live);
}

#[tokio::test]
async fn narrow_mode_tracks_width() {
    let mut app = test_app().await;
    assert!(!app.narrow);
    assert_eq!(app.active_pane(), Pane::List);

    app.set_viewport_width(NARROW_WIDTH - 1);
    assert!(app.narrow);
    assert_eq!(app.narrow_pane, Pane::List);
    assert_eq!(app.active_pane(), Pane::List);

    app.action(Action::FocusNext).await;
    assert_eq!(app.narrow_pane, Pane::Folders);

    app.set_viewport_width(NARROW_WIDTH);
    assert!(!app.narrow);
    assert_eq!(app.focus, Pane::Folders);
}

#[tokio::test]
async fn new_mail_prepends_dedups_and_bumps_unread() {
    let mut app = test_app().await;
    app.config.notifications.enabled = false;
    let account = app.active_account_name().to_string();
    let before = app.envelopes.len();
    let inbox_unread = app
        .folders
        .iter()
        .find(|f| f.name == "INBOX")
        .unwrap()
    .unread;

    let env = Envelope {
        uid: 999_999,
        flags: mail::Flags { seen: false, flagged: false },
        from: Address::new(Some("New Sender".to_string()), "new@x.io"),
        subject: "Fresh arrival".to_string(),
        date: 9_999_999_999,
        has_attachment: false,
        snippet: None,
        message_id: None,
    };
    app.on_new_mail(NewMail {
        account: account.clone(),
        folder: "INBOX".to_string(),
        envelopes: vec![env.clone()],
    })
    .await;
    assert_eq!(app.envelopes.len(), before + 1);
    assert_eq!(app.display_envelopes.first().map(|e| e.uid), Some(999_999));
    assert_eq!(
        app.folders.iter().find(|f| f.name == "INBOX").unwrap().unread,
        inbox_unread + 1
    );

    // Dedup
    app.on_new_mail(NewMail {
        account,
        folder: "INBOX".to_string(),
        envelopes: vec![env],
    })
    .await;
    assert_eq!(app.envelopes.len(), before + 1);
}

#[tokio::test]
async fn new_mail_ignores_other_accounts() {
    let mut app = test_app().await;
    app.config.notifications.enabled = false;
    let before = app.envelopes.len();
    app.on_new_mail(NewMail {
        account: "some-other-account".to_string(),
        folder: "INBOX".to_string(),
        envelopes: vec![Envelope {
            uid: 424_242,
            flags: mail::Flags::default(),
            from: Address::new(None, "x@y.io"),
            subject: "nope".to_string(),
            date: 1,
            has_attachment: false,
            snippet: None,
            message_id: None,
        }],
    })
    .await;
    assert_eq!(app.envelopes.len(), before);
}

#[tokio::test]
async fn pane_motion_hl() {
    let mut app = test_app().await;
    assert_eq!(app.active_pane(), Pane::List);
    send_key!(app, Key::ch('L'));
    assert_eq!(app.active_pane(), Pane::Folders);
    send_key!(app, Key::ch('H'));
    assert_eq!(app.active_pane(), Pane::List);
    send_key!(app, Key::ch('H'));
    assert_eq!(app.active_pane(), Pane::Folders);
    send_key!(app, Key::ch('L'));
    assert_eq!(app.active_pane(), Pane::List);
}

#[tokio::test]
async fn open_message_keys() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('o'));
    assert_eq!(app.view, View::Reader);
    assert!(app.open_message.is_some());
    send_key!(app, Key::esc());
    assert_eq!(app.view, View::Main);
    send_key!(app, Key::enter());
    assert_eq!(app.view, View::Reader);
}

#[tokio::test]
async fn tab_does_not_switch_panes() {
    let mut app = test_app().await;
    let before = app.active_pane();
    send_key!(app, Key::tab());
    assert_eq!(app.active_pane(), before);
}

#[tokio::test]
async fn o_from_folders_opens_mailbox() {
    let mut app = test_app().await;
    app.focus = Pane::Folders;
    send_key!(app, Key::ch('o'));
    assert_eq!(app.active_pane(), Pane::List);
    send_key!(app, Key::enter());
    assert_eq!(app.active_pane(), Pane::List);
}

#[tokio::test]
async fn help_toggles() {
    let mut app = test_app().await;
    assert!(!app.help_open);
    send_key!(app, Key::ch('?'));
    assert!(app.help_open);
    send_key!(app, Key::ch('?'));
    assert!(!app.help_open);
}

#[tokio::test]
async fn search_box_captures_global_hotkeys() {
    let mut app = test_app().await;
    app.focus = Pane::List;
    send_key!(app, Key::ch('/'));
    assert!(app.search.is_some());
    for ch in "zqRS".chars() {
        send_key!(app, Key::ch(ch));
    }
    assert_eq!(app.search.as_ref().unwrap().input.text(), "zqRS");
    assert!(!app.should_quit);
    assert!(!app.sidebar_collapsed);
    assert_eq!(app.view, View::Main);
    send_key!(app, Key::esc());
    assert!(app.search.is_none());
}

#[tokio::test]
async fn h_toggles_full_headers_in_reader() {
    let mut app = test_app().await;
    app.action(Action::OpenMessage).await;
    assert!(!app.show_headers);
    send_key!(app, Key::ch('h'));
    assert!(app.show_headers);
    send_key!(app, Key::ch('h'));
    assert!(!app.show_headers);
}

#[tokio::test]
async fn loads_inbox_from_cache() {
    let app = test_app().await;
    assert!(!app.folders.is_empty());
    assert_eq!(app.selected_folder_name(), "INBOX");
    assert!(!app.display_envelopes.is_empty());
    assert!(app.open_message.is_none());
}
