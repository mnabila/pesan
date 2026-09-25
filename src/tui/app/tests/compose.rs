use super::*;

#[tokio::test]
async fn compose_inline_headers_and_external_body() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('c')); // open compose
    assert_eq!(app.view, View::Compose);

    // Focus starts on To; press i to edit, type, Enter to leave edit mode.
    send_key!(app, Key::ch('i'));
    assert!(app.compose.as_ref().unwrap().editing);
    for ch in "jane@acme.io".chars() {
        send_key!(app, Key::ch(ch));
    }
    send_key!(app, Key::enter());
    assert!(!app.compose.as_ref().unwrap().editing);
    // j down to Subject (To -> Bcc -> Subject in the 2-col grid), edit it.
    for _ in 0..2 {
        send_key!(app, Key::ch('j'));
    }
    assert_eq!(app.compose.as_ref().unwrap().focus, ComposeFocus::Subject);
    send_key!(app, Key::ch('i'));
    for ch in "Hi there".chars() {
        send_key!(app, Key::ch(ch));
    }
    send_key!(app, Key::enter());
    {
        let c = app.compose.as_ref().unwrap();
        assert_eq!(c.to.text(), "jane@acme.io");
        assert_eq!(c.subject.text(), "Hi there");
    }

    // Ctrl-e queues only the body (not headers) for the external editor.
    send_key!(app, Key::cc('e'));
    let pending = app.take_pending_external().expect("editor requested");
    assert!(!pending.current_text.starts_with("To:"));
    app.apply_external_result(Some("Hello body.".to_string()));
    assert_eq!(app.compose.as_ref().unwrap().body, "Hello body.");
}

#[tokio::test]
async fn compose_help_opens_in_nav_mode_but_not_while_editing() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('c')); // open compose (nav mode, focus To)
    assert_eq!(app.view, View::Compose);
    assert!(!app.compose.as_ref().unwrap().editing);

    // `?` in nav mode opens the keymap overlay.
    send_key!(app, Key::ch('?'));
    assert!(app.help_open, "? opens help in compose nav mode");

    // Esc closes it and leaves the compose view intact.
    send_key!(app, Key::esc());
    assert!(!app.help_open);
    assert_eq!(app.view, View::Compose);

    // While editing a field, `?` is a literal character, not a help trigger.
    send_key!(app, Key::ch('i'));
    assert!(app.compose.as_ref().unwrap().editing);
    send_key!(app, Key::ch('?'));
    assert!(!app.help_open, "? types into the field while editing");
    assert_eq!(app.compose.as_ref().unwrap().to.text(), "?");
}

#[tokio::test]
async fn compose_help_overlay_lists_compose_and_global_keys() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    send_key!(app, Key::ch('c')); // compose
    send_key!(app, Key::ch('?')); // help
    assert!(app.help_open);
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer();
    let text: String = (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buf[(x, y)].symbol().to_string())
        .collect();
    assert!(text.contains("compose"), "help titled for compose ctx");
    assert!(text.contains("send"), "compose keys shown");
    assert!(text.contains("toggle help"), "global column shown too");
}

#[tokio::test]
async fn compose_send_offline_reports_error_and_keeps_compose() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('c'));
    assert_eq!(app.view, View::Compose);
    let draft = app.compose.as_ref().unwrap().to_draft();
    assert!(!draft.is_valid(), "empty new compose is not valid");
    // Fill the To (focus starts here) and Subject fields, then the body via
    // the external editor. Edit each field with i ... Enter.
    send_key!(app, Key::ch('i'));
    for ch in "test@example.com".chars() {
        send_key!(app, Key::ch(ch));
    }
    send_key!(app, Key::enter());
    for _ in 0..2 {
        send_key!(app, Key::ch('j')); // -> Subject (To -> Bcc -> Subject)
    }
    send_key!(app, Key::ch('i'));
    for ch in "Hello".chars() {
        send_key!(app, Key::ch(ch));
    }
    send_key!(app, Key::enter());
    app.apply_external_result(Some("body".to_string()));
    app.action(Action::Send).await;
    // The active account is offline (CacheSource, no live IMAP/SMTP), so the
    // send fails: the compose stays open and an error toast explains why.
    assert_eq!(app.view, View::Compose);
    assert!(app.compose.is_some());
    let toast = app.toasts.last().expect("send-failure toast");
    assert_eq!(toast.kind, ToastKind::Error);
}

