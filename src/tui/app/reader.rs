use super::*;

impl App {
    /// Apply a batch of newly-arrived messages: merge them into the inbox view
    /// (when the inbox is what's showing) and raise one coalesced desktop
    /// notification. Envelopes already present (by uid) are ignored.
    pub async fn on_new_mail(&mut self, batch: NewMail) {
        let is_active = batch.account == self.active_account_name();
        // The mailbox the arrivals landed in (carried by the watcher/daemon push),
        // so the right folder's list and counters update - not just INBOX.
        let arrival_folder = batch.folder.clone();

        // Persist new arrivals to the cache for the owning account, so they show
        // up when the user switches to that account and while offline.
        if let Some(id) = self
            .accounts
            .iter()
            .find(|a| a.name == batch.account)
            .and_then(|a| a.id)
        {
            let folder = arrival_folder.clone();
            if let Ok(cached) = self.services.cache.load_envelopes(id, &folder).await {
                let fresh: Vec<Envelope> = batch
                    .envelopes
                    .iter()
                    .filter(|e| cached.iter().all(|x| x.uid != e.uid))
                    .cloned()
                    .collect();
                if !fresh.is_empty() {
                    let _ = self
                        .services
                        .cache
                        .upsert_envelopes(id, &folder, &fresh)
                        .await;
                }
            }
        }

        // Non-focused accounts: surface an in-app toast (so the user sees it even
        // without a desktop notification daemon) and raise the desktop notify,
        // then let the cache above carry the arrivals. The focused account's UI
        // is updated below.
        if !is_active {
            self.notify_new_mail(&batch.account, &batch.envelopes);
            self.set_toast(
                format!("{}: {} new message(s)", batch.account, batch.envelopes.len()),
                ToastKind::Info,
            );
            return;
        }

        let fresh: Vec<Envelope> = batch
            .envelopes
            .into_iter()
            .filter(|e| self.envelopes.iter().all(|x| x.uid != e.uid))
            .collect();
        if fresh.is_empty() {
            return;
        }

        let viewing_folder = self.selected_folder_name().eq_ignore_ascii_case(&arrival_folder);
        if viewing_folder {
            // Newest first: prepend, keeping the current selection stable by uid.
            let selected_uid = self.selected_env().map(|e| e.uid);
            for env in fresh.iter().rev() {
                self.envelopes.insert(0, env.clone());
            }
            let filter = self.search.as_ref().map(|s| s.input.text().to_string());
            self.refresh_display_list(filter.as_deref());
            if let Some(uid) = selected_uid
                && let Some(pos) = self.display_envelopes.iter().position(|e| e.uid == uid)
            {
                self.selected_message = pos;
            }
        }

        // Bump the arrival folder's counters regardless of the active folder.
        if let Some(folder) = self
            .folders
            .iter_mut()
            .find(|f| f.name.eq_ignore_ascii_case(&arrival_folder))
        {
            folder.total += fresh.len();
            folder.unread += fresh.iter().filter(|e| !e.flags.seen).count();
        }

        self.notify_new_mail(&batch.account, &fresh);
        self.set_toast(format!("{} new message(s)", fresh.len()), ToastKind::Info);

        // Newly-arrived mail is today's by definition: pre-warm its bodies on the
        // background queue so the user opens it instantly.
        let cap = self.config.daemon.prefetch_cap();
        if self.live && viewing_folder && cap > 0 {
            let uids: Vec<u64> = fresh.iter().take(cap).map(|e| e.uid).collect();
            self.task_ctx().spawn(Effect::PrefetchBodies {
                folder: self.selected_folder_name().to_string(),
                uids,
            });
        }
    }

