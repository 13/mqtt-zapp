//! Drawing. Every string that came from the broker (topic levels, payloads,
//! disconnect reasons) goes through `sanitize`/`preview` before it reaches
//! the terminal.

use super::app::{App, Mode};
use super::rows::Row;
use mqtt_zapp_core::TopicTree;
use mqtt_zapp_core::client::ConnState;
use mqtt_zapp_core::payload::{preview, sanitize};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use std::time::Instant;

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;
pub const SELECTED_BG: Color = Color::DarkGray;
/// Rows that got a message within `app::FLASH` are drawn bold yellow.
pub const FLASH_MODIFIER: Modifier = Modifier::BOLD;
const FLASH_FG: Color = Color::Yellow;
const PREVIEW_CHARS: usize = 60;

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        f.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }
    match app.mode {
        Mode::Picker => render_picker_placeholder(f, area),
        _ => render_browse(f, app, area),
    }
}

fn render_picker_placeholder(f: &mut Frame, area: Rect) {
    let text = Paragraph::new("connection picker — coming soon (use --profile or --host)")
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title("mqtt-zapp"));
    f.render_widget(text, area);
}

fn render_browse(f: &mut Frame, app: &mut App, area: Rect) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let [tree, message] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(main);
    render_tree(f, app, tree);
    // TODO(task 9): the message pane.
    f.render_widget(Block::bordered().title("message"), message);
    f.render_widget(Paragraph::new(status_text(app)), status);
}

