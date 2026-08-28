use super::*;
use ratatui::style::{Color, Style};

/// A filled chip: ` label ` with `fill` background over the theme bg text color.
/// Used for status pills and provider/default badges so the account UI reads at a
/// glance while staying themeable (no per-provider colors).
fn pill(label: &str, fill: Color, text: Color) -> Span<'static> {
    Span::styled(format!(" {label} "), Style::new().bg(fill).fg(text))
}

/// Capitalize the first character of `s` for a tidy provider badge.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub(super) fn draw_settings(frame: &mut Frame, body: Rect, app: &App) {
    let Some(state) = &app.settings else { return };
    let skin = Skin::of(app);
    let block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(
            " Account Manager ",
            skin.theme.accent_style().add_modifier(Modifier::BOLD),
        )))
        .border_style(skin.border_style(true));
    let accounts_area = block.inner(body);
    frame.render_widget(block, body);

    // While authorizing from the form, split off a lower panel for the OAuth
    // redirect-paste prompt so it sits *under* the "New account" box instead of
    // floating over the whole screen.
    let (form_area, paste_split) = match app.oauth_paste.as_ref() {
        Some(paste) if app.oauth_paste_inline() => {
            let [top, bottom] =
                Layout::vertical([Constraint::Min(5), Constraint::Length(11)]).areas(accounts_area);
            (top, Some((bottom, paste)))
        }
        _ => (accounts_area, None),
    };

    let a_title: String = if state.choosing_provider {
        if state.is_new {
            " New account - choose provider ".into()
        } else {
            " Edit account - choose provider ".into()
        }
    } else if state.editing {
        if state.is_new {
            // Once a provider is chosen, name it in the title (e.g.
            // "New Account - Gmail") so the form's context is clear.
            let provider = state
                .form
                .as_ref()
                .and_then(|f| state.providers.get(f.provider_idx))
                .map(|p| capitalize(p))
                .unwrap_or_else(|| "Provider".to_string());
            format!(" New Account - {provider} ")
        } else {
            " Edit account ".into()
        }
    } else {
        " Accounts ".into()
    };

    let acc_block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(a_title, skin.theme.dim_style())))
        .border_style(skin.border_style(true));

    let acc_inner = acc_block.inner(form_area);

    frame.render_widget(acc_block, form_area);

    if state.choosing_provider {
        render_provider_chooser(
            frame,
            acc_inner,
            &provider_chooser_props(app, state.choose_idx),
            &skin,
        );
    } else if state.editing {
        render_account_form(frame, acc_inner, &account_form_props(app), &skin);
    } else {
        render_account_list(frame, acc_inner, &account_list_props(app), &skin);
    }

    if let Some((area, paste)) = paste_split {
        render_oauth_paste_split(frame, area, paste, &skin);
    }
}

/// The OAuth redirect-paste prompt as a bordered split panel under the account
/// form (rather than a floating overlay), so authorizing a new account stays in
/// place. Shares the body renderer with the floating overlay.
fn render_oauth_paste_split(
    frame: &mut Frame,
    area: Rect,
    paste: &crate::tui::app::OAuthPaste,
    skin: &Skin,
) {
    let provider = paste.req.account.provider.as_str();
    let block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(
            format!(" Authorize {provider} "),
            skin.theme.accent_style(),
        )))
        .border_style(skin.border_style(true));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_oauth_paste_body(frame, inner, paste, skin.theme, skin.border);
}

// Provider chooser (new-account step 1) --------------------------------

/// One provider offered on the chooser screen.
struct ProviderChoice {
    name: String,
    oauth: bool,
    host: String,
    selected: bool,
}

/// Plain-data view model for the provider chooser.
struct ProviderChooserProps {
    choices: Vec<ProviderChoice>,
}

