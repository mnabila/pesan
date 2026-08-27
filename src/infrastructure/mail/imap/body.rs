/// Readable body for the reader (and the searchable, cached copy). Prefers the
/// HTML part - usually the real content - rendered to Markdown (headings,
/// emphasis, lists, links, tables); falls back to the text/plain part, else
/// empty.
pub(crate) fn extract_body(msg: &mail_parser::Message<'_>) -> String {
    if let Some(html) = msg.body_html(0) {
        return normalize(&render_html(&html));
    }
    if let Some(text) = msg.body_text(0) {
        return normalize(&text);
    }
    String::new()
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
