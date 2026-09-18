/// Readable body for the reader (and the searchable, cached copy). Prefers an
/// HTML part - usually the real content - kept as raw HTML for the terminal
/// reader (rendered width-aware at draw time) with a plain-text conversion
/// alongside for search and quoting; falls back to a text/plain part. Some mail
/// nests the readable part (multipart/related, forwarded messages) so that part
/// index 0 is empty, so every HTML then text part is tried before giving up -
/// otherwise the reader shows a blank pane for a message that does have text.
/// When there is genuinely no text part, the attachment list is summarized so
/// the pane still shows something useful.
pub(crate) struct ExtractedBody {
    /// Plain searchable text: the text/plain part verbatim, or the HTML part
    /// converted to text. Also feeds reply/forward quoting.
    pub text: String,
    /// Original HTML source when `text` came from an HTML part (capped), so the
    /// reader can render it faithfully. `None` for plain-text mail.
    pub raw_html: Option<String>,
}

/// Cap on the stored HTML source: full newsletters can be megabytes, while the
/// visible text is a fraction of that. `html2text` tolerates truncation.
const RAW_HTML_CAP: usize = 128 * 1024;

/// Width the stored plain-text conversion is wrapped to. Only the cached copy
/// (search, quotes) uses this; the reader re-renders from `raw_html` at the
/// live pane width.
const PLAIN_WRAP_WIDTH: usize = 80;

pub(crate) fn extract_body(msg: &mail_parser::Message<'_>) -> ExtractedBody {
    use mail_parser::MimeHeaders;
    for i in 0..msg.html_body_count() {
        // `mail_parser` also lists text/plain parts as HTML bodies (it can
        // auto-convert them); only genuine text/html parts belong on the HTML
        // path, otherwise a plain message would pointlessly store converted HTML.
        let is_html = msg
            .html_part(i as u32)
            .is_some_and(|p| p.is_content_type("text", "html"));
        if !is_html {
            continue;
        }
        if let Some(html) = msg.body_html(i) {
            let text = normalize(&render_html_text(&html));
            if !text.is_empty() {
                return ExtractedBody {
                    text,
                    raw_html: Some(truncate_html(&html)),
                };
            }
        }
    }
    for i in 0..msg.text_body_count() {
        if let Some(text) = msg.body_text(i) {
            let text = normalize(&text);
            if !text.is_empty() {
                return ExtractedBody {
                    text,
                    raw_html: None,
                };
            }
        }
    }
    ExtractedBody {
        text: attachment_summary(msg),
        raw_html: None,
    }
}

