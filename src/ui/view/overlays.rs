use super::*;

use crate::ui::app;
use crate::ui::keymap;
use crate::ui::theme;
use crate::ui::widget::input;

pub(super) fn render_help(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Show help for wherever the cursor is: the focused pane in the main view,
    // or the current Compose/Settings screen.
    let active_ctx = active_help_ctx(app);

    // Build the section content first so the box can be sized to fit it exactly,
    // rather than filling the screen. Global has no second column.
    let mut left: Vec<Line> = Vec::new();
    if active_ctx == Ctx::Settings {
        // Account manager: the list keys and the edit-form keys are separate
        // keymap contexts. Show both, read straight from the active table so the
        // help always reflects the user's remaps.
        push_help_section(&mut left, app, Ctx::Settings, "account list");
        left.push(Line::raw(""));
        push_help_section(&mut left, app, Ctx::SettingsForm, "account form");
    } else {
        push_help_section(&mut left, app, active_ctx, help_ctx_name(active_ctx));
    }

    let mut right: Vec<Line> = Vec::new();
    if active_ctx == Ctx::Global {
        left.push(Line::raw(""));
        left.push(Line::from(Span::styled(
            " Esc/q closes ",
            app.theme.dim_style(),
        )));
    } else {
        push_help_section(&mut right, app, Ctx::Global, "global");
        right.push(Line::raw(""));
        right.push(Line::from(Span::styled(
            " Esc/q closes ",
            app.theme.dim_style(),
        )));
    }

    // Size the box to the taller column (+2 for borders), capped to the screen.
    let content_h = left.len().max(right.len()) as u16;
    let w = (100u16).min(area.width.saturating_sub(6));
    let h = (content_h + 2).min(area.height.saturating_sub(2));
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);

    let block = Block::bordered()
        .border_type(app.border_type())
        .title(Line::from(Span::styled(
            format!(" keymap - {} ", help_ctx_name(active_ctx)),
            app.theme.accent_style(),
        )))
        .border_style(app.theme.border_style(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if active_ctx == Ctx::Global {
        frame.render_widget(Paragraph::new(left).style(app.theme.fg_style()), inner);
        return;
    }

    let [lcol, rcol] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(inner);
    frame.render_widget(Paragraph::new(left).style(app.theme.fg_style()), lcol);
    frame.render_widget(Paragraph::new(right).style(app.theme.fg_style()), rcol);
}

/// Which keymap context the help should describe, based on the active view and
/// focused pane.
pub(super) fn active_help_ctx(app: &App) -> Ctx {
    match app.view {
        View::Compose => Ctx::Compose,
        View::Settings => Ctx::Settings,
        View::Reader => Ctx::Reader,
        View::Main => ctx_from_pane(app.active_pane()),
    }
}

pub(super) fn help_ctx_name(ctx: Ctx) -> &'static str {
    match ctx {
        Ctx::Global => "global",
        Ctx::Sidebar => "folders",
        Ctx::List => "list",
        Ctx::Reader => "reader",
        Ctx::Compose => "compose",
        Ctx::Settings => "settings",
        Ctx::SettingsForm => "account form",
        Ctx::Search => "search",
        Ctx::Confirm => "confirm",
    }
}

/// Append a titled block of `ctx`'s bindings to `lines`.
pub(super) fn push_help_section(lines: &mut Vec<Line<'static>>, app: &App, ctx: Ctx, title: &str) {
    lines.push(Line::from(Span::styled(
        format!(" {title} "),
        app.theme.accent_style().add_modifier(Modifier::BOLD),
    )));
    for row in app.keymap_table.iter().filter(|r| r.ctx == ctx) {
        let keys = row.seq.iter().map(key_label).collect::<Vec<_>>().join(" ");
        let line_text = format!("   {keys:<12} {}", row.desc);
        lines.push(Line::from(Span::styled(line_text, app.theme.fg_style())));
    }
}

pub(super) fn ctx_from_pane(pane: Pane) -> Ctx {
    match pane {
        Pane::Folders => Ctx::Sidebar,
        Pane::List => Ctx::List,
    }
}

