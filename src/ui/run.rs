//! TUI entry point and terminal lifecycle.
//!
//! Everything ratatui/crossterm touches lives here (not in the composition
//! root): setting up the alternate screen, the redraw/event loop, suspending
//! the terminal for an external editor, and restoring a sane terminal on exit
//! or panic. `main` builds config/DB/accounts and hands them to [`run`].

use std::io::{self, Stdout};
use std::path::Path;

use anyhow::{Context, Result};
use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::account::Account;
use crate::account::infrastructure::accounts;
use crate::mail::infrastructure::ipc::client;
use crate::platform::auth::oauth;
use crate::platform::boot;
use crate::platform::config;
use crate::platform::db;
use crate::platform::logging::LogMode;
use crate::ui::app;
use crate::ui::app::event;
use crate::ui::view;
use crate::wiring;

/// Default (no-subcommand) entry point: bootstrap the process (logging to file
/// only - stdout is the TUI), load the accounts, then launch the interactive UI.
pub fn start() -> Result<()> {
    let boot = boot::boot(LogMode::File)?;
    let accounts = boot
        .runtime
        .block_on(accounts::list(&boot.pool))?;
    tracing::info!("startup: {} account(s), config loaded", accounts.len());
    boot.runtime.block_on(run(boot.config, boot.pool, accounts))
}

