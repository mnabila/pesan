use super::*;

impl App {
    pub(crate) fn open_compose(&mut self, mode: ComposeMode) {
        let base = self.open_message.clone();
        let account_email = self
            .accounts
            .get(self.active_account)
            .map(|a| a.email.clone());
        let (to, subject, body) = match mode {
            ComposeMode::New => (String::new(), String::new(), String::new()),
            ComposeMode::Reply => match &base {
                Some(m) => (
                    reply_to(&m.envelope.from),
                    reply_subject(&m.envelope.subject),
                    quote_body(m, account_email.as_deref()),
                ),
                None => (String::new(), String::new(), String::new()),
            },
            ComposeMode::Forward => match &base {
                Some(m) => (
                    String::new(),
                    forward_subject(&m.envelope.subject),
                    forward_body(m, account_email.as_deref()),
                ),
                None => (String::new(), String::new(), String::new()),
            },
        };
        // Thread replies against the original Message-ID; forwards start fresh.
        let (in_reply_to, references) = match mode {
            ComposeMode::Reply => {
                let mid = base.as_ref().and_then(|m| m.envelope.message_id.clone());
                (mid.clone(), mid)
            }
            ComposeMode::New | ComposeMode::Forward => (None, None),
        };
        self.compose = Some(ComposeState::new(
            mode,
            to,
            subject,
            body,
            in_reply_to,
            references,
        ));
        self.view = View::Compose;
    }

    pub(crate) async fn send_compose(&mut self) {
        let Some(compose) = &self.compose else { return };
        let draft = compose.to_draft();
        if !draft.is_valid() {
            self.set_toast(
                "Need at least a recipient and a subject",
                ToastKind::Warning,
            );
            return;
        }
        match self.source.send(&draft).await {
            Ok(()) => {
                self.view = View::Main;
                self.compose = None;
                self.set_toast("Message sent", ToastKind::Success);
            }
            Err(e) => self.set_toast(format!("Send failed: {e}"), ToastKind::Error),
        }
    }

    pub(crate) fn save_draft(&mut self) {
        self.view = View::Main;
        self.compose = None;
        self.set_toast("Draft saving is not yet supported", ToastKind::Warning);
    }

    /// Commit the path typed in the Attach field: expand a leading `~`, verify
    /// it points at an existing file, then push it onto the attachment list and
    /// clear the input. A missing/invalid path shows a warning toast.
    pub(crate) fn add_attachment(&mut self) {
        let Some(compose) = &self.compose else { return };
        let raw = compose.attach_input.text().trim().to_string();
        if raw.is_empty() {
            return;
        }
        let path = expand_tilde(&raw);
        let meta = match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => {
                self.set_toast(format!("Not a file: {raw}"), ToastKind::Warning);
                return;
            }
            Err(_) => {
                self.set_toast(format!("No such file: {raw}"), ToastKind::Warning);
                return;
            }
        };
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.clone());
        let attachment = Attachment {
            path,
            filename: filename.clone(),
            size: meta.len(),
        };
        if let Some(compose) = &mut self.compose {
            compose.attachments.push(attachment);
            compose.attach_input = TextInput::new("");
            compose.attach_input.focus(true);
        }
        self.set_toast(format!("Attached {filename}"), ToastKind::Info);
    }

    /// Drop the most recently added attachment (bound to Ctrl-x in the Attach
    /// field).
    pub(crate) fn remove_last_attachment(&mut self) {
        if let Some(compose) = &mut self.compose
            && let Some(a) = compose.attachments.pop()
        {
            self.set_toast(format!("Removed {}", a.filename), ToastKind::Info);
        }
    }
}