#[tokio::test]
async fn compose_view_renders_without_panic() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    send_key!(app, Key::ch('c'));
    // Exercise the attachment-list path and body focus.
    if let Some(c) = &mut app.compose {
        c.attachments.push(Attachment {
            path: "/tmp/report.pdf".into(),
            filename: "report.pdf".into(),
            size: 320_000,
        });
        c.focus = ComposeFocus::Body;
    }

    // A roomy terminal and a cramped one both must lay out cleanly.
    let mut wide = Terminal::new(TestBackend::new(100, 30)).unwrap();
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let mut tiny = Terminal::new(TestBackend::new(20, 6)).unwrap();
    tiny.draw(|f| crate::tui::views::draw(f, &app)).unwrap();

    // The discard confirm must render *over* the compose view (Esc in nav
    // mode opens it). This is the regression that made Esc-discard appear
    // to do nothing.
    send_key!(app, Key::esc());
    assert!(app.confirm.is_some());
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
}

#[tokio::test]
async fn compose_jk_navigates_and_i_edits_headers() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('c'));
    // Nav mode: focus starts on To, not editing.
    assert_eq!(app.compose.as_ref().unwrap().focus, ComposeFocus::To);
    assert!(!app.compose.as_ref().unwrap().editing);

    // Typing in nav mode must NOT edit the field.
    send_key!(app, Key::ch('z'));
    assert_eq!(app.compose.as_ref().unwrap().to.text(), "");

    // h/l move left/right, j/k move up/down within the 2-column grid.
    send_key!(app, Key::ch('l')); // To -> Cc (right)
    assert_eq!(app.compose.as_ref().unwrap().focus, ComposeFocus::Cc);
    send_key!(app, Key::ch('h')); // Cc -> To (left)
    assert_eq!(app.compose.as_ref().unwrap().focus, ComposeFocus::To);
    send_key!(app, Key::ch('j')); // To -> Bcc (down)
    assert_eq!(app.compose.as_ref().unwrap().focus, ComposeFocus::Bcc);
    send_key!(app, Key::ch('k')); // Bcc -> To (up)
    assert_eq!(app.compose.as_ref().unwrap().focus, ComposeFocus::To);

    // i enters edit mode; now keys type into the field; Esc leaves.
    send_key!(app, Key::ch('i'));
    assert!(app.compose.as_ref().unwrap().editing);
    for ch in "a@b.co".chars() {
        send_key!(app, Key::ch(ch));
    }
    assert_eq!(app.compose.as_ref().unwrap().to.text(), "a@b.co");
    send_key!(app, Key::esc());
    assert!(!app.compose.as_ref().unwrap().editing);
    // Esc in nav mode discards (opens the confirm dialog).
    send_key!(app, Key::esc());
    assert!(app.confirm.is_some());
}

#[tokio::test]
async fn compose_body_scrolls_and_clamps() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('c'));
    // A five-line body, focused for review.
    if let Some(c) = &mut app.compose {
        c.body = "l1\nl2\nl3\nl4\nl5".to_string();
        c.focus = ComposeFocus::Body;
    }
    // Shift+J/K scroll the body; can't go below 0.
    send_key!(app, Key::ch('J'));
    send_key!(app, Key::ch('J'));
    assert_eq!(app.compose.as_ref().unwrap().body_scroll, 2);
    send_key!(app, Key::ch('K'));
    assert_eq!(app.compose.as_ref().unwrap().body_scroll, 1);
    // G jumps to the last line; J past the end clamps there.
    send_key!(app, Key::ch('G'));
    assert_eq!(app.compose.as_ref().unwrap().body_scroll, 4);
    send_key!(app, Key::ch('J'));
    assert_eq!(app.compose.as_ref().unwrap().body_scroll, 4);
    // Home returns to the top.
    send_key!(app, Key::home());
    assert_eq!(app.compose.as_ref().unwrap().body_scroll, 0);
    // j/k move between rows even from the body, so you can leave it.
    send_key!(app, Key::ch('k'));
    assert_eq!(app.compose.as_ref().unwrap().focus, ComposeFocus::Attach);
}
