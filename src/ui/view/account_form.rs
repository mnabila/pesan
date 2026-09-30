use super::account_manager::pill;
use super::*;

use crate::ui::widget;

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
pub(super) struct AccountFormProps {
    rows: Vec<FormRow>,
}

pub(super) fn account_form_props(app: &App) -> AccountFormProps {
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
    let field = |label: &str, input: &widget::TextInput, focus: SettingsFocus| {
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

pub(super) fn render_account_form(
    frame: &mut Frame,
    area: Rect,
    p: &AccountFormProps,
    skin: &Skin,
) {
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

    let grid_rows = boxes.len().div_ceil(2);
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
