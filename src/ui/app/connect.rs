use crate::account::application::connect_params as account_connect_params;
use crate::mail;
use crate::mail::application::folders;
use crate::platform::connect_params;
use crate::ui::app::event;

use super::*;

impl App {
    /// Register a background job in the tracker and hand back its [`JobGuard`],
    /// which the spawned task moves in: dropping it (or calling `.fail`) reports
    /// completion back over the event channel. Call from a `&mut self` spawn site
    /// right before `tokio::spawn`.
    pub(crate) fn begin_job(&mut self, kind: JobKind, account: impl Into<String>) -> JobGuard {
        let id = self.jobs.begin(kind, account.into());
        JobGuard::new(id, self.event_tx.clone())
    }

    /// Apply a background job's progress update (delivered as [`Event::JobProgress`]).
    pub fn on_job_progress(&mut self, p: event::JobProgress) {
        self.jobs.set_progress(p.id, p.done, p.total);
    }

    /// Apply a background job's completion (delivered as [`Event::JobDone`]).
    pub fn on_job_done(&mut self, done: event::JobDone) {
        self.jobs.finish(done.id, done.error);
    }

    /// Current busy label for the status bar, if an operation is in flight.
    pub fn busy(&self) -> Option<&str> {
        self.busy.as_deref()
    }

