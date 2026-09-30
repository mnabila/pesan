use super::*;

use crate::mail::application::mutations;
use crate::account::application::onboarding;

impl App {
    /// Toggle the tag on the selected message and advance one row, so tagging a
    /// run of messages with Space is quick.
    pub(crate) fn toggle_mark_selected(&mut self) {
        let Some(uid) = self.selected_env().map(|e| e.uid) else {
            return;
        };
        if !self.marked.insert(uid) {
            self.marked.remove(&uid);
        }
        if self.selected_message + 1 < self.display_envelopes.len() {
            self.selected_message += 1;
        }
    }

    /// UIDs currently visible in the list that are tagged.
    fn marked_in_view(&self) -> Vec<u64> {
        self.display_envelopes
            .iter()
            .map(|e| e.uid)
            .filter(|u| self.marked.contains(u))
            .collect()
    }

    /// How many messages a delete/archive would act on: the tagged set when any
    /// are tagged, otherwise the single selected message.
    pub(crate) fn delete_target_count(&self) -> usize {
        let marked = self.marked_in_view();
        if marked.is_empty() {
            usize::from(self.selected_env().is_some())
        } else {
            marked.len()
        }
    }

    pub(crate) async fn toggle_self_selected_flag(&mut self, flagged: bool) {
        let Some(env) = self.selected_env().cloned() else {
            return;
        };
        let uid = env.uid;
        let new_flags = if flagged {
            !env.flags.flagged
        } else {
            !env.flags.seen
        };
        let folder = self.selected_folder_name().to_string();
        // Live: push the flag change with the explicit folder so the worker
        // SELECTs the right mailbox. Offline: act on the cache source.
        match self.source.imap_handle() {
            Some(handle) => {
                let _ = if flagged {
                    handle.set_flagged(&folder, uid, new_flags).await
                } else {
                    handle.set_seen(&folder, uid, new_flags).await
                };
            }
            None if flagged => self.source.set_flagged(uid, new_flags).await,
            None => self.source.set_seen(uid, new_flags).await,
        }
        if self.live
            && let Some(id) = self.active_account_id()
        {
            let (seen, flag) = if flagged {
                (None, Some(new_flags))
            } else {
                (Some(new_flags), None)
            };
            let _ = self
                .services
                .cache
                .set_flags(&id, &folder, uid, seen, flag)
                .await;
        }
        if let Some(msg) = &mut self.open_message
            && msg.envelope.uid == uid
        {
            if flagged {
                msg.envelope.flags.flagged = new_flags;
            } else {
                msg.envelope.flags.seen = new_flags;
            }
        }
        if let Some(e) = self
            .envelopes
            .iter_mut()
            .find(|e2| e2.uid == uid)
            .or_else(|| self.display_envelopes.iter_mut().find(|e2| e2.uid == uid))
        {
            if flagged {
                e.flags.flagged = new_flags;
            } else {
                e.flags.seen = new_flags;
            }
        }
        self.refresh_folder_unread_counts();
    }

    /// Mark the opened message `\Seen` (server + local state) when it was
    /// unread. Called on OpenMessage; a no-op for already-read mail.
    pub(crate) async fn mark_selected_seen(&mut self) {
        let Some(env) = self.selected_env().cloned() else {
            return;
        };
        if env.flags.seen {
            return;
        }
        let uid = env.uid;
        let folder = self.selected_folder_name().to_string();
        // Update local state + cache immediately, then push the `\Seen` flag to
        // the server. When live the server call runs on a background task so
        // opening a message never blocks on the network; offline it's a fast
        // local no-op through the cache source.
        if self.live {
            if let Some(handle) = self.source.imap_handle() {
                let f = folder.clone();
                tokio::spawn(async move {
                    let _ = handle.set_seen(&f, uid, true).await;
                });
            }
            if let Some(id) = self.active_account_id() {
                let _ = self
                    .services
                    .cache
                    .set_flags(&id, &folder, uid, Some(true), None)
                    .await;
            }
        } else {
            self.source.set_seen(uid, true).await;
        }
        if let Some(m) = &mut self.open_message
            && m.envelope.uid == uid
        {
            m.envelope.flags.seen = true;
        }
        if let Some(e) = self.envelopes.iter_mut().find(|e| e.uid == uid) {
            e.flags.seen = true;
        }
        if let Some(e) = self.display_envelopes.iter_mut().find(|e| e.uid == uid) {
            e.flags.seen = true;
        }
        self.refresh_folder_unread_counts();
    }

