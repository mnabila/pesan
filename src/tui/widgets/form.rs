use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, layout::Rect};

use super::input::render_input;
use crate::tui::theme::Theme;
use crate::tui::widgets::TextInput;

/// One editable row of a form screen (compose header fields, account form).
pub struct FormField {
    pub label: String,
    /// Focus marker shown on the selected row ("▸"); pass `false` for screens
    /// that don't mark selection.
    pub selected: bool,
    pub input: TextInput,
    pub placeholder: String,
}

impl FormField {
    pub fn new(
        label: impl Into<String>,
        input: &TextInput,
        selected: bool,
        editing: bool,
        placeholder: impl Into<String>,
    ) -> Self {
        let mut inp = input.clone();
        inp.focus(editing);
        Self {
            label: label.into(),
            selected,
            input: inp,
            placeholder: placeholder.into(),
        }
    }
}

/// Draw one form row: marker column + label + input, split at `label_w`.
pub fn render_field_row(
    frame: &mut Frame,
    area: Rect,
    f: &FormField,
    theme: &Theme,
    label_w: u16,
    show_marker: bool,
) {
    let label_style = if f.selected {
        theme.accent_style()
    } else {
        theme.dim_style()
    };
    // With a marker column ("▸" on the selected row) the label sits two columns
    // in; without one, render the label flush-left so it lines up with sibling
    // rows that have no marker (e.g. the account form's Provider/Default rows).
    let label_line = if show_marker {
        let marker = if f.selected { "▸" } else { " " };
        Line::from(vec![
            Span::styled(marker, theme.accent_style()),
            Span::styled(
                format!(
                    " {:<width$}",
                    f.label,
                    width = (label_w as usize).saturating_sub(2)
                ),
                label_style,
            ),
        ])
    } else {
        Line::from(Span::styled(
            format!(
                "{:<width$}",
                f.label,
                width = (label_w as usize).saturating_sub(1)
            ),
            label_style,
        ))
    };
    frame.render_widget(
        Paragraph::new(label_line),
        Rect {
            x: area.x,
            y: area.y,
            width: label_w.min(area.width),
            height: 1,
        },
    );
    let input_area = Rect {
        x: area.x + label_w,
        y: area.y,
        width: area.width.saturating_sub(label_w),
        height: 1,
    };
    render_input(frame, input_area, &f.input, theme, Some(&f.placeholder));
}