/// Launch the interactive TUI: set up the terminal, run the event loop until
/// the user quits, then restore the terminal on the way out.
pub async fn run(config: config::Config, pool: db::Db, accounts: Vec<Account>) -> Result<()> {
    install_panic_hook();
    let mut terminal = init_terminal()?;
    let result = run_app(&mut terminal, config, pool, accounts).await;
    restore_terminal()?;
    tracing::info!("shutdown clean");
    result
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: config::Config,
    pool: db::Db,
    accounts: Vec<Account>,
) -> Result<()> {
    let mut events = event::Events::spawn();
    // Route mail ops through the daemon when one is running (falls back to a
    // direct IMAP connection otherwise); the daemon itself uses `sqlite_services`.
    let services = wiring::client_services(pool.clone());
    let mut app = app::App::new(config, accounts, services).await;
    app.set_event_sender(events.sender());
    // Offline-first: App::new already shows cached mail; now connect every
    // authorized account live in the background (the active one in the
    // foreground, the rest warming their caches) if authorized.
    app.connect_all_accounts().await;

    // Only repaint when something actually changed (or a toast/spinner is
    // animating). Redrawing on every 100ms tick wastes CPU and causes flicker.
    let mut dirty = true;
    // Throttled probe so an offline client comes online once the daemon appears
    // (and reflects a daemon that went away), without polling every 100ms tick.
    let sock = config::socket_path().unwrap_or_default();
    let mut last_daemon_probe = std::time::Instant::now();
    while !app.should_quit {
        if dirty {
            let width = terminal.size().map(|s| s.width).unwrap_or(u16::MAX);
            app.set_viewport_width(width);
            // Rebuild the reader's memoized Markdown render if stale, before the
            // immutable draw. Cheap when unchanged; avoids re-parsing the body on
            // every scroll keystroke and spinner tick.
            app.prepare_reader(width);
            terminal.draw(|frame| view::draw(frame, &app))?;
            dirty = false;
        }
        let editor = app.take_pending_external();
        let picker = app.take_pending_file_picker();
        let oauth = app.take_pending_oauth();
        match events.next().await {
            Some(event::Event::Input(ev)) => {
                app.handle(&ev).await;
                dirty = true;
            }
            Some(event::Event::Tick) => {
                let had_toast = !app.toasts.is_empty();
                app.tick();
                // Redraw while animating (spinner), while any toast is queued (so
                // the queue advances to the next one), on the final clear, or to
                // keep the open job tracker's elapsed/spinner/pruning live.
                if app.busy().is_some()
                    || !app.toasts.is_empty()
                    || (had_toast && app.toasts.is_empty())
                    || (app.jobs_open && !app.jobs.is_empty())
                {
                    dirty = true;
                }
                // Every ~3s, reconcile with daemon availability.
                if last_daemon_probe.elapsed() >= std::time::Duration::from_secs(3) {
                    last_daemon_probe = std::time::Instant::now();
                    let reachable = client::daemon_reachable(&sock);
                    if reachable && !app.is_live() && app.busy().is_none() {
                        // Daemon (re)appeared while we were offline: come online.
                        app.services.daemon_backed = true;
                        app.connect_all_accounts().await;
                        dirty = true;
                    } else if !reachable && app.services.daemon_backed && !app.is_live() {
                        // Daemon went away and we're offline: prompt to start it.
                        app.services.daemon_backed = false;
                        app.daemon_missing = true;
                        dirty = true;
                    }
                }
            }
            Some(event::Event::NewMail(nm)) => {
                app.on_new_mail(nm).await;
                dirty = true;
            }
            Some(event::Event::Connected(done)) => {
                // A background live-connect finished; swap in the live source.
                app.on_connected(*done).await;
                dirty = true;
            }
            Some(event::Event::FolderRefreshed(done)) => {
                // A background folder refresh finished; swap in fresh envelopes.
                app.on_folder_refreshed(done).await;
                dirty = true;
            }
            Some(event::Event::MessageFetched(done)) => {
                // A background message fetch finished; swap in the loaded body.
                app.on_message_fetched(*done).await;
                dirty = true;
            }
            Some(event::Event::OlderLoaded(done)) => {
                // A "load older messages" page finished; merge it into the list.
                app.on_older_loaded(done).await;
                dirty = true;
            }
            Some(event::Event::FoldersCounted(done)) => {
                // Background folder-count sweep finished; fill sidebar counts.
                app.on_folders_counted(done).await;
                dirty = true;
            }
            Some(event::Event::SyncProgress(progress)) => {
                // Background all-folders sync advanced; update the status bar.
                app.on_sync_progress(progress);
                dirty = true;
            }
            Some(event::Event::BackgroundError(msg)) => {
                app.set_toast(msg, app::ToastKind::Warning);
                dirty = true;
            }
            Some(event::Event::ConnectionLost(account)) => {
                // A live session wedged; drop to cache and auto-reconnect.
                app.on_connection_lost(account).await;
                dirty = true;
            }
            Some(event::Event::JobProgress(p)) => {
                app.on_job_progress(p);
                // Only the job tracker shows this; skip the repaint when it's closed.
                dirty = app.jobs_open;
            }
            Some(event::Event::JobDone(done)) => {
                app.on_job_done(done);
                dirty = app.jobs_open;
            }
            Some(event::Event::Error(e)) => return Err(e),
            None => break,
        }
        if let Some(pending) = editor {
            // Stop the background input reader before suspending the TUI so the
            // editor child owns stdin exclusively (otherwise the reader steals
            // keystrokes and the editor feels laggy). Resume it afterwards.
            events.pause_input().await;
            let result = run_external_editor(terminal, pending);
            events.resume_input();
            if let Some(result) = result {
                app.apply_external_result(result);
            }
            dirty = true;
        }
        if let Some(pending) = picker {
            // Same suspend/resume contract as the editor: the picker owns the
            // terminal while it runs, then the selected path comes back.
            events.pause_input().await;
            let result = run_external_file_picker(terminal, pending);
            events.resume_input();
            if let Some(result) = result {
                app.apply_file_picker_result(result);
            }
            dirty = true;
        }
        if let Some(req) = oauth {
            // Step 1 of the manual (copy/paste) OAuth flow: build the consent URL
            // and open the browser. No HTTP listener is bound. Browser is opened
            // via configured command or system default. Run off the Tokio runtime
            // on a scoped thread (the blocking-work rule); it returns quickly.
            // The user then pastes the redirect URL into the TUI (handled below).
            let oauth_cfg = req.oauth.clone();
            let browser_cmd = app.config.browser.oauth.as_deref();
            let flow = std::thread::scope(|s| {
                s.spawn(|| oauth::begin_auth_code_flow(&oauth_cfg, browser_cmd))
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("OAuth thread panicked")))
            });
            match flow {
                Ok(flow) => app.begin_oauth_paste(req, flow),
                Err(e) => {
                    app.oauth_in_progress = false;
                    app.set_toast(
                        format!("Authorization failed: {e:#}"),
                        app::ToastKind::Error,
                    );
                }
            }
            dirty = true;
        }
        if let Some(sub) = app.take_oauth_submit() {
            // Step 2: the user pasted the redirect URL and pressed Enter. Exchange
            // the code for tokens. This does blocking network I/O, so run it off
            // the runtime; pause the input reader for the (brief) duration so it
            // doesn't buffer keystrokes.
            events.pause_input().await;
            let pasted = sub.input.text().to_string();
            let oauth_cfg = sub.req.oauth.clone();
            let flow = sub.flow;
            let result = std::thread::scope(|s| {
                s.spawn(|| oauth::exchange_pasted_redirect(&oauth_cfg, &flow, &pasted))
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("OAuth thread panicked")))
            });
            events.resume_input();
            let _ = terminal.clear();
            app.apply_oauth_result(sub.req, result).await;
            dirty = true;
        }
    }
    Ok(())
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    // Mouse capture is intentionally off: the app is keyboard-driven, and
    // capturing mouse events floods the event loop (and lets the terminal keep
    // native scroll/selection).
    execute!(stdout, EnterAlternateScreen).context("enter alt screen")?;
    execute!(stdout, Show).ok();
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("create terminal")
}

fn restore_terminal() -> Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen).context("leave alt screen")?;
    disable_raw_mode().context("disable raw mode")?;
    Ok(())
}

/// Restore a sane terminal even when a panic unwinds through the TUI. The RAII
/// path in `run()/restore_terminal()` covers ordinary exits; this covers crashes.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = disable_raw_mode();
        default(info);
    }));
}

