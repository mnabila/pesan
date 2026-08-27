use std::str::FromStr;

use ratatui::style::{Color, Modifier, Style};

/// Parse a color string (`#rrggbb` hex, a named color, or an indexed `0-255`),
/// falling back to `fallback` when the value is unparseable.
pub fn parse_color(s: &str, fallback: Color) -> Color {
    Color::from_str(s).unwrap_or(fallback)
}

/// Parse a pane border weight from config into a ratatui `BorderType`. Terminal
/// borders are always one cell thick, so this selects the line-drawing style
/// (thin/rounded/heavy/double), not a pixel width. Unknown values fall back to
/// `Plain`.
pub fn parse_border_type(s: &str) -> ratatui::widgets::BorderType {
    use ratatui::widgets::BorderType;
    match s.trim().to_ascii_lowercase().as_str() {
        "rounded" => BorderType::Rounded,
        "double" => BorderType::Double,
        "thick" | "heavy" | "bold" => BorderType::Thick,
        _ => BorderType::Plain,
    }
}

/// Semantic color roles. Widgets reference roles, never raw colors, so a
/// different theme can be swapped in without touching view code.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub unread: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub border: Color,
    pub border_focus: Color,
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
            bg: Color::Rgb(0x1e, 0x1e, 0x2e),
            fg: Color::Rgb(0xcd, 0xd6, 0xf4),
            dim: Color::Rgb(0x6c, 0x70, 0x86),
            accent: Color::Rgb(0x89, 0xb4, 0xfa),
            unread: Color::Rgb(0xf9, 0xe2, 0xaf),
            success: Color::Rgb(0xa6, 0xe3, 0xa1),
            warning: Color::Rgb(0xfa, 0xb3, 0x87),
            error: Color::Rgb(0xf3, 0x8b, 0xa8),
            border: Color::Rgb(0x45, 0x47, 0x5a),
            border_focus: Color::Rgb(0x89, 0xb4, 0xfa),
        }
    }

    pub const fn light() -> Self {
        Self {
            bg: Color::Rgb(0xff, 0xff, 0xff),
            fg: Color::Rgb(0x24, 0x29, 0x2f),
            dim: Color::Rgb(0x57, 0x60, 0x6a),
            accent: Color::Rgb(0x09, 0x69, 0xda),
            unread: Color::Rgb(0x9a, 0x67, 0x00),
            success: Color::Rgb(0x1a, 0x7f, 0x37),
            warning: Color::Rgb(0xbc, 0x4c, 0x00),
            error: Color::Rgb(0xcf, 0x22, 0x2e),
            border: Color::Rgb(0xd0, 0xd7, 0xde),
            border_focus: Color::Rgb(0x09, 0x69, 0xda),
        }
    }

    pub const fn mono() -> Self {
        Self {
            bg: Color::Black,
            fg: Color::White,
            dim: Color::Gray,
            accent: Color::White,
            unread: Color::White,
            success: Color::White,
            warning: Color::White,
            error: Color::White,
            border: Color::Gray,
            border_focus: Color::White,
        }
    }

    /// The built-in theme for a reserved name, or `None` for any other name.
    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            "dark" => Some(Self::dark()),
            "light" => Some(Self::light()),
            "mono" => Some(Self::mono()),
            _ => None,
        }
    }

    pub fn fg_style(&self) -> Style {
        Style::new().fg(self.fg)
    }

    pub fn dim_style(&self) -> Style {
        Style::new().fg(self.dim)
    }

    pub fn accent_style(&self) -> Style {
        Style::new().fg(self.accent)
    }

    pub fn unread_style(&self) -> Style {
        Style::new().fg(self.unread).add_modifier(Modifier::BOLD)
    }

    pub fn success_style(&self) -> Style {
        Style::new().fg(self.success)
    }

    pub fn warning_style(&self) -> Style {
        Style::new().fg(self.warning)
    }

    pub fn error_style(&self) -> Style {
        Style::new().fg(self.error)
    }

    /// Border style for a pane. Focused panes use the accent border.
    pub fn border_style(&self, focused: bool) -> Style {
        Style::new().fg(if focused {
            self.border_focus
        } else {
            self.border
        })
    }

    /// Full-width selection style: inverse accent so the row reads at a glance.
    pub fn selected_style(&self) -> Style {
        Style::new()
            .fg(self.bg)
            .bg(self.accent)
            .add_modifier(Modifier::BOLD)
    }

    /// Full-row background band for tagged (marked) messages - subtler than the
    /// cursor's selection highlight, so a run of tagged rows reads as a group.
    pub fn marked_style(&self) -> Style {
        Style::new().bg(self.border)
    }
}

/// Icons; ASCII fallback when `ui.ascii` is set.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    pub flagged: &'static str,
    pub attachment: &'static str,
    pub ok: &'static str,
    pub warning: &'static str,
}

impl Glyphs {
    pub const fn new(ascii: bool) -> Self {
        if ascii {
            Self {
                flagged: "!",
                attachment: "@",
                ok: "x",
                warning: "!",
            }
        } else {
            Self {
                flagged: "★",
                attachment: "📎",
                ok: "✓",
                warning: "⚠",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::BorderType;

    #[test]
    fn border_type_parses_known_and_falls_back() {
        assert_eq!(parse_border_type("plain"), BorderType::Plain);
        assert_eq!(parse_border_type("rounded"), BorderType::Rounded);
        assert_eq!(parse_border_type("double"), BorderType::Double);
        assert_eq!(parse_border_type("thick"), BorderType::Thick);
        // Case-insensitive + aliases.
        assert_eq!(parse_border_type("THICK"), BorderType::Thick);
        assert_eq!(parse_border_type("heavy"), BorderType::Thick);
        // Unknown values fall back to Plain.
        assert_eq!(parse_border_type("wibble"), BorderType::Plain);
    }
}