    /// Apply a background folder refresh (live `list_messages` done off the UI
    /// loop). Stale results - a different account or folder is now showing - are
    /// dropped. The highlighted message is kept (by uid) across the swap.
    pub async fn on_folder_refreshed(&mut self, done: crate::tui::app::event::FolderRefreshed) {
        if done.account != self.active_account_name() || done.folder != self.selected_folder_name()
        {
            return;
        }
        let selected_uid = self.selected_env().map(|e| e.uid);
        self.envelopes = done.envelopes;
        // This is the newest window again; any paged-in older mail is dropped
        // from memory (still cached), so paging restarts from the bottom.
        self.older_exhausted = false;
        let filter = self.search.as_ref().map(|s| s.input.text().to_string());
        self.refresh_display_list(filter.as_deref());
        if let Some(uid) = selected_uid
            && let Some(pos) = self.display_envelopes.iter().position(|e| e.uid == uid)
        {
            self.selected_message = pos;
        }
        self.refresh_folder_unread_counts();
        // Pre-warm today's bodies from the fresh list so opening a recent message
        // is instant; already-cached ones are skipped in the effect.
        self.spawn_prefetch_today(&done.folder);
    }

    /// Apply a background message-body fetch. Dropped if the user has moved off
    /// this account/folder/message in the meantime (so a slow fetch never
    /// clobbers a message the user has since navigated to).
    pub async fn on_message_fetched(&mut self, done: crate::tui::app::event::MessageFetched) {
        if done.account != self.active_account_name() || done.folder != self.selected_folder_name()
        {
            return;
        }
        if self.open_message.as_ref().map(|m| m.envelope.uid) == Some(done.uid) {
            self.set_open_message(Some(done.message));
        }
    }

    /// Fire one coalesced desktop notification for a batch, honoring config.
    /// `account` is the owning account's name (not necessarily the focused one).
    fn notify_new_mail(&self, account: &str, envelopes: &[Envelope]) {
        if !self.config.notifications.enabled || envelopes.is_empty() {
            return;
        }
        let sound = self.config.notifications.sound_hint();
        if let Err(e) = self.services.notifier.new_mail_batch(
            account,
            envelopes,
            self.config.notifications.show_sender,
            sound.as_ref(),
        ) {
            tracing::warn!("desktop notification failed: {e}");
        }
    }

    pub(crate) async fn load_folder(&mut self, idx: usize) {
        if idx >= self.folders.len() {
            return;
        }
        let name = self.folders[idx].name.clone();
        self.selected_folder = idx;
        self.sidebar_sel = SidebarItem::Folder(idx);
        // Cache-first: show whatever is cached for this folder instantly (a fast
        // local SQLite read), never blocking on the network. When live, a fresh
        // copy is fetched in the background and swapped in via a `FolderRefreshed`
        // event. Offline, the cached view is all there is.
        self.envelopes = self.cached_envelopes(&name).await;
        self.selected_message = 0;
        self.reader_offset = 0;
        self.set_open_message(None);
        self.search = None;
        self.marked.clear(); // tags are folder-scoped
        self.loading_older = false;
        self.older_exhausted = false;
        self.refresh_display_list(None);
        self.spawn_folder_refresh(&name);
        // Warm today's bodies from whatever is cached now; the refresh below
        // re-runs this with the fresh list once it lands.
        self.spawn_prefetch_today(&name);
    }

    /// Read a folder's envelopes straight from the offline cache. Returns an
    /// empty list when the account has no id or nothing is cached yet.
    async fn cached_envelopes(&self, folder: &str) -> Vec<Envelope> {
        let Some(id) = self.active_account_id() else {
            // No account row (e.g. the empty source): fall back to the source.
            return self.source.list_messages(folder).await.unwrap_or_default();
        };
        self.services
            .cache
            .load_envelopes(id, folder)
            .await
            .unwrap_or_default()
    }

    /// When live, fetch the folder's messages from IMAP on a background task and
    /// deliver them as an [`Event::FolderRefreshed`]; the cache is written
    /// through there too. A no-op when offline or without an event sender.
    fn spawn_folder_refresh(&self, folder: &str) {
        if !self.live {
            return;
        }
        self.task_ctx().spawn(Effect::RefreshFolder {
            folder: folder.to_string(),
        });
    }

