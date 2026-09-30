use super::*;

use crate::ui::widget;

/// One staged attachment row for the compose view.
#[derive(Debug)]
struct AttachmentRow {
    name: String,
    size: u64,
}

/// Plain-data view model for the compose screen (an inline email form).
pub(super) struct ComposeProps {
    /// Bordered-block title (" Compose ", " Reply ", " Forward ").
    title: String,
    /// Fixed From line; empty renders the "(no account)" hint.
    from: String,
    /// Editable header/attach rows in draw order.
    fields: Vec<form::FormField>,
    attachments: Vec<AttachmentRow>,
    /// When the list overflows its rows, how many are hidden behind the
    /// "… N more" summary row (`0` shows everything).
    attachments_hidden: usize,
    body: String,
    body_focused: bool,
    body_scroll: u16,
}

/// Resolve the compose screen's contents from app state. Pure read.
fn props(app: &App) -> Option<ComposeProps> {
    let compose = app.compose.as_ref()?;
    let title = match compose.mode {
        ComposeMode::New => " Compose ",
        ComposeMode::Reply => " Reply ",
        ComposeMode::Forward => " Forward ",
    }
    .to_string();

    // Editable header/attach rows. Each is "label" + a focused clone of the
    // field's TextInput (so the caret shows on the focused row).
    let mk = |label: &str,
              input: &widget::TextInput,
              focus: ComposeFocus,
              placeholder: &str| {
        let selected = compose.focus == focus;
        form::FormField::new(
            label,
            input,
            selected,
            selected && compose.editing,
            placeholder,
        )
    };
    let fields = vec![
        mk("To", &compose.to, ComposeFocus::To, "(no recipient)"),
        mk("Cc", &compose.cc, ComposeFocus::Cc, "(none)"),
        mk("Bcc", &compose.bcc, ComposeFocus::Bcc, "(none)"),
        mk(
            "Reply-To",
            &compose.reply_to,
            ComposeFocus::ReplyTo,
            "(none)",
        ),
        mk(
            "Subject",
            &compose.subject,
            ComposeFocus::Subject,
            "(no subject)",
        ),
        mk(
            "Attach",
            &compose.attach_input,
            ComposeFocus::Attach,
            "(type a file path, Enter to add)",
        ),
    ];

    // At most 6 attachment rows fit; when more are staged the last visible row
    // summarizes the remainder so nothing is hidden silently.
    const MAX_ATTACH_ROWS: usize = 6;
    let total = compose.attachments.len();
    let attachments_hidden = total.saturating_sub(if total > MAX_ATTACH_ROWS {
        MAX_ATTACH_ROWS - 1
    } else {
        total
    });
    let attachments: Vec<AttachmentRow> = compose
        .attachments
        .iter()
        .map(|att| AttachmentRow {
            name: att.filename.clone(),
            size: att.size,
        })
        .collect();

    Some(ComposeProps {
        title,
        from: app.active_account_email().to_string(),
        fields,
        attachments,
        attachments_hidden,
        body: compose.body.clone(),
        body_focused: compose.focus == ComposeFocus::Body,
        body_scroll: compose.body_scroll as u16,
    })
}

pub(super) fn draw_compose(frame: &mut Frame, body: Rect, app: &App) {
    let Some(p) = props(app) else { return };
    render(frame, body, p, &Skin::of(app));
}

