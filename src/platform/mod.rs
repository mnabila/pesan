//! Shared kernel: cross-cutting primitives every slice may depend on - the
//! SQLite pool + migrations, config, runtime, logging, the OAuth primitive and
//! its shared boundary types, secret fallback store. A leaf: it imports no
//! sibling slice.

pub mod auth;
pub mod boot;
pub mod browser;
pub mod config;
pub mod connect_params;
pub mod db;
pub mod logging;
pub mod oauth;
pub mod runtime;
pub mod secrets;
pub mod sound;
