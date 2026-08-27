use super::*;

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
                Layout::vertical([Constraint::Min(5), Constraint::Length(9)]).areas(accounts_area);
            (top, Some((bottom, paste)))
        }
        _ => (accounts_area, None),
    };

    let a_title = if state.editing {
        if state.is_new {
            " New account "
        } else {
            " Edit account "
        }
    } else {
        " Accounts "
    };
    let acc_block = Block::bordered()
        .border_type(skin.border)
        .title(Line::from(Span::styled(a_title, skin.theme.dim_style())))
        .border_style(skin.border_style(true));
    let acc_inner = acc_block.inner(form_area);
    frame.render_widget(acc_block, form_area);
    if state.editing {
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
    render_oauth_paste_body(frame, inner, paste, skin.theme);
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
        let [list, bar] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        (list, Some(bar))
    } else {
        (area, None)
    };
    let width = list_area.width as usize;
    let (name_w, email_w) = account_cols(width);
    let provider_w = 12usize;
    let col = |s: &str, w: usize| format!("{:<w$}", truncate(s, w.saturating_sub(1)), w = w);

    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!(
            "  {}{}{}{}",
            col("ACCOUNT", name_w),
            col("EMAIL", email_w),
            col("PROVIDER", provider_w),
            "STATUS"
        ),
        skin.theme.dim_style(),
    ))];

    if p.accounts_empty {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "  No accounts yet - press  a  to add one.",
            skin.theme.dim_style(),
        )));
    } else if p.rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No accounts match the filter.",
            skin.theme.dim_style(),
        )));
    }

    for r in &p.rows {
        let status = if r.connected {
            format!("{} connected", skin.glyphs.ok)
        } else {
            format!("{} needs auth", skin.glyphs.warning)
        };
        let default = if r.is_default {
            format!("  {} default", skin.glyphs.flagged)
        } else {
            String::new()
        };
        let cursor = if r.selected { "▸ " } else { "  " };
        let text = format!(
            "{cursor}{}{}{}{status}{default}",
            col(&r.name, name_w),
            col(&r.email, email_w),
            col(&r.provider, provider_w),
        );
        if r.selected {
            lines.push(Line::from(Span::styled(
                format!("{text:<width$}"),
                skin.theme.selected_style(),
            )));
        } else {
            lines.push(Line::from(Span::styled(text, skin.theme.fg_style())));
        }
    }
    frame.render_widget(
        Paragraph::new(lines).style(skin.theme.fg_style()),
        list_area,
    );
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
    /// The provider selector: "‹ gmail ›" (ASCII brackets per `ui.ascii`).
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
        p.rows.push(FormRow::Provider {
            value: provider_name.clone(),
            focused: state.focus == SettingsFocus::Provider,
        });
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
        p.rows.push(FormRow::Provider {
            value: provider_name.clone(),
            focused: state.focus == SettingsFocus::Provider,
        });
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

fn render_account_form(frame: &mut Frame, area: Rect, p: &AccountFormProps, skin: &Skin) {
    if p.rows.is_empty() {
        return;
    }

    // One rect per row so live text inputs can draw over their own line
    // without the old index-bookkeeping overlay list.
    let rects = Layout::vertical(vec![Constraint::Length(1); p.rows.len()]).split(area);
    let field_label = |text: &str, focused: bool| -> Span<'static> {
        Span::styled(
            format!("{text:<9}"),
            if focused {
                skin.theme.accent_style()
            } else {
                skin.theme.dim_style()
            },
        )
    };

    for (i, row) in p.rows.iter().enumerate() {
        let r = rects[i];
        match row {
            FormRow::Blank => {}
            FormRow::Detail { label, value } => frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{:<9}", label), skin.theme.dim_style()),
                    Span::raw(" "),
                    Span::styled(value.clone(), skin.theme.fg_style()),
                ]))
                .style(skin.theme.fg_style()),
                r,
            ),
            FormRow::Provider { value, focused } => {
                let (popen, pclose) = if skin.ascii {
                    ("< ", " >")
                } else {
                    ("‹ ", " ›")
                };
                let style = if *focused {
                    skin.theme.fg_style().add_modifier(Modifier::BOLD)
                } else {
                    skin.theme.fg_style()
                };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        field_label("Provider", *focused),
                        Span::raw(" "),
                        Span::styled(format!("{popen}{value}{pclose}"), style),
                    ]))
                    .style(skin.theme.fg_style()),
                    r,
                );
            }
            FormRow::Field(f) => {
                form::render_field_row(frame, r, f, skin.theme, 10, false);
            }
            FormRow::EmailValue(value) => {
                let shown = match value {
                    Some(v) => Span::styled(v.clone(), skin.theme.fg_style()),
                    None => Span::styled("(filled after authorizing)", skin.theme.dim_style()),
                };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        field_label("Email", false),
                        Span::raw(" "),
                        shown,
                    ]))
                    .style(skin.theme.fg_style()),
                    r,
                );
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
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        field_label("Password", *focused),
                        Span::raw(" "),
                        shown,
                    ]))
                    .style(skin.theme.fg_style()),
                    r,
                );
            }
            FormRow::Toggle { on, focused } => {
                let mark = if *on { "yes" } else { "no" };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        field_label("Default", *focused),
                        Span::raw(" "),
                        Span::styled(mark, skin.theme.fg_style()),
                    ]))
                    .style(skin.theme.fg_style()),
                    r,
                );
            }
            FormRow::Button { text, focused } => {
                let style = if *focused {
                    skin.theme.selected_style()
                } else {
                    skin.theme.accent_style()
                };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![Span::styled(text.clone(), style)]))
                        .style(skin.theme.fg_style()),
                    r,
                );
            }
        }
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
