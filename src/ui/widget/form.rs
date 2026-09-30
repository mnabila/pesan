use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType};
use ratatui::{Frame, layout::Rect};

use super::input::render_input;
use crate::ui::theme::Theme;
use crate::ui::widget::TextInput;

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

/// Draw one form field as a bordered box: the label sits inside the top border
/// (a titled block) and the text input is drawn on the single inner line. The
/// border highlights in the accent color when the field is focused (`selected`)
/// and is dimmed otherwise. The caller must supply an `area` at least 3 rows
/// tall (top border + inner line + bottom border).
pub fn render_field_box(
    frame: &mut Frame,
    area: Rect,
    f: &FormField,
    theme: &Theme,
    border_type: BorderType,
) {
    let bstyle = if f.selected {
        theme.accent_style()
    } else {
        theme.dim_style()
    };
    let block = Block::bordered()
        .border_type(border_type)
        .border_style(bstyle)
        .title(Span::styled(f.label.clone(), bstyle));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_input(frame, inner, &f.input, theme, Some(&f.placeholder));
}
