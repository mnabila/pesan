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

#[tokio::test]
async fn auth_failure_queues_background_reauthorize_once() {
    let mut app = test_app().await;
    with_test_oauth(&mut app);

    // The stored credential was rejected: the app must queue the browser
    // consent flow automatically instead of waiting for a manual Authorize.
    let err = crate::tui::app::event::ConnectError::Auth(
        "connect: XOAUTH2 authentication failed: AUTHENTICATIONFAILED".into(),
    );
    fail_connect(&mut app, err).await;
    assert!(!app.live);
    assert!(app.oauth_in_progress, "consent overlay armed");
    let req = app.pending_oauth.as_ref().expect("auto re-auth queued");
    assert_eq!(req.account.name, "personal");
    assert_eq!(req.account.provider, "gmail");
    // Re-authorization never changes which account is default.
    assert!(!req.set_default);

    // Consume the prompt (as the event loop would), then fail again: no second
    // browser prompt for the same account within one session.
    let _ = app.take_pending_oauth();
    app.oauth_in_progress = false;
    fail_connect(
        &mut app,
        crate::tui::app::event::ConnectError::Auth("connect: IMAP LOGIN failed".into()),
    )
    .await;
    assert!(
        app.pending_oauth.is_none(),
        "auto re-auth fires at most once per session"
    );
}

#[tokio::test]
async fn network_failure_does_not_queue_reauthorize() {
    let mut app = test_app().await;
    with_test_oauth(&mut app);
    fail_connect(
        &mut app,
        crate::tui::app::event::ConnectError::Other(
            "connect: timed out connecting to IMAP server".into(),
        ),
    )
    .await;
    assert!(
        app.pending_oauth.is_none(),
        "offline is not an auth problem"
    );
}

#[tokio::test]
async fn startup_queues_consent_for_never_authorized_account() {
    let mut app = test_app().await; // no refresh token stored anywhere
    with_test_oauth(&mut app);

    // The account has no stored credential, so `connect_params` yields None -
    // exactly what `connect_all_accounts` sees on first open.
    assert!(matches!(app.connect_params(0).await, Ok(None)));
    app.connect_all_accounts().await;
    let req = app
        .pending_oauth
        .as_ref()
        .expect("first-run consent queued");
    assert_eq!(req.account.name, "personal");
    assert!(app.oauth_in_progress);
}

#[tokio::test]
async fn cancelling_consent_drops_overlay_without_error() {
    let mut app = test_app().await;
    with_test_oauth(&mut app);

    // Auth failure arms the auto re-auth consent overlay.
    fail_connect(
        &mut app,
        crate::tui::app::event::ConnectError::Auth("connect: XOAUTH2 authentication failed".into()),
    )
    .await;
    assert!(app.oauth_in_progress, "consent overlay armed");

    // The event loop takes the pending request, runs the (cancelled) flow, then
    // reports the cancel: the overlay clears and no error toast is raised.
    let _ = app.take_pending_oauth();
    app.cancel_oauth();
    assert!(!app.oauth_in_progress, "overlay cleared after cancel");
    assert!(
        matches!(app.toast.as_ref().map(|t| t.kind), Some(ToastKind::Info)),
        "cancel is informational, not an error"
    );

    // The once-per-session guard still holds, so a later failure does not nag.
    fail_connect(
        &mut app,
        crate::tui::app::event::ConnectError::Auth("connect: IMAP LOGIN failed".into()),
    )
    .await;
    assert!(
        app.pending_oauth.is_none(),
        "no re-prompt after the user cancelled this session"
    );
}

/// Build a queued OAuth request + an opened flow for the active account, as the
/// event loop would after `begin_auth_code_flow`.
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
async fn pasting_redirect_url_queues_the_code_exchange() {
    let mut app = test_app().await;
    open_paste_flow(&mut app);
    assert!(app.oauth_paste.is_some(), "paste prompt is up");

    // Type the redirect URL the browser landed on, then submit with Enter.
    for c in "http://localhost/?code=AAA&state=state".chars() {
        send_key!(app, Key::ch(c));
    }
    send_key!(app, Key::enter());

    // The paste prompt clears and the pasted URL is handed to the event loop.
    assert!(app.oauth_paste.is_none(), "prompt cleared on submit");
    let sub = app.take_oauth_submit().expect("exchange queued");
    assert!(
        sub.input.text().contains("code=AAA"),
        "pasted URL carried to the exchange: {}",
        sub.input.text()
    );
}