    /// Warm the cache with the bodies of messages that arrived today, so opening
    /// a recent one is instant (a cache hit) instead of waiting on a live fetch.
    /// Runs on the low-priority queue, so an interactive open always jumps ahead;
    /// already-cached messages are skipped in the effect. Bounded by
    /// [`PREFETCH_TODAY_CAP`] so a busy day never floods the worker. A no-op when
    /// offline.
    fn spawn_prefetch_today(&self, folder: &str) {
        if !self.live {
            return;
        }
        let cap = self.config.daemon.prefetch_cap();
        if cap == 0 {
            return;
        }
        let uids: Vec<u64> = self
            .envelopes
            .iter()
            .filter(|e| is_today(e.date))
            .take(cap)
            .map(|e| e.uid)
            .collect();
        if uids.is_empty() {
            return;
        }
        self.task_ctx().spawn(Effect::PrefetchBodies {
            folder: folder.to_string(),
            uids,
        });
    }

    /// Fetch the next older page of the current folder from IMAP on a background
    /// task, delivered as [`Event::OlderLoaded`] and written through to the
    /// cache. A no-op when offline, while a page is already loading, once the
    /// oldest message has been reached, or in a filtered/search view (the offset
    /// is derived from the full list, so paging a narrowed view is disabled).
    async fn load_more_older(&mut self) {
        if !self.live || self.loading_older || self.older_exhausted {
            return;
        }
        // Only page the unfiltered list: a search narrows `display_envelopes`, so
        // its length no longer maps to how many newest messages are loaded.
        if self.display_envelopes.len() != self.envelopes.len() {
            return;
        }
        let ctx = self.task_ctx();
        if ctx.handle.is_none() || ctx.event_tx.is_none() {
            return;
        }
        let folder = self.selected_folder_name().to_string();
        let offset = self.envelopes.len() as u32;
        self.loading_older = true;
        ctx.spawn(Effect::LoadOlder { folder, offset });
    }

    /// Apply a "load older messages" page. Merges the older batch into the list
    /// (de-duplicating by uid), keeps the highlighted message stable, and marks
    /// the folder exhausted once every server message is held. Dropped if the
    /// user has switched account/folder in the meantime.
    pub async fn on_older_loaded(&mut self, done: crate::tui::app::event::OlderLoaded) {
        if done.account != self.active_account_name() || done.folder != self.selected_folder_name()
        {
            // Stale page for a view we've left; the switch already reset the flag.
            return;
        }
        self.loading_older = false;
        if done.envelopes.is_empty() {
            // A non-empty total with no rows means we paged past the oldest
            // message; total 0 signals a failed fetch, so leave paging enabled.
            if done.total > 0 {
                self.older_exhausted = true;
            }
            return;
        }
        let selected_uid = self.selected_env().map(|e| e.uid);
        self.merge_older_envelopes(done.envelopes);
        if self.envelopes.len() as u32 >= done.total {
            self.older_exhausted = true;
        }
        let filter = self.search.as_ref().map(|s| s.input.text().to_string());
        self.refresh_display_list(filter.as_deref());
        if let Some(uid) = selected_uid
            && let Some(pos) = self.display_envelopes.iter().position(|e| e.uid == uid)
        {
            self.selected_message = pos;
        }
        self.refresh_folder_unread_counts();
    }

    /// Merge an older page of envelopes into `self.envelopes`, de-duplicating by
    /// uid (existing entries win, as they may carry fresher flags) and keeping
    /// the newest-first ordering the list and cache both use.
    fn merge_older_envelopes(&mut self, older: Vec<Envelope>) {
        let known: HashSet<u64> = self.envelopes.iter().map(|e| e.uid).collect();
        for env in older {
            if !known.contains(&env.uid) {
                self.envelopes.push(env);
            }
        }
        self.envelopes.sort_by_key(|e| std::cmp::Reverse(e.date));
    }

