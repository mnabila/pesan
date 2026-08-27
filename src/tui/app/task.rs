use tokio::sync::mpsc::UnboundedSender;

use super::Event;
use crate::application::Services;
use crate::application::mail::imap_cmd::ImapHandle;
use crate::application::outcome::Effect;

impl super::App {
    /// Snapshot the current identity into a spawn context. Call from a
    /// `&mut self` action right before scheduling an effect.
    pub(crate) fn task_ctx(&self) -> TaskCtx {
        TaskCtx {
            handle: self.source.imap_handle(),
            services: self.services.clone(),
            event_tx: self.event_tx.clone(),
            account: self.active_account_name().to_string(),
            account_id: self.active_account_id(),
        }
    }
}

/// Snapshot of the identity a spawned task needs: which account it acts for,
/// the live handle to read through, the ports, and where to report results.
/// Fields are adjustable so a caller can spawn for a *non-active* account
/// (e.g. a background warm-up connect).
pub(crate) struct TaskCtx {
    /// Live IMAP handle (`None` when offline - effects needing one no-op).
    pub handle: Option<ImapHandle>,
    pub services: Services,
    pub event_tx: Option<UnboundedSender<Event>>,
    pub account: String,
    pub account_id: Option<i64>,
}

impl TaskCtx {
    /// Schedule an untracked effect (no job-tracker entry; failures stay silent,
    /// surfacing only as stale data until the next refresh).
    pub fn spawn(&self, effect: Effect) {
        self.run(effect, None);
    }

    /// Schedule an effect tracked in the job window; the guard reports progress
    /// and completion (and turns failures into warning toasts).
    pub fn spawn_tracked(&self, effect: Effect, job: crate::tui::app::JobGuard) {
        self.run(effect, Some(job));
    }

