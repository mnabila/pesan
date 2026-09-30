//! Mail use-cases (fetch/open/search/compose orchestration) and the ports they
//! define.

pub mod fetch;
pub mod folders;
pub mod imap_cmd;
pub mod mutations;
pub mod open;
pub mod outcome;
pub mod ports;
pub mod prefetch;
pub mod search;

#[cfg(test)]
pub mod testing;
