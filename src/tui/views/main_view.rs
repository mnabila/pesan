use super::*;
use crate::tui::theme::{Glyphs, Theme};

/// Render-time lookups every component needs, resolved once per frame from
/// `App` so component draw functions stay pure over their props (no `&App`).
pub(crate) struct Skin<'a> {
    pub theme: &'a Theme,
    pub glyphs: Glyphs,
    pub border: ratatui::widgets::BorderType,
    pub ascii: bool,
}

impl<'a> Skin<'a> {
    pub fn of(app: &'a App) -> Self {
        Self {
            theme: &app.theme,
            glyphs: app.glyphs,
            border: app.border_type(),
            ascii: app.config.ui.ascii,
        }
    }

    pub(super) fn border_style(&self, focused: bool) -> ratatui::style::Style {
        self.theme.border_style(focused)
    }
}

pub(super) fn draw_main(frame: &mut Frame, body: Rect, app: &App) {
    let skin = Skin::of(app);
    render_body(frame, body, app, &skin);

    // Toast/confirm/help overlays are drawn globally by `draw`; the search bar
    // is anchored to the message list, so it stays here.
    if app.search.is_some() {
        render_search(frame, app);
    }
}

// Status bar -----------------------------------------------------------

/// Semantic text chunk for the status bar's left side; the draw step maps
/// tones to theme styles.
#[derive(Debug)]
pub(super) struct Chunk {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Accent,
    Fg,
    Dim,
}

impl Tone {
    fn style(self, s: &Skin) -> ratatui::style::Style {
        match self {
            Tone::Accent => s.theme.accent_style(),
            Tone::Fg => s.theme.fg_style(),
            Tone::Dim => s.theme.dim_style(),
        }
    }
}

/// The status bar's right-hand live slot, resolved by priority in `props`
/// (notification > sync bar > connect spinner > idle).
#[derive(Debug)]
pub(super) enum Slot {
    None,
    /// A transient notification, colored by kind; `glyph` is already chosen.
    Toast {
        text: String,
        kind: ToastKind,
        glyph: &'static str,
    },
    Sync {
        done: usize,
        total: usize,
    },
    /// Indeterminate work; `spinner` is the frame glyph to show.
    Busy {
        spinner: String,
        text: String,
    },
}

/// Plain-data view model for the single status bar shared by every view.
#[derive(Debug)]
pub(super) struct StatusProps {
    /// Identity + per-view context chunks (the ident chunk is first).
    context: Vec<Chunk>,
    jobs: Option<usize>,
    slot: Slot,
}

/// Resolve the status bar's contents from app state. Pure read; no styling.
pub(super) fn status_props(app: &App) -> StatusProps {
    // Left: `<email>` identity + per-view context/state.
    let email = app.active_account_email();
    let ident = if email.is_empty() {
        app.active_account_name().to_string()
    } else {
        format!("<{email}>")
    };
    let mut context = vec![Chunk {
        text: format!(" {ident}"),
        tone: Tone::Accent,
    }];
    match app.view {
        View::Main | View::Reader => {
            let selected = if app.display_envelopes.is_empty() {
                0
            } else {
                app.selected_message + 1
            };
            let total = app.display_envelopes.len();
            context.push(Chunk {
                text: format!(" {}", app.selected_folder_name()),
                tone: Tone::Fg,
            });
            context.push(Chunk {
                text: format!("  {selected}/{total}"),
                tone: Tone::Dim,
            });
            // Pagination hint: how many older messages remain on the server, or
            // that a page is loading. Only for the unfiltered list (a search
            // narrows the view, so the server total no longer lines up).
            let loaded = app.envelopes.len();
            let server_total = app
                .folders
                .get(app.selected_folder)
                .map(|f| f.total)
                .unwrap_or(0);
            if app.loading_older {
                context.push(Chunk {
                    text: "  loading older...".into(),
                    tone: Tone::Dim,
                });
            } else if app.search.is_none() && !app.older_exhausted && server_total > loaded {
                context.push(Chunk {
                    text: format!("  +{} older", server_total - loaded),
                    tone: Tone::Dim,
                });
            }
            if !app.marked.is_empty() {
                context.push(Chunk {
                    text: format!("  {} marked", app.marked.len()),
                    tone: Tone::Accent,
                });
            }
        }
        View::Compose => {
            let mode = match app.compose.as_ref().map(|c| c.mode) {
                Some(ComposeMode::Reply) => "Reply",
                Some(ComposeMode::Forward) => "Forward",
                _ => "Compose",
            };
            context.push(Chunk {
                text: format!(" {mode}"),
                tone: Tone::Fg,
            });
        }
        View::Settings => {
            context.push(Chunk {
                text: " Account Manager".into(),
                tone: Tone::Fg,
            });
        }
    }

    // A subtle hint that background work is running and the tracker (`` ` ``) has
    // detail; kept in the left group so it never fights the right status slot.
    let running_jobs = app.jobs.running_count();
    let jobs = (running_jobs > 0).then_some(running_jobs);

    // Right: one live status slot (notification > sync bar > connect spinner).
    let slot = if let Some(toast) = &app.toast {
        let glyph = match toast.kind {
            ToastKind::Success => app.glyphs.ok,
            ToastKind::Warning | ToastKind::Error => app.glyphs.warning,
            ToastKind::Info => "",
        };
        Slot::Toast {
            text: toast.text.clone(),
            kind: toast.kind,
            glyph,
        }
    } else if let Some(sync) = &app.sync {
        Slot::Sync {
            done: sync.done,
            total: sync.total,
        }
    } else if let Some(busy) = app.busy() {
        Slot::Busy {
            spinner: app.spinner_glyph().to_string(),
            text: busy.to_string(),
        }
    } else {
        Slot::None
    };

    StatusProps {
        context,
        jobs,
        slot,
    }
}

