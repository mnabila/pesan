//! Mail adapters: implementations of the mail ports - live IMAP, SMTP send, the
//! SQLite cache/index, the Maildir body store, desktop notifier, and the daemon
//! IPC transport.

pub mod backend;
pub mod cache;
pub mod empty;
pub mod imap;
pub mod ipc;
pub mod maildir;
pub mod notify;
pub mod smtp;
pub mod sqlite_cache;
