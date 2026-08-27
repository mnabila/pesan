use super::*;

impl App {
    /// The visible sidebar tree: every account header, with the active account's
    /// folders listed under it. With no accounts, just the folders.
    pub fn sidebar_items(&self) -> Vec<SidebarItem> {
        let mut items = Vec::new();
        if self.accounts.is_empty() {
            items.extend(
                self.visible_folder_indices()
                    .into_iter()
                    .map(SidebarItem::Folder),
            );
            return items;
        }
        let visible = self.sidebar_filtered_accounts();
        for i in 0..self.accounts.len() {
            if visible.contains(&i) {
                items.push(SidebarItem::Account(i));
            }
            // The active account's folders sit under it, but are hidden while the
            // user is typing an account filter (account-picking mode).
            if i == self.active_account && !self.sidebar_filtering {
                items.extend(
                    self.visible_folder_indices()
                        .into_iter()
                        .map(SidebarItem::Folder),
                );
            }
        }
        items
    }

    /// Account indices whose name/email match the sidebar filter. An empty
    /// filter matches every account.
    pub fn sidebar_filtered_accounts(&self) -> Vec<usize> {
        let q = self.sidebar_filter.trim().to_lowercase();
        if q.is_empty() {
            return (0..self.accounts.len()).collect();
        }
        self.accounts
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                a.name.to_lowercase().contains(&q) || a.email.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Enter account-filter mode in the mailbox sidebar, parking the cursor on
    /// the first account so the highlight is visible as the user types.
    pub(crate) fn begin_sidebar_filter(&mut self) {
        if self.accounts.is_empty() {
            return;
        }
        self.sidebar_filtering = true;
        self.sidebar_filter.clear();
        self.sidebar_sel = SidebarItem::Account(self.active_account);
    }

    /// Keep the sidebar cursor on a visible (filtered) account as the query
    /// changes.
    fn sidebar_clamp_sel(&mut self) {
        let visible = self.sidebar_filtered_accounts();
        if visible.is_empty() {
            return;
        }
        let on_match = matches!(self.sidebar_sel, SidebarItem::Account(i) if visible.contains(&i));
        if !on_match {
            self.sidebar_sel = SidebarItem::Account(visible[0]);
        }
    }

    /// Keystroke handling while the sidebar account filter is open. Printable
    /// keys edit the query; Up/Down move between matches; Enter switches to the
    /// highlighted account; Esc cancels.
    pub(crate) async fn sidebar_filter_key(&mut self, key: &Key) {
        match key.code {
            KeyCode::Esc => {
                self.sidebar_filter.clear();
                self.sidebar_filtering = false;
            }
            KeyCode::Enter => {
                self.sidebar_filtering = false;
                self.sidebar_filter.clear();
                if let SidebarItem::Account(i) = self.sidebar_sel
                    && i != self.active_account
                {
                    self.switch_to_account(i).await;
                }
            }
            KeyCode::Up => self.move_sidebar(-1).await,
            KeyCode::Down => self.move_sidebar(1).await,
            KeyCode::Backspace => {
                self.sidebar_filter.pop();
                self.sidebar_clamp_sel();
            }
            KeyCode::Char(c) if !key.ctrl => {
                self.sidebar_filter.push(c);
                self.sidebar_clamp_sel();
            }
            _ => {}
        }
    }

    async fn apply_sidebar_sel(&mut self, item: SidebarItem) {
        self.sidebar_sel = item;
        // Moving onto a folder loads it (as folder navigation always has);
        // moving onto an account header only highlights it.
        if let SidebarItem::Folder(f) = item {
            self.load_folder(f).await;
        }
    }

    /// Move the sidebar cursor through the account/folder tree.
    pub(crate) async fn move_sidebar(&mut self, delta: isize) {
        let items = self.sidebar_items();
        if items.is_empty() {
            return;
        }
        let cur = items
            .iter()
            .position(|it| *it == self.sidebar_sel)
            .unwrap_or(0);
        let next = (cur as isize + delta).clamp(0, items.len() as isize - 1) as usize;
        self.apply_sidebar_sel(items[next]).await;
    }

    pub(crate) async fn sidebar_first_last(&mut self, first: bool) {
        let items = self.sidebar_items();
        if items.is_empty() {
            return;
        }
        let item = if first {
            items[0]
        } else {
            items[items.len() - 1]
        };
        self.apply_sidebar_sel(item).await;
    }

    /// Enter on the sidebar: switch to the account, or open the folder (and jump
    /// focus to the message list).
    pub(crate) async fn activate_sidebar(&mut self) {
        match self.sidebar_sel {
            SidebarItem::Account(i) => {
                if i != self.active_account {
                    self.switch_to_account(i).await;
                }
            }
            SidebarItem::Folder(f) => {
                self.load_folder(f).await;
                self.focus = Pane::List;
                if self.narrow {
                    self.narrow_pane = Pane::List;
                }
            }
        }
    }

    /// Indices into `self.folders` that are currently shown, honoring the
    /// collapsed state of ancestors.
    pub fn visible_folder_indices(&self) -> Vec<usize> {
        self.folders
            .iter()
            .enumerate()
            .filter(|(_, f)| self.folder_visible(f))
            .map(|(i, _)| i)
            .collect()
    }

    fn folder_visible(&self, folder: &Folder) -> bool {
        let Some(parent_path) = folder.parent_path() else {
            return true;
        };
        let Some(parent_idx) = self.folders.iter().position(|f| f.name == parent_path) else {
            return true;
        };
        !self.folder_collapsed[parent_idx] && self.folder_visible(&self.folders[parent_idx])
    }

    /// Collapse the selected folder so its children are hidden.
    pub(crate) async fn collapse_selected_folder(&mut self) {
        if self.selected_folder >= self.folders.len() {
            return;
        }
        if self.folder_has_children(self.selected_folder) {
            self.folder_collapsed[self.selected_folder] = true;
        }
    }

    pub(crate) async fn expand_selected_folder(&mut self) {
        if self.selected_folder >= self.folders.len() {
            return;
        }
        if self.folder_has_children(self.selected_folder) {
            self.folder_collapsed[self.selected_folder] = false;
        }
    }

    pub fn folder_has_children(&self, idx: usize) -> bool {
        let Some(f) = self.folders.get(idx) else {
            return false;
        };
        let prefix = format!("{}/", f.name);
        self.folders.iter().any(|c| c.name.starts_with(&prefix))
    }
}
