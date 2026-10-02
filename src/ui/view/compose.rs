use super::*;

use ratatui::style::Style;

use crate::ui::widget;
use crate::ui::widget::form::FormField;

/// One staged attachment chip for the compose view.
#[derive(Debug)]
struct AttachmentChip {
    name: String,
    size: u64,
}

/// Plain-data view model for the compose screen (an inline email form).
pub(super) struct ComposeProps {
    /// Bordered-block title (" Compose ", " Reply ", " Forward ").
    title: String,
    /// Recipient (To) field (left column, row 1).
    to: FormField,
    /// Reply-To field (right column, row 1).
    reply_to: FormField,
    /// Cc field (left column, row 2).
    cc: FormField,
    /// Bcc field (right column, row 2).
    bcc: FormField,
    /// Subject field (full width, row 3).
    subject: FormField,
    attachments: Vec<AttachmentChip>,
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

    let mk = |label: &str,
              input: &widget::TextInput,
              focus: ComposeFocus,
              placeholder: &str| {
        let selected = compose.focus == focus;
        FormField::new(
            label,
            input,
            selected,
            selected && compose.editing,
            placeholder,
        )
    };
    let to = mk("To", &compose.to, ComposeFocus::To, "(no recipient)");
    let reply_to = mk(
        "Reply-To",
        &compose.reply_to,
        ComposeFocus::ReplyTo,
        "(none)",
    );
    let cc = mk("Cc", &compose.cc, ComposeFocus::Cc, "(none)");
    let bcc = mk("Bcc", &compose.bcc, ComposeFocus::Bcc, "(none)");
    let subject = mk(
        "Subject",
        &compose.subject,
        ComposeFocus::Subject,
        "(no subject)",
    );

    let attachments: Vec<AttachmentChip> = compose
        .attachments
        .iter()
        .map(|att| AttachmentChip {
            name: att.filename.clone(),
            size: att.size,
        })
        .collect();

    Some(ComposeProps {
        title,
        to,
        reply_to,
        cc,
        bcc,
        subject,
        attachments,
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

    // Pre-wrap the attachment chips so the attach area is sized to the exact
    // number of rows it needs (no fixed reserve, no empty space under it).
    let attach_lines = attachment_lines(&p.attachments, inner.width, skin);

    // Layout:
    //   From | Reply-To   (3 rows)
    //   Cc   | Bcc        (3 rows)
    //   Subject            (3 rows)
    //   Body               (Min 0; full height when no attachments)
    //   Attachments        (one row per wrapped chip line)
    let [header_area, body_area, attach_list] = Layout::vertical([
        Constraint::Length(9),                       // 3 rows of 2-col fields
        Constraint::Min(0),                          // Body container
        Constraint::Length(attach_lines.len() as u16), // staged attachments
    ])
    .areas(inner);

    // Header: 2-column grid (From|Reply-To, Cc|Bcc) + full-width Subject.
    let [row1, row2, subject_area] =
        Layout::vertical([Constraint::Length(3); 3]).areas(header_area);
    let [left1, _gap, right1] = Layout::horizontal([
        Constraint::Ratio(1, 2),
        Constraint::Length(1),
        Constraint::Ratio(1, 2),
    ])
    .areas(row1);
    let [left2, _gap2, right2] = Layout::horizontal([
        Constraint::Ratio(1, 2),
        Constraint::Length(1),
        Constraint::Ratio(1, 2),
    ])
    .areas(row2);

    form::render_field_box(frame, left1, &p.to, &skin.theme, skin.border);
    form::render_field_box(frame, right1, &p.reply_to, &skin.theme, skin.border);
    form::render_field_box(frame, left2, &p.cc, &skin.theme, skin.border);
    form::render_field_box(frame, right2, &p.bcc, &skin.theme, skin.border);
    form::render_field_box(frame, subject_area, &p.subject, &skin.theme, skin.border);

    // Body: a bordered container with the body text. Attachments are handled by
    // Ctrl-f (file picker), not an inline field.
    let body_block = Block::bordered()
        .border_type(skin.border)
        .border_style(if p.body_focused {
            skin.border_style(true)
        } else {
            skin.border_style(false)
        })
        .title(Line::from(Span::styled(
            " Body ",
            if p.body_focused {
                skin.theme.accent_style()
            } else {
                skin.theme.dim_style()
            },
        )));
    let body_inner = body_block.inner(body_area);
    frame.render_widget(body_block, body_area);

    let (body_text, body_style) = if p.body.trim().is_empty() {
        (
            "(empty - press i to edit in $EDITOR, press ctrl+f for attach file)".to_string(),
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
        body_inner,
    );

    // Staged attachments as pill-style chips, pre-wrapped to the exact rows.
    if !attach_lines.is_empty() {
        frame.render_widget(
            Paragraph::new(attach_lines).style(skin.theme.fg_style()),
            attach_list,
        );
    }
}

/// Lay the staged attachments out as pill-style chips wrapped across rows,
/// returning one `Line` per row (plus a trailing "+N more" line when the list
/// overflows `MAX_ATTACH_ROWS`). Empty when there are no attachments, so the
/// caller can size the attach area to exactly `len()` rows.
fn attachment_lines<'a>(
    attachments: &'a [AttachmentChip],
    width: u16,
    skin: &Skin,
) -> Vec<Line<'a>> {
    if attachments.is_empty() {
        return Vec::new();
    }
    let chips: Vec<Vec<Span>> = attachments
        .iter()
        .map(|att| {
            vec![
                Span::styled(
                    format!(" {} ", skin.glyphs.attachment),
                    Style::new().fg(skin.theme.bg).bg(skin.theme.border),
                ),
                Span::styled(att.name.clone(), skin.theme.fg_style()),
                Span::styled(
                    format!(" {} ", human_size(att.size)),
                    skin.theme.dim_style(),
                ),
            ]
        })
        .collect();
    let chip_widths: Vec<u16> = chips
        .iter()
        .map(|c| c.iter().map(|s| s.width() as u16).sum())
        .collect();

    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_w: u16 = 0;
    for (i, w) in chip_widths.iter().enumerate() {
        let add = if cur.is_empty() { *w } else { ATTACH_GAP + *w };
        if !cur.is_empty() && cur_w + add > width {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(i);
        cur_w += add;
    }
    if !cur.is_empty() {
        rows.push(cur);
    }

    let shown_rows = rows.len().min(MAX_ATTACH_ROWS);
    let shown_count: usize = rows[..shown_rows].iter().map(|r| r.len()).sum();
    let hidden = chips.len() - shown_count;

    let mut lines: Vec<Line> = Vec::new();
    for row in &rows[..shown_rows] {
        let mut spans: Vec<Span> = Vec::new();
        for (j, &ci) in row.iter().enumerate() {
            if j > 0 {
                spans.push(Span::raw(" ".repeat(ATTACH_GAP as usize)));
            }
            spans.extend(chips[ci].clone());
        }
        lines.push(Line::from(spans));
    }
    if hidden > 0 {
        lines.push(Line::from(Span::styled(
            format!("  +{hidden} more"),
            skin.theme.dim_style(),
        )));
    }
    lines
}

/// Max attachment rows before truncating with a "+N more" summary row.
const MAX_ATTACH_ROWS: usize = 3;

/// Horizontal gap (cells) between attachment chips.
const ATTACH_GAP: u16 = 2;

/// Human-readable byte size for the attachment list (e.g., "312 KB", "1.2 MB").
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
