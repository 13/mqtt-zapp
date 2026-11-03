//! The look in one place: palette, pane borders, selection, key hints and
//! overlay helpers. Only the 16 named ANSI colours are used, so the user's
//! terminal palette (light or dark) decides the actual shades.

use mqtt_zapp_core::payload::sanitize;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};

pub const ACCENT: Color = Color::Cyan;
pub const MUTED: Color = Color::DarkGray;
pub const OK: Color = Color::Green;
pub const WARN: Color = Color::Yellow;
pub const ERR: Color = Color::Red;
pub const SEL_BG: Color = Color::Blue;
pub const SEL_BG_INACTIVE: Color = Color::DarkGray;

/// A bordered pane. Focus shows without colour too: a thick border and a
/// `●` in the title.
pub fn pane(title: &str, focused: bool) -> Block<'static> {
    let title = sanitize(title).replace('\n', " ");
    if focused {
        Block::bordered()
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(ACCENT))
            .title(Line::from(Span::styled(
                format!(" ● {title} "),
                Style::new()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            )))
    } else {
        Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(MUTED))
            .title(Line::from(Span::styled(
                format!(" {title} "),
                Style::new().fg(MUTED),
            )))
    }
}

/// The selected row: bright in the focused pane, grey elsewhere.
pub fn selection(focused: bool) -> Style {
    if focused {
        Style::new()
            .bg(SEL_BG)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(SEL_BG_INACTIVE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn draw_pane(focused: bool) -> Buffer {
        let mut t = Terminal::new(TestBackend::new(20, 3)).unwrap();
        t.draw(|f| f.render_widget(pane("topics", focused), f.area()))
            .unwrap();
        t.backend().buffer().clone()
    }

    fn top_row(b: &Buffer) -> String {
        (0..b.area.width).map(|x| b[(x, 0)].symbol()).collect()
    }

    #[test]
    fn focused_pane_uses_thick_border_and_dot() {
        let b = draw_pane(true);
        assert_eq!(b[(0, 0)].symbol(), "┏");
        assert!(top_row(&b).contains("● topics"), "{}", top_row(&b));
        assert_eq!(b[(0, 0)].fg, ACCENT);
        let b = draw_pane(false);
        assert_eq!(b[(0, 0)].symbol(), "╭");
        assert!(!top_row(&b).contains('●'));
    }

    #[test]
    fn selection_differs_by_focus() {
        assert_eq!(selection(true).bg, Some(SEL_BG));
        assert_eq!(selection(false).bg, Some(SEL_BG_INACTIVE));
    }
}