    /// Manually re-fetch the currently selected mailbox from the server and
    /// write it through to the cache, preserving the highlighted message (by uid)
    /// when it survives the refresh. When the account isn't live yet this kicks
    /// off a background (re)connect instead. The message-list fetch is a single,
    /// fast round-trip, so it runs inline.
    pub(crate) async fn refresh_current_folder(&mut self) {
        if !self.live {
            self.set_toast("Offline - reconnecting...", ToastKind::Info);
            self.spawn_connect(self.active_account).await;
            return;
        }
        if self.folders.is_empty() {
            return;
        }
        let folder = self.selected_folder_name().to_string();
        // Non-blocking: the fetch runs on a background task tracked as a job, so
        // the event loop keeps drawing/handling input. Fresh envelopes come back
        // as `Event::FolderRefreshed` (cursor preserved by uid, cache written
        // through); the toast plus the job tracker are the interactive feedback.
        self.set_toast(format!("Refreshing {folder}..."), ToastKind::Info);
        self.spawn_folder_refresh_job(&folder);
    }

    /// Re-fetch a folder from the live server on a background task, tracked as a
    /// [`JobKind::Refresh`] job so the round-trip never blocks the UI loop. On
    /// success the fresh envelopes are written through to the cache and delivered
    /// as an [`Event::FolderRefreshed`]; on failure the job is marked failed and
    /// a warning toast is raised. A no-op without a live handle / event sender.
    fn spawn_folder_refresh_job(&mut self, folder: &str) {
        let ctx = self.task_ctx();
        if ctx.handle.is_none() || ctx.event_tx.is_none() {
            return;
        }
        let job = self.begin_job(JobKind::Refresh, self.active_account_name().to_string());
        ctx.spawn_tracked(
            Effect::RefreshFolder {
                folder: folder.to_string(),
            },
            job,
        );
    }

    pub(crate) fn refresh_display_list(&mut self, filter: Option<&str>) {
        self.display_envelopes = match filter.filter(|f| !f.is_empty()) {
            Some(term) => {
                let t = term.to_lowercase();
                self.envelopes
                    .iter()
                    .filter(|e| {
                        e.subject.to_lowercase().contains(&t)
                            || e.from.short().to_lowercase().contains(&t)
                            || e.from.email.to_lowercase().contains(&t)
                            || e.snippet
                                .as_ref()
                                .is_some_and(|s| s.to_lowercase().contains(&t))
                    })
                    .cloned()
                    .collect()
            }
            None => self.envelopes.clone(),
        };
        if self.selected_message >= self.display_envelopes.len() {
            self.selected_message = self.display_envelopes.len().saturating_sub(1);
        }
    }

    /// Apply a search term. When the account has cached mail, this runs a
    /// full-text search over the *whole* cached folder (subject/sender/body);
    /// otherwise it filters the in-memory list (subject/sender/snippet).
    pub(crate) async fn run_search(&mut self, term: &str) {
        if term.is_empty() {
            self.refresh_display_list(None);
            return;
        }
        let folder = self.selected_folder_name().to_string();
        match crate::application::mail::search::search_cache(
            &self.services,
            self.active_account_id(),
            &folder,
            term,
        )
        .await
        {
            Ok(Some(results)) => {
                self.display_envelopes = results;
                self.selected_message = 0;
            }
            Ok(None) => self.refresh_display_list(Some(term)),
            Err(e) => {
                tracing::warn!("cache search failed, filtering in-memory: {e}");
                self.refresh_display_list(Some(term));
            }
        }
    }

