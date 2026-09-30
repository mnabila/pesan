//! Email-management slice: mail domain types, read/list/search/compose
//! use-cases, and the IMAP/SMTP/cache/notify adapters behind the mail ports.

pub mod application;
pub mod domain;
pub mod infrastructure;

pub use application::fetch::MailSource;
pub use domain::{
    Address, Attachment, Draft, Envelope, Flags, Folder, FolderCategory, MailUpdate, Message,
    MessageWindow, NewMail,
};