    fn run(&self, effect: Effect, mut job: Option<crate::tui::app::JobGuard>) {
        let Some(tx) = self.event_tx.clone() else {
            return;
        };
        let ctx = self.clone_for();
        tokio::spawn(async move {
            match effect {
                Effect::RefreshFolder { folder, .. } => {
                    let Some(handle) = ctx.handle else {
                        return;
                    };
                    let envelopes = match handle.list_messages(&folder).await {
                        Ok(envelopes) => envelopes,
                        Err(e) => {
                            if let Some(job) = &mut job {
                                job.fail(format!("{e}"));
                                let _ =
                                    tx.send(Event::BackgroundError(format!("Refresh failed: {e}")));
                            }
                            return;
                        }
                    };
                    if let Some(id) = ctx.account_id {
                        let _ = ctx
                            .services
                            .cache
                            .upsert_envelopes(id, &folder, &envelopes)
                            .await;
                    }
                    let _ = tx.send(Event::FolderRefreshed(
                        crate::tui::app::event::FolderRefreshed {
                            account: ctx.account,
                            folder,
                            envelopes,
                        },
                    ));
                }
                Effect::FetchMessage { folder, env } => {
                    let Some(handle) = ctx.handle else {
                        return;
                    };
                    let uid = env.uid;
                    let flags = env.flags;
                    let message = match handle.fetch_message(&folder, uid).await {
                        Ok(mut msg) => {
                            msg.envelope.flags = flags;
                            if let Some(id) = ctx.account_id {
                                let _ = ctx
                                    .services
                                    .cache
                                    .store_body(
                                        id,
                                        &folder,
                                        uid,
                                        &msg.body,
                                        msg.raw_headers.as_deref(),
                                    )
                                    .await;
                            }
                            msg
                        }
                        // Surface the failure in the reader instead of leaving a
                        // stuck "Loading..." placeholder. Keeps the envelope so
                        // headers still show.
                        Err(e) => crate::domain::Message {
                            envelope: env,
                            body: format!(
                                "Could not load this message: {e}\n\nPress Esc, then open it again to retry."
                            ),
                            raw_headers: None,
                        },
                    };
                    let _ = tx.send(Event::MessageFetched(Box::new(
                        crate::tui::app::event::MessageFetched {
                            account: ctx.account,
                            folder,
                            uid,
                            message,
                        },
                    )));
                }
                Effect::DeleteOnServer { folder, uids, dest } => {
                    let Some(handle) = ctx.handle else {
                        return;
                    };
                    let count = uids.len();
                    let mut failed = 0usize;
                    for (i, uid) in uids.into_iter().enumerate() {
                        let res = match &dest {
                            Some(archive) => handle.move_to(&folder, uid, archive).await,
                            None => handle.delete(&folder, uid).await,
                        };
                        if res.is_err() {
                            failed += 1;
                        }
                        if let Some(job) = &mut job {
                            job.progress(i + 1, count);
                        }
                    }
                    if failed > 0 {
                        let verb = if dest.is_some() { "archive" } else { "delete" };
                        if let Some(job) = &mut job {
                            job.fail(format!("{failed} of {count} failed"));
                        }
                        let _ = tx.send(Event::BackgroundError(format!(
                            "Failed to {verb} {failed} message(s) on the server; they may reappear on refresh"
                        )));
                    }
                }
                Effect::LoadOlder { folder, offset } => {
                    let Some(handle) = ctx.handle else {
                        return;
                    };
                    // On failure, report an empty page with total 0 so the app
                    // clears the loading flag without marking the folder
                    // exhausted (retry stays on).
                    let window = handle
                        .list_messages_older(
                            &folder,
                            offset,
                            crate::application::mail::imap_cmd::LIST_WINDOW,
                        )
                        .await
                        .unwrap_or(crate::domain::MessageWindow {
                            envelopes: Vec::new(),
                            total: 0,
                        });
                    if let Some(id) = ctx.account_id
                        && !window.envelopes.is_empty()
                    {
                        let _ = ctx
                            .services
                            .cache
                            .upsert_envelopes(id, &folder, &window.envelopes)
                            .await;
                    }
                    let _ = tx.send(Event::OlderLoaded(crate::tui::app::event::OlderLoaded {
                        account: ctx.account,
                        folder,
                        envelopes: window.envelopes,
                        total: window.total,
                    }));
                }
                Effect::FolderCounts { names } => {
                    let Some(handle) = ctx.handle else {
                        return;
                    };
                    let counts = match handle.folder_counts(names).await {
                        Ok(counts) => counts,
                        Err(_) => {
                            if let Some(job) = &mut job {
                                job.fail("folder count sweep failed");
                            }
                            return;
                        }
                    };
                    let _ = tx.send(Event::FoldersCounted(
                        crate::tui::app::event::FoldersCounted {
                            account: ctx.account,
                            counts,
                        },
                    ));
                }
                Effect::Connect {
                    params,
                    want_folder,
                    sync_all_folders,
                    on_lost,
                } => {
                    let result = crate::application::account::connect::connect_account(
                        &ctx.services,
                        params,
                        ctx.account_id,
                        want_folder,
                        sync_all_folders,
                        on_lost,
                    )
                    .await;
                    if let Err(e) = &result
                        && let Some(job) = &mut job
                    {
                        job.fail(e.to_string());
                    }
                    let _ = tx.send(Event::Connected(Box::new(
                        crate::tui::app::event::Connected {
                            account: ctx.account,
                            result,
                        },
                    )));
                    // `job` drops here, reporting completion to the tracker.
                }
                Effect::SyncAllFolders { folders } => {
                    let Some(handle) = ctx.handle else {
                        return;
                    };
                    let total = folders.len();
                    for (i, name) in folders.into_iter().enumerate() {
                        // Low-priority: yields to interactive opens/switches on
                        // the shared connection so the reader never waits behind
                        // this sweep.
                        if let Ok(envs) = handle.list_messages_bg(&name).await
                            && let Some(id) = ctx.account_id
                        {
                            let _ = ctx.services.cache.upsert_envelopes(id, &name, &envs).await;
                        }
                        // Report progress so the status bar's sync bar advances;
                        // the final `done == total` event clears the bar.
                        if let Some(progress_tx) = ctx.event_tx.as_ref() {
                            let _ = progress_tx.send(Event::SyncProgress(
                                crate::tui::app::event::SyncProgress {
                                    account: ctx.account.clone(),
                                    done: i + 1,
                                    total,
                                },
                            ));
                        }
                        // Mirror the progress into the job tracker.
                        if let Some(job) = &mut job {
                            job.progress(i + 1, total);
                        }
                    }
                    // `job` drops here, reporting completion to the tracker.
                }
            }
        });
    }

    /// A cheap per-task clone (ports are `Arc`s, the handle is a channel pair).
    fn clone_for(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            services: self.services.clone(),
            event_tx: self.event_tx.clone(),
            account: self.account.clone(),
            account_id: self.account_id,
        }
    }
}