/// Split the frame into the single statusbar row and the body the active view
/// fills. The bar sits at the top or bottom per `ui.statusbar_position`.
pub(super) fn chrome_layout(area: Rect, app: &App) -> (Rect, Rect) {
    if app.config.ui.statusbar_position.eq_ignore_ascii_case("top") {
        let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        (bar, body)
    } else {
        let [body, bar] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
        (bar, body)
    }
}

/// Draw the status bar from its resolved [`StatusProps`]: left identity +
/// context chunks, a jobs hint, and the right-hand live slot.
pub(super) fn render_statusbar(frame: &mut Frame, area: Rect, app: &App) {
    let skin = Skin::of(app);
    let p = status_props(app);

    let mut left: Vec<Span> = p
        .context
        .into_iter()
        .map(|c| Span::styled(c.text, c.tone.style(&skin)))
        .collect();
    if let Some(jobs) = p.jobs {
        left.push(Span::styled(
            format!("  {jobs} jobs"),
            skin.theme.dim_style(),
        ));
    }

    // Right: one live status slot (notification > sync bar > connect spinner).
    let right: Vec<Span> = match p.slot {
        Slot::None => Vec::new(),
        Slot::Toast { text, kind, glyph } => {
            let style = match kind {
                ToastKind::Info => skin.theme.fg_style(),
                ToastKind::Success => skin.theme.success_style(),
                ToastKind::Warning => skin.theme.warning_style(),
                ToastKind::Error => skin.theme.error_style(),
            };
            let text = if glyph.is_empty() {
                format!("{text} ")
            } else {
                format!("{glyph} {text} ")
            };
            vec![Span::styled(text, style)]
        }
        Slot::Sync { done, total } => {
            let bar = progress_bar(done, total, 10, skin.ascii);
            vec![Span::styled(
                format!("Syncing {done}/{total} {bar} "),
                skin.theme.dim_style(),
            )]
        }
        Slot::Busy { spinner, text } => vec![Span::styled(
            format!("{spinner} {text} "),
            skin.theme.accent_style(),
        )],
    };

    let right_w: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let left_w: usize = left.iter().map(|s| s.content.chars().count()).sum();
    let pad = (area.width as usize).saturating_sub(left_w + right_w);
    left.push(Span::raw(" ".repeat(pad)));
    left.extend(right);
    frame.render_widget(
        Paragraph::new(Line::from(left)).style(skin.theme.fg_style()),
        area,
    );
}

/// A fixed-`width` determinate progress bar, e.g. `[######----]`. Uses block
/// glyphs normally, ASCII (`#`/`-`) when `ui.ascii` is set. Pure formatting.
pub(super) fn progress_bar(done: usize, total: usize, width: usize, ascii: bool) -> String {
    let (full, empty) = if ascii { ('#', '-') } else { ('█', '░') };
    let filled = ((done * width) + total / 2)
        .checked_div(total)
        .unwrap_or(0)
        .min(width);
    let mut bar = String::with_capacity(width + 2);
    bar.push('[');
    for i in 0..width {
        bar.push(if i < filled { full } else { empty });
    }
    bar.push(']');
    bar
}

