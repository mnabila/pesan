use super::*;

#[tokio::test]
async fn quits_on_q() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('q'));
    assert!(app.should_quit);
}

#[tokio::test]
async fn refresh_key_on_offline_account_attempts_reconnect() {
    let mut app = test_app().await;
    // The test harness serves a cached (offline) account, so pressing R can't
    // fetch live - it should fall back to a reconnect attempt, never panic.
    send_key!(app, Key::ch('R'));
    let toast = app.toasts.last().expect("refresh toast");
    let text = toast.text.to_lowercase();
    assert!(
        text.contains("reconnect") || text.contains("offline"),
        "unexpected refresh toast: {}",
        toast.text
    );
}

#[tokio::test]
async fn no_account_shows_empty_not_fixtures() {
    // With no accounts and no cache, the app shows nothing (EmptySource) -
    // there are no mock fixtures anymore.
    let conn = database::open(Path::new(":memory:")).await.unwrap();
    let app = App::new(
        Config::default(),
        conn.clone(),
        vec![],
        crate::infrastructure::sqlite_services(conn.clone()),
    )
    .await;
    assert!(app.folders.is_empty(), "no fake folders without an account");
    assert!(app.envelopes.is_empty(), "no fake mail without an account");
    assert!(app.open_message.is_none());
}

#[tokio::test]
async fn h_toggles_full_headers_in_reader() {
    let mut app = test_app().await;
    app.action(Action::OpenMessage).await;
    assert_eq!(app.view, View::Reader);
    assert!(!app.show_headers);
    send_key!(app, Key::ch('h'));
    assert!(app.show_headers, "h should turn full headers on");
    send_key!(app, Key::ch('h'));
    assert!(!app.show_headers, "h should turn full headers off again");
}

#[tokio::test]
async fn loads_inbox_from_cache() {
    let app = test_app().await;
    assert!(!app.folders.is_empty());
    assert_eq!(app.selected_folder_name(), "INBOX");
    assert!(!app.display_envelopes.is_empty());
    // Bodies load only when a message is opened, so nothing is fetched at startup.
    assert!(app.open_message.is_none());
}

#[tokio::test]
async fn move_down_selects_next_message() {
    let mut app = test_app().await;
    let first = app.selected_message;
    send_key!(app, Key::ch('j'));
    assert_eq!(app.selected_message, first + 1);
    assert_ne!(app.selected_message, first);
}

#[tokio::test]
async fn folders_collapse_hides_subtree_and_skips_it() {
    let mut app = test_app().await;
    app.focus = Pane::Folders;
    let all = app.folders.len();
    assert_eq!(app.visible_folder_indices().len(), all);

    // Select "Work", the parent of Work/projectA and Work/projectB.
    let work = app.folders.iter().position(|f| f.name == "Work").unwrap();
    app.load_folder(work).await;
    assert!(app.folder_has_children(work));
    assert_eq!(app.visible_folder_indices().len(), all);

    // Collapse with 'h': subfolders disappear from the visible list.
    send_key!(app, Key::ch('h'));
    assert!(app.folder_collapsed[work]);
    let visible = app.visible_folder_indices();
    assert_eq!(visible.len(), all - 2);
    assert!(!visible.contains(&(work + 1)));
    assert!(!visible.contains(&(work + 2)));

    // Moving down now skips the hidden children and lands on "Personal".
    send_key!(app, Key::ch('j'));
    assert_eq!(app.selected_folder_name(), "Personal");

    // Move back up to the parent and expand with 'l': children return.
    send_key!(app, Key::ch('k'));
    assert_eq!(app.selected_folder_name(), "Work");
    send_key!(app, Key::ch('l'));
    assert!(!app.folder_collapsed[work]);
    assert_eq!(app.visible_folder_indices().len(), all);
}

#[tokio::test]
async fn connection_lost_reconnects_and_restores_reader() {
    let mut app = test_app().await;

    // Pretend we're live and the user is reading message uid 1 (the one with a
    // cached body), which is the newest so it sits at display index 0.
    app.live = true;
    app.selected_message = app
        .display_envelopes
        .iter()
        .position(|e| e.uid == 1)
        .unwrap();
    app.view = View::Reader;
    app.load_selected_preview().await;
    assert!(app.open_message.is_some(), "message opened before the drop");

    // The live session wedges: we go offline and remember what was on screen.
    app.on_connection_lost("personal".to_string()).await;
    assert!(!app.live, "drops offline while reconnecting");
    let ctx = app.reconnect.as_ref().expect("reconnect context captured");
    assert_eq!(ctx.select_uid, Some(1));
    assert_eq!(ctx.open_uid, Some(1));
    assert!(ctx.was_reading);

    // Simulate the background reconnect finishing successfully.
    let data = crate::tui::app::event::LiveData {
        source: Box::new(crate::infrastructure::mail::empty::EmptySource::new()),
        folders: app.folders.clone(),
        folder: "INBOX".to_string(),
        envelopes: app.envelopes.clone(),
    };
    app.on_connected(crate::tui::app::event::Connected {
        account: "personal".to_string(),
        result: Ok(data),
    })
    .await;

    assert!(app.live, "live again after reconnect");
    assert!(app.reconnect.is_none(), "reconnect context consumed");
    assert_eq!(
        app.display_envelopes[app.selected_message].uid, 1,
        "highlighted message restored by uid"
    );
    assert_eq!(app.view, View::Reader, "still in the reader");
    assert_eq!(
        app.open_message.as_ref().map(|m| m.envelope.uid),
        Some(1),
        "the open message was transparently reloaded"
    );
}

