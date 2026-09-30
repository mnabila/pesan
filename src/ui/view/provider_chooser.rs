use super::account_manager::{capitalize, pill};
use super::*;

//
// Provider chooser (new-account step 1)
//

/// One provider offered on the chooser screen.
struct ProviderChoice {
    name: String,
    oauth: bool,
    host: String,
    selected: bool,
}

/// Plain-data view model for the provider chooser.
pub(super) struct ProviderChooserProps {
    choices: Vec<ProviderChoice>,
}

pub(super) fn provider_chooser_props(app: &App, selected_idx: usize) -> ProviderChooserProps {
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
pub(super) fn render_provider_chooser(
    frame: &mut Frame,
    area: Rect,
    p: &ProviderChooserProps,
    skin: &Skin,
) {
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