#[tokio::test]
async fn paste_prompt_floats_when_not_in_the_account_form() {
    use ratatui::{Terminal, backend::TestBackend};
    // An auto re-auth / startup consent happens outside the Account Manager, so
    // the paste prompt floats as a centered overlay instead of splitting a form.
    let mut app = test_app().await; // view defaults to Main
    open_paste_flow(&mut app);
    assert!(
        !app.oauth_paste_inline(),
        "no form open -> prompt must float"
    );
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(text.contains("authorize gmail"), "floating overlay shown");
    assert!(text.contains("Redirect URL"), "paste field shown");
}

#[tokio::test]
async fn submitting_a_blank_paste_is_rejected() {
    let mut app = test_app().await;
    open_paste_flow(&mut app);
    // Enter with nothing pasted must not queue an exchange; the prompt stays up.
    send_key!(app, Key::enter());
    assert!(app.oauth_submit.is_none(), "blank paste is not submitted");
    assert!(app.oauth_paste.is_some(), "prompt stays up for another try");
}

#[tokio::test]
async fn escape_cancels_the_paste_prompt() {
    let mut app = test_app().await;
    open_paste_flow(&mut app);
    send_key!(app, Key::esc());
    assert!(app.oauth_paste.is_none(), "prompt dismissed");
    assert!(!app.oauth_in_progress, "overlay cleared");
    assert!(app.oauth_submit.is_none());
}

#[tokio::test]
async fn sidebar_switches_account_from_tree() {
    // Build an app with two accounts and drive the sidebar tree.
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
    // Give each account real cached folders so the sidebar tree renders.
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

    // The tree lists both accounts, with the active one's folders under it.
    let items = app.sidebar_items();
    assert_eq!(items.first(), Some(&SidebarItem::Account(0)));
    assert!(items.contains(&SidebarItem::Account(1)));
    assert!(items.iter().any(|i| matches!(i, SidebarItem::Folder(_))));

    // Move the cursor onto the "work" account header and activate it.
    app.sidebar_sel = SidebarItem::Account(1);
    app.action(Action::SelectFolder).await; // Enter in the sidebar
    assert_eq!(app.active_account, 1);
}

#[tokio::test]
async fn sidebar_account_filter_narrows_and_switches() {
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
    // Give each account real cached folders so the sidebar tree renders.
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

    // `/` opens the filter; folders vanish while account-picking.
    send_key!(app, Key::ch('/'));
    assert!(app.sidebar_filtering);
    for c in "work".chars() {
        send_key!(app, Key::ch(c));
    }
    let items = app.sidebar_items();
    assert_eq!(items, vec![SidebarItem::Account(1)]);
    assert_eq!(app.sidebar_sel, SidebarItem::Account(1));

    // Enter switches to the sole match and closes the filter.
    send_key!(app, Key::enter());
    assert!(!app.sidebar_filtering);
    assert!(app.sidebar_filter.is_empty());
    assert_eq!(app.active_account, 1);
    // Folders are back under the now-active account.
    assert!(
        app.sidebar_items()
            .iter()
            .any(|i| matches!(i, SidebarItem::Folder(_)))
    );

    // Esc cancels without switching.
    send_key!(app, Key::ch('/'));
    for c in "personal".chars() {
        send_key!(app, Key::ch(c));
    }
    send_key!(app, Key::esc());
    assert!(!app.sidebar_filtering);
    assert!(app.sidebar_filter.is_empty());
    assert_eq!(app.active_account, 1, "Esc must not switch accounts");
}

