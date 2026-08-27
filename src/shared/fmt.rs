use chrono::{DateTime, Local, TimeZone};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate `s` to fit `max` columns; appends an ellipsis when truncated.
pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let max = max.saturating_sub(1); // room for the ellipsis
    let mut out = String::new();
    for ch in s.chars() {
        if out.width() + ch.width().unwrap_or(0) > max {
            break;
        }
        out.push(ch);
    }
    format!("{out}…")
}

/// Relative date for list rows: today -> "10:24", this week -> weekday, else
/// "d Mon".
pub fn relative_date(ts: i64) -> String {
    let dt = Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(Local::now);
    let now = Local::now();

    if dt.date_naive() == now.date_naive() {
        dt.format("%H:%M").to_string()
    } else {
        let days = (now.date_naive() - dt.date_naive()).num_days();
        if days < 1 {
            dt.format("%H:%M").to_string()
        } else if days < 7 {
            dt.format("%a").to_string()
        } else {
            dt.format("%d %b").to_string()
        }
    }
}

/// Absolute, fully expanded timestamp for the reader header.
pub fn full_timestamp(ts: i64) -> String {
    let dt: DateTime<Local> = Local
        .timestamp_opt(ts, 0)
        .single()
        .unwrap_or_else(Local::now);
    dt.format("%a %d %b %Y, %H:%M").to_string()
}
