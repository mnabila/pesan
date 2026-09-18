use super::*;

/// Full-screen message reader (its own `View`): a title bar, the message body,
/// and a footer of reader keys. Entered by opening a message from the list.
pub(crate) fn draw_reader(frame: &mut Frame, body: Rect, app: &App) {
    render(frame, body, app);
}

/// Extract a header field's value from a raw RFC822 header block, unfolding
/// continuation lines (those beginning with whitespace). Case-insensitive on the
/// field name; returns `None` if the header is absent or its value is blank.
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
        // Unfold: subsequent lines starting with a space/tab continue this header.
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

#[cfg(test)]
mod reader_header_tests {
    use super::header_value;

    #[test]
    fn reads_to_recipient_not_case_sensitive() {
        let raw = "From: a@b.com\nTO: dest@x.io\nSubject: hi";
        assert_eq!(header_value(raw, "To").as_deref(), Some("dest@x.io"));
        assert_eq!(header_value(raw, "from").as_deref(), Some("a@b.com"));
    }

    #[test]
    fn unfolds_and_joins_multiline_recipients() {
        let raw = "To: one@x.io,\n\ttwo@x.io,\n three@x.io\nSubject: hi";
        assert_eq!(
            header_value(raw, "To").as_deref(),
            Some("one@x.io, two@x.io, three@x.io")
        );
    }

    #[test]
    fn missing_or_blank_header_is_none() {
        assert_eq!(header_value("From: a@b.com", "To"), None);
        assert_eq!(header_value("To:   \nSubject: hi", "To"), None);
    }
}

/// The reader's header section, resolved to plain data by [`reader_props`].
#[derive(Debug)]
pub(super) enum ReaderHeader {
    /// Structured fields we always know. In the collapsed view the subject is
    /// drawn as the bold title line instead of a field row.
    Fields {
        subject: String,
        from: String,
        to: String,
        date: String,
        /// Only shown in the expanded (headers) view.
        message_id: Option<String>,
    },
    /// Full raw RFC822 header block (available after a live fetch).
    Raw(Vec<String>),
}

/// Plain-data view model for the reader pane: resolved header fields plus the
/// final body lines (memoized HTML/Markdown render, or an inline re-render when
/// the cache wasn't primed this frame).
#[derive(Debug)]
pub(super) struct ReaderProps {
    pub header: Option<ReaderHeader>,
    pub body: Vec<Line<'static>>,
    pub offset: usize,
}

/// Resolve the reader's contents from app state. Pure read; styling happens in
/// the draw step. `width` is the *body* pane width in cells, needed only for
/// the inline Markdown fallback (the memoized path ignores it).
pub(super) fn reader_props(app: &App, width: usize) -> ReaderProps {
    let Some(msg) = &app.open_message else {
        return ReaderProps {
            header: None,
            body: Vec::new(),
            offset: 0,
        };
    };

    // The real recipient comes from the message's own `To` header (present once
    // the body/headers are fetched); only when we have no headers do we fall back
    // to the viewing account. Hardcoding the account was wrong for sent mail,
    // where the recipient is someone else.
    let reader_to = msg
        .raw_headers
        .as_deref()
        .and_then(|h| header_value(h, "To"))
        .unwrap_or_else(|| app.active_account_email().to_string());

    let header = if app.show_headers && msg.raw_headers.is_some() {
        ReaderHeader::Raw(
            msg.raw_headers
                .as_deref()
                .unwrap_or("")
                .lines()
                .map(str::to_string)
                .collect(),
        )
    } else {
        ReaderHeader::Fields {
            subject: msg.envelope.subject.clone(),
            from: msg.envelope.from.display(),
            to: reader_to,
            date: full_timestamp(msg.envelope.date),
            message_id: msg.envelope.message_id.clone(),
        }
    };

    // Memoized body render (HTML via html2text, else Markdown - headings, bold,
    // lists, links, tables, ...). `App::prepare_reader` builds these once per
    // open/resize/theme change - off the draw path - so a repaint (scroll,
    // spinner tick) never re-parses the whole body. The event loop primes the
    // cache before every real draw; the inline fallback covers a standalone
    // draw (a body was set without a `prepare_reader` pass).
    let memoized = app.reader_lines();
    let body = if memoized.is_empty() && !msg.body.is_empty() {
        crate::tui::html::render_message(msg, width, &app.theme, app.config.ui.ascii)
    } else {
        memoized.to_vec()
    };

    ReaderProps {
        header: Some(header),
        body,
        offset: app.reader_offset,
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, app: &App) {
    let skin = Skin::of(app);

    let block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(
            " message ",
            skin.theme.dim_style(),
        )))
        .border_style(skin.border_style(true));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.open_message.is_none() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no message open",
                skin.theme.dim_style(),
            ))),
            inner,
        );
        return;
    }

    let p = reader_props(app, inner.width as usize);
    let mut lines: Vec<Line> = Vec::new();
    match &p.header {
        None => unreachable!("open_message checked above"),
        Some(ReaderHeader::Raw(raw)) => {
            for header_line in raw {
                lines.push(Line::from(Span::styled(
                    header_line.clone(),
                    skin.theme.dim_style(),
                )));
            }
        }
        Some(ReaderHeader::Fields {
            subject,
            from,
            to,
            date,
            message_id,
        }) => {
            let field = |k: &'static str, v: String| {
                Line::from(vec![
                    Span::styled(k, skin.theme.dim_style()),
                    Span::raw(": "),
                    Span::raw(v),
                ])
            };
            if app.show_headers {
                // Expanded view: the structured fields we always know, plus a
                // note that raw headers aren't available for cached messages.
                lines.push(field("Subject", subject.clone()));
                lines.push(field("From", from.clone()));
                lines.push(field("To", to.clone()));
                lines.push(field("Date", date.clone()));
                if let Some(id) = message_id {
                    lines.push(field("Message-ID", id.clone()));
                }
                lines.push(Line::from(Span::styled(
                    "(full headers unavailable for cached message)",
                    skin.theme.dim_style(),
                )));
            } else {
                // Collapsed view: the subject as a bold title line, then From/
                // To/Date field rows.
                lines.push(Line::from(Span::styled(
                    subject.clone(),
                    skin.theme.fg_style().add_modifier(Modifier::BOLD),
                )));
                lines.push(field("From", from.clone()));
                lines.push(field("To", to.clone()));
                lines.push(field("Date", date.clone()));
            }
        }
    }
    lines.push(Line::raw(""));
    lines.extend(p.body.iter().cloned());

    // Scroll counts all rows (headers + separator + body), so the whole
    // message stays reachable.
    let max = lines.len().saturating_sub(1);
    let offset = p.offset.min(max);
    let view_h = inner.height as usize;
    let end = (offset + view_h).min(lines.len());
    let window: Vec<Line> = lines[offset..end].to_vec();
    frame.render_widget(Paragraph::new(window).style(skin.theme.fg_style()), inner);
}