fn provider_chooser_props(app: &App, selected_idx: usize) -> ProviderChooserProps {
    let Some(state) = &app.settings else {
        return ProviderChooserProps {
            choices: Vec::new(),
        };
    };
    let choices = state
        .providers
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let p = app.config.providers.get(key);
            let oauth = p.map(|p| p.is_oauth()).unwrap_or(true);
            let host = p.map(|p| p.imap.host.clone()).unwrap_or_default();
            ProviderChoice {
                name: capitalize(key),
                oauth,
                host,
                selected: i == selected_idx,
            }
        })
        .collect();
    ProviderChooserProps { choices }
}

/// Draw the provider chooser as a vertical stack of selectable cards, each
/// showing the provider name, an auth-kind badge (OAuth2 / Password), and the
/// IMAP host as a hint. The selected card gets the accent border + highlight.
fn render_provider_chooser(frame: &mut Frame, area: Rect, p: &ProviderChooserProps, skin: &Skin) {
    let mut y = area.y;
    for c in &p.choices {
        let h = 3u16;
        if y + h > area.y + area.height {
            break;
        }
        let rect = Rect {
            x: area.x,
            y,
            width: area.width,
            height: h,
        };
        let bstyle = if c.selected {
            skin.theme.accent_style()
        } else {
            skin.theme.dim_style()
        };
        let block = Block::bordered()
            .border_type(skin.border)
            .border_style(bstyle);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let auth_label = if c.oauth {
            format!("{} OAuth2", skin.glyphs.ok)
        } else {
            format!("{} Password", skin.glyphs.flagged)
        };
        let auth_fill = if c.oauth {
            skin.theme.accent
        } else {
            skin.theme.warning
        };
        let name_style = if c.selected {
            skin.theme.fg_style().add_modifier(Modifier::BOLD)
        } else {
            skin.theme.fg_style()
        };
        let line1 = Line::from(vec![
            Span::styled(format!(" {:<14}", c.name), name_style),
            pill(&auth_label, auth_fill, skin.theme.bg),
        ]);
        let hint_w = (inner.width as usize).saturating_sub(2);
        let line2 = Line::from(Span::styled(
            format!(" {:<hint_w$}", c.host, hint_w = hint_w),
            skin.theme.dim_style(),
        ));
        let [l1, l2, _] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);
        frame.render_widget(Paragraph::new(line1), l1);
        frame.render_widget(Paragraph::new(line2), l2);
        y += h;
    }
    // Footer hint once the cards are drawn.
    if y < area.y + area.height {
        let hint = Line::from(Span::styled(
            " j/k move   Enter select   Esc cancel ",
            skin.theme.dim_style(),
        ));
        frame.render_widget(
            Paragraph::new(hint),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
        );
    }
}

// Account list ---------------------------------------------------------

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
struct AccountListProps {
    /// True when no accounts exist at all (shows the "press a" hint).
    accounts_empty: bool,
    filtering: bool,
    filter: String,
    rows: Vec<AccountRow>,
}

