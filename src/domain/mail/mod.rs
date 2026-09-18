/// Kept plain and serializable-friendly so the M6 offline cache can persist
/// them without reshaping the model.
// Plain, serializable-friendly types so the offline cache can persist them
// without reshaping the model.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

impl Address {
    pub fn new(name: impl Into<Option<String>>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
        }
    }

    /// "Name <email>" when a name is present, else just the email. Falls back to
    /// a placeholder when neither is known, so callers never render a blank.
    pub fn display(&self) -> String {
        match &self.name {
            Some(n) if !n.is_empty() => format!("{n} <{}>", self.email),
            _ if !self.email.is_empty() => self.email.clone(),
            _ => "(unknown sender)".to_string(),
        }
    }

    /// Short display used in list rows: name, else the local part of the email,
    /// else a placeholder so a message with no parseable sender is never blank.
    pub fn short(&self) -> String {
        match &self.name {
            Some(n) if !n.is_empty() => n.clone(),
            _ => {
                let local = self.email.split('@').next().unwrap_or(&self.email);
                if local.is_empty() {
                    "(unknown)".to_string()
                } else {
                    local.to_string()
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flags {
    pub seen: bool,
    pub flagged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub uid: u64,
    pub flags: Flags,
    pub from: Address,
    pub subject: String,
    /// Unix timestamp (seconds).
    pub date: i64,
    pub has_attachment: bool,
    pub snippet: Option<String>,
    /// RFC5322 `Message-ID` (with angle brackets), used to thread replies.
    pub message_id: Option<String>,
}

/// One page of a folder's envelopes plus the server-side total, returned by the
/// windowed (paginated) list. `total` lets callers tell whether older messages
/// remain: more are available while the number loaded is below `total`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageWindow {
    pub envelopes: Vec<Envelope>,
    /// Total messages in the mailbox on the server at fetch time (`EXISTS`).
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FolderCategory {
    Mailbox,
    Label,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    pub total: usize,
    pub unread: usize,
    pub category: FolderCategory,
}

impl Folder {
    /// Path depth: number of hierarchy separators ("/"), 0 = top level.
    pub fn depth(&self) -> usize {
        self.name.matches('/').count()
    }

    /// Immediate parent path (everything before the last "/"), if any.
    pub fn parent_path(&self) -> Option<&str> {
        self.name.rsplit_once('/').map(|(p, _)| p)
    }

    /// Last path segment, used for the sidebar label.
    pub fn short_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub envelope: Envelope,
    /// Plain searchable text: the text/plain part verbatim, or the HTML part
    /// converted to text. Feeds search, reply/forward quoting, and the reader
    /// fallback when no HTML source is stored.
    pub body: String,
    /// Original HTML source when `body` came from an HTML part (capped), so
    /// the reader can render width-aware styled lines. `None` for plain-text
    /// mail, errors, and placeholders.
    #[serde(default)]
    pub raw_html: Option<String>,
    /// Raw RFC822 header block (everything before the first blank line), when
    /// available. Populated by the live IMAP fetch; `None` for cached messages
    /// (only the body is cached). Shown by the reader's "full headers" toggle.
    pub raw_headers: Option<String>,
}

/// A file staged to be sent with a message. The bytes are read lazily at send
/// time (see `smtp::build_message`); we keep only the path plus display info.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: PathBuf,
    pub filename: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Draft {
    pub to: String,
    pub cc: String,
    pub bcc: String,
    /// Optional `Reply-To` address (where replies should be sent instead of From).
    pub reply_to: String,
    pub subject: String,
    pub body: String,
    pub attachments: Vec<Attachment>,
    /// Set on replies: the `Message-ID` being answered.
    pub in_reply_to: Option<String>,
    /// Reply/forward `References` chain (may equal `in_reply_to` for one level).
    pub references: Option<String>,
}

impl Draft {
    pub fn is_valid(&self) -> bool {
        !self.to.trim().is_empty() && !self.subject.trim().is_empty()
    }
}

/// A batch of newly-arrived messages for one account's watched mailbox.
/// Emitted by the background IDLE/poll watcher over a dedicated channel; the
/// app adapts it into its own event stream (infrastructure never constructs
/// UI events directly).
#[derive(Debug)]
pub struct NewMail {
    pub account: String,
    /// The mailbox the arrivals landed in (the watched folder).
    pub folder: String,
    pub envelopes: Vec<Envelope>,
}

/// An update emitted by a mailbox watcher. `Arrived` is incremental new mail to
/// merge into the view; `FolderSynced` is a full newest-window snapshot to
/// replace the folder with, carrying server-side changes (flag updates, removals)
/// that the daemon's periodic re-sync detected - not just new arrivals.
#[derive(Debug)]
pub enum MailUpdate {
    Arrived(NewMail),
    FolderSynced {
        account: String,
        folder: String,
        envelopes: Vec<Envelope>,
    },
}