#[tokio::test]
async fn authorize_no_longer_requires_name_and_email() {
    let mut app = test_app().await;
    focus_authorize(&mut app).await;
    // Name/email are auto-filled from OAuth now, so activating Authorize with an
    // empty form does NOT show the old "enter name and email" warning. With the
    // default gmail provider's empty client_id it surfaces a config error instead
    // (and still queues nothing).
    send_key!(app, Key::enter());
    assert!(app.pending_oauth.is_none());
    let toast = app.toast.as_ref().expect("a toast");
    assert_eq!(toast.kind, ToastKind::Error);
    assert!(toast.text.to_lowercase().contains("oauth config"));
}

#[tokio::test]
async fn authorize_reports_missing_provider_credentials() {
    // Default config has a gmail provider with an empty client_id, so a
    // fully-filled form should surface a config error rather than launch
    // a browser (and must not queue an OAuth request).
    let mut app = test_app().await;
    focus_authorize(&mut app).await;
    app.settings.as_mut().unwrap().focus = SettingsFocus::Name;
    app.settings.as_mut().unwrap().editing = true;
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
    app.request_authorize();
    assert!(app.pending_oauth.is_none());
    let toast = app.toast.as_ref().expect("config error toast");
    assert_eq!(toast.kind, ToastKind::Error);
    assert!(toast.text.to_lowercase().contains("oauth config"));
}

#[tokio::test]
async fn password_provider_saves_account_and_stores_password() {
    let mut app = test_app_with_password_provider().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a')); // add a new account
    // Select the password provider; the form must switch to the password fields.
    let idx = app
        .settings
        .as_ref()
        .unwrap()
        .providers
        .iter()
        .position(|p| p == "fastmail")
        .unwrap();
    app.settings
        .as_mut()
        .unwrap()
        .form
        .as_mut()
        .unwrap()
        .provider_idx = idx;
    assert!(!app.form_provider_is_oauth());
    assert!(
        app.form_focus_order().contains(&SettingsFocus::Password),
        "password provider exposes a Password field"
    );
    // Fill name/email/password.
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

    // Back on the account list, and the row persisted with the password ref.
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
}

#[tokio::test]
async fn save_password_account_requires_a_password() {
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
    {
        let state = app.settings.as_mut().unwrap();
        state.form.as_mut().unwrap().provider_idx = idx;
        let form = state.form.as_mut().unwrap();
        for c in "Fast".chars() {
            form.name.handle(&key_ev(&Key::ch(c)));
        }
        for c in "me@fastmail.com".chars() {
            form.email.handle(&key_ev(&Key::ch(c)));
        }
    }
    app.save_password_account().await; // no password typed
    assert!(app.settings.as_ref().unwrap().editing, "form stays open");
    assert_eq!(app.toast.as_ref().unwrap().kind, ToastKind::Warning);
    let accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    assert!(accounts.iter().all(|a| a.name != "Fast"));
}