    pub(crate) async fn archive_selected(&mut self) {
        // Archive the tagged set when any are tagged, else the selected message.
        let mut targets = self.marked_in_view();
        if targets.is_empty() {
            targets.extend(self.selected_env().map(|e| e.uid));
        }
        if targets.is_empty() {
            return;
        }
        let current = self.selected_folder_name().to_string();
        // Move to an Archive folder when one exists; otherwise fall back to a
        // plain delete from the current mailbox (e.g. Gmail-style archive).
        let has_archive = self
            .folders
            .iter()
            .any(|f| f.name.eq_ignore_ascii_case("Archive"));
        let mutation = if has_archive && !current.eq_ignore_ascii_case("Archive") {
            mutations::Mutation::MoveTo("Archive".to_string())
        } else {
            mutations::Mutation::Delete
        };
        let n_targets = targets.len();
        let plan = if self.live {
            // Optimistic: drop the rows now, do the server moves/deletes in the
            // background so a bulk archive never blocks the UI.
            mutations::DeletePlan::online(&current, &mutation, &targets)
        } else {
            // Offline: act on the cache source directly (fast, no network).
            let mut succeeded = Vec::new();
            for uid in targets {
                let result = match &mutation {
                    mutations::Mutation::MoveTo(dest) => {
                        self.source.move_to(uid, dest).await
                    }
                    mutations::Mutation::Delete => {
                        self.source.delete(uid).await
                    }
                };
                if result.is_ok() {
                    self.marked.remove(&uid);
                    succeeded.push(uid);
                }
            }
            mutations::DeletePlan::offline(succeeded)
        };
        for uid in &plan.remove_now {
            self.marked.remove(uid);
            self.remove_local(*uid).await;
        }
        if let Some(effect) = plan.effect {
            self.spawn_server_effect(effect, JobKind::ServerArchive);
        }
        let n = if plan.optimistic {
            n_targets
        } else {
            plan.remove_now.len()
        };
        self.set_toast(format!("Archived {n} message(s)"), ToastKind::Success);
    }

    /// Schedule a server-side mutation effect as a tracked job of `kind`.
    fn spawn_server_effect(&mut self, effect: Effect, kind: JobKind) {
        let job = self.begin_job(kind, self.active_account_name().to_string());
        self.task_ctx().spawn_tracked(effect, job);
    }

    /// Drop a message from the in-memory lists after a successful server-side
    /// delete/move, keeping the selection and unread counts consistent.
    async fn remove_local(&mut self, uid: u64) {
        if self.live
            && let Some(id) = self.active_account_id()
        {
            let folder = self.selected_folder_name().to_string();
            let _ = self.services.cache.delete_message(&id, &folder, uid).await;
        }
        let idx = self.display_envelopes.iter().position(|e| e.uid == uid);
        self.envelopes.retain(|e| e.uid != uid);
        self.display_envelopes.retain(|e| e.uid != uid);
        if let Some(ix) = idx {
            self.selected_message = if self.display_envelopes.is_empty() {
                0
            } else {
                ix.min(self.display_envelopes.len() - 1)
            };
        }
        // If the removed message was the one being read, drop back to the list -
        // there is nothing left to show in the reader.
        if self.open_message.as_ref().map(|m| m.envelope.uid) == Some(uid) {
            self.set_open_message(None);
            if self.view == View::Reader {
                self.view = View::Main;
            }
        }
        self.refresh_folder_unread_counts();
    }

    pub(crate) fn refresh_folder_unread_counts(&mut self) {
        let name = self.selected_folder_name().to_string();
        let unread = self.envelopes.iter().filter(|e| !e.flags.seen).count();
        if let Some(f) = self.folders.iter_mut().find(|f| f.name == name) {
            f.unread = unread;
        }
    }

    pub(crate) async fn confirm_yes(&mut self) {
        let Some(confirm) = &self.confirm else { return };
        let action = confirm.action;
        self.confirm = None;
        match action {
            ConfirmAction::DeleteMessage => {
                // Delete the tagged set when any are tagged, else the selected one.
                let mut targets = self.marked_in_view();
                if targets.is_empty() {
                    targets.extend(self.selected_env().map(|e| e.uid));
                }
                if targets.is_empty() {
                    return;
                }
                let n = targets.len();
                let folder = self.selected_folder_name().to_string();
                let plan = if self.live {
                    // Optimistic: remove the rows + cache entries now, delete on
                    // the server in the background so a bulk delete never blocks
                    // the UI.
                    mutations::DeletePlan::online(
                        &folder,
                        &mutations::Mutation::Delete,
                        &targets,
                    )
                } else {
                    // Offline: delete straight from the cache (fast, no network).
                    let mut succeeded = Vec::new();
                    for uid in targets {
                        if self.source.delete(uid).await.is_ok() {
                            self.marked.remove(&uid);
                            succeeded.push(uid);
                        }
                    }
                    mutations::DeletePlan::offline(succeeded)
                };
                for uid in &plan.remove_now {
                    self.marked.remove(uid);
                    self.remove_local(*uid).await;
                }
                if let Some(effect) = plan.effect {
                    self.spawn_server_effect(effect, JobKind::ServerDelete);
                }
                self.set_toast(format!("Deleted {n} message(s)"), ToastKind::Success);
            }
            ConfirmAction::DiscardDraft => {
                self.compose = None;
                self.view = View::Main;
                self.set_toast("Draft discarded", ToastKind::Info);
            }
            ConfirmAction::DeleteAccount(idx) => {
                if let Some(state) = &mut self.settings
                    && let Some(account) = state.accounts.get(idx).cloned()
                    && let Some(account_id) = account.id
                {
                    // Remove the row and the account's secrets (keychain first,
                    // DB fallback cleared too).
                    let _ = onboarding::delete_account(
                        self.services.accounts.as_ref(),
                        self.services.tokens.as_ref(),
                        &account_id,
                        &account.keychain_ref,
                    )
                    .await;
                    self.reload_accounts().await;
                    self.set_toast(
                        format!("Account \"{}\" deleted", account.name),
                        ToastKind::Success,
                    );
                }
            }
        }
    }
}