/// Truncate HTML to [`RAW_HTML_CAP`] on a char boundary.
fn truncate_html(html: &str) -> String {
    if html.len() <= RAW_HTML_CAP {
        return html.to_string();
    }
    let mut end = RAW_HTML_CAP;
    while !html.is_char_boundary(end) {
        end -= 1;
    }
    html[..end].to_string()
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

/// Convert an HTML body to plain searchable text. `display:none` content and
/// `script`/`style` never reach the output (via the `css` feature + doc CSS),
/// so embedded CSS/JS and tracking pixels don't leak into search or quotes.
/// On conversion failure returns empty, letting the caller try the next part.
///
/// Known upstream quirk: a doc-CSS rule whose last declaration lacks the
/// trailing semicolon (e.g. `.hide{display:none}`) is ignored by the CSS
/// parser; inline `style="display:none"` always applies.
fn render_html_text(html: &str) -> String {
    plain_config()
        .string_from_read(html.as_bytes(), PLAIN_WRAP_WIDTH)
        .unwrap_or_default()
}

/// Agent stylesheet mirrored in `tui::html` (duplicated, not shared: neither
/// layer may import the other for rendering). Drops 1px tracking pixels while
/// leaving real images alone.
const AGENT_CSS: &str =
    "img[width=\"1\"] { display: none; }\nimg[height=\"1\"] { display: none; }\n";

/// Plain converter with doc CSS plus [`AGENT_CSS`], falling back to doc CSS
/// alone if the static rules ever fail to parse.
fn plain_config() -> html2text::config::Config<html2text::render::PlainDecorator> {
    let conf = html2text::config::plain().use_doc_css();
    conf.add_agent_css(AGENT_CSS)
        .unwrap_or_else(|_| html2text::config::plain().use_doc_css())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::MessageParser;

    #[test]
    fn normalize_collapses_blank_runs() {
        let input = "a\n\n\n\nb\n\n";
        assert_eq!(normalize(input), "a\n\nb");
    }

    #[test]
    fn extract_body_keeps_tags_in_plain_text_verbatim() {
        // A text/plain part is stored literally even when it looks like HTML;
        // the reader shows it verbatim rather than parsing it.
        let raw =
            b"From: a@b.com\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\n<p>Hello</p>\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let out = extract_body(&msg);
        assert_eq!(out.text, "<p>Hello</p>");
        assert_eq!(out.raw_html, None);
    }

    #[test]
    fn extract_body_uses_plain_text_when_no_html() {
        let raw =
            b"From: a@b.com\r\nSubject: Hi\r\nContent-Type: text/plain\r\n\r\nHello there\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let out = extract_body(&msg);
        assert_eq!(out.text, "Hello there");
        assert_eq!(out.raw_html, None);
    }

    #[test]
    fn extract_body_keeps_raw_html_and_plain_text() {
        let raw = b"From: a@b.com\r\nSubject: Hi\r\nContent-Type: text/html\r\n\r\n\
            <ul><li>Alpha</li><li>Beta</li></ul><p>See <a href=\"https://acme.io/x\">doc</a>.</p>\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let out = extract_body(&msg);
        assert!(
            out.text.contains("Alpha"),
            "list item missing: {:?}",
            out.text
        );
        assert!(
            out.text.contains("Beta"),
            "list item missing: {:?}",
            out.text
        );
        assert!(
            out.text.contains("doc"),
            "link text missing: {:?}",
            out.text
        );
        assert!(
            out.text.contains("https://acme.io/x"),
            "link target missing: {:?}",
            out.text
        );
        let raw_html = out.raw_html.expect("HTML source must be preserved");
        assert!(raw_html.contains("<ul>"), "raw HTML kept: {raw_html:?}");
    }

    #[test]
    fn extract_body_drops_hidden_and_script_content() {
        let raw = b"From: a@b.com\r\nSubject: Hi\r\nContent-Type: text/html\r\n\r\n\
            <style>p.hide { display: none; }</style><script>var x = 1;</script>\
            <p class=\"hide\">tracking pixel text</p>\
            <p style=\"display:none\">inline hidden</p><p>Visible hello</p>\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let out = extract_body(&msg);
        assert!(
            !out.text.contains("tracking pixel"),
            "hidden text leaked: {:?}",
            out.text
        );
        assert!(
            !out.text.contains("inline hidden"),
            "inline hidden text leaked: {:?}",
            out.text
        );
        assert!(!out.text.contains("var x"), "script leaked: {:?}", out.text);
        assert!(
            out.text.contains("Visible hello"),
            "visible lost: {:?}",
            out.text
        );
    }

    #[test]
    fn extract_body_drops_tracking_pixels_from_plain_text() {
        let raw = b"From: a@b.com\r\nSubject: Hi\r\nContent-Type: text/html\r\n\r\n\
            <p>Hello</p><img src=\"https://tracker.x/p.gif\" width=\"1\" height=\"1\" alt=\"\">\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let out = extract_body(&msg);
        assert!(
            !out.text.contains("tracker.x"),
            "beacon leaked: {:?}",
            out.text
        );
        assert!(out.text.contains("Hello"), "content lost: {:?}", out.text);
    }

    #[test]
    fn extract_body_prefers_html_over_plain() {
        let raw = b"From: a@b.com\r\nSubject: Hi\r\nMIME-Version: 1.0\r\n\
            Content-Type: multipart/alternative; boundary=\"BOUND\"\r\n\r\n\
            --BOUND\r\nContent-Type: text/plain\r\n\r\nplain fallback\r\n\
            --BOUND\r\nContent-Type: text/html\r\n\r\n<p>Rich <b>HTML</b> body</p>\r\n\
            --BOUND--\r\n";
        let msg = MessageParser::default().parse(raw.as_slice()).unwrap();
        let out = extract_body(&msg);
        assert!(
            out.text.contains("Rich"),
            "HTML part not rendered: {:?}",
            out.text
        );
        assert!(
            out.text.contains("HTML"),
            "HTML part not rendered: {:?}",
            out.text
        );
        assert!(
            !out.text.contains("plain fallback"),
            "should prefer HTML over the text/plain part: {:?}",
            out.text
        );
        assert!(out.raw_html.is_some());
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
        let out = extract_body(&msg);
        assert!(
            out.text.contains("no text content"),
            "unexpected body: {:?}",
            out.text
        );
        assert!(
            out.text.contains("invoice.pdf"),
            "attachment not listed: {:?}",
            out.text
        );
        assert_eq!(out.raw_html, None);
    }

    #[test]
    fn raw_html_is_capped() {
        let big = format!("<p>{}</p>", "x".repeat(RAW_HTML_CAP + 100));
        let capped = truncate_html(&big);
        assert!(capped.len() <= RAW_HTML_CAP);
        assert!(big.starts_with(&capped[..capped.len().min(100)]));
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
