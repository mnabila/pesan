use html2text::render::{RichAnnotation, TaggedLine};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::mail::Message;
use crate::ui::theme::Theme;

/// Render a message body to width-wrapped styled lines: the stored raw HTML
/// via [`render`] when present (HTML mail), else the plain-text body rendered
/// as wrapped plain text. Falls back to the plain body when the HTML carries
/// no visible text (e.g. image-only mail whose alt text only survives in the
/// conversion).
pub(super) fn render_message(
    msg: &Message,
    width: usize,
    theme: &Theme,
    ascii: bool,
) -> Vec<Line<'static>> {
    if let Some(html) = msg.raw_html.as_deref().filter(|h| !h.trim().is_empty()) {
        let lines = render(html, width, theme, ascii);
        if !lines.is_empty() {
            return lines;
        }
    }
    // Plain-text body: wrap at width, preserve paragraphs.
    let lines = render_plain(&msg.body, width, theme, ascii);
    if !lines.is_empty() {
        return lines;
    }
    // Last resort: the body has content neither renderer could shape.
    if msg.body.trim().is_empty() {
        return lines;
    }
    msg.body
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect()
}

/// Render raw HTML email to width-wrapped styled lines via `html2text`'s rich
/// interface (one hop: DOM -> wrapped annotated lines). `width` is the body
/// pane width in cells; `ascii` swaps unicode table/list glyphs for ASCII.
/// Pure separator lines (`----------`, `======`, ...) become clean full-width
/// dim dividers, collapsed when repeated. Returns an empty vec when the HTML
/// carries no visible text, so the caller can fall back to the plain-text body.
pub(super) fn render(html: &str, width: usize, theme: &Theme, ascii: bool) -> Vec<Line<'static>> {
    let width = width.max(1);
    let tagged = match rich_config().lines_from_read(html.as_bytes(), width) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    // Blank lines are buffered, not emitted, so a run of dividers separated
    // only by blanks still collapses into one.
    let mut pending_blanks: Vec<Line<'static>> = Vec::new();
    let mut prev_divider = false;
    for tagged_line in &tagged {
        if let Some(div) = divider_line(tagged_line, width, theme, ascii) {
            if prev_divider {
                pending_blanks.clear();
                continue;
            }
            out.append(&mut pending_blanks);
            out.push(div);
            prev_divider = true;
            continue;
        }
        let mut spans: Vec<Span<'static>> = Vec::new();
        for seg in tagged_line.tagged_strings() {
            let mut text = if ascii {
                to_ascii(&seg.s)
            } else {
                seg.s.clone()
            };
            if let Some(url) = image_url(&seg.tag) {
                text = format!("[image: {}]", truncate_url(url));
            }
            spans.push(Span::styled(text, style_for(&seg.tag, theme)));
            if let Some(url) = link_url(&seg.tag, &seg.s) {
                spans.push(Span::styled(format!(" ({url})"), theme.dim_style()));
            }
        }
        let line = Line::from(spans);
        if line.spans.iter().all(|s| s.content.trim().is_empty()) {
            pending_blanks.push(line);
            continue;
        }
        out.append(&mut pending_blanks);
        // `html2text` wraps to `width`, but an unbreakable run (long URL) can
        // still overflow; hard-break those so the pane never clips content.
        push_wrapped(&mut out, line, width);
        prev_divider = false;
    }
    // Trailing buffered blanks are dropped; any left in `out` are trimmed here.
    while out
        .last()
        .is_some_and(|l: &Line<'static>| l.spans.iter().all(|s| s.content.trim().is_empty()))
    {
        out.pop();
    }
    out
}

/// Render plain text body to width-wrapped styled lines. Preserves paragraph
/// breaks (blank lines) and wraps long lines at word boundaries.
fn render_plain(text: &str, width: usize, theme: &Theme, _ascii: bool) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut out = Vec::new();
    let style = theme.fg_style();
    let mut paragraph = String::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            // Blank line = paragraph break
            if !paragraph.is_empty() {
                out.extend(wrap_paragraph(&paragraph, width, style));
                paragraph.clear();
            }
            out.push(Line::from(""));
            continue;
        }
        if !paragraph.is_empty() {
            paragraph.push(' ');
        }
        paragraph.push_str(line.trim());
    }
    if !paragraph.is_empty() {
        out.extend(wrap_paragraph(&paragraph, width, style));
    }
    // Trim trailing blank lines
    while out.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
        out.pop();
    }
    out
}

