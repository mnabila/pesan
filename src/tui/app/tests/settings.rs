use super::*;


#[tokio::test]
async fn shift_s_with_modifier_opens_settings() {
    let mut app = test_app().await;
    let ev = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('S'),
        crossterm::event::KeyModifiers::SHIFT,
    );
    app.on_key(&ev).await;
    assert_eq!(app.view, View::Settings);
}

#[tokio::test]
async fn account_form_field_modal_i_edits_esc_exits() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a'));
    send_key!(app, Key::enter());
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Authorize);

    // j/k navigate fields
    send_key!(app, Key::ch('j'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    send_key!(app, Key::ch('j'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::IsDefault);
    send_key!(app, Key::ch('k'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Name);

    // Not editing: key doesn't reach field
    send_key!(app, Key::ch('z'));
    assert!(!app.settings.as_ref().unwrap().field_editing);
    assert_eq!(app.settings.as_ref().unwrap().form.as_ref().unwrap().name.text(), "");

    // i enters edit mode
    send_key!(app, Key::ch('i'));
    assert!(app.settings.as_ref().unwrap().field_editing);
    for c in "Bee".chars() {
        send_key!(app, Key::ch(c));
    }
    app.on_key(&crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Backspace,
        crossterm::event::KeyModifiers::NONE,
    ))
    .await;
    assert_eq!(app.settings.as_ref().unwrap().form.as_ref().unwrap().name.text(), "Be");
    send_key!(app, Key::ch('e'));
    assert_eq!(app.settings.as_ref().unwrap().form.as_ref().unwrap().name.text(), "Bee");

    // Esc exits field edit
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().field_editing);
    assert!(app.settings.as_ref().unwrap().editing);

    // Second Esc exits form
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().editing);
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Accounts);
}

#[tokio::test]
async fn settings_form_accepts_typed_input() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a'));
    send_key!(app, Key::enter());
    assert!(app.settings.as_ref().unwrap().editing);
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Authorize);

    send_key!(app, Key::ch('j'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    send_key!(app, Key::ch('i'));
    assert!(app.settings.as_ref().unwrap().field_editing);
    for c in "Work".chars() {
        send_key!(app, Key::ch(c));
    }
    assert_eq!(app.settings.as_ref().unwrap().form.as_ref().unwrap().name.text(), "Work");
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().field_editing);

    send_key!(app, Key::ch('j'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::IsDefault);
    send_key!(app, Key::ch('j'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Authorize);
}

#[tokio::test]
async fn settings_jk_moves_account_selection() {
    let mut app = test_app().await;
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
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Accounts);
    let start = app.settings.as_ref().unwrap().selected;
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
        s.accounts = vec![mk("personal", "gmail"), mk("work", "outlook"), mk("side", "gmail")];
        s.selected = 0;
        s.focus = SettingsFocus::Accounts;
    }

    send_key!(app, Key::ch('/'));
    assert!(app.settings.as_ref().unwrap().filtering);
    for c in "work".chars() {
        send_key!(app, Key::ch(c));
    }
    assert_eq!(app.settings.as_ref().unwrap().filtered_accounts(), vec![1]);
    assert_eq!(app.settings.as_ref().unwrap().selected, 1);

    send_key!(app, Key::enter());
    assert!(!app.settings.as_ref().unwrap().filtering);
    assert_eq!(app.settings.as_ref().unwrap().filtered_accounts(), vec![1]);

    send_key!(app, Key::ch('/'));
    send_key!(app, Key::esc());
    assert!(app.settings.as_ref().unwrap().account_filter.is_empty());
    assert!(!app.settings.as_ref().unwrap().filtering);
    assert_eq!(app.settings.as_ref().unwrap().filtered_accounts().len(), 3);
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
    send_key!(app, Key::ch('S'));
    assert_eq!(app.view, View::Settings);
    send_key!(app, Key::ch('?'));
    assert!(app.help_open);
    send_key!(app, Key::esc());
    assert!(!app.help_open);
    assert_eq!(app.view, View::Settings);
}

#[tokio::test]
async fn settings_view_renders_without_panic() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    let mut wide = Terminal::new(TestBackend::new(100, 30)).unwrap();
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    send_key!(app, Key::ch('i'));
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    send_key!(app, Key::esc());
    send_key!(app, Key::ch('H'));
    send_key!(app, Key::ch('l'));
    wide.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let mut tiny = Terminal::new(TestBackend::new(24, 8)).unwrap();
    tiny.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
}

#[tokio::test]
async fn settings_o_opens_account_and_esc_exits() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    assert_eq!(app.settings.as_ref().unwrap().focus, SettingsFocus::Accounts);
    send_key!(app, Key::enter());
    assert!(!app.settings.as_ref().unwrap().editing);
    send_key!(app, Key::ch('o'));
    assert!(app.settings.as_ref().unwrap().editing);
    send_key!(app, Key::esc());
    assert!(!app.settings.as_ref().unwrap().editing);
    assert_eq!(app.view, View::Settings);
}

#[tokio::test]
async fn settings_save_creates_account() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a'));
    send_key!(app, Key::enter());
    assert!(app.settings.as_ref().unwrap().is_new);
    assert!(app.settings.as_ref().unwrap().editing);
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
    app.settings.as_mut().unwrap().focus = SettingsFocus::Provider;
    send_key!(app, Key::ch('W'));
    assert!(app.accounts.iter().any(|a| a.name == "Work"));
    assert_eq!(app.view, View::Settings);
    assert!(app.settings.as_ref().unwrap().accounts.iter().any(|a| a.name == "Work"));
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
    assert!(screen.contains("Account Manager"));
    assert!(!screen.contains("Preferences"));
    assert!(!screen.contains("Keymap"));
}

#[tokio::test]
async fn account_filter_bar_renders_at_pane_bottom() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
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
    send_key!(app, Key::ch('/'));

    let (w, h) = (100u16, 20u16);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    let rows: Vec<String> = (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    let filter_row = rows.iter().position(|r| r.contains("filter")).unwrap();
    let header_row = rows.iter().position(|r| r.contains("ACCOUNT")).unwrap();
    assert!(filter_row > header_row);
    assert!(filter_row >= (h as usize) * 2 / 3);
}
