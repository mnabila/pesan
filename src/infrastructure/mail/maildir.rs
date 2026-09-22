//! On-disk Maildir mirror of cached message bodies.
//!
//! The SQLite cache stays the fast index (envelopes, flags, plain-text body for
//! FTS search). This module writes the *content of record*: the full raw RFC822
//! message as an interoperable Maildir file that mutt / mbsync / notmuch can read
//! directly. Only messages whose body has actually been fetched get a file;
//! envelope-only rows (listed but not opened) have none yet.
//!
//! Layout: `<root>/<account_id>/<encoded_folder>/{tmp,new,cur}`. Files land in
//! `cur/` named `<secs>.<pid>_<seq>.pesan,U=<uid>:2,<flags>` (the `,U=<uid>` is
//! the mbsync convention that lets us find a message by its IMAP UID; `<flags>`
//! are the standard Maildir info flags, `S`=Seen, `F`=Flagged, in ASCII order).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::infrastructure::mail::imap::body::extract_body;

/// Monotonic counter making each written filename unique within a process.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// A Maildir tree rooted at `root`. Cheap to clone (just a path), so every cache
/// adapter and offline source can hold its own copy.
#[derive(Clone)]
pub struct MaildirStore {
    root: PathBuf,
}

/// The pieces re-derived from a stored raw message on a cached open, mirroring
/// what a live fetch produced: plain text (for display fallback), the original
/// HTML source, and the raw header block.
pub struct Parsed {
    pub text: String,
    pub raw_html: Option<String>,
    pub raw_headers: Option<String>,
}

impl MaildirStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Persist `raw` (the complete RFC822 message) for a message, replacing any
    /// existing file for the same UID so a re-fetch or flag change never leaves a
    /// duplicate. Written to `tmp/` then atomically renamed into `cur/`.
    pub fn write(
        &self,
        account_id: &str,
        folder: &str,
        uid: u64,
        seen: bool,
        flagged: bool,
        raw: &[u8],
    ) -> Result<()> {
        let dir = self.folder_dir(account_id, folder);
        ensure_maildir(&dir)?;
        // Drop any prior file for this UID (its flags/name may differ).
        if let Some(existing) = find(&dir, uid) {
            let _ = fs::remove_file(existing);
        }
        let name = file_name(uid, seen, flagged);
        let tmp = dir.join("tmp").join(&name);
        let cur = dir.join("cur").join(&name);
        fs::write(&tmp, raw).with_context(|| format!("write maildir tmp {}", tmp.display()))?;
        fs::rename(&tmp, &cur).with_context(|| format!("commit maildir file {}", cur.display()))?;
        Ok(())
    }

    /// Read the raw message bytes for a UID, or `None` when no file exists yet
    /// (body not fetched).
    pub fn read(&self, account_id: &str, folder: &str, uid: u64) -> Option<Vec<u8>> {
        let dir = self.folder_dir(account_id, folder);
        let path = find(&dir, uid)?;
        fs::read(path).ok()
    }

    /// Read and parse a stored message into display pieces, or `None` when there
    /// is no file for the UID.
    pub fn read_parsed(&self, account_id: &str, folder: &str, uid: u64) -> Option<Parsed> {
        let raw = self.read(account_id, folder, uid)?;
        Some(parse(&raw))
    }

    /// Rename a message's file to reflect new flags. No-op when no file exists.
    pub fn set_flags(&self, account_id: &str, folder: &str, uid: u64, seen: bool, flagged: bool) {
        let dir = self.folder_dir(account_id, folder);
        let Some(path) = find(&dir, uid) else { return };
        let base = base_name(&path);
        let renamed = dir.join("cur").join(format!("{base}:2,{}", flag_str(seen, flagged)));
        if renamed != path {
            let _ = fs::rename(&path, &renamed);
        }
    }

    /// Move a message's file to another folder's `cur/`, preserving its name and
    /// flags. No-op when no file exists.
    pub fn move_to(&self, account_id: &str, from: &str, uid: u64, to: &str) {
        let from_dir = self.folder_dir(account_id, from);
        let Some(path) = find(&from_dir, uid) else { return };
        let to_dir = self.folder_dir(account_id, to);
        if ensure_maildir(&to_dir).is_err() {
            return;
        }
        if let Some(name) = path.file_name() {
            let _ = fs::rename(&path, to_dir.join("cur").join(name));
        }
    }

    /// Delete a message's file. No-op when no file exists.
    pub fn delete(&self, account_id: &str, folder: &str, uid: u64) {
        let dir = self.folder_dir(account_id, folder);
        if let Some(path) = find(&dir, uid) {
            let _ = fs::remove_file(path);
        }
    }

    fn folder_dir(&self, account_id: &str, folder: &str) -> PathBuf {
        self.root
            .join(account_id)
            .join(encode_folder(folder))
    }
}

/// Parse a raw RFC822 message into the same pieces a live fetch derives.
fn parse(raw: &[u8]) -> Parsed {
    let parsed = mail_parser::MessageParser::default().parse(raw);
    let (text, raw_html) = match parsed.as_ref().map(extract_body) {
        Some(b) => (b.text, b.raw_html),
        None => (String::new(), None),
    };
    Parsed {
        text,
        raw_html,
        raw_headers: raw_header_block(raw),
    }
}

/// Everything before the first blank line, `\r\n` normalized to `\n`; `None`
/// when there is no header/body separator.
fn raw_header_block(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let end = text.find("\r\n\r\n").or_else(|| text.find("\n\n"))?;
    Some(text[..end].replace("\r\n", "\n"))
}