    pub(crate) async fn load_selected_preview(&mut self) {
        let Some(env) = self.selected_env().cloned() else {
            self.set_open_message(None);
            return;
        };
        let uid = env.uid;
        let folder = self.selected_folder_name().to_string();

        // Stale-while-revalidate: the cached copy (or a "Loading..." placeholder)
        // shows immediately, and when live a background refetch swaps in the
        // fresh copy via `Event::MessageFetched`. A *complete* cached copy
        // (body + full headers) skips the refetch - bodies are immutable
        // server-side, so re-downloading one is pure waste.
        let live = self.live && self.source.imap_handle().is_some();
        let out = crate::application::mail::open::open_message(
            &self.services,
            self.active_account_id(),
            &folder,
            &env,
            live,
        )
        .await;
        let showed_cached = out.immediate.is_some();
        if let Some(msg) = out.immediate {
            self.set_open_message(Some(msg));
        }
        if let Some(effect) = out.effect {
            self.task_ctx().spawn(effect);
        } else if !live && !showed_cached {
            // Offline with nothing cached: best-effort direct read.
            let fetched = match self.source.fetch_message(uid).await {
                Ok(mut msg) => {
                    msg.envelope.flags = env.flags;
                    Some(msg)
                }
                Err(_) => None,
            };
            self.set_open_message(fetched);
        }
    }

    /// Refetch the currently open message so its full RFC822 header block is
    /// available. Cached messages are stored without raw headers, so the reader
    /// can only show them after pulling the message from the live server. The
    /// message on screen is left untouched if the fetch fails.
    pub(crate) async fn reload_open_headers(&mut self) {
        let Some(uid) = self.open_message.as_ref().map(|m| m.envelope.uid) else {
            return;
        };
        let folder = self.selected_folder_name().to_string();
        // When live, fetch via the handle with the explicit folder so the worker
        // SELECTs the right mailbox (a background sweep may have moved it); offline
        // the cache source reads by its tracked folder.
        let fetched = match self.source.imap_handle() {
            Some(handle) => handle.fetch_message(&folder, uid).await,
            None => self.source.fetch_message(uid).await,
        };
        match fetched {
            Ok(mut msg) => {
                // Keep the flags we already track for this envelope.
                if let Some(env) = self.envelopes.iter().find(|e| e.uid == uid) {
                    msg.envelope.flags = env.flags;
                }
                if let Some(id) = self.active_account_id() {
                    let _ = self
                        .services
                        .cache
                        .store_body(id, &folder, uid, &msg.body, msg.raw_headers.as_deref())
                        .await;
                }
                self.set_open_message(Some(msg));
            }
            Err(e) => self.set_toast(format!("Could not load headers: {e}"), ToastKind::Warning),
        }
    }

    pub(crate) async fn select_message(&mut self, idx: usize) {
        let prev = self.selected_message;
        let max = self.display_envelopes.len().saturating_sub(1);
        self.selected_message = idx.min(max);
        if prev != self.selected_message {
            // No body fetch here: bodies load only when a message is opened, so
            // navigating the list never blocks on the network.
            self.reader_offset = 0;
        }
    }

    pub(crate) async fn move_message(&mut self, delta: isize) {
        if self.display_envelopes.is_empty() {
            return;
        }
        let next = self.selected_message as isize + delta;
        let next = next.clamp(0, self.display_envelopes.len() as isize - 1) as usize;
        self.select_message(next).await;
        // Infinite scroll: when moving down and the cursor nears the bottom of
        // the loaded list, prefetch the next older page in the background so the
        // rows are already there by the time the user scrolls to them.
        if delta > 0 {
            self.maybe_prefetch_older().await;
        }
    }

    /// Kick off a background fetch of the next older page once the cursor is
    /// within [`PREFETCH_ROWS`] of the bottom of the loaded list. Cheap to call
    /// repeatedly: [`Self::load_more_older`] no-ops when a page is already in
    /// flight, exhausted, offline, or filtered.
    pub(crate) async fn maybe_prefetch_older(&mut self) {
        let remaining = self
            .display_envelopes
            .len()
            .saturating_sub(self.selected_message + 1);
        if remaining <= PREFETCH_ROWS {
            self.load_more_older().await;
        }
    }
}
