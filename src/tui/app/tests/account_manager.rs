use super::*;
use crate::application::account::{keychain_ref_for, keychain_ref_for_password};

/// Deliver a failed connect result for the active account.
async fn fail_connect(app: &mut App, err: crate::tui::app::event::ConnectError) {
    app.on_connected(crate::tui::app::event::Connected {
        account: app.active_account_name().to_string(),
        result: Err(err),
    })
    .await;
}

/// Give the gmail provider dummy OAuth credentials so `ResolvedOAuth::
/// from_config` accepts it and the auto re-authorization path can arm.
fn with_test_oauth(app: &mut App) {
    let provider = app.config.providers.get_mut("gmail").unwrap();
    if let Some(oauth) = provider.oauth.as_mut() {
        oauth.client_id = "test-client".into();
        oauth.client_secret = "test-secret".into();
    }
}

/// Build a queued OAuth request + an opened flow for the active account.
fn open_paste_flow(app: &mut App) {
    let account = app.accounts[app.active_account].clone();
    let req = PendingOAuth {
        oauth: ResolvedOAuth {
            auth_url: "https://a".into(),
            token_url: "https://t".into(),
            scopes: vec![],
            client_id: "cid".into(),
            client_secret: String::new(),
        },
        account,
        set_default: false,
    };
    let flow = crate::application::oauth::AuthCodeFlow {
        authorize_url: "https://consent.example/auth".into(),
        redirect_uri: "http://localhost".into(),
        pkce_verifier: "verifier".into(),
        csrf_state: "state".into(),
    };
    app.begin_oauth_paste(req, flow);
}

#[tokio::test]
async fn oauth_reauth_flows() {
    // Auth failure queues re-auth once per session
    let mut app = test_app().await;
    with_test_oauth(&mut app);
    fail_connect(
        &mut app,
        crate::tui::app::event::ConnectError::Auth("connect: XOAUTH2 failed".into()),
    )
    .await;
    assert!(app.oauth_in_progress);
    assert!(app.pending_oauth.is_some());
    let _ = app.take_pending_oauth();
    app.oauth_in_progress = false;

    // Second auth failure in same session does NOT queue again
    fail_connect(
        &mut app,
        crate::tui::app::event::ConnectError::Auth("connect: IMAP LOGIN failed".into()),
    )
    .await;
    assert!(app.pending_oauth.is_none());

    // Network failure does not queue re-auth
    let mut app2 = test_app().await;
    with_test_oauth(&mut app2);
    fail_connect(
        &mut app2,
        crate::tui::app::event::ConnectError::Other("connect: timed out".into()),
    )
    .await;
    assert!(app2.pending_oauth.is_none());

    // Startup with no stored credential queues consent
    let mut app3 = test_app().await;
    with_test_oauth(&mut app3);
    assert!(matches!(app3.connect_params(0).await, Ok(None)));
    app3.connect_all_accounts().await;
    assert!(app3.pending_oauth.is_some());
    assert!(app3.oauth_in_progress);

    // Cancelling consent clears overlay, no error toast, and guards against re-prompt
    let mut app4 = test_app().await;
    with_test_oauth(&mut app4);
    fail_connect(
        &mut app4,
        crate::tui::app::event::ConnectError::Auth("connect: XOAUTH2 failed".into()),
    )
    .await;
    let _ = app4.take_pending_oauth();
    app4.cancel_oauth();
    assert!(!app4.oauth_in_progress);
    assert!(matches!(
        app4.toasts.last().map(|t| t.kind),
        Some(ToastKind::Info)
    ));
    fail_connect(
        &mut app4,
        crate::tui::app::event::ConnectError::Auth("connect: IMAP LOGIN failed".into()),
    )
    .await;
    assert!(app4.pending_oauth.is_none());
}

#[tokio::test]
async fn oauth_paste_flow() {
    let mut app = test_app().await;
    open_paste_flow(&mut app);
    assert!(app.oauth_paste.is_some());

    // Valid redirect URL submits exchange
    for c in "http://localhost/?code=AAA&state=state".chars() {
        send_key!(app, Key::ch(c));
    }
    send_key!(app, Key::enter());
    assert!(app.oauth_paste.is_none());
    let sub = app.take_oauth_submit().expect("exchange queued");
    assert!(sub.input.text().contains("code=AAA"));

    // Blank paste is rejected, prompt stays
    let mut app2 = test_app().await;
    open_paste_flow(&mut app2);
    send_key!(app2, Key::enter());
    assert!(app2.oauth_submit.is_none());
    assert!(app2.oauth_paste.is_some());

    // Escape cancels prompt and clears overlay
    let mut app3 = test_app().await;
    open_paste_flow(&mut app3);
    send_key!(app3, Key::esc());
    assert!(app3.oauth_paste.is_none());
    assert!(!app3.oauth_in_progress);
    assert!(app3.oauth_submit.is_none());

    // Outside account form, prompt floats as overlay
    let mut app4 = test_app().await;
    open_paste_flow(&mut app4);
    assert!(!app4.oauth_paste_inline());
    use ratatui::{Terminal, backend::TestBackend};
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app4)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("authorize gmail"));
    assert!(text.contains("Redirect URL"));
}

