use std::path::{Path, PathBuf};

use crate::mail::{Address, Attachment, Draft, Message};
use crate::ui::fmt;
use crate::ui::widget::TextInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeMode {
    New,
    Reply,
    Forward,
}

/// Which compose field currently has keyboard focus. Tab/Shift-Tab cycle
/// through these; the header fields and `Attach` are inline `TextInput`s while
/// `Body` is a preview row that opens `$EDITOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeFocus {
    To,
    Cc,
    Bcc,
    ReplyTo,
    Subject,
    Attach,
    Body,
}

impl ComposeFocus {
    /// Focus order, top to bottom, wrapping.
    const ORDER: [ComposeFocus; 7] = [
        ComposeFocus::To,
        ComposeFocus::Cc,
        ComposeFocus::Bcc,
        ComposeFocus::ReplyTo,
        ComposeFocus::Subject,
        ComposeFocus::Attach,
        ComposeFocus::Body,
    ];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|&f| f == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ORDER[(self.index() + 1) % Self::ORDER.len()]
    }

    pub fn prev(self) -> Self {
        Self::ORDER[(self.index() + Self::ORDER.len() - 1) % Self::ORDER.len()]
    }

    /// Grid-aware movement matching the 2-column compose layout. `dir` is one of
    /// 'h' (left), 'j' (down), 'k' (up), 'l' (right). The six header/attach
    /// fields form a 3x2 grid; `Body` sits below it. Used so h/j/k/l walk the
    /// grid the way it is drawn.
    pub(crate) fn grid_step(self, dir: char) -> Self {
        if self == ComposeFocus::Body {
            // Leaving the body upward returns to the bottom-right cell.
            return match dir {
                'k' => ComposeFocus::Attach,
                _ => ComposeFocus::Body,
            };
        }
        let (r, c): (usize, usize) = match self {
            ComposeFocus::To => (0, 0),
            ComposeFocus::Cc => (0, 1),
            ComposeFocus::Bcc => (1, 0),
            ComposeFocus::ReplyTo => (1, 1),
            ComposeFocus::Subject => (2, 0),
            ComposeFocus::Attach => (2, 1),
            ComposeFocus::Body => unreachable!(),
        };
        let (r, c) = match dir {
            'k' => (r.saturating_sub(1), c),
            'j' => {
                if r == 2 {
                    return ComposeFocus::Body;
                }
                (r + 1, c)
            }
            'h' => (r, c.saturating_sub(1)),
            'l' => (r, (c + 1).min(1)),
            _ => (r, c),
        };
        match (r, c) {
            (0, 0) => ComposeFocus::To,
            (0, 1) => ComposeFocus::Cc,
            (1, 0) => ComposeFocus::Bcc,
            (1, 1) => ComposeFocus::ReplyTo,
            (2, 0) => ComposeFocus::Subject,
            (2, 1) => ComposeFocus::Attach,
            _ => self,
        }
    }
}

/// A draft under composition. Header fields are edited inline in the TUI; only
/// the body is edited in the external editor.
pub struct ComposeState {
    pub mode: ComposeMode,
    pub to: TextInput,
    pub cc: TextInput,
    pub bcc: TextInput,
    pub reply_to: TextInput,
    pub subject: TextInput,
    /// The "type a path" field; committing it pushes onto `attachments`.
    pub attach_input: TextInput,
    pub attachments: Vec<Attachment>,
    /// The body is composed in `$EDITOR`, not inline.
    pub body: String,
    /// First visible body line when reviewing a long body (see `ComposeFocus::Body`).
    pub body_scroll: usize,
    pub focus: ComposeFocus,
    /// Vim-style modes: navigate rows with j/k when false, type into the focused
    /// field when true (entered with `e`, left with Enter/Esc). Only meaningful
    /// for the header/attach text fields.
    pub editing: bool,
    /// Threading headers carried over from the message being replied to.
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
}

impl ComposeState {
    pub fn new(
        mode: ComposeMode,
        to: String,
        subject: String,
        body: String,
        in_reply_to: Option<String>,
        references: Option<String>,
    ) -> Self {
        Self {
            mode,
            to: TextInput::new(to),
            cc: TextInput::new(""),
            bcc: TextInput::new(""),
            reply_to: TextInput::new(""),
            subject: TextInput::new(subject),
            attach_input: TextInput::new(""),
            attachments: Vec::new(),
            body,
            body_scroll: 0,
            focus: ComposeFocus::To,
            editing: false,
            in_reply_to,
            references,
        }
    }

    pub fn to_draft(&self) -> Draft {
        Draft {
            to: self.to.text().trim().to_string(),
            cc: self.cc.text().trim().to_string(),
            bcc: self.bcc.text().trim().to_string(),
            reply_to: self.reply_to.text().trim().to_string(),
            subject: self.subject.text().trim().to_string(),
            body: self.body.clone(),
            attachments: self.attachments.clone(),
            in_reply_to: self.in_reply_to.clone(),
            references: self.references.clone(),
        }
    }

    /// The `TextInput` for the currently focused header/attach field, if the
    /// focus is on a text field (not `Body`).
    pub fn focused_input_mut(&mut self) -> Option<&mut TextInput> {
        match self.focus {
            ComposeFocus::To => Some(&mut self.to),
            ComposeFocus::Cc => Some(&mut self.cc),
            ComposeFocus::Bcc => Some(&mut self.bcc),
            ComposeFocus::ReplyTo => Some(&mut self.reply_to),
            ComposeFocus::Subject => Some(&mut self.subject),
            ComposeFocus::Attach => Some(&mut self.attach_input),
            ComposeFocus::Body => None,
        }
    }
}

pub(crate) fn reply_to(addr: &Address) -> String {
    addr.email.clone()
}

/// Expand a leading `~` (or `~/`) in a path to the user's home directory.
/// Other paths are returned unchanged.
pub(crate) fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return Path::new(&home).join(rest);
        }
    } else if raw == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(raw)
}

pub(crate) fn reply_subject(subject: &str) -> String {
    if subject.starts_with("Re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

pub(crate) fn forward_subject(subject: &str) -> String {
    if subject.starts_with("Fwd:") {
        subject.to_string()
    } else {
        format!("Fwd: {subject}")
    }
}

pub(crate) fn quote_body(msg: &Message, _from: Option<&str>) -> String {
    let from = msg.envelope.from.display();
    let date = fmt::full_timestamp(msg.envelope.date);
    let quoted = msg
        .body
        .lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n\nOn {date}, {from} wrote:\n{quoted}")
}

pub(crate) fn forward_body(msg: &Message, _from: Option<&str>) -> String {
    let from = msg.envelope.from.display();
    let date = fmt::full_timestamp(msg.envelope.date);
    format!(
        "---------- Forwarded message ----------\nFrom: {from}\nDate: {date}\nSubject: {}\n\n{}",
        msg.envelope.subject, msg.body
    )
}