fn wrap_paragraph(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0;
    for word in text.split_whitespace() {
        let w = word.width();
        if cur_w + w > width && !cur.is_empty() {
            out.push(Line::from(Span::styled(std::mem::take(&mut cur), style)));
            cur_w = 0;
        }
        if !cur.is_empty() {
            cur.push(' ');
            cur_w += 1;
        }
        cur.push_str(word);
        cur_w += w;
    }
    if !cur.is_empty() {
        out.push(Line::from(Span::styled(cur, style)));
    }
    out
}

/// Agent stylesheet applied to every rendered message: drops 1px tracking
/// pixels (and similar beacon images) while leaving real images alone.
/// Mirrors the extraction-side rules - keep the two in sync (the TUI must not
/// import infrastructure, so the literal is duplicated, not shared).
const AGENT_CSS: &str =
    "img[width=\"1\"] { display: none; }\nimg[height=\"1\"] { display: none; }\n";

/// Rich converter with doc CSS (honours the message's own `display:none`
/// rules) plus [`AGENT_CSS`]. Falls back to doc CSS alone if the static rules
/// ever fail to parse (a test pins that they parse).
fn rich_config() -> html2text::config::Config<html2text::render::RichDecorator> {
    let conf = html2text::config::rich().use_doc_css();
    conf.add_agent_css(AGENT_CSS)
        .unwrap_or_else(|_| html2text::config::rich().use_doc_css())
}

/// Separator punctuation that, covering a whole line, reads as a divider
/// rather than content. Table joints (`┬┴┼│`) are deliberately absent, so
/// structured table borders never match.
const RULE_CHARS: &[char] = &['-', '─', '═', '=', '*', '_'];
/// Minimum run length for a punctuation-only line to count as a divider.
/// Short runs (`--` signature separators) stay verbatim as meaningful content.
const MIN_RULE_LEN: usize = 5;

/// A clean full-width divider for a separator-only line, or `None` when the
/// tagged line is real content. Lines carrying code/pre/link/image annotations
/// are never dividers, so snippets, ASCII diagrams, and linked text survive
/// verbatim.
fn divider_line(
    tagged_line: &TaggedLine<Vec<RichAnnotation>>,
    width: usize,
    theme: &Theme,
    ascii: bool,
) -> Option<Line<'static>> {
    let mut run = 0usize;
    for seg in tagged_line.tagged_strings() {
        if seg.tag.iter().any(|t| {
            matches!(
                t,
                RichAnnotation::Code
                    | RichAnnotation::Preformat(_)
                    | RichAnnotation::Link(_)
                    | RichAnnotation::Image(_)
            )
        }) {
            return None;
        }
        for ch in seg.s.chars() {
            if ch.is_whitespace() {
                continue;
            }
            if !RULE_CHARS.contains(&ch) {
                return None;
            }
            run += 1;
        }
    }
    if run < MIN_RULE_LEN {
        return None;
    }
    let ch = if ascii { '-' } else { '─' };
    Some(Line::from(Span::styled(
        ch.to_string().repeat(width),
        theme.dim_style(),
    )))
}

/// The link target when this span is a link whose visible text is not already
/// the URL (mirrors the Markdown renderer's suffix rule).
fn link_url(tags: &[RichAnnotation], text: &str) -> Option<String> {
    for tag in tags {
        if let RichAnnotation::Link(url) = tag {
            let shown = text.trim();
            let bare = url.strip_prefix("mailto:").unwrap_or(url);
            if !url.is_empty() && shown != url && shown != bare {
                return Some(url.clone());
            }
            return None;
        }
    }
    None
}

/// Extract the image URL from an Image annotation.
fn image_url(tags: &[RichAnnotation]) -> Option<&str> {
    for tag in tags {
        if let RichAnnotation::Image(url) = tag
            && !url.is_empty()
        {
            return Some(url);
        }
    }
    None
}