#[tokio::test]
async fn sidebar_account_switching() {
    let config = Config::default();
    let conn = database::open(Path::new(":memory:")).await.unwrap();
    let mk = |name: &str, def: bool| Account {
        id: None,
        name: name.into(),
        email: format!("{name}@x.io"),
        provider: "gmail".into(),
        keychain_ref: String::new(),
        is_default: def,
        created_at: 1,
    };
    crate::infrastructure::database::accounts::upsert(&conn, &mk("personal", true))
        .await
        .unwrap();
    crate::infrastructure::database::accounts::upsert(&conn, &mk("work", false))
        .await
        .unwrap();
    let accounts = crate::infrastructure::database::accounts::list(&conn)
        .await
        .unwrap();
    for a in &accounts {
        seed_fixture_cache(&conn, a.id.unwrap()).await;
    }
    let mut app = App::new(
        config,
        conn.clone(),
        accounts,
        crate::infrastructure::sqlite_services(conn.clone()),
    )
    .await;
    app.focus = Pane::Folders;
    assert_eq!(app.active_account, 0);

    // Click account in tree switches active
    app.sidebar_sel = SidebarItem::Account(1);
    app.action(Action::SelectFolder).await;
    assert_eq!(app.active_account, 1);

    // Filter narrows to match, Enter switches, Esc cancels
    send_key!(app, Key::ch('/'));
    assert!(app.sidebar_filtering);
    for c in "personal".chars() {
        send_key!(app, Key::ch(c));
    }
    assert_eq!(app.sidebar_items(), vec![SidebarItem::Account(0)]);
    send_key!(app, Key::enter());
    assert_eq!(app.active_account, 0);
    assert!(!app.sidebar_filtering);

    send_key!(app, Key::ch('/'));
    for c in "work".chars() {
        send_key!(app, Key::ch(c));
    }
    send_key!(app, Key::esc());
    assert_eq!(app.active_account, 0, "Esc must not switch");
}

#[tokio::test]
async fn authorize_form_validation() {
    // Empty form shows config error (not "enter name/email")
    let mut app = test_app().await;
    focus_authorize(&mut app).await;
    send_key!(app, Key::enter());
    assert!(app.pending_oauth.is_none());
    let toast = app.toasts.last().expect("a toast");
    assert_eq!(toast.kind, ToastKind::Error);
    assert!(toast.text.to_lowercase().contains("oauth config"));

    // Filled form with missing provider creds also shows config error
    let mut app2 = test_app().await;
    focus_authorize(&mut app2).await;
    app2.settings.as_mut().unwrap().focus = SettingsFocus::Name;
    app2.settings.as_mut().unwrap().editing = true;
    for ch in "Work".chars() {
        app2.settings
            .as_mut()
            .unwrap()
            .form
            .as_mut()
            .unwrap()
            .name
            .handle(&key_ev(&Key::ch(ch)));
    }
    for ch in "work@corp.io".chars() {
        app2.settings
            .as_mut()
            .unwrap()
            .form
            .as_mut()
            .unwrap()
            .email
            .handle(&key_ev(&Key::ch(ch)));
    }
    app2.request_authorize();
    assert!(app2.pending_oauth.is_none());
    let toast = app2.toasts.last().expect("config error toast");
    assert_eq!(toast.kind, ToastKind::Error);
    assert!(toast.text.to_lowercase().contains("oauth config"));
}