#[tokio::test]
async fn connection_lost_ignored_for_inactive_or_offline() {
    let mut app = test_app().await;

    // Offline already: nothing to reconnect.
    assert!(!app.live);
    app.on_connection_lost("personal".to_string()).await;
    assert!(app.reconnect.is_none(), "no reconnect when already offline");

    // Live, but the event is for a different account: ignored.
    app.live = true;
    app.on_connection_lost("someone-else".to_string()).await;
    assert!(
        app.reconnect.is_none(),
        "no reconnect for an inactive account"
    );
    assert!(app.live, "stays live");
}

#[tokio::test]
async fn narrow_collapse_tracks_terminal_width() {
    let mut app = test_app().await;
    assert!(!app.narrow);
    assert_eq!(app.active_pane(), Pane::List);

    // Shrink below the threshold: collapse engages and the visible pane
    // inherits the previous wide focus.
    app.set_viewport_width(NARROW_WIDTH - 1);
    assert!(app.narrow);
    assert_eq!(app.narrow_pane, Pane::List);
    assert_eq!(app.active_pane(), Pane::List);

    // Focus cycling in narrow mode moves the single visible pane (only the
    // sidebar and list remain now that the reader is a separate view).
    app.action(Action::FocusNext).await;
    assert_eq!(app.narrow_pane, Pane::Folders);

    // Grow back: wide focus inherits the last-visible narrow pane.
    app.set_viewport_width(NARROW_WIDTH);
    assert!(!app.narrow);
    assert_eq!(app.focus, Pane::Folders);
}

#[tokio::test]
async fn on_new_mail_prepends_dedups_and_bumps_unread() {
    let mut app = test_app().await;
    app.config.notifications.enabled = false; // no D-Bus in tests
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
        flags: crate::domain::Flags {
            seen: false,
            flagged: false,
        },
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
        app.folders
            .iter()
            .find(|f| f.name == "INBOX")
            .unwrap()
            .unread,
        inbox_unread + 1
    );

    // A repeat of the same uid is ignored (dedup).
    app.on_new_mail(NewMail {
        account,
        folder: "INBOX".to_string(),
        envelopes: vec![env],
    })
    .await;
    assert_eq!(app.envelopes.len(), before + 1);
}

#[tokio::test]
async fn on_new_mail_ignores_other_accounts() {
    let mut app = test_app().await;
    app.config.notifications.enabled = false;
    let before = app.envelopes.len();
    let env = Envelope {
        uid: 424_242,
        flags: crate::domain::Flags::default(),
        from: Address::new(None, "x@y.io"),
        subject: "nope".to_string(),
        date: 1,
        has_attachment: false,
        snippet: None,
        message_id: None,
    };
    app.on_new_mail(NewMail {
        account: "some-other-account".to_string(),
        folder: "INBOX".to_string(),
        envelopes: vec![env],
    })
    .await;
    assert_eq!(app.envelopes.len(), before);
}

#[tokio::test]
async fn pane_motion_with_h_l() {
    let mut app = test_app().await;
    assert_eq!(app.active_pane(), Pane::List); // default focus
    send_key!(app, Key::ch('L')); // right -> Folders (only sidebar + list now)
    assert_eq!(app.active_pane(), Pane::Folders);
    send_key!(app, Key::ch('H')); // left -> List
    assert_eq!(app.active_pane(), Pane::List);
    send_key!(app, Key::ch('H')); // left -> Folders
    assert_eq!(app.active_pane(), Pane::Folders);
    send_key!(app, Key::ch('L')); // right -> List
    assert_eq!(app.active_pane(), Pane::List);
}

#[tokio::test]
async fn o_and_enter_open_message_full_screen() {
    let mut app = test_app().await;
    // From the message list, `o` opens the selected message in the full-screen
    // reader and fetches its body (once).
    assert_eq!(app.view, View::Main);
    assert!(app.open_message.is_none());
    send_key!(app, Key::ch('o'));
    assert_eq!(app.view, View::Reader);
    assert!(app.open_message.is_some());
    // Esc returns to the list.
    send_key!(app, Key::esc());
    assert_eq!(app.view, View::Main);
    // Enter also opens the message.
    send_key!(app, Key::enter());
    assert_eq!(app.view, View::Reader);
}

#[tokio::test]
async fn tab_no_longer_switches_panes() {
    let mut app = test_app().await;
    let before = app.active_pane();
    send_key!(app, Key::tab());
    assert_eq!(app.active_pane(), before, "Tab must not move pane focus");
}

#[tokio::test]
async fn o_opens_mailbox_and_focuses_list() {
    let mut app = test_app().await;
    app.focus = Pane::Folders;
    // Cursor starts on the INBOX folder; `o` opens it and jumps to the list.
    send_key!(app, Key::ch('o'));
    assert_eq!(app.active_pane(), Pane::List);
    // Enter no longer opens the mailbox from the sidebar.
    app.focus = Pane::Folders;
    send_key!(app, Key::enter());
    assert_eq!(app.active_pane(), Pane::Folders);
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