pub(super) fn key_label(k: &keymap::Key) -> String {
    use crossterm::event::KeyCode as KC;
    match k.code {
        KC::Char(c) => {
            let mut s = c.to_string();
            if k.ctrl {
                s = format!("<C-{c}>");
            }
            if k.shift {
                s = s.to_uppercase();
            }
            if k.alt {
                s = format!("<M-{s}>");
            }
            s
        }
        KC::Esc => "Esc".to_string(),
        KC::Enter => "Enter".to_string(),
        KC::Tab => "Tab".to_string(),
        KC::Up => "up".to_string(),
        KC::Down => "down".to_string(),
        KC::Left => "left".to_string(),
        KC::Right => "right".to_string(),
        KC::Home => "Home".to_string(),
        KC::End => "End".to_string(),
        KC::PageUp => "PgUp".to_string(),
        KC::PageDown => "PgDn".to_string(),
        _ => "?".to_string(),
    }
}

pub(super) fn render_confirm(frame: &mut Frame, prompt: &str, app: &App) {
    let area = frame.area();
    let w = (60u16).min(area.width.saturating_sub(8));
    let h = 5;
    let x = area.x + (area.width - w) / 2;
    let y = area.y + area.height.saturating_sub(h).max(2) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(app.border_type())
        .title(Line::from(Span::styled(
            " confirm ",
            app.theme.accent_style(),
        )))
        .border_style(app.theme.border_style(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let lines = vec![
        Line::from(Span::styled(prompt, app.theme.fg_style())),
        Line::raw(""),
        Line::from(Span::styled(" y yes   n no ", app.theme.dim_style())),
    ];
    frame.render_widget(Paragraph::new(lines).style(app.theme.fg_style()), inner);
}

/// Modal for the manual (copy/paste) OAuth flow. While `oauth_paste` is set it
/// shows the consent URL and an input where the user pastes the redirect URL the
/// browser landed on; otherwise (during the code exchange) it shows a brief
/// "authorizing" note. Clears once the flow resolves or is cancelled.
pub(super) fn render_oauth_progress(frame: &mut Frame, app: &App) {
    match &app.oauth_paste {
        Some(paste) => render_oauth_paste(frame, app, paste),
        None => render_oauth_exchanging(frame, app),
    }
}

/// The redirect-paste step as a floating overlay: consent URL + a focused input
/// for the pasted URL. Used when the flow isn't shown inline in the Account
/// Manager (auto re-auth, startup consent).
fn render_oauth_paste(frame: &mut Frame, app: &App, paste: &app::OAuthPaste) {
    let provider = paste.req.account.provider.as_str();
    let area = frame.area();
    let w = (72u16).min(area.width.saturating_sub(6)).max(20);
    let h = 11;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + area.height.saturating_sub(h).max(2) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(app.border_type())
        .title(Line::from(Span::styled(
            format!(" authorize {provider} "),
            app.theme.accent_style(),
        )))
        .border_style(app.theme.border_style(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    render_oauth_paste_body(frame, inner, paste, &app.theme, app.border_type());
}

/// Render the redirect-paste instructions + input into `inner`. Shared by the
/// floating overlay and the Account Manager's inline split panel, so both stay
/// in step. The caller owns the surrounding block/title.
pub(super) fn render_oauth_paste_body(
    frame: &mut Frame,
    inner: Rect,
    paste: &app::OAuthPaste,
    theme: &theme::Theme,
    border_type: ratatui::widgets::BorderType,
) {
    if inner.height == 0 {
        return;
    }
    // Split: instructions (top) / a bordered box for the pasted URL (bottom).
    let box_area = Rect {
        x: inner.x,
        y: inner.y + inner.height.saturating_sub(3),
        width: inner.width,
        height: 3,
    };
    let text_area = Rect {
        height: inner.height.saturating_sub(4),
        ..inner
    };
    let lines = vec![
        Line::from(Span::styled(
            "1. Approve access in your browser (opened automatically).",
            theme.dim_style(),
        )),
        Line::from(Span::styled(
            "2. Copy the URL it redirects to, paste it below, Enter.",
            theme.dim_style(),
        )),
        Line::from(Span::styled(
            format!("If the browser didn't open: {}", paste.flow.authorize_url),
            theme.dim_style(),
        )),
        Line::raw(""),
        Line::from(Span::styled("Redirect URL:", theme.fg_style())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .style(theme.fg_style())
            .wrap(ratatui::widgets::Wrap { trim: true }),
        text_area,
    );
    let block = Block::bordered()
        .border_type(border_type)
        .border_style(theme.accent_style())
        .title(Span::styled("Redirect URL", theme.accent_style()));
    let input_area = block.inner(box_area);
    frame.render_widget(block, box_area);
    input::render_input(
        frame,
        input_area,
        &paste.input,
        theme,
        Some("paste here, or Esc to cancel"),
    );
}

/// The brief window while the pasted code is being exchanged for tokens.
fn render_oauth_exchanging(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let w = (48u16).min(area.width.saturating_sub(8)).max(16);
    let h = 4;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + area.height.saturating_sub(h).max(2) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(app.border_type())
        .title(Line::from(Span::styled(
            " authorizing ",
            app.theme.accent_style(),
        )))
        .border_style(app.theme.border_style(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Completing authorization...",
            app.theme.fg_style(),
        )))
        .style(app.theme.fg_style()),
        inner,
    );
}

/// The background job tracker window (toggled with `` ` ``). A headed table of
/// tracked background work - connects, folder-count sweeps, all-folders sync,
/// server deletes/moves - newest first, with a live spinner, per-job progress
/// bar, and elapsed time; finished jobs carry an outcome glyph (and any error).
pub(super) fn render_jobs(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let running = app.jobs.running_count();

    // Size the box: header + one row per job (min 1 for the empty state) + a
    // footer hint, plus borders, capped to the screen.
    let body_rows = app.jobs.iter().count().max(1) as u16;
    let w = (76u16).min(area.width.saturating_sub(6));
    let want_h = body_rows + 1 /*header*/ + 1 /*footer*/ + 2 /*borders*/;
    let h = want_h.min(area.height.saturating_sub(2));
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    frame.render_widget(Clear, rect);

    let title = if running > 0 {
        format!(" jobs ({running} running) ")
    } else {
        " jobs ".to_string()
    };
    let block = Block::bordered()
        .border_type(app.border_type())
        .title(Line::from(Span::styled(title, app.theme.accent_style())))
        .border_style(app.theme.border_style(true));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // Reserve the last inner row for the key hint, the rest for the table.
    let [table_area, footer_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    if app.jobs.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " No background jobs.",
                app.theme.dim_style(),
            )))
            .style(app.theme.fg_style()),
            table_area,
        );
    } else {
        let header = Row::new(vec![
            Cell::from(""), // status glyph column
            Cell::from("JOB"),
            Cell::from("ACCOUNT"),
            Cell::from("PROGRESS"),
            Cell::from(Line::from("ELAPSED ").alignment(Alignment::Right)),
        ])
        .style(app.theme.dim_style());

        let rows: Vec<Row> = app.jobs.iter().map(|job| job_row(job, app)).collect();
        let table = Table::new(rows, jobs_widths(inner.width))
            .header(header)
            .column_spacing(1)
            .block(Block::default());
        frame.render_widget(table, table_area);

        // Overlay a ratatui `Gauge` on the progress column of every job that
        // reports determinate progress. The column x-range is the same one the
        // `Table` laid out (same constraints + spacing), so the gauge lands
        // exactly over the blank progress cells above.
        let [_, _, _, progress_col, _] =
            Layout::horizontal(jobs_widths(table_area.width))
                .spacing(1)
                .areas(table_area);
        for (i, job) in app.jobs.iter().enumerate() {
            let Some((done, total)) = job.progress else {
                continue;
            };
            let y = table_area.y + 1 + i as u16; // header occupies row 0
            if y >= table_area.y + table_area.height {
                break;
            }
            let gauge_area = Rect {
                x: progress_col.x,
                y,
                width: progress_col.width,
                height: 1,
            };
            let ratio = if total == 0 {
                0.0
            } else {
                done as f64 / total as f64
            };
            let gauge = Gauge::default()
                .ratio(ratio)
                .label(format!("{done}/{total}"))
                .gauge_style(app.theme.accent_style());
            frame.render_widget(gauge, gauge_area);
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " `/Esc close   c clear finished ",
            app.theme.dim_style(),
        )))
        .style(app.theme.fg_style()),
        footer_area,
    );
}