#[tokio::test]
async fn password_provider_account_creation() {
    let mut app = test_app_with_password_provider().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a'));
    let idx = app
        .settings
        .as_ref()
        .unwrap()
        .providers
        .iter()
        .position(|p| p == "fastmail")
        .unwrap();
    app.settings.as_mut().unwrap().choose_idx = idx;
    app.choose_provider();
    assert!(!app.form_provider_is_oauth());
    assert!(app.form_focus_order().contains(&SettingsFocus::Password));

    // Fill and save
    {
        let form = app.settings.as_mut().unwrap().form.as_mut().unwrap();
        for c in "Fast".chars() {
            form.name.handle(&key_ev(&Key::ch(c)));
        }
        for c in "me@fastmail.com".chars() {
            form.email.handle(&key_ev(&Key::ch(c)));
        }
        for c in "s3cret".chars() {
            form.password.handle(&key_ev(&Key::ch(c)));
        }
    }
    app.save_password_account().await;
    assert!(!app.settings.as_ref().unwrap().editing);

    let accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    let acct = accounts.iter().find(|a| a.name == "Fast").expect("saved");
    assert_eq!(acct.provider, "fastmail");
    assert_eq!(acct.email, "me@fastmail.com");
    assert!(acct.keychain_ref.ends_with("/password"));
    let pw = crate::infrastructure::auth::token::load_password(&app.pool, &acct.keychain_ref)
        .await
        .unwrap();
    assert_eq!(pw.as_deref(), Some("s3cret"));

    // Missing password shows warning, doesn't save
    let mut app2 = test_app_with_password_provider().await;
    send_key!(app2, Key::ch('S'));
    send_key!(app2, Key::ch('a'));
    let idx2 = app2
        .settings
        .as_ref()
        .unwrap()
        .providers
        .iter()
        .position(|p| p == "fastmail")
        .unwrap();
    app2.settings.as_mut().unwrap().choose_idx = idx2;
    app2.choose_provider();
    {
        let form = app2.settings.as_mut().unwrap().form.as_mut().unwrap();
        for c in "Fast2".chars() {
            form.name.handle(&key_ev(&Key::ch(c)));
        }
        for c in "me2@fastmail.com".chars() {
            form.email.handle(&key_ev(&Key::ch(c)));
        }
    }
    app2.save_password_account().await;
    assert!(app2.settings.as_ref().unwrap().editing);
    assert_eq!(app2.toasts.last().unwrap().kind, ToastKind::Warning);
    let accounts2 = crate::infrastructure::database::accounts::list(&app2.pool)
        .await
        .unwrap();
    assert!(accounts2.iter().all(|a| a.name != "Fast2"));
}

#[tokio::test]
async fn connect_params_by_provider() {
    let mut app = test_app_with_password_provider().await;

    // Password account -> ImapAuth::Password
    let kref = keychain_ref_for_password("fast");
    crate::infrastructure::auth::token::store_password(&app.pool, &kref, "pw")
        .await
        .unwrap();
    let pw_acct = Account {
        id: None,
        name: "fast".into(),
        email: "me@fastmail.com".into(),
        provider: "fastmail".into(),
        keychain_ref: kref.clone(),
        is_default: false,
        created_at: 1,
    };
    crate::infrastructure::database::accounts::upsert(&app.pool, &pw_acct)
        .await
        .unwrap();
    app.accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    let pidx = app.accounts.iter().position(|a| a.name == "fast").unwrap();
    let pparams = app
        .connect_params(pidx)
        .await
        .unwrap()
        .expect("password params");
    assert!(matches!(
        pparams.auth,
        crate::application::account::connect_params::ImapAuth::Password { .. }
    ));

    // OAuth account with refresh token -> ImapAuth::OAuth
    if let Some(p) = app.config.providers.get_mut("gmail") {
        p.oauth.as_mut().unwrap().client_id = "cid".into();
    }
    let gref = keychain_ref_for("personal");
    crate::infrastructure::auth::token::store_refresh_token(&app.pool, &gref, "rt")
        .await
        .unwrap();
    let mut personal = app
        .accounts
        .iter()
        .find(|a| a.name == "personal")
        .unwrap()
        .clone();
    personal.keychain_ref = gref;
    crate::infrastructure::database::accounts::upsert(&app.pool, &personal)
        .await
        .unwrap();
    app.accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    let gidx = app
        .accounts
        .iter()
        .position(|a| a.name == "personal")
        .unwrap();
    let gparams = app
        .connect_params(gidx)
        .await
        .unwrap()
        .expect("oauth params");
    assert!(matches!(
        gparams.auth,
        crate::application::account::connect_params::ImapAuth::OAuth { .. }
    ));
}