fn account_list_props(app: &App) -> AccountListProps {
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

fn render_account_list(frame: &mut Frame, area: Rect, p: &AccountListProps, skin: &Skin) {
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

// Account form ---------------------------------------------------------

/// One content row of the account form, resolved to plain data by
/// [`account_form_props`]; the draw step applies styles in order.
enum FormRow {
    /// Read-only detail line (Status/Auth/Created for an existing account).
    Detail {
        label: String,
        value: String,
    },
    Blank,
    /// The provider selector. Shown only when editing an existing account (the
    /// new-account wizard already chose the provider on the chooser screen and
    /// names it in the title). Enter/`o` re-opens the chooser to switch it.
    Provider {
        value: String,
        focused: bool,
    },
    /// An editable text row rendered by the shared form component.
    Field(form::FormField),
    /// OAuth email is filled after authorizing; `None` shows the hint.
    EmailValue(Option<String>),
    /// Masked password row; `chars` counts dots (never the actual password).
    Password {
        chars: usize,
        focused: bool,
        editing: bool,
    },
    Toggle {
        on: bool,
        focused: bool,
    },
    Button {
        text: String,
        focused: bool,
    },
}

/// Plain-data view model for the account form.
#[derive(Default)]
struct AccountFormProps {
    rows: Vec<FormRow>,
}

fn account_form_props(app: &App) -> AccountFormProps {
    let Some(state) = &app.settings else {
        return AccountFormProps::default();
    };
    let Some(form) = &state.form else {
        return AccountFormProps::default();
    };
    let is_oauth = app.form_provider_is_oauth();
    let provider_name = state
        .providers
        .get(form.provider_idx)
        .cloned()
        .unwrap_or_else(|| "gmail".to_string());
    // In the new-account wizard the provider was already chosen on the chooser
    // screen, so the form shows it as a read-only badge instead of a dropdown.
    let new_wizard = state.is_new && !state.choosing_provider;
    let mut p = AccountFormProps::default();

    // Read-only detail block for an existing account (the `o`-opened view).
    if !state.is_new
        && let Some(acct) = state.accounts.get(state.selected)
    {
        let connected = app.account_connected(state.selected);
        p.rows.push(FormRow::Detail {
            label: "Status".into(),
            value: if connected { "connected" } else { "needs auth" }.into(),
        });
        let auth_kind = if is_oauth { "OAuth2" } else { "Password" };
        p.rows.push(FormRow::Detail {
            label: "Auth".into(),
            value: auth_kind.into(),
        });
        p.rows.push(FormRow::Detail {
            label: "Created".into(),
            value: full_timestamp(acct.created_at),
        });
        p.rows.push(FormRow::Blank);
    }

    // Editable fields (order depends on the provider's auth kind).
    let field = |label: &str, input: &crate::tui::widgets::TextInput, focus: SettingsFocus| {
        form::FormField::new(
            label,
            input,
            state.focus == focus,
            state.field_editing && state.focus == focus,
            "",
        )
    };

    if is_oauth {
        // Name is optional for OAuth: blank auto-fills from the provider's real
        // display name after authorizing. The placeholder makes that clear.
        p.rows.push(FormRow::Field(form::FormField::new(
            "Name",
            &form.name,
            state.focus == SettingsFocus::Name,
            state.field_editing && state.focus == SettingsFocus::Name,
            "(defaults to your account name)",
        )));
        // Email is filled by OAuth; show it read-only (a hint when still blank).
        p.rows.push(FormRow::EmailValue(
            (!form.email.text().is_empty()).then(|| form.email.text().to_string()),
        ));
        // The provider is named in the form title (chosen on the chooser), so it
        // is not repeated as a field here; it stays editable only when editing
        // an existing account.
        if !new_wizard {
            p.rows.push(FormRow::Provider {
                value: provider_name.clone(),
                focused: state.focus == SettingsFocus::Provider,
            });
        }
    } else {
        p.rows.push(FormRow::Field(field(
            "Name",
            &form.name,
            SettingsFocus::Name,
        )));
        p.rows.push(FormRow::Field(field(
            "Email",
            &form.email,
            SettingsFocus::Email,
        )));
        // The provider is named in the form title (chosen on the chooser), so it
        // is not repeated as a field here; it stays editable only when editing
        // an existing account.
        if !new_wizard {
            p.rows.push(FormRow::Provider {
                value: provider_name.clone(),
                focused: state.focus == SettingsFocus::Provider,
            });
        }
        // Password is masked (never rendered in clear text).
        p.rows.push(FormRow::Password {
            chars: form.password.text().chars().count(),
            focused: state.focus == SettingsFocus::Password,
            editing: state.field_editing,
        });
    }

    // Default toggle.
    p.rows.push(FormRow::Toggle {
        on: form.is_default,
        focused: state.focus == SettingsFocus::IsDefault,
    });
    p.rows.push(FormRow::Blank);

    // Action button: Authorize (OAuth) or Save (password).
    let (btn_focus, btn_text) = if is_oauth {
        (
            SettingsFocus::Authorize,
            format!(" [ Authorize with {provider_name} ] "),
        )
    } else {
        (SettingsFocus::Save, " [ Save account ] ".to_string())
    };
    p.rows.push(FormRow::Button {
        text: btn_text,
        focused: state.focus == btn_focus,
    });
    p
}

/// Draw a single-line value inside a bordered box titled `title` (pass `None`
/// for an untitled box, e.g. a button). The border highlights in the accent
/// color when `focused`. The caller renders the inner content via `draw`, so
/// the provider/password/toggle/button rows match the boxed text-field style.
fn render_boxed(
    frame: &mut Frame,
    area: Rect,
    title: Option<&str>,
    focused: bool,
    skin: &Skin,
    draw: impl FnOnce(&mut Frame, Rect),
) {
    let bstyle = if focused {
        skin.theme.accent_style()
    } else {
        skin.theme.dim_style()
    };
    let mut block = Block::bordered()
        .border_type(skin.border)
        .border_style(bstyle);
    if let Some(t) = title {
        block = block.title(Span::styled(t.to_string(), bstyle));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    draw(frame, inner);
}

/// Left-aligned, dim/accent field label shared by the read-only header rows
/// (mirrors the inline "From" label on the compose screen).
fn field_label(text: &str, focused: bool, skin: &Skin) -> Span<'static> {
    Span::styled(
        format!("{text:<9}"),
        if focused {
            skin.theme.accent_style()
        } else {
            skin.theme.dim_style()
        },
    )
}

/// Draw a read-only (non-boxed) header row: Details and the OAuth email hint.
fn render_header_row(frame: &mut Frame, r: Rect, row: &FormRow, skin: &Skin) {
    match row {
        FormRow::Blank => {}
        FormRow::Detail { label, value } => {
            let value_span = if label == "Status" {
                if value == "connected" {
                    pill(
                        &format!("{} connected", skin.glyphs.ok),
                        skin.theme.success,
                        skin.theme.bg,
                    )
                } else {
                    pill(
                        &format!("{} needs auth", skin.glyphs.warning),
                        skin.theme.warning,
                        skin.theme.bg,
                    )
                }
            } else {
                Span::styled(value.clone(), skin.theme.fg_style())
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{:<9}", label), skin.theme.dim_style()),
                    Span::raw(" "),
                    value_span,
                ]))
                .style(skin.theme.fg_style()),
                r,
            );
        }
        FormRow::EmailValue(value) => {
            let shown = match value {
                Some(v) => Span::styled(v.clone(), skin.theme.fg_style()),
                None => Span::styled("(filled after authorizing)", skin.theme.dim_style()),
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    field_label("Email", false, skin),
                    Span::raw(" "),
                    shown,
                ]))
                .style(skin.theme.fg_style()),
                r,
            );
        }
        _ => {}
    }
}