/// Column widths for the job table: a 1-wide status glyph, a fixed job-name and
/// account column, a flexible progress column, and a right-aligned elapsed one.
fn jobs_widths(total: u16) -> [Constraint; 5] {
    const STATUS: u16 = 1;
    const JOB: u16 = 17;
    const ELAPSED: u16 = 8;
    let flex = total.saturating_sub(2); // room for borders
    let account = ((flex * 22) / 100).clamp(8, 18);
    let progress = flex
        .saturating_sub(STATUS + JOB + account + ELAPSED)
        .max(10);
    [
        Constraint::Length(STATUS),
        Constraint::Length(JOB),
        Constraint::Length(account),
        Constraint::Min(progress),
        Constraint::Length(ELAPSED),
    ]
}

/// Build one job-table row.
fn job_row(job: &app::Job, app: &App) -> Row<'static> {
    use crate::ui::app::JobState;

    let (glyph, glyph_style) = match job.state {
        JobState::Running => (app.spinner_glyph().to_string(), app.theme.accent_style()),
        JobState::Done => (app.glyphs.ok.to_string(), app.theme.success_style()),
        JobState::Failed => (app.glyphs.warning.to_string(), app.theme.error_style()),
    };

    // Progress column: a bar + count while a job reports progress, the error
    // message when it failed, else a short status word.
    let (progress_text, progress_style) = match job.state {
        JobState::Failed => (
            job.error.clone().unwrap_or_else(|| "failed".to_string()),
            app.theme.error_style(),
        ),
        _ => match job.progress {
            // A ratatui `Gauge` is overlaid on this column by `render_jobs`,
            // so leave the cell blank here to avoid drawing over the gauge.
            Some(_) => ("".to_string(), app.theme.fg_style()),
            None => (
                match job.state {
                    JobState::Done => "done".to_string(),
                    _ => "working...".to_string(),
                },
                app.theme.dim_style(),
            ),
        },
    };

    let secs = job.elapsed().as_secs_f32();
    let elapsed = if secs >= 100.0 {
        format!("{secs:.0}s ")
    } else {
        format!("{secs:.1}s ")
    };

    Row::new(vec![
        Cell::from(Span::styled(glyph, glyph_style)),
        Cell::from(Span::styled(job.kind.label(), app.theme.fg_style())),
        Cell::from(Span::styled(job.account.clone(), app.theme.dim_style())),
        Cell::from(Span::styled(progress_text, progress_style)),
        Cell::from(
            Line::from(Span::styled(elapsed, app.theme.dim_style())).alignment(Alignment::Right),
        ),
    ])
}