#[tokio::test]
async fn apply_oauth_result_saves_account() {
    // Without display name -> name falls back to email
    let mut app = test_app().await;
    let req = PendingOAuth {
        oauth: ResolvedOAuth {
            auth_url: "https://a".into(),
            token_url: "https://t".into(),
            scopes: vec![],
            client_id: "cid".into(),
            client_secret: String::new(),
        },
        account: Account {
            id: None,
            name: String::new(),
            email: String::new(),
            provider: "gmail".into(),
            keychain_ref: String::new(),
            is_default: false,
            created_at: now_ts(),
        },
        set_default: false,
    };
    let tokens = TokenSet {
        access_token: "at".into(),
        refresh_token: Some("rt".into()),
        expires_in: None,
        email: Some("new@gmail.com".into()),
        name: None,
    };
    app.apply_oauth_result(req, Ok(tokens)).await;

    let accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    let acct = accounts
        .iter()
        .find(|a| a.email == "new@gmail.com")
        .expect("account saved");
    assert_eq!(acct.name, "new@gmail.com");
    assert!(acct.keychain_ref.ends_with("/refresh"));
    let rt = crate::infrastructure::auth::token::load_refresh_token(&app.pool, &acct.keychain_ref)
        .await
        .unwrap();
    assert_eq!(rt.as_deref(), Some("rt"));
    assert!(!app.oauth_in_progress);

    // With display name -> uses real name
    let mut app2 = test_app().await;
    let req2 = PendingOAuth {
        oauth: ResolvedOAuth {
            auth_url: "https://a".into(),
            token_url: "https://t".into(),
            scopes: vec![],
            client_id: "cid".into(),
            client_secret: String::new(),
        },
        account: Account {
            id: None,
            name: String::new(),
            email: String::new(),
            provider: "gmail".into(),
            keychain_ref: String::new(),
            is_default: false,
            created_at: now_ts(),
        },
        set_default: false,
    };
    let tokens2 = TokenSet {
        access_token: "at".into(),
        refresh_token: Some("rt".into()),
        expires_in: None,
        email: Some("ada@gmail.com".into()),
        name: Some("Ada Lovelace".into()),
    };
    app2.apply_oauth_result(req2, Ok(tokens2)).await;
    let accounts2 = crate::infrastructure::database::accounts::list(&app2.pool)
        .await
        .unwrap();
    let acct2 = accounts2
        .iter()
        .find(|a| a.email == "ada@gmail.com")
        .expect("account saved");
    assert_eq!(acct2.name, "Ada Lovelace");
}

#[tokio::test]
async fn forms_render_without_panic() {
    use ratatui::{Terminal, backend::TestBackend};

    // Password form renders
    let mut app = test_app_with_password_provider().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a'));
    let idx = app
        .settings
        .as_ref()
        .unwrap()
        .providers
        .iter()
        .position(|p| p == "fastmail")
        .unwrap();
    app.settings.as_mut().unwrap().choose_idx = idx;
    app.choose_provider();

    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("Password"));
    assert!(text.contains("Save account"));

    // OAuth paste overlay renders inline in form
    let req = PendingOAuth {
        oauth: ResolvedOAuth {
            auth_url: "https://a".into(),
            token_url: "https://t".into(),
            scopes: vec![],
            client_id: "cid".into(),
            client_secret: String::new(),
        },
        account: Account {
            id: None,
            name: "x".into(),
            email: "x@y.z".into(),
            provider: "gmail".into(),
            keychain_ref: String::new(),
            is_default: false,
            created_at: 1,
        },
        set_default: false,
    };
    let flow = crate::application::oauth::AuthCodeFlow {
        authorize_url: "https://consent.example/auth?x=1".into(),
        redirect_uri: "http://localhost".into(),
        pkce_verifier: "verifier".into(),
        csrf_state: "state".into(),
    };
    app.begin_oauth_paste(req, flow);
    assert!(app.oauth_paste_inline());
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("New Account"));
    assert!(text.contains("Authorize gmail"));
    assert!(text.contains("Redirect URL"));
}

#[tokio::test]
async fn help_and_key_handling() {
    use ratatui::{Terminal, backend::TestBackend};

    // ? opens help from account manager
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    assert_eq!(app.view, View::Settings);
    send_key!(app, Key::ch('?'));
    assert!(app.help_open);

    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("account list"));
    assert!(text.contains("open / edit account"));
    assert!(text.contains("add account"));
    assert!(text.contains("account form"));

    send_key!(app, Key::ch('?'));
    assert!(!app.help_open);

    // ? is swallowed when editing a form field
    let mut app2 = test_app().await;
    send_key!(app2, Key::ch('S'));
    send_key!(app2, Key::ch('a'));
    send_key!(app2, Key::enter());
    send_key!(app2, Key::ch('j'));
    send_key!(app2, Key::ch('e'));
    assert!(app2.settings.as_ref().unwrap().field_editing);
    send_key!(app2, Key::ch('?'));
    assert!(!app2.help_open);
    assert_eq!(
        app2.settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .name
            .text(),
        "?"
    );
}