fn render_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered().title("topics");
    let height = block.inner(area).height as usize;
    let rows = app.rows();
    let selected = app
        .selected
        .and_then(|s| rows.iter().position(|r| r.id == s));
    app.tree_offset = scroll_offset(app.tree_offset, selected, rows.len(), height);

    let now = Instant::now();
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(app.tree_offset)
        .take(height)
        .map(|(i, row)| {
            let mut style = Style::new();
            if app.is_flashing(row.id, now) {
                style = style.fg(FLASH_FG).add_modifier(FLASH_MODIFIER);
            }
            if Some(i) == selected {
                style = style.bg(SELECTED_BG);
            }
            Line::styled(row_text(&app.tree, row), style)
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Keep the selected row inside a window of `height` rows.
fn scroll_offset(offset: usize, selected: Option<usize>, len: usize, height: usize) -> usize {
    let mut offset = offset;
    if let Some(s) = selected {
        if s < offset {
            offset = s;
        } else if height > 0 && s >= offset + height {
            offset = s + 1 - height;
        }
    }
    offset.min(len.saturating_sub(height))
}

/// `▸ name = preview (count)`, indented by depth.
fn row_text(tree: &TopicTree, row: &Row) -> String {
    let node = tree.node(row.id);
    let marker = match (row.has_children, row.expanded) {
        (false, _) => ' ',
        (true, false) => '▸',
        (true, true) => '▾',
    };
    let name = if node.name.is_empty() {
        "∅".to_owned()
    } else {
        single_line(&node.name)
    };
    let mut s = format!("{}{marker} {name}", "  ".repeat(row.depth as usize));
    if let Some(m) = &node.latest {
        s.push_str(" = ");
        s.push_str(&preview(&m.payload, PREVIEW_CHARS));
    }
    s.push_str(&format!(" ({})", node.msg_count));
    s
}

fn single_line(s: &str) -> String {
    sanitize(s).replace('\n', "\u{23ce}")
}

fn status_text(app: &App) -> String {
    let state = match &app.conn_state {
        ConnState::Connecting => "connecting…".to_owned(),
        ConnState::Connected => "connected".to_owned(),
        ConnState::Disconnected {
            reason,
            retry_in_ms: 0,
        } => format!("disconnected: {}", single_line(reason)),
        ConnState::Disconnected {
            reason,
            retry_in_ms,
        } => format!(
            "disconnected: {} – retry in {}s",
            single_line(reason),
            retry_in_ms.div_ceil(1000)
        ),
    };
    format!(
        "{state} · {:.1} msg/s · {} topics",
        app.msg_rate.per_sec(Instant::now()),
        app.tree.node(TopicTree::ROOT).topic_count
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Launch;
    use crate::tui::app::{App, Mode};
    use crate::tui::keymap::Action;
    use mqtt_zapp_core::Message;
    use mqtt_zapp_core::client::CoreEvent;
    use mqtt_zapp_core::config::{ProfileStore, Settings};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use std::time::SystemTime;

    fn app_with(items: &[(&str, &[u8])]) -> App {
        let mut a = App::new(Launch::Picker, ProfileStore::default(), Settings::default()).unwrap();
        a.mode = Mode::Browse;
        for (topic, payload) in items {
            a.apply_core_event(CoreEvent::Message {
                topic: (*topic).into(),
                msg: Message {
                    payload: payload.to_vec(),
                    qos: 0,
                    retain: false,
                    received_at: SystemTime::now(),
                },
            });
        }
        a
    }

    fn draw(a: &mut App, w: u16, h: u16) -> Buffer {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(f, a)).unwrap();
        t.backend().buffer().clone()
    }

    fn text(b: &Buffer) -> String {
        let w = b.area.width as usize;
        b.content
            .chunks(w)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn too_small_terminal_renders_notice() {
        let mut a = app_with(&[("a", b"1")]);
        assert!(text(&draw(&mut a, 30, 8)).contains("terminal too small"));
        assert!(text(&draw(&mut a, 39, 24)).contains("terminal too small"));
        assert!(!text(&draw(&mut a, 40, 10)).contains("terminal too small"));
    }

    #[test]
    fn escape_sequence_in_payload_is_not_rendered_raw() {
        let mut a = app_with(&[("x\x1b[31m", b"\x1b[2J")]);
        let b = draw(&mut a, 80, 24);
        assert!(b.content.iter().all(|c| !c.symbol().contains('\x1b')));
    }

    #[test]
    fn tree_row_shows_marker_name_preview_and_count() {
        let mut a = app_with(&[("a/b", b"hello"), ("a/b", b"world"), ("a//c", b"1")]);
        a.handle_action(Action::Down);
        a.handle_action(Action::Expand);
        let s = text(&draw(&mut a, 80, 24));
        assert!(s.contains("▾ a (3)"), "{s}");
        assert!(s.contains("  ▸ ∅ (1)"), "{s}");
        assert!(s.contains("b = world (2)"), "{s}");
        assert!(s.contains("message"), "right pane placeholder: {s}");
    }

    #[test]
    fn fresh_row_flashes_and_selection_has_background() {
        let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
        a.handle_action(Action::Down);
        let b = draw(&mut a, 80, 24);
        let s = text(&b);
        let row_of = |needle: &str| s.lines().position(|l| l.contains(needle)).unwrap() as u16;
        let sel = &b[(3, row_of("a = 1"))];
        let other = &b[(3, row_of("b = 2"))];
        assert_eq!(sel.bg, SELECTED_BG);
        assert_ne!(other.bg, SELECTED_BG);
        assert!(other.modifier.contains(FLASH_MODIFIER));
    }

    #[test]
    fn status_bar_shows_state_and_rate() {
        let mut a = app_with(&[("a", b"1")]);
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Disconnected {
                reason: "refused".into(),
                retry_in_ms: 4000,
            },
        ));
        let s = text(&draw(&mut a, 80, 24));
        let last = s.lines().last().unwrap();
        assert!(
            last.contains("disconnected: refused – retry in 4s"),
            "{last}"
        );
        assert!(last.contains("msg/s"), "{last}");
    }

    #[test]
    fn picker_mode_renders_placeholder() {
        let mut a = app_with(&[]);
        a.mode = Mode::Picker;
        let s = text(&draw(&mut a, 80, 24));
        assert!(
            s.contains("connection picker — coming soon (use --profile or --host)"),
            "{s}"
        );
    }
}
