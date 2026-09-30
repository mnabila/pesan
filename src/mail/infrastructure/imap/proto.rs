use anyhow::{Context, Result, anyhow};
use async_imap::types::Flag;
use futures::StreamExt;
use mail_parser::MessageParser;

use crate::mail::{Address, Envelope, Flags, Folder, FolderCategory, Message, MessageWindow};

use super::body::extract_body;
use super::client::ImapSession;
use super::client::LIST_WINDOW;

/// List folder *names* only (a single LIST round-trip). Message/unread counts
/// are intentionally left at zero and filled in later by [`folder_counts`] on a
/// background task - the per-folder STATUS loop is the slowest part of connect
/// (one round-trip per folder, and Gmail exposes many labels), so it must not
/// block the initial display.
pub(crate) async fn list_folders(session: &mut ImapSession) -> Result<Vec<Folder>> {
    let mut stream = session
        .list(Some(""), Some("*"))
        .await
        .context("LIST folders")?;
    let mut folders = Vec::new();
    while let Some(item) = stream.next().await {
        let name = item.context("read folder name")?;
        if name
            .attributes()
            .iter()
            .any(|a| matches!(a, async_imap::imap_proto::NameAttribute::NoSelect))
        {
            continue;
        }
        folders.push(Folder {
            name: name.name().to_string(),
            total: 0,
            unread: 0,
            category: FolderCategory::Mailbox,
        });
    }
    drop(stream);
    // INBOX first, then case-insensitive alphabetical.
    folders.sort_by(|a, b| {
        let rank = |n: &str| {
            if n.eq_ignore_ascii_case("INBOX") {
                0
            } else {
                1
            }
        };
        rank(&a.name)
            .cmp(&rank(&b.name))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(folders)
}

/// Fetch (MESSAGES, UNSEEN) counts for the given folders via one STATUS per
/// folder. Best effort: a folder whose STATUS fails is reported with zero
/// counts. Runs off the connect path so a slow chain of round-trips never delays
/// the initial folder/message display.
pub(crate) async fn folder_counts(
    session: &mut ImapSession,
    names: &[String],
) -> Vec<(String, usize, usize)> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        match session.status(name, "(MESSAGES UNSEEN)").await {
            Ok(mbox) => out.push((
                name.clone(),
                mbox.exists as usize,
                mbox.unseen.unwrap_or(0) as usize,
            )),
            Err(e) => {
                tracing::debug!("STATUS {name} failed: {e}");
                out.push((name.clone(), 0, 0));
            }
        }
    }
    out
}

pub(crate) async fn list_messages(
    session: &mut ImapSession,
    folder: &str,
) -> Result<Vec<Envelope>> {
    Ok(list_messages_window(session, folder, 0, LIST_WINDOW)
        .await?
        .envelopes)
}

/// Fetch one page of a folder's envelopes, newest-first. `offset` newest
/// messages are skipped and up to `limit` older ones are fetched; `offset == 0`
/// yields the newest window. Returns the page plus the mailbox total so callers
/// can tell whether older mail remains.
///
/// Windowing is by sequence number (`start:end`), which is what most clients use
/// for "load more". Sequence numbers shift if messages are added/expunged
/// between pages, so a concurrent mutation can duplicate or skip a couple of
/// boundary rows; upserting by uid dedups the overlap and a full refresh
/// corrects any gap.
pub(crate) async fn list_messages_window(
    session: &mut ImapSession,
    folder: &str,
    offset: u32,
    limit: u32,
) -> Result<MessageWindow> {
    let mailbox = session
        .select(folder)
        .await
        .with_context(|| format!("SELECT {folder}"))?;
    let total = mailbox.exists;
    if total == 0 || offset >= total {
        return Ok(MessageWindow {
            envelopes: Vec::new(),
            total,
        });
    }
    // `end` is the highest sequence number we want (offset newest ones skipped);
    // walk back `limit` from there, clamped to the first message.
    let end = total - offset;
    let start = end.saturating_sub(limit.saturating_sub(1)).max(1);
    let seq = format!("{start}:{end}");

    let mut stream = session
        .fetch(seq, ENVELOPE_QUERY)
        .await
        .context("FETCH envelopes")?;
    let mut envelopes = Vec::new();
    while let Some(item) = stream.next().await {
        let fetch = item.context("read FETCH item")?;
        if let Some(env) = envelope_from_fetch(&fetch) {
            envelopes.push(env);
        }
    }
    drop(stream);
    // Newest first.
    envelopes.sort_by_key(|e| std::cmp::Reverse(e.date));
    Ok(MessageWindow { envelopes, total })
}

pub(crate) async fn fetch_message(session: &mut ImapSession, uid: u64) -> Result<Message> {
    let mut stream = session
        .uid_fetch(uid.to_string(), "(UID FLAGS BODY.PEEK[])")
        .await
        .context("UID FETCH body")?;
    let fetch = stream
        .next()
        .await
        .transpose()
        .context("read message body")?
        .ok_or_else(|| anyhow!("message {uid} not found"))?;

    let flags = read_flags(&fetch);
    let raw = fetch.body().unwrap_or_default();
    let parsed = MessageParser::default().parse(raw);
    let (from, subject, date, message_id) = header_meta(parsed.as_ref(), &fetch);
    let has_attachment = parsed
        .as_ref()
        .map(|m| m.attachment_count() > 0)
        .unwrap_or(false);
    let body = parsed.as_ref().map(extract_body);
    let (text, raw_html) = match body {
        Some(b) => (b.text, b.raw_html),
        None => ("(unable to parse message body)".to_string(), None),
    };
    let raw_headers = raw_header_block(raw);

    Ok(Message {
        envelope: Envelope {
            uid,
            flags,
            from,
            subject,
            date,
            has_attachment,
            snippet: None,
            message_id,
        },
        body: text,
        raw_html,
        raw_headers,
        raw: Some(raw.to_vec()),
    })
}

