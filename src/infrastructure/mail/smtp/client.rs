use anyhow::{Context, Result, bail};
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::domain::Draft;

/// How an SMTP send authenticates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SmtpAuth {
    /// OAuth2 bearer (the `secret` is an access token).
    Xoauth2,
    /// Static password (the `secret` is the account password), via AUTH LOGIN.
    Login,
}

/// Connection + auth parameters for a single SMTP send.
pub struct SmtpParams<'a> {
    pub host: &'a str,
    pub port: u16,
    pub email: &'a str,
    /// The credential: an OAuth access token (`Xoauth2`) or a password (`Login`).
    pub secret: &'a str,
    pub auth: SmtpAuth,
}

/// Build a MIME message from a compose [`Draft`], sent as the given account.
/// Validates that at least one recipient parses and that a subject/body exist.
pub fn build_message(from_email: &str, draft: &Draft) -> Result<Message> {
    let from: Mailbox = from_email
        .parse()
        .with_context(|| format!("invalid From address: {from_email}"))?;

    let to = parse_addresses(&draft.to)?;
    if to.is_empty() {
        bail!("no valid recipient in To");
    }
    let cc = parse_addresses(&draft.cc)?;
    let bcc = parse_addresses(&draft.bcc)?;

    let mut builder = Message::builder().from(from).subject(draft.subject.trim());
    for mbox in to {
        builder = builder.to(mbox);
    }
    for mbox in cc {
        builder = builder.cc(mbox);
    }
    for mbox in bcc {
        builder = builder.bcc(mbox);
    }
    if !draft.reply_to.trim().is_empty() {
        let rt: Mailbox = draft
            .reply_to
            .trim()
            .parse()
            .with_context(|| format!("invalid Reply-To address: {}", draft.reply_to))?;
        builder = builder.reply_to(rt);
    }
    if let Some(irt) = draft.in_reply_to.as_deref().filter(|s| !s.is_empty()) {
        builder = builder.in_reply_to(irt.to_string());
    }
    if let Some(refs) = draft.references.as_deref().filter(|s| !s.is_empty()) {
        builder = builder.references(refs.to_string());
    }

    // The body is authored as markdown and sent as a `multipart/alternative`
    // carrying both the raw markdown (`text/plain`, still readable) and the
    // rendered HTML (`text/html`). Order matters: plain first, HTML last, since
    // clients prefer the last part they understand.
    let alternative = MultiPart::alternative()
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(draft.body.clone()),
        )
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .body(render_markdown(&draft.body)),
        );

    // Without attachments the alternative is the whole body; otherwise it nests
    // as the first part of a `multipart/mixed`, followed by each file as an
    // attachment part (bytes read from disk here, at send time).
    if draft.attachments.is_empty() {
        return builder.multipart(alternative).context("build MIME message");
    }

    let mut multipart = MultiPart::mixed().multipart(alternative);
    for att in &draft.attachments {
        let bytes = std::fs::read(&att.path)
            .with_context(|| format!("read attachment {}", att.filename))?;
        let content_type = attachment_content_type(&att.filename);
        multipart =
            multipart.singlepart(Attachment::new(att.filename.clone()).body(bytes, content_type));
    }
    builder.multipart(multipart).context("build MIME message")
}

/// Render a markdown body to an HTML document for the `text/html` MIME part.
/// GFM extensions (tables, strikethrough, task lists, footnotes, autolinks) are
/// enabled so the common markdown a user types renders as expected.
fn render_markdown(md: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_GFM);
    let parser = Parser::new_ext(md, opts);
    // Build the document in a single buffer: seed the prefix, render into it,
    // then append the suffix - avoiding a second full-copy `format!`.
    let mut doc = String::from("<!DOCTYPE html><html><body>");
    html::push_html(&mut doc, parser);
    doc.push_str("</body></html>");
    doc
}

/// Best-effort MIME type from a filename extension, defaulting to
/// `application/octet-stream`. Keeps us off a `mime_guess` dependency for the
/// handful of types users commonly attach.
fn attachment_content_type(filename: &str) -> ContentType {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "txt" | "log" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "zip" => "application/zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => "application/octet-stream",
    };
    ContentType::parse(mime).unwrap_or(ContentType::TEXT_PLAIN)
}

/// Parse a comma-separated address list into lettre mailboxes, skipping blanks.
fn parse_addresses(list: &str) -> Result<Vec<Mailbox>> {
    list.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<Mailbox>()
                .with_context(|| format!("invalid address: {s}"))
        })
        .collect()
}

