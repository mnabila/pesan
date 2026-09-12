pub mod fmt;

/// Whether a message timestamp (unix seconds) falls on the local calendar day,
/// used to decide which recent messages to pre-warm into the body cache. Shared
/// by the TUI reader and the headless daemon so both prefetch the same window.
pub fn is_today(ts: i64) -> bool {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(ts, 0)
        .single()
        .is_some_and(|dt| dt.date_naive() == Local::now().date_naive())
}