/// The raw RFC822 header block: everything before the first blank line. Returns
/// `None` when the message has no header/body separator.
fn raw_header_block(raw: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    let end = text.find("\r\n\r\n").or_else(|| text.find("\n\n"))?;
    Some(text[..end].replace("\r\n", "\n"))
}

/// Permanently delete a message: mark `\Deleted`, then EXPUNGE it. Prefers UID
/// EXPUNGE (removes only this message) and falls back to plain EXPUNGE (which
/// only removes the messages we just flagged).
pub(crate) async fn delete_uid(session: &mut ImapSession, uid: u64) -> Result<()> {
    store_flag(session, uid, "\\Deleted", true).await?;

    // Prefer UID EXPUNGE; drain it in its own scope so the borrow releases
    // before any fallback. `uid_ok` is false if UID EXPUNGE is unsupported.
    let uid_ok = match session.uid_expunge(uid.to_string()).await {
        Ok(stream) => {
            futures::pin_mut!(stream);
            let mut ok = true;
            while let Some(item) = stream.next().await {
                if item.is_err() {
                    ok = false;
                    break;
                }
            }
            ok
        }
        Err(_) => false,
    };

    if !uid_ok {
        let stream = session.expunge().await.context("EXPUNGE")?;
        futures::pin_mut!(stream);
        while let Some(item) = stream.next().await {
            item.context("EXPUNGE")?;
        }
    }
    Ok(())
}

/// SELECT the given mailbox. Every folder-scoped worker op (fetch/flag/delete/
/// move) selects first, because the shared session's selected folder is mutated
/// by background sweeps - otherwise an op could hit the wrong mailbox ("message
/// N not found", or a delete/flag on the wrong message).
pub(crate) async fn select_folder(session: &mut ImapSession, folder: &str) -> Result<()> {
    session
        .select(folder)
        .await
        .with_context(|| format!("SELECT {folder}"))?;
    Ok(())
}

/// Move a message to `folder` via UID MOVE, falling back to COPY + delete when
/// the server lacks the MOVE extension.
pub(crate) async fn move_uid(session: &mut ImapSession, uid: u64, folder: &str) -> Result<()> {
    if session.uid_mv(uid.to_string(), folder).await.is_ok() {
        return Ok(());
    }
    session
        .uid_copy(uid.to_string(), folder)
        .await
        .with_context(|| format!("COPY to {folder}"))?;
    delete_uid(session, uid).await
}

pub(crate) async fn store_flag(
    session: &mut ImapSession,
    uid: u64,
    flag: &str,
    on: bool,
) -> Result<()> {
    let op = if on { "+FLAGS" } else { "-FLAGS" };
    let query = format!("{op} ({flag})");
    let mut stream = session
        .uid_store(uid.to_string(), &query)
        .await
        .with_context(|| format!("UID STORE {query}"))?;
    // Drain the response stream so the command completes.
    while let Some(item) = stream.next().await {
        item.context("read STORE response")?;
    }
    Ok(())
}

/// FETCH item spec that yields everything [`envelope_from_fetch`] needs.
pub(crate) const ENVELOPE_QUERY: &str = "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])";

/// Map one FETCH item to an [`Envelope`], or `None` if it carries no UID.
pub(crate) fn envelope_from_fetch(fetch: &async_imap::types::Fetch) -> Option<Envelope> {
    let uid = fetch.uid?;
    let flags = read_flags(fetch);
    let header = fetch.header().unwrap_or_default();
    let parsed = MessageParser::default().parse(header);
    let (from, subject, date, message_id) = header_meta(parsed.as_ref(), fetch);
    Some(Envelope {
        uid: uid as u64,
        flags,
        from,
        subject,
        date,
        has_attachment: false,
        snippet: None,
        message_id,
    })
}

fn read_flags(fetch: &async_imap::types::Fetch) -> Flags {
    let mut flags = Flags::default();
    for flag in fetch.flags() {
        match flag {
            Flag::Seen => flags.seen = true,
            Flag::Flagged => flags.flagged = true,
            _ => {}
        }
    }
    flags
}

/// Resolve From / Subject / Date from the parsed header, falling back to the
/// server's INTERNALDATE when the message carries no (valid) Date header.
fn header_meta(
    parsed: Option<&mail_parser::Message<'_>>,
    fetch: &async_imap::types::Fetch,
) -> (Address, String, i64, Option<String>) {
    let from = parsed
        .and_then(|m| m.from())
        .and_then(|addr| addr.first())
        .map(|a| {
            Address::new(
                a.name().map(str::to_string),
                a.address().unwrap_or_default().to_string(),
            )
        })
        // A present-but-empty From (no name and no address) is as good as missing;
        // fall back so it isn't cached/shown as a blank sender.
        .filter(|a| !(a.name.as_deref().unwrap_or("").is_empty() && a.email.is_empty()))
        .unwrap_or_else(|| Address::new(None, "unknown@unknown"));

    let subject = parsed
        .and_then(|m| m.subject())
        .filter(|s| !s.is_empty())
        .unwrap_or("(no subject)")
        .to_string();

    let date = parsed
        .and_then(|m| m.date())
        .map(|d| d.to_timestamp())
        .or_else(|| fetch.internal_date().map(|d| d.timestamp()))
        .unwrap_or(0);

    let message_id = parsed
        .and_then(|m| m.message_id())
        .map(|id| format!("<{}>", id.trim_matches(['<', '>'])));

    (from, subject, date, message_id)
}