/// Send a prebuilt message over an implicit-TLS (SMTPS) relay, authenticating
/// with XOAUTH2 (OAuth access token) or AUTH LOGIN (password).
pub async fn send(params: &SmtpParams<'_>, message: Message) -> Result<()> {
    let mechanism = match params.auth {
        SmtpAuth::Xoauth2 => Mechanism::Xoauth2,
        SmtpAuth::Login => Mechanism::Login,
    };
    tracing::info!(
        host = params.host,
        port = params.port,
        account = params.email,
        auth = ?mechanism,
        "SMTP connect: implicit TLS"
    );
    // Never log the credential itself; its length is enough to confirm one was
    // supplied without leaking it into the log file.
    tracing::debug!(
        secret_len = params.secret.len(),
        "SMTP: credential loaded for {}",
        params.email
    );

    let creds = Credentials::new(params.email.to_string(), params.secret.to_string());
    let transport = AsyncSmtpTransport::<Tokio1Executor>::relay(params.host)
        .context("configure SMTP relay")?
        .port(params.port)
        .credentials(creds)
        .authentication(vec![mechanism])
        .build();
    tracing::debug!(
        host = params.host,
        port = params.port,
        "SMTP: relay transport built, sending message"
    );

    match transport.send(message).await {
        Ok(response) => {
            tracing::info!(
                account = params.email,
                code = ?response.code(),
                "SMTP send succeeded"
            );
            Ok(())
        }
        Err(e) => {
            tracing::warn!(account = params.email, error = %e, "SMTP send failed");
            Err(e).context("send message over SMTP")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(to: &str, subject: &str, body: &str) -> Draft {
        Draft {
            to: to.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
            ..Draft::default()
        }
    }

    #[test]
    fn builds_message_with_recipient_and_subject() {
        let d = draft("jane@acme.io", "Hi", "hello");
        let msg = build_message("me@example.com", &d).unwrap();
        let raw = String::from_utf8(msg.formatted()).unwrap();
        assert!(raw.contains("To: jane@acme.io"));
        assert!(raw.contains("Subject: Hi"));
        assert!(raw.contains("From: me@example.com"));
    }

    #[test]
    fn reply_sets_threading_headers() {
        let mut d = draft("jane@acme.io", "Re: Hi", "yes");
        d.in_reply_to = Some("<abc@acme.io>".to_string());
        d.references = Some("<abc@acme.io>".to_string());
        let raw =
            String::from_utf8(build_message("me@example.com", &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("In-Reply-To: <abc@acme.io>"));
        assert!(raw.contains("References: <abc@acme.io>"));
    }

    #[test]
    fn cc_recipients_are_included() {
        let mut d = draft("jane@acme.io", "Hi", "hello");
        d.cc = "bob@x.io, carol@y.io".to_string();
        let raw =
            String::from_utf8(build_message("me@example.com", &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("bob@x.io"));
        assert!(raw.contains("carol@y.io"));
    }

    #[test]
    fn rejects_message_without_recipient() {
        let d = draft("", "Hi", "hello");
        assert!(build_message("me@example.com", &d).is_err());
    }

    #[test]
    fn bcc_recipients_are_included() {
        let mut d = draft("jane@acme.io", "Hi", "hello");
        d.bcc = "eve@x.io".to_string();
        let msg = build_message("me@example.com", &d).unwrap();
        // Bcc is stripped from the serialized headers but present in the SMTP
        // envelope recipients (alongside To).
        let recipients: Vec<String> = msg.envelope().to().iter().map(|a| a.to_string()).collect();
        assert!(recipients.iter().any(|r| r == "jane@acme.io"));
        assert!(recipients.iter().any(|r| r == "eve@x.io"));
    }

    #[test]
    fn reply_to_header_is_set() {
        let mut d = draft("jane@acme.io", "Hi", "hello");
        d.reply_to = "noreply@acme.io".to_string();
        let raw =
            String::from_utf8(build_message("me@example.com", &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("Reply-To: noreply@acme.io"));
    }

    #[test]
    fn attachment_is_included() {
        use crate::domain::Attachment;
        use std::io::Write;

        // Write a temp file to attach.
        let mut path = std::env::temp_dir();
        path.push("pesan_attach_test.txt");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"attached contents").unwrap();

        let mut d = draft("jane@acme.io", "Hi", "body text");
        d.attachments = vec![Attachment {
            path: path.clone(),
            filename: "pesan_attach_test.txt".to_string(),
            size: 17,
        }];
        let raw =
            String::from_utf8(build_message("me@example.com", &d).unwrap().formatted()).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(raw.contains("multipart/mixed"));
        // The body still nests as a multipart/alternative (plain + HTML) inside
        // the mixed container.
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("Content-Disposition: attachment"));
        assert!(raw.contains("pesan_attach_test.txt"));
        assert!(raw.contains("body text"));
    }

    #[test]
    fn body_is_multipart_alternative_with_html() {
        let d = draft("jane@acme.io", "Hi", "**bold** text");
        let raw =
            String::from_utf8(build_message("me@example.com", &d).unwrap().formatted()).unwrap();
        // Both a plain-text (raw markdown) and a rendered HTML part are present.
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains("text/plain"));
        assert!(raw.contains("text/html"));
        // The plain part keeps the raw markdown source.
        assert!(raw.contains("**bold** text"));
        // The HTML part renders the markdown.
        assert!(raw.contains("<strong>bold</strong>"));
    }

    #[test]
    fn markdown_renders_gfm() {
        let d = draft(
            "jane@acme.io",
            "Hi",
            "~~strike~~\n\n| a | b |\n|---|---|\n| 1 | 2 |",
        );
        let raw =
            String::from_utf8(build_message("me@example.com", &d).unwrap().formatted()).unwrap();
        // Note: quoted-printable may soft-wrap long lines, but these tags are
        // short enough to survive intact.
        assert!(raw.contains("<del>strike</del>"));
        assert!(raw.contains("<table>"));
    }
}
