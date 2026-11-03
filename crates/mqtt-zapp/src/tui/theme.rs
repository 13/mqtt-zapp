//! The look in one place: palette, pane borders, selection, key hints and
//! overlay helpers. Only the 16 named ANSI colours are used, so the user's
//! terminal palette (light or dark) decides the actual shades.

use mqtt_zapp_core::config::ThemeName;
use mqtt_zapp_core::payload::sanitize;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};

pub const ACCENT: Color = Color::Cyan;
pub const MUTED: Color = Color::DarkGray;
pub const OK: Color = Color::Green;
pub const WARN: Color = Color::Yellow;
pub const ERR: Color = Color::Red;
/// Black on cyan, like the focused pane title: cyan is light in practically
/// every palette, where blue (and "white" text on it) often is not.
pub const SEL_BG: Color = ACCENT;
pub const SEL_FG: Color = Color::Black;
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

/// `key label` for the footer: the key in the accent colour, the label muted.
pub fn key_hint(key: &str, label: &str) -> [Span<'static>; 2] {
    [
        Span::styled(
            key.to_owned(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}  "), Style::new().fg(MUTED)),
    ]
}

/// A `width`×`height` rect centred in `area`, clamped to it and kept one
/// cell off its edges when there is room.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let fit = |want: u16, have: u16| want.min(if have > 2 { have - 2 } else { have });
    let (w, h) = (fit(width, area.width), fit(height, area.height));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Dim everything already drawn in `area`, behind an overlay.
pub fn dim_backdrop(f: &mut Frame, area: Rect) {
    f.buffer_mut()
        .set_style(area, Style::new().add_modifier(Modifier::DIM));
}

/// The theme to use: `NO_COLOR` set to anything non-empty forces mono
/// (no-color.org).
pub fn effective(name: ThemeName, no_color: Option<&str>) -> ThemeName {
    match no_color {
        Some(v) if !v.is_empty() => ThemeName::Mono,
        _ => name,
    }
}

/// Adapt a drawn frame to the theme. Everything is drawn for dark; light
/// lifts the grey selection background, mono drops all colour and shows
/// any background as reverse video so selection and badges stay visible.
pub fn apply(buf: &mut Buffer, area: Rect, theme: ThemeName) {
    if theme == ThemeName::Dark {
        return;
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            match theme {
                ThemeName::Light if cell.bg == SEL_BG_INACTIVE => cell.bg = Color::Gray,
                ThemeName::Mono => {
                    if cell.bg != Color::Reset {
                        cell.modifier.insert(Modifier::REVERSED);
                    }
                    cell.fg = Color::Reset;
                    cell.bg = Color::Reset;
                }
                _ => {}
            }
        }
    }
}

/// The selected row: bright in the focused pane, grey elsewhere. The
/// foreground is always set, so muted or accent text cannot vanish into the
/// selection background.
pub fn selection(focused: bool) -> Style {
    if focused {
        Style::new()
            .bg(SEL_BG)
            .fg(SEL_FG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(SEL_BG_INACTIVE).fg(Color::Reset)
    }
}

/// Draw `line` as the selected row. The selection colours go on every span,
/// not just the line, since span colours win over the line's; modifiers
/// (bold, underlined matches) are kept.
pub fn select(mut line: Line<'_>, focused: bool) -> Line<'_> {
    let sel = selection(focused);
    for span in &mut line.spans {
        span.style = span.style.patch(sel);
    }
    line.style(sel)
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
    fn centered_clamps() {
        let area = Rect::new(0, 0, 40, 10);
        let r = centered(area, 76, 22);
        assert!(r.x >= area.x && r.right() <= area.right(), "{r:?}");
        assert!(r.y >= area.y && r.bottom() <= area.bottom(), "{r:?}");
        let r = centered(Rect::new(0, 0, 100, 30), 76, 22);
        assert_eq!(r, Rect::new(12, 4, 76, 22));
    }

    #[test]
    fn select_overrides_span_colours_keeps_modifiers() {
        let line = Line::from(vec![
            Span::styled("a", Style::new().fg(ACCENT)),
            Span::styled(
                "b",
                Style::new().fg(MUTED).add_modifier(Modifier::UNDERLINED),
            ),
        ]);
        for focused in [true, false] {
            let l = select(line.clone(), focused);
            let want = selection(focused);
            for s in &l.spans {
                assert_eq!(s.style.fg, want.fg, "{focused}");
                assert_eq!(s.style.bg, want.bg, "{focused}");
                assert_ne!(s.style.fg, s.style.bg);
            }
            assert!(l.spans[1].style.add_modifier.contains(Modifier::UNDERLINED));
        }
    }

    #[test]
    fn selection_differs_by_focus() {
        assert_eq!(selection(true).bg, Some(SEL_BG));
        assert_eq!(selection(false).bg, Some(SEL_BG_INACTIVE));
    }
}