pub(super) fn render_search(frame: &mut Frame, app: &App) {
    let Some(search) = &app.search else { return };
    let area = frame.area();

    // Anchor the search bar to the bottom of the inbox (message list) pane,
    // spanning its inner width as a bordered box. If the list pane isn't
    // visible (narrow mode on folders/reader), fall back to a centered box
    // near the top.
    let rect = if let Some(list) = message_list_rect(area, app) {
        Rect {
            x: list.x + 1,
            y: list.y + list.height.saturating_sub(4),
            width: list.width.saturating_sub(2),
            height: 3,
        }
    } else {
        let w = (70u16).min(area.width.saturating_sub(8));
        Rect {
            x: area.x + (area.width - w) / 2,
            y: area.y + 1,
            width: w,
            height: 3,
        }
    };

    frame.render_widget(Clear, rect);
    let mut input = search.input.clone();
    input.focus(true);
    let block = Block::bordered()
        .border_type(app.border_type())
        .border_style(app.theme.accent_style())
        .title(Span::styled(" Search ", app.theme.accent_style()))
        .title_alignment(Alignment::Left);
    let input_area = block.inner(rect);
    frame.render_widget(block, rect);
    input::render_input(
        frame,
        input_area,
        &input,
        &app.theme,
        Some("filter list..."),
    );
}