#[tokio::test]
async fn connect_while_in_account_manager_stays() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    assert_eq!(app.view, View::Settings);

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

    assert_eq!(app.view, View::Settings);
    let toast = app.toasts.last().expect("success toast");
    assert_eq!(toast.kind, ToastKind::Success);
    assert!(toast.text.contains("Connected"));
}

#[tokio::test]
async fn account_list_columns_expand() {
    use ratatui::{Terminal, backend::TestBackend};

    async fn provider_col(w: u16) -> usize {
        let mut app = test_app().await;
        send_key!(app, Key::ch('S'));
        let mut term = Terminal::new(TestBackend::new(w, 20)).unwrap();
        term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
        let buf = term.backend().buffer().clone();
        for y in 0..buf.area().height {
            let row: String = (0..buf.area().width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if let Some(idx) = row.find("PROVIDER") {
                return idx;
            }
        }
        panic!("PROVIDER header not found");
    }

    let narrow = provider_col(90).await;
    let wide = provider_col(180).await;
    assert!(
        wide > narrow + 40,
        "columns should expand: narrow={narrow}, wide={wide}"
    );
}

#[tokio::test]
async fn provider_dropdown_and_form_navigation() {
    let mut app = test_app().await;
    let acct = Account {
        id: None,
        name: "Existing".into(),
        email: "existing@example.com".into(),
        provider: "gmail".into(),
        keychain_ref: "pesan/test/refresh".into(),
        is_default: true,
        created_at: 1,
    };
    crate::infrastructure::database::accounts::upsert(&app.pool, &acct)
        .await
        .unwrap();
    app.accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('o'));
    assert!(app.settings.as_ref().unwrap().editing);
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Authorize
    );

    // Open provider chooser from edit form
    send_key!(app, Key::ch('j')); // to Provider field
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Provider
    );
    send_key!(app, Key::ch('o'));
    assert!(app.settings.as_ref().unwrap().choosing_provider);

    // Navigate and confirm
    if app.settings.as_ref().unwrap().providers.len() > 1 {
        send_key!(app, Key::ch('j'));
    }
    send_key!(app, Key::enter());
    assert!(!app.settings.as_ref().unwrap().choosing_provider);
    assert_eq!(
        app.settings.as_ref().unwrap().focus,
        SettingsFocus::Authorize
    );

    // Form h/l navigation (2-column grid)
    let mut app2 = test_app().await;
    send_key!(app2, Key::ch('S'));
    send_key!(app2, Key::ch('a'));
    send_key!(app2, Key::enter());
    send_key!(app2, Key::ch('j')); // to Name
    assert_eq!(app2.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    send_key!(app2, Key::ch('l')); // to IsDefault
    assert_eq!(
        app2.settings.as_ref().unwrap().focus,
        SettingsFocus::IsDefault
    );
    send_key!(app2, Key::ch('h')); // back to Name
    assert_eq!(app2.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    send_key!(app2, Key::ch('h')); // no-op at left edge
    assert_eq!(app2.settings.as_ref().unwrap().focus, SettingsFocus::Name);
    send_key!(app2, Key::ch('l'));
    send_key!(app2, Key::ch('l')); // no-op at right edge
    assert_eq!(
        app2.settings.as_ref().unwrap().focus,
        SettingsFocus::IsDefault
    );
}

#[tokio::test]
async fn edit_account_switches_provider_and_persists() {
    let mut app = test_app().await;
    let acct = Account {
        id: None,
        name: "Existing".into(),
        email: "existing@example.com".into(),
        provider: "gmail".into(),
        keychain_ref: "pesan/test/refresh".into(),
        is_default: true,
        created_at: 1,
    };
    crate::infrastructure::database::accounts::upsert(&app.pool, &acct)
        .await
        .unwrap();
    app.accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    let n = app
        .settings
        .as_ref()
        .map(|s| s.providers.len())
        .unwrap_or(0);
    if n < 2 {
        return;
    }
    let other = app.settings.as_ref().unwrap().providers[1].clone();
    if other == "gmail" {
        return;
    }

    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('o'));
    send_key!(app, Key::ch('o'));
    assert!(app.settings.as_ref().unwrap().choosing_provider);
    send_key!(app, Key::ch('j'));
    send_key!(app, Key::enter());
    assert_eq!(
        app.settings
            .as_ref()
            .unwrap()
            .form
            .as_ref()
            .unwrap()
            .provider_idx,
        1
    );

    // Save and verify persistence
    send_key!(app, Key::ch('W'));
    let saved = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    let updated = saved
        .iter()
        .find(|a| a.email == "existing@example.com")
        .unwrap();
    assert_eq!(updated.provider, other);
}
