//! Drawing. Every string that came from the broker (topic levels, payloads,
//! disconnect reasons) goes through `sanitize`/`preview` before it reaches
//! the terminal.

use super::app::{App, Focus, Mode};
use super::rows::Row;
use mqtt_zapp_core::TopicTree;
use mqtt_zapp_core::client::ConnState;
use mqtt_zapp_core::payload::{preview, sanitize};
use mqtt_zapp_core::search::match_ranges;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use std::time::Instant;

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;
pub const SELECTED_BG: Color = Color::DarkGray;
/// Rows that got a message within `app::FLASH` are drawn bold yellow.
pub const FLASH_MODIFIER: Modifier = Modifier::BOLD;
const FLASH_FG: Color = Color::Yellow;
const PREVIEW_CHARS: usize = 60;
/// Search matches in tree rows.
const MATCH_MODIFIER: Modifier = Modifier::BOLD.union(Modifier::UNDERLINED);

/// Border of a pane: highlighted while it has focus.
pub fn border_style(focused: bool) -> Style {
    if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    }
}

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        f.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }
    match app.mode {
        Mode::Picker | Mode::Password => render_front(f, app, area),
        _ => render_browse(f, app, area),
    }
}

/// The picker and the password prompt: one pane above the status bar.
fn render_front(f: &mut Frame, app: &App, area: Rect) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    match (app.mode, &app.password) {
        (Mode::Password, Some(st)) => super::picker::render_password(f, main, st),
        _ => super::picker::render_picker(f, main, &app.picker, app.store_error.as_deref()),
    }
    f.render_widget(Paragraph::new(status_line(app)), status);
}

fn render_browse(f: &mut Frame, app: &mut App, area: Rect) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let [tree, message] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(main);
    render_tree(f, app, tree);
    super::message_view::render_message(f, message, app);
    f.render_widget(Paragraph::new(status_line(app)), status);
}

fn render_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title("topics")
        .border_style(border_style(app.focus == Focus::Tree));
    let height = block.inner(area).height as usize;
    let rows = app.rows();
    let selected = app
        .selected
        .and_then(|s| rows.iter().position(|r| r.id == s));
    app.tree_offset = scroll_offset(app.tree_offset, selected, rows.len(), height);

    let now = Instant::now();
    let query = app.filter.as_ref().map(|f| f.query.as_str());
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
            Line::from(row_spans(&app.tree, row, query)).style(style)
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

/// `▸ name = preview (count)`, indented by depth; `query` matches in the
/// name and the preview are highlighted.
fn row_spans<'a>(tree: &TopicTree, row: &Row, query: Option<&str>) -> Vec<Span<'a>> {
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
    let mut spans = vec![Span::raw(format!(
        "{}{marker} ",
        "  ".repeat(row.depth as usize)
    ))];
    highlight(&mut spans, name, query);
    if let Some(m) = &node.latest {
        spans.push(Span::raw(" = "));
        highlight(&mut spans, preview(&m.payload, PREVIEW_CHARS), query);
    }
    spans.push(Span::raw(format!(" ({})", node.msg_count)));
    spans
}

/// Push `text` as spans, styling the byte ranges where `query` matches.
fn highlight<'a>(out: &mut Vec<Span<'a>>, text: String, query: Option<&str>) {
    let ranges = query.map(|q| match_ranges(&text, q)).unwrap_or_default();
    let mut at = 0;
    for r in ranges {
        if r.start > at {
            out.push(Span::raw(text[at..r.start].to_owned()));
        }
        out.push(Span::styled(
            text[r.clone()].to_owned(),
            Style::new().add_modifier(MATCH_MODIFIER),
        ));
        at = r.end;
    }
    if at < text.len() {
        out.push(Span::raw(text[at..].to_owned()));
    }
}

fn single_line(s: &str) -> String {
    sanitize(s).replace('\n', "\u{23ce}")
}