/// Draw one bordered control box (text field, provider, password, toggle, or
/// action button) into its 3-row cell.
fn render_box_row(frame: &mut Frame, r: Rect, row: &FormRow, skin: &Skin) {
    match row {
        FormRow::Field(f) => {
            form::render_field_box(frame, r, f, skin.theme, skin.border);
        }
        FormRow::Provider { value, focused } => {
            let style = if *focused {
                skin.theme.fg_style().add_modifier(Modifier::BOLD)
            } else {
                skin.theme.fg_style()
            };
            // Provider shown as an accent badge; Enter/`o` re-opens the chooser
            // to switch it, so hint at that with a small affordance.
            let badge = pill(value, skin.theme.accent, skin.theme.bg);
            let hint = if *focused {
                Span::styled(" enter↩", skin.theme.dim_style())
            } else {
                Span::raw("")
            };
            let val = Line::from(vec![badge, hint]);
            render_boxed(frame, r, Some("Provider"), *focused, skin, |f, area| {
                f.render_widget(Paragraph::new(val).style(style), area);
            });
        }
        FormRow::Password {
            chars,
            focused,
            editing,
        } => {
            // Masked (never rendered in clear text).
            let dots = "•".repeat(*chars);
            let shown = if dots.is_empty() {
                Span::styled("(enter password)", skin.theme.dim_style())
            } else if *focused && *editing {
                Span::styled(format!("{dots}▏"), skin.theme.fg_style())
            } else {
                Span::styled(dots, skin.theme.fg_style())
            };
            render_boxed(frame, r, Some("Password"), *focused, skin, |f, area| {
                f.render_widget(
                    Paragraph::new(Line::from(shown)).style(skin.theme.fg_style()),
                    area,
                );
            });
        }
        FormRow::Toggle { on, focused } => {
            let mark = if *on { "yes" } else { "no" };
            let fill = if *on {
                skin.theme.success
            } else {
                skin.theme.dim
            };
            render_boxed(frame, r, Some("Default"), *focused, skin, |f, area| {
                f.render_widget(
                    Paragraph::new(Line::from(pill(mark, fill, skin.theme.bg)))
                        .style(skin.theme.fg_style()),
                    area,
                );
            });
        }
        FormRow::Button { text, focused } => {
            let style = if *focused {
                skin.theme.selected_style()
            } else {
                skin.theme.accent_style()
            };
            // A plain, centered label (the `[ … ]` brackets already mark it as
            // the action) - no surrounding border box.
            let [_, btn_area, _] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(1), Constraint::Min(0)])
                    .areas(r);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(text.clone(), style)))
                    .style(skin.theme.fg_style())
                    .alignment(ratatui::layout::Alignment::Center),
                btn_area,
            );
        }
        _ => {}
    }
}