fn render(frame: &mut Frame, body: Rect, p: ComposeProps, skin: &Skin) {
    let block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(
            &p.title,
            skin.theme.accent_style().add_modifier(Modifier::BOLD),
        )))
        .border_style(skin.border_style(true));
    let inner = block.inner(body);
    frame.render_widget(block, body);

    // An inline form: a read-only From line, six editable header/attach rows,
    // the attachment list, a rule, then the body preview.
    let attach_rows = if p.attachments_hidden > 0 {
        // The summary row replaces the last slot.
        MAX_VISIBLE_ATTACH_ROWS - 1
    } else {
        p.attachments.len() as u16
    };
    let [from_row, fields_area, attach_list, rule, body_area] = Layout::vertical([
        Constraint::Length(1),           // From (read-only)
        Constraint::Length(9),           // 6 fields as a 2-col grid (3 boxes, 3 rows each)
        Constraint::Length(attach_rows), // staged attachments
        Constraint::Length(1),           // rule
        Constraint::Min(3),              // body
    ])
    .areas(inner);

    // From is fixed to the active account.
    let (from_text, from_style) = if p.from.is_empty() {
        ("(no account)".to_string(), skin.theme.dim_style())
    } else {
        (p.from.clone(), skin.theme.fg_style())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{:<9}", "From"), skin.theme.dim_style()),
            Span::raw(" "),
            Span::styled(from_text, from_style),
        ])),
        from_row,
    );

    // Editable header/attach rows, laid out as a 2-column grid of bordered
    // boxes (3 rows of boxes, each box 3 rows tall = 9 rows total).
    let [left, _gap, right] = Layout::horizontal([
        Constraint::Ratio(1, 2),
        Constraint::Length(1),
        Constraint::Ratio(1, 2),
    ])
    .areas(fields_area);
    let left_rows = Layout::vertical([Constraint::Length(3); 3]).split(left);
    let right_rows = Layout::vertical([Constraint::Length(3); 3]).split(right);
    for (i, field) in p.fields.iter().enumerate() {
        let area = if i % 2 == 0 {
            left_rows[i / 2]
        } else {
            right_rows[i / 2]
        };
        form::render_field_box(frame, area, field, skin.theme, skin.border);
    }

    // Staged attachments, one dim row each: "<glyph> name  (size)". When there
    // are more than fit, the last row summarizes the remainder so nothing is
    // hidden silently.
    if attach_rows > 0 {
        let truncated = p.attachments_hidden > 0;
        let shown = if truncated {
            attach_rows as usize - 1
        } else {
            p.attachments.len()
        };
        let mut lines: Vec<Line> = p
            .attachments
            .iter()
            .take(shown)
            .map(|att| {
                Line::from(vec![
                    Span::styled(
                        format!("  {} ", skin.glyphs.attachment),
                        skin.theme.dim_style(),
                    ),
                    Span::styled(att.name.clone(), skin.theme.fg_style()),
                    Span::styled(
                        format!("  ({})", human_size(att.size)),
                        skin.theme.dim_style(),
                    ),
                ])
            })
            .collect();
        if truncated {
            lines.push(Line::from(Span::styled(
                format!("  … {} more", p.attachments_hidden),
                skin.theme.dim_style(),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines).style(skin.theme.fg_style()),
            attach_list,
        );
    }

    // Divider between the fields and the body.
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(rule.width as usize),
            skin.theme.border_style(false),
        ))),
        rule,
    );

    // Body: a focus-aware label then the wrapped, read-only body text ($EDITOR).
    let [body_label, body_text_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body_area);
    let body_label_style = if p.body_focused {
        skin.theme.accent_style()
    } else {
        skin.theme.dim_style()
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("Body", body_label_style))),
        body_label,
    );
    let (body_text, body_style) = if p.body.trim().is_empty() {
        (
            "(empty - press i to edit in $EDITOR)".to_string(),
            skin.theme.dim_style(),
        )
    } else {
        (p.body.clone(), skin.theme.fg_style())
    };
    frame.render_widget(
        Paragraph::new(body_text)
            .style(body_style)
            .wrap(Wrap { trim: false })
            .scroll((p.body_scroll, 0)),
        body_text_area,
    );
}

/// Rows available for staged attachments before the "... more" summary kicks in.
const MAX_VISIBLE_ATTACH_ROWS: u16 = 6;

/// Human-readable byte size for the attachment list (e.g. "312 KB", "1.2 MB").
pub(super) fn human_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{} KB", bytes.div_ceil(KB))
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::human_size;

    #[test]
    fn human_size_scales_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1 KB");
        assert_eq!(human_size(312 * 1024 + 500), "313 KB");
        assert_eq!(human_size(1536 * 1024), "1.5 MB");
    }
}
