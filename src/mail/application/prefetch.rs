use chrono::{Local, NaiveDate, TimeZone};

use crate::mail::Envelope;

/// UIDs of today's messages among `envelopes` (newest-first as stored), capped
/// at `cap`; a zero cap (prefetch disabled) yields none.
///
/// The body pre-warm window, owned here so both drivers pick the same set: the
/// TUI schedules [`crate::mail::application::outcome::Effect::PrefetchBodies`] from it
/// and the daemon runs its own prefetch, and an open of a recent message is a
/// cache hit either way. `now` (unix seconds) is passed in rather than read from
/// the clock so the window is deterministic and testable.
pub fn today_uids(envelopes: &[Envelope], cap: usize, now: i64) -> Vec<u64> {
    let Some(today) = local_date(now) else {
        return Vec::new();
    };
    envelopes
        .iter()
        .filter(|e| local_date(e.date) == Some(today))
        .take(cap)
        .map(|e| e.uid)
        .collect()
}

/// The local calendar day a unix timestamp falls on.
fn local_date(ts: i64) -> Option<NaiveDate> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{Address, Flags};

    fn env(uid: u64, date: i64) -> Envelope {
        Envelope {
            uid,
            flags: Flags::default(),
            from: Address::new(None, "a@b.c"),
            subject: String::new(),
            date,
            has_attachment: false,
            snippet: None,
            message_id: None,
        }
    }

    /// Three days back is always a different local date (DST shifts are at most
    /// an hour or two), so these cases hold at any time of day.
    const THREE_DAYS: i64 = 3 * 24 * 60 * 60;

    #[test]
    fn keeps_todays_messages_and_drops_older() {
        let now = Local::now().timestamp();
        let envs = vec![env(3, now), env(2, now - THREE_DAYS), env(1, now)];
        assert_eq!(today_uids(&envs, 10, now), vec![3, 1]);
    }

    #[test]
    fn caps_the_window() {
        let now = Local::now().timestamp();
        let envs = vec![env(3, now), env(2, now), env(1, now)];
        assert_eq!(today_uids(&envs, 2, now), vec![3, 2]);
    }

    #[test]
    fn a_zero_cap_disables_prefetch() {
        let now = Local::now().timestamp();
        assert!(today_uids(&[env(1, now)], 0, now).is_empty());
    }

    #[test]
    fn an_out_of_range_now_selects_nothing() {
        let now = Local::now().timestamp();
        assert!(today_uids(&[env(1, now)], 10, i64::MAX).is_empty());
    }
}
