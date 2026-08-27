pub mod body;
pub mod client;
pub mod idle;
pub mod proto;
pub mod worker;

pub use client::ImapSource;
// (ConnectParams/ImapAuth/LIST_WINDOW are application types; import from there.)
