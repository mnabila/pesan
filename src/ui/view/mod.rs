// Presentation layer, split by screen. `draw` dispatches to the active view;
// each submodule owns the widgets for one area. Shared imports are re-exported
// here so the submodules can pull them in with a single `use super::*`.
// Reusable, domain-agnostic components live in `crate::ui::widget` instead.
mod account_form;
mod account_list;
mod account_manager;
mod compose;
mod list;
mod main_view;
mod overlays;
mod provider_chooser;
mod reader;
mod sidebar;

use crate::ui::widget::form;

pub(crate) use ratatui::Frame;
pub(crate) use ratatui::layout::{Alignment, Constraint, Layout, Rect};
pub(crate) use ratatui::style::Modifier;
pub(crate) use ratatui::text::{Line, Span};
pub(crate) use ratatui::widgets::{
    Block, Cell, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};

pub(crate) use crate::ui::fmt::{full_timestamp, relative_date, truncate};
pub(crate) use crate::ui::app::{App, ComposeFocus, ComposeMode};
pub(crate) use crate::ui::app::{Pane, SettingsFocus, ToastKind, View};
pub(crate) use crate::ui::keymap::Ctx;

use account_manager::draw_account_manager;
use compose::*;
use main_view::*;
use overlays::*;
use reader::draw_reader;

pub fn draw(frame: &mut Frame, app: &App) {
    // A single statusbar is the only chrome; it sits at the top or bottom per
    // `ui.statusbar_position`, and each view fills the remaining body area.
    let (bar, body) = main_view::chrome_layout(frame.area(), app);
    main_view::render_statusbar(frame, bar, app);
    match app.view {
        View::Main => draw_main(frame, body, app),
        View::Reader => draw_reader(frame, body, app),
        View::Compose => draw_compose(frame, body, app),
        View::Settings => draw_account_manager(frame, body, app),
    }
    // Overlays float above whichever view is active (e.g. the discard-draft
    // confirm over the compose screen, delete-account confirm over settings).
    // Notifications are not floated: they render inline in the status bar's
    // right slot (see `render_statusbar`).
    if let Some(confirm) = &app.confirm {
        render_confirm(frame, &confirm.prompt, app);
    }
    // The OAuth flow's progress modal. When the redirect-paste prompt is shown
    // inline under the Account Manager form (`oauth_paste_inline`), `draw_settings`
    // renders it as a split panel instead of floating it here.
    if app.oauth_in_progress && !app.oauth_paste_inline() {
        render_oauth_progress(frame, app);
    }
    if app.help_open {
        render_help(frame, app);
    }
    if app.jobs_open {
        render_jobs(frame, app);
    }
}
