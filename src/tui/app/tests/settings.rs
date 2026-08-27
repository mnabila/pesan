use super::*;

#[tokio::test]
async fn shift_s_with_shift_modifier_opens_settings() {
    // Reproduces the real-terminal case: Shift+S arrives as Char('S') *with*
    // the SHIFT modifier set, which must still resolve to the settings action.
    let mut app = test_app().await;
    let ev = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('S'),
        crossterm::event::KeyModifiers::SHIFT,
    );
    app.on_key(&ev).await;
    assert_eq!(app.view, View::Settings);
}

#[tokio::test]
async fn account_form_field_is_modal_e_edits_esc_exits() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a')); // add account -> form (gmail: focus Provider)
    // j/k move between fields (not Tab). OAuth order: Provider -> Name -> ...
    send_key!(app, Key::ch('j'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    send_key!(app, Key::ch('k'));
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Provider
    );
    send_key!(app, Key::ch('j')); // back to Name for the modal-edit checks
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    // Not editing yet: a letter must NOT reach the field.
    send_key!(app, Key::ch('z'));
    assert!(!app.settings.as_ref().unwrap().field_editing);
    assert_eq!(
        app.settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .name
            .text(),
        ""
    );
    // e enters field edit; typing now lands in the field; Esc leaves field edit.
    send_key!(app, Key::ch('e'));
    assert!(app.settings.as_ref().unwrap().field_editing);
    for c in "Bee".chars() {
        send_key!(app, Key::ch(c));
    }
    // Backspace must not panic on the freshly-focused field (cursor was init'd).
    app.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Backspace,
        crossterm::event::KeyModifiers::NONE,
    ))
    .await;
    assert_eq!(
        app.settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .name
            .text(),
        "Be"
    );
    send_key!(app, Key::ch('e'));
    assert_eq!(
        app.settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .name
            .text(),
        "Bee"
    );
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().field_editing);
    assert!(app.settings.as_ref().unwrap().editing); // still in the form
    // A second Esc exits the form back to the accounts list.
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().editing);
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Accounts
    );
}

#[tokio::test]
async fn settings_form_accepts_typed_input_through_keys() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S')); // open settings
    send_key!(app, Key::ch('a')); // add account -> editing
    assert!(app.settings.as_ref().unwrap().editing);
    // gmail is an OAuth provider, so the form starts on Provider (email is
    // auto-filled by OAuth, not typed).
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Provider
    );
    // j moves to the Name field; fields are modal: `e` starts typing.
    send_key!(app, Key::ch('j'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    send_key!(app, Key::ch('e'));
    assert!(app.settings.as_ref().unwrap().field_editing);
    for c in "Work".chars() {
        send_key!(app, Key::ch(c));
    }
    assert_eq!(
        app.settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .name
            .text(),
        "Work"
    );
    // Esc leaves field-edit.
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().field_editing);
    // The OAuth field order is Provider -> Name -> Default -> Authorize.
    send_key!(app, Key::ch('j'));
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::IsDefault
    );
    send_key!(app, Key::ch('j'));
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Authorize
    );
}

#[tokio::test]
async fn settings_jk_moves_account_selection() {
    let mut app = test_app().await;
    // Seed two accounts so j/k has somewhere to move.
    for (i, name) in ["personal", "work"].iter().enumerate() {
        crate::infrastructure::database::accounts::upsert(
            &app.pool,
            &Account {
                id: None,
                name: (*name).into(),
                email: format!("{name}@gmail.com"),
                provider: "gmail".into(),
                keychain_ref: String::new(),
                is_default: i == 0,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    }
    app.reload_accounts().await;
    send_key!(app, Key::ch('S'));
    // The account manager opens focused on the accounts list.
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Accounts
    );
    let start = app.settings.as_ref().unwrap().selected;
    // j moves the selection down; k moves it back.
    send_key!(app, Key::ch('j'));
    assert_ne!(app.settings.as_ref().unwrap().selected, start);
    send_key!(app, Key::ch('k'));
    assert_eq!(app.settings.as_ref().unwrap().selected, start);
}

#[tokio::test]
async fn settings_account_filter_narrows_and_clears() {
    let mut app = test_app().await;
    app.action(Action::Settings).await;
    let mk = |name: &str, provider: &str| Account {
        id: None,
        name: name.into(),
        email: format!("{name}@x.io"),
        provider: provider.into(),
        keychain_ref: String::new(),
        is_default: false,
        created_at: 1,
    };
    {
        let s = app.settings.as_mut().unwrap();
        s.accounts = vec![
            mk("personal", "gmail"),
            mk("work", "outlook"),
            mk("side", "gmail"),
        ];
        s.selected = 0;
        s.focus = SettingsFocus::Accounts;
    }

    // `/` enters filter mode; typing narrows the list to the sole match and
    // moves the selection onto it.
    send_key!(app, Key::ch('/'));
    assert!(app.settings.as_ref().unwrap().filtering);
    for c in "work".chars() {
        send_key!(app, Key::ch(c));
    }
    {
        let s = app.settings.as_ref().unwrap();
        assert_eq!(s.filtered_accounts(), vec![1]);
        assert_eq!(s.selected, 1);
    }

    // Enter commits the filter and drops back to list navigation.
    send_key!(app, Key::enter());
    assert!(!app.settings.as_ref().unwrap().filtering);
    assert_eq!(app.settings.as_ref().unwrap().filtered_accounts(), vec![1]);

    // Re-enter and Esc clears the query, restoring every account.
    send_key!(app, Key::ch('/'));
    send_key!(app, Key::esc());
    let s = app.settings.as_ref().unwrap();
    assert!(s.account_filter.is_empty());
    assert!(!s.filtering);
    assert_eq!(s.filtered_accounts().len(), 3);
}

#[tokio::test]
async fn open_settings_and_back() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    assert_eq!(app.view, View::Settings);
    send_key!(app, Key::esc());
    assert_eq!(app.view, View::Main);
    assert!(app.settings.is_none());
}

