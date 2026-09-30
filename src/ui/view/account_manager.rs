use super::account_form::{account_form_props, render_account_form};
use super::account_list::{account_list_props, render_account_list};
use super::provider_chooser::{provider_chooser_props, render_provider_chooser};
use super::*;
use ratatui::style::{Color, Style};

use crate::ui::app;

/// A filled chip: ` label ` with `fill` background over the theme bg text color.
/// Used for status pills and provider/default badges so the account UI reads at a
/// glance while staying themeable (no per-provider colors).
pub(super) fn pill(label: &str, fill: Color, text: Color) -> Span<'static> {
    Span::styled(format!(" {label} "), Style::new().bg(fill).fg(text))
}

/// Capitalize the first character of `s` for a tidy provider badge.
pub(super) fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub(super) fn draw_account_manager(frame: &mut Frame, body: Rect, app: &App) {
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
    paste: &app::OAuthPaste,
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
