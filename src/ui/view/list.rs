use super::*;

/// One resolved message-list row; the draw step applies glyphs and styles.
#[derive(Debug)]
pub(super) struct ListRow {
    pub flagged: bool,
    pub has_attachment: bool,
    pub sender: String,
    pub subject: String,
    /// Relative date ("Fri", "15:33"), already formatted.
    pub date: String,
    pub unread: bool,
    pub marked: bool,
}

/// Plain-data view model for the message list pane.
#[derive(Debug)]
pub(super) struct ListProps {
    pub title: String,
    pub focused: bool,
    pub rows: Vec<ListRow>,
    /// Highlighted row index (`None` renders the empty-state message).
    pub selected: Option<usize>,
}

/// Map one envelope (plus its membership in `App::marked`) to a row. Pure.
pub(super) fn list_row(env: &crate::mail::Envelope, marked: bool) -> ListRow {
    ListRow {
        flagged: env.flags.flagged,
        has_attachment: env.has_attachment,
        sender: truncate(&env.from.short(), 28),
        // A message cached with no subject still gets a readable placeholder
        // rather than a blank cell.
        subject: if env.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            env.subject.clone()
        },
        date: relative_date(env.date),
        unread: !env.flags.seen,
        marked,
    }
}

/// Resolve the message list's contents from app state. Pure read; no styling.
pub(super) fn props(app: &App) -> ListProps {
    let rows = app
        .display_envelopes
        .iter()
        .map(|env| list_row(env, app.marked.contains(&env.uid)))
        .collect();
    ListProps {
        title: format!(" {} ", app.selected_folder_name()),
        focused: app.active_pane() == Pane::List,
        selected: if app.display_envelopes.is_empty() {
            None
        } else {
            Some(app.selected_message)
        },
        rows,
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, p: &ListProps, skin: &Skin) {
    let block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(&p.title, skin.theme.dim_style())))
        .border_style(skin.border_style(p.focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows: Vec<Row> = p.rows.iter().map(|r| draw_row(r, skin)).collect();

    let header = Row::new(vec![
        // Leading star column (flagged/starred marker); header shows the glyph.
        Cell::from(skin.glyphs.flagged),
        Cell::from("SENDER"),
        Cell::from("SUBJECT"),
        // Right-aligned so the DATE header sits over its right-aligned values.
        // The trailing space keeps a one-column margin from the pane border.
        Cell::from(Line::from("DATE ").alignment(Alignment::Right)),
    ])
    .style(skin.theme.dim_style());

    match p.selected {
        None => frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no messages",
                skin.theme.dim_style(),
            )))
            .style(skin.theme.fg_style()),
            inner,
        ),
        Some(sel) => {
            let table = Table::new(rows, list_widths(inner.width))
                .header(header)
                .row_highlight_style(skin.theme.selected_style())
                .column_spacing(1)
                .block(Block::default());
            let mut state = TableState::default().with_selected(Some(sel));
            frame.render_stateful_widget(table, inner, &mut state);
        }
    }
}

/// Apply theme + glyphs to one resolved row. Pure.
fn draw_row(r: &ListRow, skin: &Skin) -> Row<'static> {
    // The star column shows the flagged glyph for starred messages; the
    // attachment marker still rides inline with the subject.
    let star = if r.flagged {
        Span::styled(skin.glyphs.flagged, skin.theme.warning_style())
    } else {
        Span::raw(" ")
    };
    let mut markers = String::new();
    if r.has_attachment {
        markers.push_str(skin.glyphs.attachment);
    }

    let base = if r.unread {
        skin.theme.unread_style()
    } else {
        skin.theme.fg_style()
    };

    let row = Row::new(vec![
        Cell::from(star),
        Cell::from(Span::styled(r.sender.clone(), base)),
        Cell::from(Span::styled(format!("{}{}", markers, r.subject), base)),
        // Right-aligned relative dates ("Fri", "15:33") with a trailing space so
        // they keep a one-column margin from the pane border.
        Cell::from(
            Line::from(Span::styled(format!("{} ", r.date), skin.theme.dim_style()))
                .alignment(Alignment::Right),
        ),
    ]);
    // Tagged (marked) rows are shown with a full-row background band rather than
    // a leading glyph, so a run of tagged rows reads as a group at a glance.
    if r.marked {
        row.style(skin.theme.marked_style())
    } else {
        row
    }
}

pub(super) fn list_widths(total: u16) -> [Constraint; 4] {
    // Fixed-ish split: a 2-wide star column, sender ~30%, subject flexible, date
    // ~14% of the column.
    const STAR: u16 = 2;
    let flex = total.saturating_sub(2); // room for borders
    let sender = ((flex * 30) / 100).clamp(8, 40);
    let date = ((flex * 14) / 100).clamp(6, 16);
    let subject = flex.saturating_sub(STAR + sender + date).max(10);
    [
        Constraint::Length(STAR),
        Constraint::Length(sender),
        Constraint::Min(subject),
        Constraint::Length(date),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(subject: &str, seen: bool) -> crate::mail::Envelope {
        crate::mail::Envelope {
            uid: 1,
            flags: crate::mail::Flags {
                seen,
                flagged: true,
            },
            from: crate::mail::Address::new(Some("Sender".into()), "s@x.io"),
            subject: subject.into(),
            date: 0,
            has_attachment: true,
            snippet: None,
            message_id: None,
        }
    }

    #[test]
    fn list_row_maps_flags_and_placeholders() {
        let row = list_row(&env("", false), true);
        assert!(row.unread);
        assert!(row.flagged);
        assert!(row.has_attachment);
        assert!(row.marked);
        assert_eq!(row.subject, "(no subject)");
        assert_eq!(row.sender, "Sender");

        let read = list_row(&env("hi", true), false);
        assert!(!read.unread);
        assert_eq!(read.subject, "hi");
    }

    #[test]
    fn list_widths_fit_minimums() {
        let w = list_widths(40);
        assert!(matches!(w[0], Constraint::Length(2)));
        assert!(matches!(w[3], Constraint::Length(_)));
        // Tiny panes keep the minimum subject width.
        let tiny = list_widths(8);
        assert!(matches!(tiny[2], Constraint::Min(10)));
    }
}