/// Create the `tmp`/`new`/`cur` subdirs of a Maildir if absent.
fn ensure_maildir(dir: &Path) -> Result<()> {
    for sub in ["tmp", "new", "cur"] {
        fs::create_dir_all(dir.join(sub))
            .with_context(|| format!("create maildir {}", dir.join(sub).display()))?;
    }
    Ok(())
}

/// Build a Maildir filename carrying the UID and flags.
fn file_name(uid: u64, seen: bool, flagged: bool) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{secs}.{pid}_{seq}.pesan,U={uid}:2,{}", flag_str(seen, flagged))
}

/// The Maildir info flags string in ASCII order: `F` (Flagged) before `S` (Seen).
fn flag_str(seen: bool, flagged: bool) -> String {
    let mut s = String::new();
    if flagged {
        s.push('F');
    }
    if seen {
        s.push('S');
    }
    s
}

/// The part of a filename before the `:2,` flags separator (the unique base plus
/// the `,U=<uid>` marker), used to rebuild the name with new flags.
fn base_name(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.split(":2,").next().unwrap_or(name).to_string()
}

/// Find the file for a UID under a folder's `cur/` (then `new/`), matching the
/// `,U=<uid>` marker on a boundary so UID 12 never matches 123.
fn find(dir: &Path, uid: u64) -> Option<PathBuf> {
    let marker = format!(",U={uid}");
    for sub in ["cur", "new"] {
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(rest) = name.split_once(&marker).map(|(_, r)| r) {
                // The marker must end the name or be immediately followed by the
                // `:2,` flags separator - not more UID digits.
                if rest.is_empty() || rest.starts_with(":2,") {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}

/// Filesystem-safe, stable encoding of an IMAP folder name (which may contain
/// `/`, spaces, brackets). Percent-encodes anything outside `[A-Za-z0-9._-]`.
/// One-way is fine: the folder name is always known from the caller/SQLite, we
/// never map a directory back to a folder.
fn encode_folder(folder: &str) -> String {
    let mut out = String::with_capacity(folder.len());
    for b in folder.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch Maildir root that removes itself on drop. Keyed by pid + a
    /// counter so parallel tests never share a directory.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (MaildirStore, Scratch) {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("pesan-maildir-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        (MaildirStore::new(dir.clone()), Scratch(dir))
    }

    const RAW: &[u8] =
        b"From: Jane <jane@acme.io>\r\nSubject: Q3 plan\r\nContent-Type: text/plain\r\n\r\nthe body text\r\n";

    #[test]
    fn write_read_round_trip() {
        let (s, _d) = store();
        s.write("acct-a", "INBOX", 10, true, false, RAW).unwrap();
        assert_eq!(s.read("acct-a", "INBOX", 10).as_deref(), Some(RAW));
        let p = s.read_parsed("acct-a", "INBOX", 10).unwrap();
        assert_eq!(p.text, "the body text");
        assert!(p.raw_headers.unwrap().contains("Subject: Q3 plan"));
    }

    #[test]
    fn absent_uid_reads_none() {
        let (s, _d) = store();
        assert!(s.read("acct-a", "INBOX", 99).is_none());
        assert!(s.read_parsed("acct-a", "INBOX", 99).is_none());
    }

    #[test]
    fn set_flags_renames_in_place_and_keeps_uid() {
        let (s, _d) = store();
        s.write("acct-a", "INBOX", 10, false, false, RAW).unwrap();
        s.set_flags("acct-a", "INBOX", 10, true, true);
        // Still findable by UID, and the raw content is untouched.
        assert_eq!(s.read("acct-a", "INBOX", 10).as_deref(), Some(RAW));
    }

    #[test]
    fn find_matches_uid_on_boundary_not_prefix() {
        let (s, _d) = store();
        s.write("acct-a", "INBOX", 12, true, false, RAW).unwrap();
        s.write("acct-a", "INBOX", 123, true, false, RAW).unwrap();
        // Deleting 12 must not remove 123.
        s.delete("acct-a", "INBOX", 12);
        assert!(s.read("acct-a", "INBOX", 12).is_none());
        assert!(s.read("acct-a", "INBOX", 123).is_some());
    }

    #[test]
    fn move_across_folders() {
        let (s, _d) = store();
        s.write("acct-a", "INBOX", 10, true, false, RAW).unwrap();
        s.move_to("acct-a", "INBOX", 10, "Archive");
        assert!(s.read("acct-a", "INBOX", 10).is_none());
        assert_eq!(s.read("acct-a", "Archive", 10).as_deref(), Some(RAW));
    }

    #[test]
    fn write_replaces_existing_uid_file() {
        let (s, _d) = store();
        s.write("acct-a", "INBOX", 10, false, false, RAW).unwrap();
        s.write("acct-a", "INBOX", 10, true, true, RAW).unwrap();
        // Exactly one file for the UID (no duplicate left behind).
        let dir = s.folder_dir("acct-a", "INBOX").join("cur");
        let count = std::fs::read_dir(dir).unwrap().count();
        assert_eq!(count, 1);
    }

    #[test]
    fn encode_folder_is_fs_safe() {
        assert_eq!(encode_folder("INBOX"), "INBOX");
        assert_eq!(encode_folder("[Gmail]/All Mail"), "%5BGmail%5D%2FAll%20Mail");
    }
}