fn mode_hint(mode: Mode) -> &'static str {
    match mode {
        Mode::Browse => "/ search  p publish  ? help",
        Mode::Picker => "enter connect  q quit",
        Mode::Password => "enter connect  esc back",
        _ => "",
    }
}

/// `● name  X.X msg/s  N topics  hint`. While searching or filtering, the
/// search text replaces it. Before a connection exists (picker, password)
/// only the hint is shown.
fn status_line(app: &App) -> Line<'static> {
    if app.mode == Mode::SearchInput {
        return Line::raw(format!("/{}▏", single_line(&app.search)));
    }
    if let Some(f) = &app.filter {
        return Line::raw(format!(
            "filter: {} ({} matches)",
            single_line(&f.query),
            f.matches.len()
        ));
    }
    let hint = mode_hint(app.mode);
    if matches!(app.mode, Mode::Picker | Mode::Password) {
        return Line::raw(hint);
    }
    let (state, color) = match &app.conn_state {
        ConnState::Connected => ("●".to_owned(), Color::Green),
        ConnState::Connecting => ("◌".to_owned(), Color::Yellow),
        ConnState::Disconnected {
            reason,
            retry_in_ms: 0,
        } => (format!("✕ {}", single_line(reason)), Color::Red),
        ConnState::Disconnected {
            reason,
            retry_in_ms,
        } => (
            format!(
                "✕ {} – retry in {}s",
                single_line(reason),
                retry_in_ms.div_ceil(1000)
            ),
            Color::Red,
        ),
    };
    let name = app.profile_name.as_deref().map(single_line);
    let rest = format!(
        "{}  {:.1} msg/s  {} topics  {hint}",
        name.map(|n| format!(" {n}")).unwrap_or_default(),
        app.msg_rate.per_sec(Instant::now()),
        app.tree.node(TopicTree::ROOT).topic_count
    );
    Line::from(vec![
        Span::styled(state, Style::new().fg(color)),
        Span::raw(rest),
    ])
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
        assert!(last.contains("✕ refused – retry in 4s"), "{last}");
        assert!(last.contains("msg/s"), "{last}");
    }

    #[test]
    fn search_highlights_matches_and_shows_status() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut a = app_with(&[("a/esp-WZR1", b"hello wzr"), ("b", b"1")]);
        a.handle_action(Action::Search);
        for c in "wzr".chars() {
            a.on_search_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let b = draw(&mut a, 80, 24);
        let s = text(&b);
        assert_eq!(s.lines().last().unwrap().trim_end(), "/wzr▏");
        assert!(!s.contains("= 1"), "non-matching row hidden: {s}");
        let row = s.lines().position(|l| l.contains("esp-WZR1 =")).unwrap();
        let line = s.lines().nth(row).unwrap();
        let line = &line[..line.char_indices().nth(40).unwrap().0]; // tree pane
        let col_of = |byte: usize| line[..byte].chars().count() as u16;
        let (name, prev) = (line.find("WZR").unwrap(), line.rfind("wzr").unwrap());
        let row = row as u16;
        assert!(b[(col_of(name), row)].modifier.contains(MATCH_MODIFIER));
        assert!(
            !b[(col_of(name) - 1, row)]
                .modifier
                .contains(Modifier::UNDERLINED)
        );
        assert!(
            b[(col_of(prev), row)].modifier.contains(MATCH_MODIFIER),
            "preview"
        );
        a.on_search_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let s = text(&draw(&mut a, 80, 24));
        assert!(
            s.lines()
                .last()
                .unwrap()
                .contains("filter: wzr (1 matches)"),
            "{s}"
        );
    }

    fn picker_app(profiles: Vec<mqtt_zapp_core::config::Profile>) -> App {
        let store = ProfileStore { profiles };
        App::new(Launch::Picker, store, Settings::default()).unwrap()
    }

    fn profile(
        name: &str,
        pw: mqtt_zapp_core::config::PasswordSource,
    ) -> mqtt_zapp_core::config::Profile {
        use mqtt_zapp_core::config::{Protocol, TlsConfig};
        mqtt_zapp_core::config::Profile {
            name: name.into(),
            host: "broker.lan".into(),
            port: 8883,
            protocol: Protocol::Mqtts,
            ws_path: "/".into(),
            client_id: None,
            username: Some("alice".into()),
            password: pw,
            tls: TlsConfig::default(),
            subscriptions: vec!["#".into()],
        }
    }

    fn last_line(b: &Buffer) -> String {
        text(b).lines().last().unwrap().trim_end().to_owned()
    }

    #[test]
    fn empty_picker_hint() {
        let mut a = picker_app(vec![]);
        let s = text(&draw(&mut a, 90, 24));
        assert!(
            s.contains("no profiles – add one with: mqtt-zapp profile add --name … --host …"),
            "{s}"
        );
        assert!(!s.contains("load error"));
        a.store_error = Some("load error: bad toml".into());
        let s = text(&draw(&mut a, 90, 24));
        let err = s
            .lines()
            .position(|l| l.contains("load error: bad toml"))
            .unwrap();
        let hint = s.lines().position(|l| l.contains("no profiles")).unwrap();
        assert!(err < hint, "error above hint: {s}");
    }

    #[test]
    fn picker_lists_profiles_with_selection_and_hint() {
        use mqtt_zapp_core::config::PasswordSource::None as N;
        let mut a = picker_app(vec![profile("home", N), profile("lab", N)]);
        a.picker.sel = 1;
        let b = draw(&mut a, 80, 24);
        let s = text(&b);
        assert!(s.contains("home  mqtts://broker.lan:8883  alice"), "{s}");
        let row = s.lines().position(|l| l.contains("lab  mqtts")).unwrap() as u16;
        assert_eq!(b[(3, row)].bg, SELECTED_BG);
        let other = s.lines().position(|l| l.contains("home  mqtts")).unwrap() as u16;
        assert_ne!(b[(3, other)].bg, SELECTED_BG);
        assert!(last_line(&b).contains("enter connect  q quit"), "{s}");
    }

    #[test]
    fn password_render_is_masked() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use mqtt_zapp_core::config::PasswordSource::Prompt;
        let mut a = picker_app(vec![profile("home", Prompt)]);
        a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        for c in "pw".chars() {
            a.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let b = draw(&mut a, 80, 24);
        let s = text(&b);
        assert!(s.contains("••"), "{s}");
        assert!(!s.contains("pw"), "{s}");
        assert!(!s.contains("•••"), "one dot per char: {s}");
        assert!(last_line(&b).contains("enter connect  esc back"), "{s}");
    }

    #[test]
    fn disconnected_state_in_status_bar() {
        let mut a = app_with(&[]);
        a.profile_name = Some("home".into());
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Disconnected {
                reason: "connection refused".into(),
                retry_in_ms: 3200,
            },
        ));
        let b = draw(&mut a, 100, 24);
        let last = last_line(&b);
        assert!(
            last.contains("✕ connection refused – retry in 4s"),
            "{last}"
        );
        assert!(last.contains("home"), "{last}");
        assert!(last.contains("0.0 msg/s  0 topics"), "{last}");
        assert!(last.contains("/ search  p publish  ? help"), "{last}");
        let col = last.chars().position(|c| c == '✕').unwrap() as u16;
        let row = text(&b).lines().count() as u16 - 1;
        assert_eq!(b[(col, row)].fg, Color::Red);
    }

    #[test]
    fn connected_and_connecting_markers() {
        let mut a = app_with(&[("a", b"1")]);
        a.profile_name = Some("home".into());
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Connected,
        ));
        let b = draw(&mut a, 100, 24);
        let last = last_line(&b);
        assert!(last.starts_with("● home"), "{last}");
        assert!(last.contains("1 topics"), "{last}");
        assert_eq!(b[(0, 23)].fg, Color::Green);
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Connecting,
        ));
        let b = draw(&mut a, 100, 24);
        assert!(last_line(&b).starts_with("◌ home"));
        assert_eq!(b[(0, 23)].fg, Color::Yellow);
    }
}