fn render_account_form(frame: &mut Frame, area: Rect, p: &AccountFormProps, skin: &Skin) {
    if p.rows.is_empty() {
        return;
    }

    // Group rows into a read-only header (Detail/Email lines, like compose's
    // "From" line), a 2-column grid of bordered boxes (matching compose), and a
    // full-width trailing action button.
    let mut header: Vec<&FormRow> = Vec::new();
    let mut boxes: Vec<&FormRow> = Vec::new();
    let mut button: Option<&FormRow> = None;
    for row in &p.rows {
        match row {
            FormRow::Field(_)
            | FormRow::Provider { .. }
            | FormRow::Password { .. }
            | FormRow::Toggle { .. } => boxes.push(row),
            FormRow::Button { .. } => button = Some(row),
            _ => header.push(row),
        }
    }

    let grid_rows = (boxes.len() + 1) / 2;
    let mut constraints: Vec<Constraint> = header.iter().map(|_| Constraint::Length(1)).collect();
    if grid_rows > 0 {
        constraints.push(Constraint::Length((3 * grid_rows) as u16));
    }
    if button.is_some() {
        constraints.push(Constraint::Length(3));
    }
    let rects = Layout::vertical(&constraints).split(area);

    let mut idx = 0;
    for row in &header {
        render_header_row(frame, rects[idx], row, skin);
        idx += 1;
    }
    let grid_area = if grid_rows > 0 {
        let a = rects[idx];
        idx += 1;
        Some(a)
    } else {
        None
    };
    let button_area = button.map(|_| {
        let a = rects[idx];
        idx += 1;
        a
    });

        // Lay the boxed controls out as a 2-column grid, like compose's header rows.
        if let Some(grid_area) = grid_area {
            let [left, _gap, right] = Layout::horizontal([
                Constraint::Ratio(1, 2),
                Constraint::Length(1),
                Constraint::Ratio(1, 2),
            ])
            .areas(grid_area);
            let left_rects = Layout::vertical(vec![Constraint::Length(3); grid_rows]).split(left);
            let right_rects = Layout::vertical(vec![Constraint::Length(3); grid_rows]).split(right);
            for (i, row) in boxes.iter().enumerate() {
                let r = if i % 2 == 0 {
                    left_rects[i / 2]
                } else {
                    right_rects[i / 2]
                };
                render_box_row(frame, r, row, skin);
            }
        }

    if let (Some(area), Some(row)) = (button_area, button) {
        render_box_row(frame, area, row, skin);
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