#[tokio::test]
async fn settings_help_opens_in_nav_mode() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S')); // open settings / account manager
    assert_eq!(app.view, View::Settings);
    send_key!(app, Key::ch('?'));
    assert!(app.help_open, "? opens help in settings nav mode");
    send_key!(app, Key::esc());
    assert!(!app.help_open);
    assert_eq!(app.view, View::Settings, "closing help stays in settings");
}

#[tokio::test]
async fn settings_view_renders_without_panic() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    // List mode (left preferences, right accounts).
    let mut wide = Terminal::new(TestBackend::new(100, 30)).unwrap();
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    // Edit-form mode.
    send_key!(app, Key::ch('e'));
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    send_key!(app, Key::esc()); // leave edit form
    // Preferences section with an inline value change.
    send_key!(app, Key::ch('H')); // switch to preferences
    send_key!(app, Key::ch('l')); // cycle the focused value
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    // Cramped terminal must still lay out cleanly.
    let mut tiny = Terminal::new(TestBackend::new(24, 8)).unwrap();
    tiny.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
}

#[tokio::test]
async fn settings_o_opens_account_and_esc_exits() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    // Cursor starts on the accounts list.
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Accounts
    );
    // Enter does not open the edit form.
    send_key!(app, Key::enter());
    assert!(!app.settings.as_ref().unwrap().editing);
    // `o` opens the selected account for editing (its form is the detail view).
    send_key!(app, Key::ch('o'));
    assert!(app.settings.as_ref().unwrap().editing);
    // Esc exits edit mode (without closing settings).
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().editing);
    assert_eq!(app.view, View::Settings);
}

#[tokio::test]
async fn settings_save_creates_account() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    // a to add account
    send_key!(app, Key::ch('a'));
    assert!(app.settings.as_ref().unwrap().is_new);
    assert!(app.settings.as_ref().unwrap().editing);
    // fill name + email
    for ch in "Work".chars() {
        app.settings
            .as_mut()
            .unwrap()
            .form
            .as_mut()
            .unwrap()
            .name
            .handle(&key_ev(&Key::ch(ch)));
    }
    app.settings.as_mut().unwrap().focus = SettingsFocus::Email;
    for ch in "work@corp.io".chars() {
        app.settings
            .as_mut()
            .unwrap()
            .form
            .as_mut()
            .unwrap()
            .email
            .handle(&key_ev(&Key::ch(ch)));
    }
    // Move off the text field so Shift+W is read as the save command, not
    // typed into the input, then save.
    app.settings.as_mut().unwrap().focus = SettingsFocus::Provider;
    send_key!(app, Key::ch('W'));
    assert!(app.accounts.iter().any(|a| a.name == "Work"));
    // Saved: stays in settings, account list refreshed, form closed.
    assert_eq!(app.view, View::Settings);
    assert!(
        app.settings
            .as_ref()
            .unwrap()
            .accounts
            .iter()
            .any(|a| a.name == "Work")
    );
    assert!(!app.settings.as_ref().unwrap().editing);
}

#[tokio::test]
async fn settings_screen_is_account_manager_without_preferences() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    app.action(Action::Settings).await;
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let screen: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        screen.contains("Account Manager"),
        "settings screen should be titled Account Manager"
    );
    // The old preferences pane and its rows are gone.
    assert!(!screen.contains("Preferences"));
    assert!(!screen.contains("Keymap"));
}

#[tokio::test]
async fn account_filter_bar_renders_at_pane_bottom() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    // Seed an account so the list (and its header) has content above the bar.
    crate::infrastructure::database::accounts::upsert(
        &app.pool,
        &Account {
            id: None,
            name: "personal".into(),
            email: "you@gmail.com".into(),
            provider: "gmail".into(),
            keychain_ref: String::new(),
            is_default: true,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    app.reload_accounts().await;
    app.action(Action::Settings).await;
    send_key!(app, Key::ch('/')); // start filtering

    let (w, h) = (100u16, 20u16);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    // Reconstruct per-row strings to locate the filter bar vs the list header.
    let rows: Vec<String> = (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>())
        .collect();
    let filter_row = rows.iter().position(|r| r.contains("filter")).unwrap();
    let header_row = rows.iter().position(|r| r.contains("ACCOUNT")).unwrap();
    // The filter bar sits below the list header, in the lower third of the pane
    // (it used to render as the first row, above the header).
    assert!(
        filter_row > header_row,
        "filter bar should render below the ACCOUNT header, not above it"
    );
    assert!(
        filter_row >= (h as usize) * 2 / 3,
        "filter bar should be near the pane's bottom, got row {filter_row} of {h}"
    );
}
