use super::account_manager::{capitalize, pill};
use super::*;

/// One resolved row of the accounts list.
#[derive(Debug)]
struct AccountRow {
    name: String,
    email: String,
    provider: String,
    connected: bool,
    is_default: bool,
    selected: bool,
}

/// Plain-data view model for the accounts list pane.
#[derive(Debug)]
pub(super) struct AccountListProps {
    /// True when no accounts exist at all (shows the "press a" hint).
    accounts_empty: bool,
    filtering: bool,
    filter: String,
    rows: Vec<AccountRow>,
}

pub(super) fn account_list_props(app: &App) -> AccountListProps {
    let Some(state) = &app.settings else {
        return AccountListProps {
            accounts_empty: true,
            filtering: false,
            filter: String::new(),
            rows: Vec::new(),
        };
    };
    // Selected = list cursor, only while not filtering.
    let rows = state
        .filtered_accounts()
        .into_iter()
        .filter_map(|i| state.accounts.get(i).map(|acct| (i, acct)))
        .map(|(i, acct)| AccountRow {
            name: acct.name.clone(),
            email: acct.email.clone(),
            provider: acct.provider.clone(),
            connected: app.account_connected(i),
            is_default: acct.is_default,
            selected: state.focus == SettingsFocus::Accounts
                && !state.filtering
                && i == state.selected,
        })
        .collect();
    AccountListProps {
        accounts_empty: state.accounts.is_empty(),
        filtering: state.filtering || !state.account_filter.is_empty(),
        filter: state.account_filter.clone(),
        rows,
    }
}

/// Column widths for the list: expand to fill the pane so the table isn't a
/// narrow strip on a wide window - name/provider get sensible shares and the
/// email column absorbs the rest. Falls back to compact fixed widths when the
/// pane is too narrow to split up. Pure.
fn account_cols(width: usize) -> (usize, usize) {
    const CURSOR_W: usize = 2;
    const PROVIDER_W: usize = 12;
    const STATUS_W: usize = 24; // trailing "<glyph> connected  <glyph> default"
    let fixed = CURSOR_W + PROVIDER_W + STATUS_W;
    if width > fixed + 28 {
        let flex = width - fixed;
        let name_w = (flex * 2 / 5).clamp(14, 32);
        let email_w = flex.saturating_sub(name_w).max(20);
        (name_w, email_w)
    } else {
        (14, 26)
    }
}

pub(super) fn render_account_list(
    frame: &mut Frame,
    area: Rect,
    p: &AccountListProps,
    skin: &Skin,
) {
    // A live filter bar occupies the pane's bottom row once filtering starts;
    // it persists while a query narrows the list, matching the other sections.
    let (list_area, filter_bar) = if p.filtering {
        let [list, bar] = Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(area);
        (list, Some(bar))
    } else {
        (area, None)
    };

    // Empty states keep the plain-text hint (no table needed).
    if p.accounts_empty {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  No accounts yet - press  a  to add one.",
                skin.theme.dim_style(),
            )))
            .style(skin.theme.fg_style()),
            list_area,
        );
    } else if p.rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  No accounts match the filter.",
                skin.theme.dim_style(),
            )))
            .style(skin.theme.fg_style()),
            list_area,
        );
    } else {
        // Column widths: name/email absorb the slack (the cursor column is added
        // automatically by `highlight_symbol`), provider is fixed, status flexes.
        let width = list_area.width as usize;
        let (name_w, email_w) = account_cols(width);
        let provider_w = 12u16;
        let status_w = width
            .saturating_sub(2 + name_w + email_w + provider_w as usize)
            .max(1);
        let widths = [
            Constraint::Length(name_w as u16),
            Constraint::Min(email_w as u16),
            Constraint::Length(provider_w),
            Constraint::Min(status_w as u16),
        ];

        let header = Row::new(vec![
            Cell::from("ACCOUNT"),
            Cell::from("EMAIL"),
            Cell::from("PROVIDER"),
            Cell::from("STATUS"),
        ])
        .style(skin.theme.dim_style());

        let rows: Vec<Row> = p
            .rows
            .iter()
            .map(|r| {
                let provider_badge = capitalize(&r.provider);
                let mut status_spans: Vec<Span<'static>> = if r.connected {
                    vec![pill(
                        &format!("{} connected", skin.glyphs.ok),
                        skin.theme.success,
                        skin.theme.bg,
                    )]
                } else {
                    vec![pill(
                        &format!("{} needs auth", skin.glyphs.warning),
                        skin.theme.warning,
                        skin.theme.bg,
                    )]
                };
                if r.is_default {
                    status_spans.push(Span::raw(" "));
                    status_spans.push(pill("default", skin.theme.accent, skin.theme.bg));
                }
                Row::new(vec![
                    Cell::from(r.name.clone()),
                    Cell::from(r.email.clone()),
                    Cell::from(Line::from(pill(&provider_badge, skin.theme.accent, skin.theme.bg))),
                    Cell::from(Line::from(status_spans)),
                ])
            })
            .collect();

        // Drive the row highlight + cursor arrow through the table's selection.
        let selected = p.rows.iter().position(|r| r.selected);
        let mut table_state = TableState::default();
        if let Some(i) = selected {
            table_state.select(Some(i));
        }

        let table = Table::new(rows, widths)
            .header(header)
            .column_spacing(1)
            .highlight_symbol("▸ ")
            .row_highlight_style(skin.theme.selected_style());
        frame.render_stateful_widget(table, list_area, &mut table_state);
    }

    if let Some(bar) = filter_bar {
        render_filter_bar(frame, bar, skin, &p.filter, p.filtering);
    }
}

#[cfg(test)]
mod tests {
    use super::account_cols;

    #[test]
    fn account_cols_expand_then_fall_back() {
        // Wide pane: name/email split the flexible space.
        let (name_w, email_w) = account_cols(100);
        assert!((14..=32).contains(&name_w));
        assert!(email_w >= 20);
        // Narrow pane: compact fixed widths.
        assert_eq!(account_cols(30), (14, 26));
    }
}