/// Suspend the TUI, write the compose body to a temp file, open the configured
/// editor (`compose.editor` -> `$VISUAL` -> `$EDITOR` -> `vi`), read the result
/// back, and restore the terminal. Returns `Some(Some(text))` on success,
/// `Some(None)` when the editor exited non-zero, and `None` on setup failure.
fn run_external_editor(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    pending: app::PendingExternalEditor,
) -> Option<Option<String>> {
    let tmp = editor_temp_file("pesan-compose")?;
    std::fs::write(&tmp.path, pending.current_text).ok()?;

    let (program, args) = resolve_editor_cmd(pending.editor_cmd.as_deref());

    if let Err(e) = suspend_terminal() {
        tracing::warn!("suspend terminal: {e}");
        return None;
    }
    let status = std::process::Command::new(&program)
        .args(args)
        .arg(&tmp.path)
        .status();
    let resumed = resume_terminal(terminal).is_ok();
    tracing::info!("external editor child exited: {status:?}, resumed={resumed}");

    let result = match status {
        Ok(st) if st.success() => Some(std::fs::read_to_string(&tmp.path).ok()),
        _ => None,
    };
    let _ = std::fs::remove_file(&tmp.path);
    result
}

/// Suspend the TUI, run the configured external file picker (yazi/lf/ranger),
/// read the selected file paths from the temp selection file, and restore the
/// terminal. Returns `Some(paths)` when the picker exited zero (one path per
/// line; empty when the user cancelled), and `None` on setup failure or a
/// non-zero exit.
fn run_external_file_picker(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    pending: app::PendingFilePicker,
) -> Option<Vec<String>> {
    let tmp = picker_temp_file()?;
    // Create the selection file up front so the picker always has a valid target.
    std::fs::write(&tmp.path, b"").ok()?;

    let (program, args) = resolve_picker_cmd(&pending.picker_cmd, &tmp.path);

    if let Err(e) = suspend_terminal() {
        tracing::warn!("suspend terminal: {e}");
        return None;
    }
    let status = std::process::Command::new(&program).args(args).status();
    let resumed = resume_terminal(terminal).is_ok();
    tracing::info!("external picker child exited: {status:?}, resumed={resumed}");

    let result = match status {
        Ok(st) if st.success() => Some(
            std::fs::read_to_string(&tmp.path)
                .map(|s| {
                    s.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        _ => None,
    };
    let _ = std::fs::remove_file(&tmp.path);
    result
}

fn picker_temp_file() -> Option<EditorTempFile> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "pesan-picker-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    Some(EditorTempFile { path })
}

/// Resolve the configured picker command. `{}` is replaced with the temp
/// selection-file path; when the command has no `{}` the path is appended as a
/// positional argument (some tools take it that way). Splits on whitespace,
/// matching [`resolve_editor_cmd`].
fn resolve_picker_cmd(configured: &str, tmp_path: &Path) -> (String, Vec<String>) {
    let mut parts = configured.split_whitespace();
    let program = parts.next().unwrap_or("").to_string();
    let mut args: Vec<String> = parts.map(str::to_string).collect();
    let mut replaced = false;
    for arg in args.iter_mut() {
        if arg == "{}" {
            *arg = tmp_path.to_string_lossy().into_owned();
            replaced = true;
        }
    }
    if !replaced {
        args.push(tmp_path.to_string_lossy().into_owned());
    }
    (program, args)
}

/// Release the terminal (alt screen + raw mode) before handing it to a child
/// process so the editor enjoys a normal, line-buffered shell.
fn suspend_terminal() -> std::io::Result<()> {
    let mut out = io::stdout();
    execute!(out, Show, LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// Re-enter the TUI and force ratatui to fully repaint the next frame.
fn resume_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> std::io::Result<()> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let size = terminal.size()?;
    terminal.resize(ratatui::layout::Rect::new(0, 0, size.width, size.height))?;
    Ok(())
}

struct EditorTempFile {
    path: std::path::PathBuf,
}

fn editor_temp_file(tag: &str) -> Option<EditorTempFile> {
    let dir = std::env::temp_dir();
    // `.md`: the compose body is Markdown (rendered to HTML on send), so the
    // editor gets Markdown filetype detection/highlighting.
    let path = dir.join(format!(
        "{tag}-{}-{}.md",
        std::process::id(),
        unique_suffix()
    ));
    Some(EditorTempFile { path })
}

fn unique_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Resolve `compose.editor` -> `$VISUAL` -> `$EDITOR` -> `vi`. Supports args
/// such as `"code --wait"` by splitting on whitespace.
fn resolve_editor_cmd(configured: Option<&str>) -> (String, Vec<String>) {
    let raw = configured
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("VISUAL").ok().filter(|s| !s.is_empty()))
        .or_else(|| std::env::var("EDITOR").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "vi".to_string());
    let mut parts = raw.split_whitespace();
    let program = parts.next().unwrap_or("vi").to_string();
    let args: Vec<String> = parts.map(str::to_string).collect();
    (program, args)
}