pub(super) fn render_body(frame: &mut Frame, area: Rect, app: &App, skin: &Skin) {
    if app.narrow {
        render_narrow_body(frame, area, app, skin);
        return;
    }

    let [sidebar, list] = pane_layout(area, app, app.sidebar_collapsed);
    sidebar::render(frame, sidebar, &sidebar::props(app), skin);
    list::render(frame, list, &list::props(app), skin);
}

/// Split the body into the sidebar + message list. The sidebar width comes from
/// the first `ui.layout` slot; the list fills the rest. Messages open in a
/// separate full-screen reader view, so there is no reader pane here.
pub(super) fn pane_layout(area: Rect, app: &App, collapsed: bool) -> [Rect; 2] {
    let sidebar_c = if collapsed {
        Constraint::Length(1)
    } else {
        sidebar_ratio(app.config.ui.layout)
    };
    Layout::horizontal([sidebar_c, Constraint::Min(0)]).areas(area)
}

/// Turn the sidebar slot of the 10-grid `layout` into a width `Constraint`. The
/// list/reader slots are legacy and ignored; the list takes the remaining width.
pub(super) fn sidebar_ratio(layout: [u8; 3]) -> Constraint {
    const GRID: u32 = 10;
    let s = (layout[0] as u32).clamp(1, GRID - 1);
    Constraint::Ratio(s, GRID)
}

pub(super) fn render_narrow_body(frame: &mut Frame, area: Rect, app: &App, skin: &Skin) {
    let s = sidebar_ratio(app.config.ui.layout);
    match app.narrow_pane {
        Pane::Folders => {
            let [sidebar, rest] = Layout::horizontal([s, Constraint::Min(0)]).areas(area);
            sidebar::render(frame, sidebar, &sidebar::props(app), skin);
            render_empty_pane(frame, rest, skin);
        }
        Pane::List => {
            let [sidebar, list] = Layout::horizontal([s, Constraint::Min(0)]).areas(area);
            sidebar::render(frame, sidebar, &sidebar::props(app), skin);
            list::render(frame, list, &list::props(app), skin);
        }
    }
}

pub(super) fn render_empty_pane(frame: &mut Frame, area: Rect, skin: &Skin) {
    let block = Block::bordered()
        .border_type(skin.border)
        .border_style(skin.border_style(false));
    frame.render_widget(block, area);
}

/// Recompute the message-list ("inbox") pane rect from the full frame area,
/// mirroring `render_body`'s layout. Returns `None` when the list pane isn't
/// visible (narrow mode showing folders or the reader). Used to anchor the
/// search bar to the bottom of the inbox.
pub(super) fn message_list_rect(frame_area: Rect, app: &App) -> Option<Rect> {
    let (_bar, body) = chrome_layout(frame_area, app);

    if app.narrow {
        let s = sidebar_ratio(app.config.ui.layout);
        return match app.narrow_pane {
            Pane::List => {
                let [_sidebar, list] = Layout::horizontal([s, Constraint::Min(0)]).areas(body);
                Some(list)
            }
            _ => None,
        };
    }

    let [_sidebar, list] = pane_layout(body, app, app.sidebar_collapsed);
    Some(list)
}

/// Render a one-line filter bar on the bottom `bar` row of a pane. Every
/// filterable section shares this look (matching the message-list search bar):
/// an accent " filter " label, the live query, and a caret while actively
/// typing (or a hint when the query is still empty).
pub(super) fn render_filter_bar(
    frame: &mut Frame,
    bar: Rect,
    skin: &Skin,
    query: &str,
    active: bool,
) {
    let caret = if active { "_" } else { "" };
    let text = if query.is_empty() && active {
        "type to filter...".to_string()
    } else {
        format!("{query}{caret}")
    };
    let q_style = if active {
        skin.theme.fg_style()
    } else {
        skin.theme.dim_style()
    };
    let line = Line::from(vec![
        Span::styled(" filter ", skin.theme.accent_style()),
        Span::styled(text, q_style),
    ]);
    frame.render_widget(Paragraph::new(line), bar);
}

#[cfg(test)]
mod status_tests {
    use super::progress_bar;

    #[test]
    fn progress_bar_rounds_and_clamps() {
        assert_eq!(progress_bar(0, 10, 10, true), "[----------]");
        assert_eq!(progress_bar(5, 10, 10, true), "[#####-----]");
        // Rounding to nearest cell.
        assert_eq!(progress_bar(1, 3, 10, true), "[###-------]");
        // done > total clamps to full.
        assert_eq!(progress_bar(20, 10, 10, true), "[##########]");
        // Empty progress (total == 0) renders empty.
        assert_eq!(progress_bar(0, 0, 4, true), "[----]");
        assert_eq!(progress_bar(2, 2, 4, false), "[████]");
    }
}