    /// True when connected to a live source (via the daemon). Read by the event
    /// loop's daemon-availability probe.
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// The current spinner frame glyph (ASCII fallback honored).
    pub fn spinner_glyph(&self) -> &'static str {
        if self.config.ui.ascii {
            const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
            FRAMES[self.spinner_frame % FRAMES.len()]
        } else {
            const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            FRAMES[self.spinner_frame % FRAMES.len()]
        }
    }

    /// Apply the result of a background live-connect (delivered as
    /// [`Event::Connected`]). Stale results - the user switched accounts while
    /// the connect was in flight - are dropped. On success the live source is
    /// swapped in and mail is loaded; on failure the cached view stands.
    pub async fn on_connected(&mut self, done: event::Connected) {
        // Ignore a connect that finished for an account we're no longer on.
        if done.account != self.active_account_name() {
            return;
        }
        self.busy = None;
        // A pending reconnect (after a wedged session) restores the pre-drop
        // selection instead of resetting to the top; a fresh connect does not.
        let reconnect = self.reconnect.take();
        match done.result {
            Ok(data) => {
                // Everything below is already fetched (on the background task) and
                // cached, so applying it does zero network work on the UI loop.
                self.source = data.source;
                self.live = true;
                // The user can navigate folders while a (re)connect is in flight -
                // navigation stays live on the cache during the reconnect window.
                // Capture where they are *now* so a connect that loaded a different
                // folder (the one they were on when it started) doesn't yank the
                // view back to it. Empty = startup with no folders yet.
                let current_folder = self.selected_folder_name().to_string();
                let stayed = current_folder.is_empty() || current_folder == data.folder;
                self.set_folders(data.folders).await;
                // Show cached counts immediately; the live sweep below corrects them.
                self.apply_cached_counts().await;
                if stayed {
                    self.selected_folder = self
                        .folders
                        .iter()
                        .position(|f| f.name == data.folder)
                        .unwrap_or(0);
                    self.sidebar_sel = SidebarItem::Folder(self.selected_folder);
                    self.envelopes = data.envelopes;
                    self.selected_message = 0;
                    self.reader_offset = 0;
                    // A fresh connect / reconnect loads the newest window, so any
                    // paged-in older messages are gone and paging restarts.
                    self.loading_older = false;
                    self.older_exhausted = false;
                    // On a fresh connect, clear the reader/search; on a reconnect keep
                    // them so the current filter and open message can be restored.
                    let filter = if reconnect.is_some() {
                        self.search.as_ref().map(|s| s.input.text().to_string())
                    } else {
                        self.set_open_message(None);
                        self.search = None;
                        None
                    };
                    self.refresh_display_list(filter.as_deref());
                    // Restore the highlighted row after a reconnect (by uid).
                    if let Some(ctx) = &reconnect
                        && let Some(uid) = ctx.select_uid
                        && let Some(pos) = self.display_envelopes.iter().position(|e| e.uid == uid)
                    {
                        self.selected_message = pos;
                    }
                } else {
                    // The user moved to another folder mid-connect: keep them there
                    // (its cached view is already showing) and pull a fresh copy of
                    // *that* folder from the now-live source in the background.
                    self.selected_folder = self
                        .folders
                        .iter()
                        .position(|f| f.name == current_folder)
                        .unwrap_or_else(|| {
                            self.selected_folder
                                .min(self.folders.len().saturating_sub(1))
                        });
                    self.sidebar_sel = SidebarItem::Folder(self.selected_folder);
                    self.spawn_folder_refresh(&current_folder);
                }
                // Folder names are in; fetch their counts in the background so
                // the (slow, per-folder) STATUS sweep never delays this point.
                self.spawn_folder_counts();
                // Warm the cache for this account's other folders too - but only
                // when connecting directly. A running daemon already keeps every
                // folder synced in the shared cache, so this all-folders sweep
                // over IPC would just duplicate its work.
                if !self.services.daemon_backed {
                    self.spawn_folder_sync_all();
                }
                self.start_account_watcher(self.active_account).await;
                // Stay wherever the user is when a connect finishes (e.g. the
                // account manager): just surface the success toast below rather
                // than yanking them to the inbox.
                // If the user was reading a message when the session dropped,
                // transparently reload it so the reconnect is seamless.
                if let Some(ctx) = &reconnect {
                    // Only restore the open message if the user is still on the
                    // folder they were reading; if they navigated away mid-connect
                    // there is nothing to seamlessly reload.
                    if stayed
                        && ctx.was_reading
                        && self.view == View::Reader
                        && let Some(uid) = ctx.open_uid
                        && let Some(pos) = self.display_envelopes.iter().position(|e| e.uid == uid)
                    {
                        self.selected_message = pos;
                        self.load_selected_preview().await;
                    }
                    self.set_toast(format!("Reconnected: {}", done.account), ToastKind::Success);
                } else {
                    self.set_toast(format!("Connected: {}", done.account), ToastKind::Success);
                }
            }
            Err(e) => {
                // A failed (re)connect drops to the cache and does not schedule
                // another attempt, so a broken server can't spin a reconnect loop.
                self.show_cached_source().await;
                let what = if reconnect.is_some() {
                    "reconnect failed"
                } else {
                    "offline"
                };
                self.set_toast(
                    format!("{}: {what} ({e})", done.account),
                    ToastKind::Warning,
                );
                // The credential itself was rejected (expired/revoked refresh
                // token): recover automatically by running the browser consent
                // flow in the background instead of waiting for a manual trip
                // through Settings -> Authorize.
                if e.is_auth()
                    && let Some(account) = self
                        .accounts
                        .iter()
                        .find(|a| a.name == done.account)
                        .cloned()
                {
                    self.queue_auto_reauth(&account).await;
                }
            }
        }
    }

    /// Handle a live IMAP worker reporting a wedged session ([`Event::ConnectionLost`]).
    /// For the active account, drop to a cache-backed source and kick off a single
    /// automatic reconnect, remembering what the user was looking at so it can be
    /// restored. Stale events (a background account, or already offline/reconnecting)
    /// are ignored.
    pub async fn on_connection_lost(&mut self, account: String) {
        if account != self.active_account_name() || !self.live || self.reconnect.is_some() {
            return;
        }
        self.reconnect = Some(ReconnectCtx {
            select_uid: self.selected_env().map(|e| e.uid),
            open_uid: self.open_message.as_ref().map(|m| m.envelope.uid),
            was_reading: self.view == View::Reader,
        });
        // Stop trusting the dead session: go offline on the cache so navigation
        // still works during the reconnect window, then reconnect the account.
        self.idle_watchers.remove(&account);
        self.live = false;
        self.source = offline_source(&self.services, self.active_account_id().as_deref()).await;
        self.set_toast(
            format!("Connection lost - reconnecting {account}..."),
            ToastKind::Warning,
        );
        self.spawn_connect(self.active_account).await;
    }

    /// Called once per frame from the event loop; expires stale toasts.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|t| now < t.expires);
        // Let finished jobs linger briefly in the tracker, then drop them so the
        // window and the running-job indicator settle back to idle.
        self.jobs.prune(Duration::from_secs(10));
        if self.busy.is_some() || self.jobs.has_running() {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
        }
    }

    /// Ask the event loop to run the external editor on the message body.
    /// Header fields are edited inline in the TUI, so only the body round-trips
    /// through `$EDITOR`. (`config.compose.edit_headers` is now vestigial.)
    pub fn request_external_editor(&mut self) {
        let Some(compose) = &self.compose else { return };
        let current_text = compose.body.clone();
        let editor_cmd = self.config.compose.editor.clone();
        self.pending_external = Some(PendingExternalEditor {
            current_text,
            editor_cmd,
        });
    }

    pub fn take_pending_external(&mut self) -> Option<PendingExternalEditor> {
        self.pending_external.take()
    }

    /// Store the text returned by an external editor back as the compose body.
    pub fn apply_external_result(&mut self, result: Option<String>) {
        match (&mut self.compose, result) {
            (Some(compose), Some(text)) => {
                compose.body = text;
                compose.body_scroll = 0;
                self.set_toast("Draft updated from editor", ToastKind::Success);
            }
            _ => self.set_toast("Editor closed - draft unchanged", ToastKind::Info),
        }
    }

    pub(crate) async fn switch_to_account(&mut self, idx: usize) {
        if idx >= self.accounts.len() {
            self.set_toast("No such account", ToastKind::Warning);
            return;
        }
        self.active_account = idx;
        self.selected_folder = 0;
        self.view = View::Main;
        let name = self.accounts[idx].name.clone();
        // Show cached mail instantly, then (for authorized accounts) refresh
        // live in the background so the UI never freezes on the connect.
        self.show_cached_source().await;
        self.spawn_connect(idx).await;
        if self.busy.is_none() {
            // No live connect started (unauthorized): just acknowledge the switch.
            self.set_toast(format!("Now: {name}"), ToastKind::Info);
        }
    }

    /// Show cached mail for the active account instantly,
    /// without any network. Used for offline-first display and as the fallback
    /// when a live connection can't be established.
    async fn show_cached_source(&mut self) {
        // Arrival watchers are per-account and persist across switches, so the
        // focused account keeps being watched independently of this cached view.
        self.live = false;
        self.source = offline_source(&self.services, self.active_account_id().as_deref()).await;
        let folders = self.source.list_folders().await.unwrap_or_default();
        self.set_folders(folders).await;
        self.apply_cached_counts().await;
        self.load_folder(self.selected_folder).await;
    }

    async fn set_folders(&mut self, mut folders: Vec<Folder>) {
        // Carry over any counts we already have by name so the sidebar doesn't
        // blink to zero before the counts sweep lands.
        folders::carry_counts(&self.folders, &mut folders);
        self.folder_collapsed = vec![false; folders.len()];
        self.folders = folders;
        self.selected_folder = self
            .selected_folder
            .min(self.folders.len().saturating_sub(1));
    }

    /// Fill the sidebar with counts derived from the cached messages, so numbers
    /// show instantly (before the live STATUS sweep in `spawn_folder_counts`
    /// replaces them with the server truth). Only overwrites folders the cache
    /// knows about; unknown folders keep whatever `carry_counts` preserved.
    pub(crate) async fn apply_cached_counts(&mut self) {
        let Some(id) = self.active_account_id() else {
            return;
        };
        let counts = self
            .services
            .cache
            .count_by_folder(&id)
            .await
            .unwrap_or_default();
        let by_name: std::collections::HashMap<String, (usize, usize)> =
            counts.into_iter().map(|(n, t, u)| (n, (t, u))).collect();
        for f in self.folders.iter_mut() {
            if let Some(&(total, unread)) = by_name.get(&f.name) {
                f.total = total;
                f.unread = unread;
            }
        }
    }

    /// After a fast connect (folder names only), fetch each folder's
    /// (MESSAGES, UNSEEN) counts on a background task and deliver them as
    /// [`Event::FoldersCounted`]. A no-op without a live handle / event sender.
    fn spawn_folder_counts(&mut self) {
        let names: Vec<String> = self.folders.iter().map(|f| f.name.clone()).collect();
        if names.is_empty() {
            return;
        }
        let ctx = self.task_ctx();
        if ctx.handle.is_none() || ctx.event_tx.is_none() {
            return;
        }
        let job = self.begin_job(
            JobKind::FolderCounts,
            self.active_account_name().to_string(),
        );
        ctx.spawn_tracked(Effect::FolderCounts { names }, job);
    }

    /// Warm the offline cache for the active account's *other* folders (the
    /// current one is already loaded) by fetching each folder's envelopes on a
    /// background task via the retained live handle. Cache-only: a later switch
    /// to one of these folders shows it instantly, then refreshes.
    fn spawn_folder_sync_all(&mut self) {
        let current = self.selected_folder_name().to_string();
        let names: Vec<String> = self
            .folders
            .iter()
            .map(|f| f.name.clone())
            .filter(|n| n != &current)
            .collect();
        if names.is_empty() {
            return;
        }
        let ctx = self.task_ctx();
        if ctx.handle.is_none() || ctx.event_tx.is_none() || ctx.account_id.is_none() {
            return;
        }
        let job = self.begin_job(JobKind::FolderSync, self.active_account_name().to_string());
        ctx.spawn_tracked(Effect::SyncAllFolders { folders: names }, job);
    }

    /// Apply a background all-folders sync progress update (delivered as
    /// [`Event::SyncProgress`]). Stale results for a since-switched account are
    /// dropped; the bar clears once every folder is synced.
    pub fn on_sync_progress(&mut self, progress: event::SyncProgress) {
        if progress.account != self.active_account_name() {
            return;
        }
        if progress.done >= progress.total {
            self.sync = None;
        } else {
            self.sync = Some(progress);
        }
    }

    /// Apply background folder counts to the sidebar (and write them through to
    /// the cache). Stale results for a since-switched account are dropped.
    pub async fn on_folders_counted(&mut self, done: event::FoldersCounted) {
        if done.account != self.active_account_name() {
            return;
        }
        for (name, total, unread) in &done.counts {
            if let Some(f) = self.folders.iter_mut().find(|f| &f.name == name) {
                f.total = *total;
                f.unread = *unread;
            }
        }
        if let Some(id) = self.active_account_id() {
            let _ = self.services.cache.upsert_folders(&id, &self.folders).await;
        }
    }

    /// Kick off a live IMAP connect for `idx` in the background. The (slow) TLS +
    /// OAuth + auth handshake runs off the UI thread; when it finishes the result
    /// arrives as [`Event::Connected`] and is applied by [`Self::on_connected`].
    /// A no-op (cached view stands) when the account has no stored credentials or
    /// there is no event sender. Sets the `busy` label while the connect is live.
    pub(crate) async fn spawn_connect(&mut self, idx: usize) {
        // Foreground connect for the account the user is looking at: shows the
        // busy spinner and only syncs the current folder up front (fast). The
        // rest of its folders are synced in the background from `on_connected`.
        let want_folder = self.selected_folder_name().to_string();
        self.spawn_connect_inner(idx, true, false, want_folder, false)
            .await;
    }

    /// Connect the active account quietly (no busy spinner, no tracked job):
    /// used on open in daemon mode, where "connecting" is just linking to the
    /// daemon + loading the current folder - not an IMAP dial worth surfacing.
    async fn spawn_connect_quiet(&mut self, idx: usize) {
        let want_folder = self.selected_folder_name().to_string();
        self.spawn_connect_inner(idx, false, false, want_folder, true)
            .await;
    }

    /// Connect every authorized account on startup: the active one in the
    /// foreground, the rest in the background to warm their offline caches (all
    /// folders' envelopes), so switching to any account is instant. If the
    /// active account was saved but never authorized, queue the consent flow
    /// right away so the first open ends with live mail (the post-consent path
    /// connects and fetches the current mailbox automatically) instead of a
    /// manual trip through Settings -> Authorize.
    pub async fn connect_all_accounts(&mut self) {
        // Strict client: only the daemon talks to IMAP. Consent (browser + paste)
        // is client-side, so it is still offered when offline; only the actual
        // online work (connect + watchers) is gated on a running daemon.
        let daemon = self.services.daemon_backed;
        self.daemon_missing = !daemon;

        let active = self.active_account;
        if let Some(account) = self.accounts.get(active).cloned() {
            if matches!(self.connect_params(active).await, Ok(None)) {
                self.queue_auto_reauth(&account).await;
            }
            if daemon {
                // Quiet: linking to the daemon isn't an IMAP dial, so no job.
                self.spawn_connect_quiet(active).await;
                // Watch every authorized account for new mail, not just the
                // active one, so arrivals are notified regardless of focus.
                self.start_account_watcher(active).await;
            }
        }
        if daemon {
            // Only the active account is connected on open. The daemon already
            // keeps every other account's cache warm, so they show instantly from
            // cache and connect on demand when switched to (see `spawn_connect` in
            // the account-switch path) - no per-account "Warm up" jobs each open.
            // We still subscribe each for arrivals so new mail notifies regardless
            // of focus (a cheap push subscription, not an IMAP connection).
            for idx in 0..self.accounts.len() {
                if idx != active {
                    self.start_account_watcher(idx).await;
                }
            }
        } else {
            self.set_toast(
                "No pesan daemon running - start it with: pesan daemon".to_string(),
                ToastKind::Warning,
            );
        }
    }

    /// Core connect spawner. `set_busy` shows the status spinner; `sync_all`
    /// makes `build_live_data` fetch every folder's envelopes (used for
    /// background account warm-up, where the live source is discarded afterward).
    async fn spawn_connect_inner(
        &mut self,
        idx: usize,
        set_busy: bool,
        sync_all: bool,
        want_folder: String,
        quiet: bool,
    ) {
        let params = match self.connect_params(idx).await {
            Ok(Some(p)) => p,
            Ok(None) => return, // not authorized; cached display stands
            Err(e) => {
                if set_busy {
                    self.set_toast(format!("Connect config error: {e}"), ToastKind::Error);
                }
                return;
            }
        };
        // The effect may target a *non-active* account (background warm-up), so
        // override the identity snapshot for this spawn.
        let mut ctx = self.task_ctx();
        if let Some(a) = self.accounts.get(idx) {
            ctx.account = a.name.clone();
            ctx.account_id = a.id.clone();
        }
        if ctx.event_tx.is_none() {
            return;
        }
        let account = ctx.account.clone();
        if set_busy {
            self.busy = Some(format!("Connecting {account}..."));
        }
        // A quiet connect (on-open daemon link) shows no spinner and no tracked
        // job: it hands back an inert guard so nothing appears in the tracker.
        let job = if quiet {
            JobGuard::new(0, None)
        } else {
            let kind = if set_busy {
                JobKind::Connect
            } else {
                JobKind::Warmup
            };
            self.begin_job(kind, account)
        };
        // The live source (the foreground/active one) reports a wedged session so
        // the app can auto-reconnect. Dependency inversion: the worker only
        // sends the account label; this adapter turns it into an app event, so
        // `mail` never imports `event`. Kept hand-spawned (not an Effect): it is
        // transport plumbing, not orchestration.
        let Some(fwd_tx) = ctx.event_tx.clone() else {
            return;
        };
        let (lost_tx, mut lost_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(account) = lost_rx.recv().await {
                let _ = fwd_tx.send(Event::ConnectionLost(account));
            }
        });
        ctx.spawn_tracked(
            Effect::Connect {
                params,
                want_folder,
                sync_all_folders: sync_all,
                on_lost: Some(lost_tx),
            },
            job,
        );
    }

    /// Assemble IMAP/SMTP/OAuth connection parameters for an account, or
    /// `Ok(None)` when it has no stored refresh token (not yet authorized).
    pub(crate) async fn connect_params(
        &self,
        idx: usize,
    ) -> anyhow::Result<Option<connect_params::ConnectParams>> {
        let Some(account) = self.accounts.get(idx) else {
            return Ok(None);
        };
        // Single source of truth for credential assembly, shared with the
        // headless daemon (`crate::daemon`).
        let provider = self.config.provider_spec(&account.provider)?;
        account_connect_params::resolve_connect_params(
            &provider,
            account,
            self.services.tokens.as_ref(),
        )
        .await
    }

    /// Start (or restart) the background new-mail watcher for the active
    /// account. Dropping the previous watcher stops it. No-op without an event
    /// sender or stored credentials.
    /// Start (or replace) the background arrival watcher for one account. The
    /// watcher opens its own IMAP connection and polls/IDLEs the watched folder,
    /// pushing [`NewMail`] batches into the app event stream. It is started for
    /// every authorized account so new mail is pulled and notified even when the
    /// account is not in focus. The poll cadence comes from `daemon.poll_interval_secs`.
    async fn start_account_watcher(&mut self, idx: usize) {
        let Some(tx) = self.event_tx.clone() else {
            return;
        };
        let params = match self.connect_params(idx).await {
            Ok(Some(p)) => p,
            _ => return,
        };
        let account = params.label.clone();
        let mailbox = self
            .config
            .notifications
            .folders
            .first()
            .cloned()
            .unwrap_or_else(|| "INBOX".to_string());
        // Poll cadence is a daemon concern; the IPC watcher ignores it anyway.
        let poll = self.config.daemon.poll_interval();
        // Dependency inversion: the watcher (infrastructure) speaks the domain
        // `MailUpdate`; this adapter maps each variant to an app event, so `mail`
        // never imports `event`. Arrivals merge into the list; a folder re-sync
        // (from the daemon's periodic sync) replaces the viewed folder.
        let (mail_tx, mut mail_rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(update) = mail_rx.recv().await {
                let event = match update {
                    mail::MailUpdate::Arrived(nm) => event::Event::NewMail(nm),
                    mail::MailUpdate::FolderSynced {
                        account,
                        folder,
                        envelopes,
                    } => event::Event::FolderRefreshed(event::FolderRefreshed {
                        account,
                        folder,
                        envelopes,
                    }),
                };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        let handle = self
            .services
            .watcher
            .watch(params, mailbox, account.clone(), poll, mail_tx);
        self.idle_watchers.insert(account, handle);
    }
}
