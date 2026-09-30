use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::mail::Message;
use crate::platform::browser;
use crate::ui::fmt::full_timestamp;

/// Fixed staging directory for browser exports (per product requirement).
pub fn export_dir() -> PathBuf {
    PathBuf::from("/tmp/pesan")
}

/// Keep filenames safe: alphanumerics plus `-_.`, everything else becomes `_`.
/// Truncates the stem so the final path stays well under `NAME_MAX`.
pub fn sanitize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Collapse repeats of `_` from e.g. `a/b` -> `a_b` (single already).
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let trimmed = out.trim_matches(['_', '.']).to_string();
    let s = if trimmed.is_empty() {
        "message".to_string()
    } else {
        trimmed
    };
    if s.len() > 80 { s[..80].to_string() } else { s }
}

/// `account-folder-uid.html` under [`export_dir`], sanitized against traversal.
pub fn export_path(account: &str, folder: &str, uid: u64) -> PathBuf {
    let stem = sanitize(&format!("{account}-{folder}-{uid}"));
    export_dir().join(format!("{stem}.html"))
}

/// Minimal HTML escaping for header fields interpolated into the document.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Resolve the `To` header from a raw RFC822 block (unfolding continuations),
/// falling back to the viewing account when headers are unavailable.
pub fn resolve_to(raw_headers: Option<&str>, fallback: &str) -> String {
    raw_headers
        .and_then(|h| header_value(h, "To"))
        .unwrap_or_else(|| fallback.to_string())
}

fn header_value(raw: &str, name: &str) -> Option<String> {
    let target = name.to_ascii_lowercase();
    let lines: Vec<&str> = raw.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let Some(colon) = line.find(':') else {
            continue;
        };
        if line[..colon].trim().to_ascii_lowercase() != target {
            continue;
        }
        let mut val = line[colon + 1..].trim().to_string();
        for cont in &lines[i + 1..] {
            if cont.starts_with(' ') || cont.starts_with('\t') {
                val.push(' ');
                val.push_str(cont.trim());
            } else {
                break;
            }
        }
        let val = val.trim().to_string();
        return (!val.is_empty()).then_some(val);
    }
    None
}

/// Full HTML document for `msg`, with a header block and the message content:
/// the stored raw HTML source when available (faithful), else the plain-text
/// body wrapped in <pre> for faithful rendering. `to` is the resolved recipient
/// (see [`resolve_to`]).
pub fn message_to_html(msg: &Message, to: &str) -> String {
    let subject = escape_html(&msg.envelope.subject);
    let from = escape_html(&msg.envelope.from.display());
    let to = escape_html(to);
    let date = escape_html(&full_timestamp(msg.envelope.date));
    let body = match msg.raw_html.as_deref().filter(|h| !h.trim().is_empty()) {
        Some(html) => html.to_string(),
        None => format!("<pre>{}</pre>", escape_html(&msg.body)),
    };
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{subject}</title></head>\n<body>\n<header>\n<h1>{subject}</h1>\n\
         <p>From: {from}<br>To: {to}<br>Date: {date}</p>\n<hr>\n</header>\n\
         <main>\n{body}</main>\n</body></html>\n"
    )
}

/// Write `msg` to `/tmp/pesan/<account>-<folder>-<uid>.html` and return the path.
pub fn export_message(account: &str, folder: &str, msg: &Message, to: &str) -> Result<PathBuf> {
    let dir = export_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = export_path(account, folder, msg.envelope.uid);
    let doc = message_to_html(msg, to);
    std::fs::write(&path, doc).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Export message and open in browser with optional custom browser command.
pub fn export_and_open_message(
    account: &str,
    folder: &str,
    msg: &Message,
    to: &str,
    browser_cmd: Option<&str>,
) -> Result<PathBuf> {
    let path = export_message(account, folder, msg, to)?;
    browser::open_in_browser(&path, browser_cmd)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::{Address, Envelope, Flags};

    fn msg(uid: u64, subject: &str, body: &str, raw: Option<&str>) -> Message {
        Message {
            envelope: Envelope {
                uid,
                flags: Flags::default(),
                from: Address {
                    name: Some("Ada".to_string()),
                    email: "ada@example.com".into(),
                },
                subject: subject.into(),
                date: 0,
                has_attachment: false,
                message_id: None,
                snippet: None,
            },
            body: body.into(),
            raw_html: None,
            raw_headers: raw.map(str::to_string),
            raw: None,
        }
    }

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize("INBOX/Work"), "INBOX_Work");
        assert_eq!(sanitize(""), "message");
        let p = export_path("acct", "INBOX/Work", 42);
        assert_eq!(p.parent().unwrap(), export_dir().as_path());
        assert!(p.file_name().unwrap().to_str().unwrap().ends_with(".html"));
        assert!(
            !p.to_str()
                .unwrap()
                .contains('/'.to_string().repeat(2).as_str())
        );
    }

    #[test]
    fn escape_and_render() {
        let m = msg(1, "<Hi> & \"bye\"", "# Title\n\nHello", None);
        let doc = message_to_html(&m, "bob@x.io");
        assert!(doc.contains("&lt;Hi&gt; &amp; &quot;bye&quot;"));
        assert!(doc.contains("<h1>"));
        assert!(doc.contains("Hello"));
        assert!(doc.contains("ada@example.com"));
    }

    #[test]
    fn prefers_stored_raw_html() {
        let mut m = msg(1, "subj", "plain fallback", None);
        m.raw_html = Some("<p>Faithful <b>source</b></p>".to_string());
        let doc = message_to_html(&m, "bob@x.io");
        assert!(doc.contains("Faithful <b>source</b>"));
        assert!(!doc.contains("plain fallback"));
    }

    #[test]
    fn resolve_to_prefers_header() {
        assert_eq!(
            resolve_to(Some("From: a@b.com\nTo: dest@x.io\n"), "me@x.io"),
            "dest@x.io"
        );
        assert_eq!(resolve_to(None, "me@x.io"), "me@x.io");
    }

    #[test]
    fn export_message_writes_to_tmp_pesan() {
        // End-to-end through the real export path: file must land in /tmp/pesan.
        // The account name carries the pid so parallel runs never collide.
        let account = format!("proof-{}", std::process::id());
        let m = msg(7, "proof subject", "# Hello\n\nbody text", None);
        let path = export_message(&account, "INBOX", &m, "me@x.io").unwrap();
        assert_eq!(path.parent().unwrap(), export_dir().as_path());
        let doc = std::fs::read_to_string(&path).unwrap();
        assert!(doc.contains("proof subject"));
        assert!(doc.contains("body text"));
        std::fs::remove_file(&path).ok();
    }
}
