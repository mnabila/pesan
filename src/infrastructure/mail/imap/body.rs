/// Readable body for the reader (and the searchable, cached copy). Prefers an
/// HTML part - usually the real content - rendered to Markdown (headings,
/// emphasis, lists, links, tables); falls back to a text/plain part. Some mail
/// nests the readable part (multipart/related, forwarded messages) so that part
/// index 0 is empty, so every HTML then text part is tried before giving up -
/// otherwise the reader shows a blank pane for a message that does have text.
/// When there is genuinely no text part, the attachment list is summarized so
/// the pane still shows something useful.
pub(crate) fn extract_body(msg: &mail_parser::Message<'_>) -> String {
    for i in 0..msg.html_body_count() {
        if let Some(html) = msg.body_html(i) {
            let body = normalize(&render_html(&html));
            if !body.is_empty() {
                return body;
            }
        }
    }
    for i in 0..msg.text_body_count() {
        if let Some(text) = msg.body_text(i) {
            let body = normalize(&text);
            if !body.is_empty() {
                return body;
            }
        }
    }
    attachment_summary(msg)
}

/// Fallback body for a message with no readable text part (e.g. an attachment-
/// only mail or a bare calendar invite): list the attachment names so the reader
/// pane is never mysteriously blank. Empty when there is nothing at all to show.
fn attachment_summary(msg: &mail_parser::Message<'_>) -> String {
    use mail_parser::MimeHeaders;
    let names: Vec<&str> = msg
        .attachments()
        .filter_map(|part| part.attachment_name())
        .collect();
    if names.is_empty() {
        return String::new();
    }
    let mut out = String::from("(This message has no text content.)\n\nAttachments:\n");
    for name in names {
        out.push_str("- ");
        out.push_str(name);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Collapse runs of blank lines to one and trim trailing whitespace so the
/// reader pane renders bodies consistently.
fn normalize(body: &str) -> String {
    let mut out = String::new();
    let mut blanks = 0;
    for line in body.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(trimmed);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Render an HTML body to Markdown for the terminal reader (and the searchable,
/// cached copy). Headings, emphasis, lists, links, and tables become their
/// Markdown equivalents; `<script>`/`<style>`/`<head>` are dropped so embedded
/// CSS/JS never leaks into the body. On a parse failure, fall back to a crude
/// tag/entity strip so a malformed part never yields an empty body.
fn render_html(html: &str) -> String {
    // Build the converter once per thread rather than per message: the skip-tag
    // set is constant, and bodies are converted on the single IMAP worker thread.
    thread_local! {
        static CONVERTER: htmd::HtmlToMarkdown = htmd::HtmlToMarkdown::builder()
            .skip_tags(vec!["script", "style", "head", "title"])
            .build();
    }
    CONVERTER
        .with(|c| c.convert(html))
        .unwrap_or_else(|_| strip_html(html))
}

/// Last-resort HTML-to-text fallback used only when the Markdown conversion
/// fails: remove `<...>` tags and decode the handful of most common entities.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::MessageParser;

    #[test]
    fn strip_html_removes_tags_and_entities() {
        let html = "<p>Hello&nbsp;<b>world</b> &amp; goodbye</p>";
        assert_eq!(strip_html(html), "Hello world & goodbye");
    }

    #[test]
    fn normalize_collapses_blank_runs() {
        let input = "a\n\n\n\nb\n\n";
        assert_eq!(normalize(input), "a\n\nb");
    }

    #[test]
    fn extract_body_uses_plain_text_when_no_html() {
        let raw =
            b"From: a@b.com\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\nHello there\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        assert_eq!(extract_body(&msg), "Hello there");
    }

    #[test]
    fn extract_body_renders_html_structure() {
        let raw = b"From: a@b.com\r\nSubject: Hi\r\nContent-Type: text/html\r\n\r\n\
            <ul><li>Alpha</li><li>Beta</li></ul><p>See <a href=\"https://acme.io/x\">doc</a>.</p>\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let body = extract_body(&msg);
        // List items render as Markdown bullets and the link becomes an inline
        // `[text](url)`, rather than the tags being crudely stripped away.
        assert!(body.contains("Alpha"), "list item missing: {body:?}");
        assert!(body.contains("Beta"), "list item missing: {body:?}");
        assert!(
            body.lines()
                .any(|l| l.trim_start().starts_with(['*', '-']) && l.contains("Alpha")),
            "not a Markdown list: {body:?}"
        );
        assert!(
            body.contains("[doc](https://acme.io/x)"),
            "link not rendered as Markdown: {body:?}"
        );
    }

    #[test]
    fn extract_body_prefers_html_over_plain() {
        let raw = b"From: a@b.com\r\nSubject: Hi\r\nMIME-Version: 1.0\r\n\
            Content-Type: multipart/alternative; boundary=\"BOUND\"\r\n\r\n\
            --BOUND\r\nContent-Type: text/plain\r\n\r\nplain fallback\r\n\
            --BOUND\r\nContent-Type: text/html\r\n\r\n<p>Rich <b>HTML</b> body</p>\r\n\
            --BOUND--\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let body = extract_body(&msg);
        assert!(body.contains("Rich"), "HTML part not rendered: {body:?}");
        assert!(body.contains("HTML"), "HTML part not rendered: {body:?}");
        assert!(
            !body.contains("plain fallback"),
            "should prefer HTML over the text/plain part: {body:?}"
        );
    }

    #[test]
    fn extract_body_summarizes_attachments_when_no_text() {
        // An attachment-only message (no text/html part) must not render blank.
        let raw = b"From: a@b.com\r\nSubject: Invoice\r\nMIME-Version: 1.0\r\n\
            Content-Type: multipart/mixed; boundary=\"B\"\r\n\r\n\
            --B\r\nContent-Type: application/pdf\r\n\
            Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n\r\n%PDF-1.4\r\n\
            --B--\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let body = extract_body(&msg);
        assert!(body.contains("no text content"), "unexpected body: {body:?}");
        assert!(body.contains("invoice.pdf"), "attachment not listed: {body:?}");
    }

    #[test]
    fn header_meta_reads_from_subject() {
        let raw = b"From: Jane <jane@acme.io>\r\nSubject: Q3 plan\r\nDate: Wed, 20 Aug 2025 10:24:00 +0000\r\n\r\nbody";
        let msg = MessageParser::default().parse(raw.as_slice());
        // A bare Fetch can't be constructed here, so exercise the parser side
        // by checking the mail-parser accessors the mapper relies on.
        let m = msg.unwrap();
        assert_eq!(
            m.from().unwrap().first().unwrap().address(),
            Some("jane@acme.io")
        );
        assert_eq!(m.subject(), Some("Q3 plan"));
        assert!(m.date().unwrap().to_timestamp() > 0);
    }
}