#[tokio::test]
async fn connect_params_selects_auth_by_provider() {
    let mut app = test_app_with_password_provider().await;

    // Password account -> ImapAuth::Password.
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

    // OAuth account with credentials + a stored refresh token -> ImapAuth::OAuth.
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
async fn apply_oauth_result_autofills_email_and_defaults_name() {
    let mut app = test_app().await;
    // A queued authorization with an empty name/email (as a fresh OAuth add).
    let req = PendingOAuth {
        oauth: ResolvedOAuth {
            auth_url: "https://accounts.example/auth".into(),
            token_url: "https://accounts.example/token".into(),
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
        name: None, // no display name -> name falls back to the email
    };
    app.apply_oauth_result(req, Ok(tokens)).await;

    // The email is filled from the token and, with no name claim, the name
    // label falls back to the email.
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
    // The progress overlay was cleared once the flow resolved.
    assert!(!app.oauth_in_progress);
}

#[tokio::test]
async fn password_form_and_oauth_overlay_render_without_panic() {
    use ratatui::{Terminal, backend::TestBackend};

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
    app.settings
        .as_mut()
        .unwrap()
        .form
        .as_mut()
        .unwrap()
        .provider_idx = idx;

    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(text.contains("Password"), "password field label is shown");
    assert!(
        text.contains("Save account"),
        "password provider shows Save"
    );

    // The OAuth redirect-paste overlay renders once consent has been opened.
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
    // Launched from the account-manager form, so the paste prompt renders inline
    // as a split panel under "New account" rather than as a floating overlay.
    assert!(app.oauth_paste_inline(), "paste renders inline in the form");
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = buf.content().iter().map(|c| c.symbol()).collect();
    assert!(text.contains("New account"), "form box stays visible above");
    assert!(
        text.contains("Authorize gmail"),
        "split panel shows provider"
    );
    assert!(
        text.contains("Redirect URL"),
        "split panel shows the paste field"
    );
}

#[tokio::test]
async fn account_manager_question_mark_opens_help_window() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut app = test_app().await;
    send_key!(app, Key::ch('S')); // open the account manager
    assert_eq!(app.view, View::Settings);
    assert!(!app.help_open);
    // `?` opens the help overlay from the account manager.
    send_key!(app, Key::ch('?'));
    assert!(app.help_open);

    // The overlay lists the account-manager keys (not just Tab/W/Esc).
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| crate::tui::views::draw(f, &app)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        text.contains("account list"),
        "help shows the account-list keys"
    );
    assert!(text.contains("open / edit account"));
    assert!(text.contains("add account"));
    assert!(text.contains("account form"), "help shows the form keys");

    // `?` again closes it; typing is not swallowed while the form field is active.
    send_key!(app, Key::ch('?'));
    assert!(!app.help_open);
}

#[tokio::test]
async fn account_form_field_edit_swallows_question_mark() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S'));
    send_key!(app, Key::ch('a')); // new account form (gmail: focus Provider)
    send_key!(app, Key::ch('j')); // -> Name
    send_key!(app, Key::ch('e')); // start typing into Name
    assert!(app.settings.as_ref().unwrap().field_editing);
    send_key!(app, Key::ch('?')); // must reach the field, not open help
    assert!(!app.help_open);
    assert_eq!(
        app.settings
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
async fn connect_while_in_account_manager_stays_and_only_toasts() {
    let mut app = test_app().await;
    send_key!(app, Key::ch('S')); // open the account manager
    assert_eq!(app.view, View::Settings);

    // A background connect finishing must NOT yank the user to the inbox; it just
    // shows the success toast and leaves them in the account manager.
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

    assert_eq!(app.view, View::Settings, "stays in the account manager");
    let toast = app.toast.as_ref().expect("success toast");
    assert_eq!(toast.kind, ToastKind::Success);
    assert!(toast.text.contains("Connected"));
}

#[tokio::test]
async fn account_list_columns_expand_to_window_width() {
    use ratatui::{Terminal, backend::TestBackend};

    // Column of "PROVIDER" in the account-list header when rendered `w` wide.
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

    // On a wide window the PROVIDER column sits far to the right (email expanded);
    // on a narrow one it stays in the compact position.
    let narrow = provider_col(90).await;
    let wide = provider_col(180).await;
    assert!(
        wide > narrow + 40,
        "columns should expand with width: narrow={narrow}, wide={wide}"
    );
}

#[tokio::test]
async fn apply_oauth_result_uses_real_display_name() {
    let mut app = test_app().await;
    let req = PendingOAuth {
        oauth: ResolvedOAuth {
            auth_url: "https://accounts.example/auth".into(),
            token_url: "https://accounts.example/token".into(),
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
        email: Some("ada@gmail.com".into()),
        name: Some("Ada Lovelace".into()),
    };
    app.apply_oauth_result(req, Ok(tokens)).await;

    // The account label uses the real display name, not the email.
    let accounts = crate::infrastructure::database::accounts::list(&app.pool)
        .await
        .unwrap();
    let acct = accounts
        .iter()
        .find(|a| a.email == "ada@gmail.com")
        .expect("account saved");
    assert_eq!(acct.name, "Ada Lovelace");
}
