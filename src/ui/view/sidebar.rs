use super::*;

/// One resolved row of the sidebar tree, built by [`props`] from app state so
/// the draw step never touches `App`.
#[derive(Debug)]
pub(super) enum RowKind {
    /// An account header line; `active` marks the account being viewed.
    Account { label: String, active: bool },
    /// The MAILBOX/MSGS/UNREAD column header (drawn above the first folder).
    ColumnHeader,
    /// A folder line with its display columns already formatted.
    Folder {
        name: String,
        msgs: String,
        unread: String,
    },
}

#[derive(Debug)]
pub(super) struct SideRow {
    pub kind: RowKind,
    pub selected: bool,
}

/// Plain-data view model for the folder/account sidebar pane.
#[derive(Debug)]
pub(super) struct SidebarProps {
    /// Whether the folders pane holds focus (drives border emphasis).
    pub focused: bool,
    pub collapsed: bool,
    /// While filtering accounts, a live filter bar occupies the bottom row.
    pub filtering: bool,
    pub filter: String,
    pub rows: Vec<SideRow>,
    /// Index into `rows` that the cursor currently highlights (the row drawn
    /// with the inverse selection style). `None` when nothing is selected.
    pub selected: Option<usize>,
}

/// Resolve the sidebar's contents from app state. Pure read; no styling.
pub(super) fn props(app: &App) -> SidebarProps {
    use crate::ui::app::SidebarItem;
    let focused = app.active_pane() == Pane::Folders;

    let mut rows: Vec<SideRow> = Vec::new();
    let mut header_drawn = false;
    for item in app.sidebar_items() {
        let selected = focused && item == app.sidebar_sel;
        match item {
            SidebarItem::Account(i) => {
                let Some(acct) = app.accounts.get(i) else {
                    continue;
                };
                let label = if app.config.ui.account_label.eq_ignore_ascii_case("email") {
                    acct.email.clone()
                } else {
                    acct.name.clone()
                };
                rows.push(SideRow {
                    kind: RowKind::Account {
                        label,
                        active: i == app.active_account,
                    },
                    selected,
                });
            }
            SidebarItem::Folder(idx) => {
                let Some(folder) = app.folders.get(idx) else {
                    continue;
                };
                // Column header, drawn once above the first folder row.
                if !header_drawn {
                    rows.push(SideRow {
                        kind: RowKind::ColumnHeader,
                        selected: false,
                    });
                    header_drawn = true;
                }
                // Indent one level under the account, plus folder hierarchy depth.
                let base = if app.accounts.is_empty() { 0 } else { 1 };
                let is_parent = app.folder_has_children(idx);
                let toggle = if is_parent {
                    if app.folder_collapsed[idx] {
                        "+ "
                    } else {
                        "- "
                    }
                } else {
                    "  "
                };
                let indent = " ".repeat(base + folder.depth());
                let name = format!("{indent}{toggle}{}", folder.short_name());
                // "" hides a zero count.
                let msgs = if folder.total > 0 {
                    folder.total.to_string()
                } else {
                    String::new()
                };
                let unread = if folder.unread > 0 {
                    folder.unread.to_string()
                } else {
                    String::new()
                };
                rows.push(SideRow {
                    kind: RowKind::Folder { name, msgs, unread },
                    selected,
                });
            }
        }
    }

    let selected = rows.iter().position(|r| r.selected);

    SidebarProps {
        focused,
        collapsed: app.sidebar_collapsed,
        filtering: app.sidebar_filtering,
        filter: app.sidebar_filter.clone(),
        rows,
        selected,
    }
}

pub(super) fn render(frame: &mut Frame, area: Rect, p: &SidebarProps, skin: &Skin) {
    let block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(
            " mailboxes ",
            skin.theme.dim_style(),
        )))
        .border_style(skin.border_style(p.focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if p.collapsed {
        return;
    }

    // While filtering accounts, a live filter bar occupies the pane's bottom row.
    let (tree_area, filter_bar) = if p.filtering {
        let [tree, bar] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(inner);
        (tree, Some(bar))
    } else {
        (inner, None)
    };

    // Three-column folder table: MAILBOX (name) | MSGS (total) | UNREAD. The
    // numeric columns are right-aligned into fixed-width slots at the pane edge.
    // The mailboxes section itself is drawn with a ratatui `List` so cursor
    // highlighting, scrolling, and keyboard navigation are handled by the widget,
    // while each folder row keeps its three-column table layout.
    let width = tree_area.width as usize;
    let msgs_w = 6usize;
    let unread_w = 7usize;
    let name_w = width.saturating_sub(msgs_w + unread_w);
    // Pad/truncate the mailbox name into its column, then append the two numeric
    // columns right-aligned.
    let three_col = |name: &str, msgs: &str, unread: &str| -> String {
        let name = truncate(name, name_w);
        format!("{name:<name_w$}{msgs:>msgs_w$}{unread:>unread_w$}")
    };

    // Resolve each row into a `ListItem` carrying its own (unselected) style.
    // `List` applies `highlight_style` to whichever item is selected, so an
    // item's base style is only used when it is not the cursor row.
    let mut items: Vec<ListItem> = Vec::new();
    for row in &p.rows {
        // Active account is shown in bold; the column header and inactive
        // account lines are dimmed. The selected row's style is supplied by the
        // list's `highlight_style` below, not here.
        let style = match &row.kind {
            RowKind::Account { active: true, .. } => {
                skin.theme.fg_style().add_modifier(Modifier::BOLD)
            }
            RowKind::Account { active: false, .. } => skin.theme.dim_style(),
            RowKind::ColumnHeader => skin.theme.dim_style(),
            RowKind::Folder { .. } => skin.theme.fg_style(),
        };
        let body = match &row.kind {
            RowKind::Account { label, .. } => label.clone(),
            RowKind::ColumnHeader => three_col("MAILBOX", "MSGS", "UNREAD"),
            RowKind::Folder { name, msgs, unread } => three_col(name, msgs, unread),
        };
        items.push(ListItem::new(Line::from(Span::styled(body, style))));
    }
    if p.filtering && items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            " no accounts match",
            skin.theme.dim_style(),
        ))));
    }

    let mut state = ListState::default().with_selected(p.selected);
    let list = List::new(items)
        .style(skin.theme.fg_style())
        .highlight_style(skin.theme.selected_style())
        .highlight_symbol("")
        .scroll_padding(1);
    frame.render_stateful_widget(list, tree_area, &mut state);
    if let Some(bar) = filter_bar {
        render_filter_bar(frame, bar, skin, &p.filter, p.filtering);
    }
}