/// Truncate a URL to a reasonable display length.
fn truncate_url(url: &str) -> String {
    const MAX_LEN: usize = 60;
    if url.len() <= MAX_LEN {
        return url.to_string();
    }
    // Try to keep the domain + a bit of path
    if let Some(rest) = url.strip_prefix("https://")
        && let Some(slash) = rest.find('/')
    {
        let domain = &rest[..slash];
        let path = &rest[slash..];
        let keep = MAX_LEN.saturating_sub(domain.len() + 3); // "://" + "..."
        if keep > 10 {
            return format!("https://{domain}{}", &path[..keep.min(path.len())]) + "...";
        }
    }
    format!("{}...", &url[..MAX_LEN - 3])
}

fn style_for(tags: &[RichAnnotation], theme: &Theme) -> Style {
    let mut style = theme.fg_style();
    for tag in tags {
        match tag {
            RichAnnotation::Default | RichAnnotation::Preformat(_) => {}
            RichAnnotation::Strong => {
                style = style.add_modifier(Modifier::BOLD);
            }
            RichAnnotation::Emphasis => {
                style = style.add_modifier(Modifier::ITALIC);
            }
            RichAnnotation::Strikeout => {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
            RichAnnotation::Code => {
                style = style.patch(Style::new().fg(theme.fg).bg(theme.border));
            }
            RichAnnotation::Link(_) => {
                style = style.patch(theme.accent_style().add_modifier(Modifier::UNDERLINED));
            }
            RichAnnotation::Image(_) => {
                style = style.patch(theme.dim_style());
            }
            _ => {}
        }
    }
    style
}

/// ASCII fallback for the unicode glyphs `html2text` emits in tables/lists.
fn to_ascii(s: &str) -> String {
    s.replace(['•', '─'], "-")
        .replace('│', "|")
        .replace(['┌', '┐', '└', '┘', '├', '┤', '┬', '┴', '┼'], "+")
}

/// Push `line`, hard-breaking it on char boundaries when it exceeds `width`.
fn push_wrapped(out: &mut Vec<Line<'static>>, line: Line<'static>, width: usize) {
    if line.width() <= width {
        out.push(line);
        return;
    }
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0usize;
    let mut spans = line.spans.into_iter().peekable();
    while let Some(span) = spans.next() {
        let w = span.content.width();
        if cur_w + w <= width {
            cur_w += w;
            cur.push(span);
            continue;
        }
        // Split this span char by char; flush full rows as we go.
        let style = span.style;
        let last = spans.peek().is_none();
        let mut buf = String::new();
        let mut buf_w = 0usize;
        for ch in span.content.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if cur_w + buf_w + cw > width && (buf_w > 0 || !cur.is_empty()) {
                if !buf.is_empty() {
                    cur.push(Span::styled(std::mem::take(&mut buf), style));
                    buf_w = 0;
                }
                out.push(Line::from(std::mem::take(&mut cur)));
                cur_w = 0;
            }
            buf.push(ch);
            buf_w += cw;
        }
        if !buf.is_empty() {
            cur.push(Span::styled(buf, style));
            cur_w += buf_w;
        }
        if last {
            break;
        }
    }
    // `cur` is only empty when the span split landed exactly on the boundary
    // (already flushed); don't emit a spurious blank row for that.
    if !cur.is_empty() {
        out.push(Line::from(cur));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;

    fn theme() -> Theme {
        Theme::dark()
    }

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn has_modifier(lines: &[Line<'_>], m: Modifier) -> bool {
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.style.add_modifier.contains(m))
    }

    #[test]
    fn layout_table_has_no_pipe_grid() {
        // Newsletter-style layout tables must not become Markdown pipe grids.
        let html = "<table><tr><td>Left promo</td><td>Right deal</td></tr></table><p>Shop now</p>";
        let lines = render(html, 60, &theme(), false);
        let out = text(&lines);
        assert!(out.contains("Left promo"), "cell lost: {out:?}");
        assert!(out.contains("Right deal"), "cell lost: {out:?}");
        assert!(out.contains("Shop now"), "body lost: {out:?}");
        assert!(!out.contains('|'), "table borders leaked: {out:?}");
    }

    #[test]
    fn drops_hidden_and_script_content() {
        let html = "<style>p.hide { display: none; }</style><script>var x = 1;</script>\
            <p class=\"hide\">tracking pixel text</p>\
            <p style=\"display:none\">inline hidden</p><p>Visible hello</p>";
        let out = text(&render(html, 60, &theme(), false));
        assert!(!out.contains("tracking pixel"), "hidden leaked: {out:?}");
        assert!(
            !out.contains("inline hidden"),
            "inline hidden leaked: {out:?}"
        );
        assert!(!out.contains("var x"), "script leaked: {out:?}");
        assert!(out.contains("Visible hello"), "visible lost: {out:?}");
    }

    #[test]
    fn renders_links_lists_and_emphasis() {
        let html = "<ul><li>Alpha</li><li>Beta</li></ul>\
            <p>See <a href=\"https://acme.io/x\">doc</a> for <b>bold</b> and <i>italic</i>.</p>";
        let lines = render(html, 60, &theme(), false);
        let out = text(&lines);
        assert!(
            out.contains("Alpha") && out.contains("Beta"),
            "list lost: {out:?}"
        );
        // Link target is appended once, dimmed, when the text is not the URL.
        assert!(
            out.contains("doc (https://acme.io/x)"),
            "link lost: {out:?}"
        );
        assert!(
            !out.contains("https://acme.io/x) ("),
            "URL duplicated: {out:?}"
        );
        assert!(has_modifier(&lines, Modifier::BOLD), "bold lost");
        assert!(has_modifier(&lines, Modifier::ITALIC), "italic lost");
    }

    #[test]
    fn wraps_to_width_and_breaks_long_runs() {
        let html = "<p>lorem ipsum dolor sit amet consectetur adipiscing elit sed do</p>\
            <p>https://example.com/a-very-long-url-that-cannot-break-naturally-at-all</p>";
        let width = 30;
        let lines = render(html, width, &theme(), false);
        assert!(lines.len() > 2, "should wrap to several rows");
        for line in &lines {
            assert!(
                line.width() <= width,
                "row overflows width {width}: {line:?}"
            );
        }
    }

    #[test]
    fn ascii_mode_uses_ascii_borders() {
        let html = "<table><tr><td>a</td><td>b</td></tr></table>";
        let out = text(&render(html, 40, &theme(), true));
        assert!(
            !out.contains(['─', '│', '┬', '┴']),
            "unicode leaked: {out:?}"
        );
    }

    #[test]
    fn empty_html_yields_empty_for_fallback() {
        assert!(render("", 60, &theme(), false).is_empty());
        assert!(render("<script>var x = 1;</script>", 60, &theme(), false).is_empty());
    }

    #[test]
    fn dash_runs_become_full_width_dim_dividers() {
        let theme = theme();
        let width = 40;
        let lines = render(
            "<p>----------</p><p>hi</p><p>======</p>",
            width,
            &theme,
            false,
        );
        let div = "─".repeat(width);
        let dividers: Vec<_> = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content == div))
            .collect();
        assert_eq!(dividers.len(), 2, "both runs become dividers: {lines:?}");
        for d in dividers {
            assert_eq!(d.spans.len(), 1);
            assert_eq!(d.spans[0].style, theme.dim_style());
        }
        let out = text(&lines);
        assert!(!out.contains("----------"), "raw run leaked: {out:?}");
    }

    #[test]
    fn repeated_dividers_collapse_into_one() {
        let width = 40;
        let lines = render(
            "<p>----------</p><p>----------</p><p>hello</p><p>----------</p>",
            width,
            &theme(),
            false,
        );
        let div = "─".repeat(width);
        let count = lines
            .iter()
            .filter(|l| l.spans.iter().any(|s| s.content == div))
            .count();
        assert_eq!(count, 2, "adjacent repeats collapse: {lines:?}");
        assert!(text(&lines).contains("hello"));
    }

    #[test]
    fn text_bearing_and_short_dash_lines_untouched() {
        let out = text(&render(
            "<p>---------- Forwarded message ----------</p><p>--</p>",
            60,
            &theme(),
            false,
        ));
        assert!(
            out.contains("---------- Forwarded message ----------"),
            "forward header mangled: {out:?}"
        );
        assert!(out.contains("--"), "signature separator lost: {out:?}");
    }

    #[test]
    fn pre_blocks_keep_their_dashes() {
        let out = text(&render("<pre>----------</pre>", 40, &theme(), false));
        assert!(out.contains("----------"), "pre mangled: {out:?}");
        assert!(
            !out.contains(&"─".repeat(40)),
            "pre converted to divider: {out:?}"
        );
    }

    #[test]
    fn tag_bearing_plain_text_renders_literally() {
        // A text/plain body containing tags (e.g. a mislabeled message) must
        // show literally, never a blank pane.
        use crate::mail::{Address, Envelope, Flags, Message};
        let msg = Message {
            envelope: Envelope {
                uid: 1,
                flags: Flags::default(),
                from: Address::new(None, "a@b.c"),
                subject: "t".into(),
                date: 0,
                has_attachment: false,
                snippet: None,
                message_id: None,
            },
            body: "<p>Hello <b>there</b></p>".into(),
            raw_html: None,
            raw_headers: None,
            raw: None,
        };
        let out = text(&render_message(&msg, 60, &theme(), false));
        assert!(!out.trim().is_empty(), "message rendered blank");
        assert!(out.contains("Hello"), "content lost: {out:?}");
    }

    #[test]
    fn agent_css_drops_tracking_pixels_not_real_images() {
        let html = "<p>Hello</p>\
            <img src=\"https://tracker.x/p.gif\" width=\"1\" height=\"1\" alt=\"\">\
            <img src=\"https://cdn.x/photo.jpg\" width=\"600\" alt=\"photo\">";
        let out = text(&render(html, 60, &theme(), false));
        assert!(!out.contains("tracker.x"), "beacon leaked: {out:?}");
        assert!(out.contains("Hello"), "content lost: {out:?}");
        assert!(out.contains("photo"), "real image lost: {out:?}");
    }

    #[test]
    fn image_urls_are_truncated_to_placeholder() {
        let html = r#"<p>Before</p><img src="https://messhi.example.com/very/long/path/to/image.png" alt="https://messhi.example.com/very/long/path/to/image.png"><p>After</p>"#;
        let out = text(&render(html, 80, &theme(), false));
        assert!(out.contains("[image: https://messhi.example.com/very/long/path/to/image.png"), "short URL shown fully: {}", out);
        assert!(out.contains("Before"), "content before lost");
        assert!(out.contains("After"), "content after lost");
    }

    #[test]
    fn long_image_urls_are_truncated() {
        let long_url = "https://cdn.example.com/a/very/long/path/that/exceeds/the/display/limit/for/image/urls/in/the/terminal.jpg";
        let html = format!("<img src=\"{}\" alt=\"{}\">", long_url, long_url);
        let out = text(&render(&html, 80, &theme(), false));
        assert!(out.contains("[image: "), "placeholder present");
        assert!(out.contains("..."), "truncated with ellipsis");
        assert!(!out.contains(long_url), "full URL not shown");
    }

    #[test]
    fn ascii_dividers_use_dashes() {
        let width = 40;
        let lines = render("<p>----------</p>", width, &theme(), true);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content == "-".repeat(width))),
            "no ascii divider: {lines:?}"
        );
    }

    #[test]
    fn render_message_prefers_html_then_plain() {
        use crate::mail::{Address, Envelope, Flags, Message};
        let msg = Message {
            envelope: Envelope {
                uid: 1,
                flags: Flags::default(),
                from: Address::new(None, "a@b.c"),
                subject: "s".into(),
                date: 0,
                has_attachment: false,
                snippet: None,
                message_id: None,
            },
            body: "plain fallback".into(),
            raw_html: Some("<p>Rich <b>html</b></p>".into()),
            raw_headers: None,
            raw: None,
        };
        let out = text(&render_message(&msg, 60, &theme(), false));
        assert!(out.contains("Rich"), "HTML path not used: {out:?}");
        assert!(!out.contains("plain fallback"), "wrong source: {out:?}");

        let plain = Message {
            raw_html: None,
            ..msg
        };
        let out = text(&render_message(&plain, 60, &theme(), false));
        assert!(out.contains("plain fallback"), "fallback lost: {out:?}");
    }
}
