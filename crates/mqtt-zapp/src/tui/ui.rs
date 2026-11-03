//! Drawing. Every string that came from the broker (topic levels, payloads,
//! disconnect reasons) goes through `sanitize`/`preview` before it reaches
//! the terminal.

use super::app::{App, Focus, Mode};
use super::keymap::Action;
use super::rows::Row;
use super::theme;
use mqtt_zapp_core::TopicTree;
use mqtt_zapp_core::client::ConnState;
use mqtt_zapp_core::payload::{preview, sanitize};
use mqtt_zapp_core::search::match_ranges;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::time::Instant;

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;
/// Narrower terminals show the tree above the message pane.
const STACK_BELOW: u16 = 80;
/// Rows that got a message within `app::FLASH` are drawn bold yellow.
pub const FLASH_MODIFIER: Modifier = Modifier::BOLD;
const FLASH_FG: Color = Color::Yellow;
const PREVIEW_CHARS: usize = 60;
/// Longest status-bar note; broker error text can be long.
const NOTE_MAX_CHARS: usize = 120;
/// Search matches in tree rows.
const MATCH_MODIFIER: Modifier = Modifier::BOLD.union(Modifier::UNDERLINED);

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
        (Mode::Password, Some(st)) => {
            if !app.picker.profiles.is_empty() {
                super::picker::render_picker(f, main, &app.picker, None);
                theme::dim_backdrop(f, main);
            }
            super::picker::render_password(f, main, st)
        }
        _ => super::picker::render_picker(f, main, &app.picker, app.store_error.as_deref()),
    }
    f.render_widget(Paragraph::new(footer_line(app, area.width)), status);
}

fn render_browse(f: &mut Frame, app: &mut App, area: Rect) {
    let [header, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(header_line(app, area.width)), header);
    // Payloads need the width; below `STACK_BELOW` columns the panes stack.
    let [tree, message] = if main.width < STACK_BELOW {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main)
    } else {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main)
    };
    render_tree(f, app, tree);
    super::message_view::render_message(f, message, app);
    if let (Mode::Publish, Some(form)) = (app.mode, &app.publish) {
        super::publish::render(f, main, form);
    }
    if app.mode == Mode::Help {
        render_help(f, main, app);
    }
    f.render_widget(Paragraph::new(footer_line(app, area.width)), status);
}

/// The help overlay's groups; every action is in exactly one.
const HELP_GROUPS: [(&str, &[Action]); 3] = [
    (
        "navigate",
        &[
            Action::Up,
            Action::Down,
            Action::Collapse,
            Action::Expand,
            Action::ToggleExpand,
            Action::Top,
            Action::Bottom,
            Action::Select,
        ],
    ),
    (
        "view",
        &[
            Action::FocusNext,
            Action::ScrollDown,
            Action::ScrollUp,
            Action::ToggleDiff,
        ],
    ),
    (
        "actions",
        &[
            Action::Search,
            Action::Publish,
            Action::Help,
            Action::Cancel,
            Action::Quit,
        ],
    ),
];

/// Effective keymap, grouped, on the left; keys that bypass the keymap on
/// the right.
fn render_help(f: &mut Frame, area: Rect, app: &App) {
    let bindings = app.keymap.bindings();
    debug_assert!(
        bindings
            .iter()
            .all(|(a, _)| { HELP_GROUPS.iter().filter(|(_, g)| g.contains(a)).count() == 1 })
    );
    theme::dim_backdrop(f, area);
    let area = theme::centered(area, 76, 22);
    let block = theme::pane("help · any key closes", true).title_bottom(
        Line::styled(
            " remap in settings.toml [keys] ",
            Style::new().fg(theme::MUTED),
        )
        .right_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(inner);
    let heading = Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD);
    let key_style = Style::new().add_modifier(Modifier::BOLD);
    let row = |name: &str, keys: String| {
        Line::from(vec![
            Span::styled(format!(" {name:<14}"), Style::new().fg(theme::MUTED)),
            Span::styled(keys, key_style),
        ])
    };
    let mut lines = Vec::new();
    for (title, actions) in HELP_GROUPS {
        lines.push(Line::styled(format!(" {title}"), heading));
        for action in actions {
            let keys = bindings
                .iter()
                .find(|(a, _)| a == action)
                .map(|(_, k)| k.iter().map(|k| k.to_string()).collect::<Vec<_>>())
                .unwrap_or_default();
            let keys = if keys.is_empty() {
                "(unbound)".to_owned()
            } else {
                keys.join(", ")
            };
            lines.push(row(action.name(), keys));
        }
    }
    f.render_widget(Paragraph::new(lines), left);
    let mut lines = vec![Line::styled(" publish form", heading)];
    for (k, what) in [
        ("ctrl-s", "send"),
        ("ctrl-f", "format json"),
        ("tab", "next field"),
        ("esc", "close"),
    ] {
        lines.push(row(k, what.into()));
    }
    lines.push(Line::default());
    lines.push(Line::styled(" search", heading));
    for (k, what) in [
        ("type", "filter topics"),
        ("enter", "keep filter"),
        ("esc", "clear"),
    ] {
        lines.push(row(k, what.into()));
    }
    f.render_widget(Paragraph::new(lines), right);
}

fn render_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Tree;
    let block = theme::pane("topics", focused);
    let height = block.inner(area).height as usize;
    let rows = app.rows();
    let selected = app
        .selected
        .and_then(|s| rows.iter().position(|r| r.id == s));
    app.tree_offset = scroll_offset(app.tree_offset, selected, rows.len(), height);

    if rows.is_empty() {
        let inner = block.inner(area);
        f.render_widget(block, area);
        render_tree_empty(f, app, inner);
        return;
    }
    let now = Instant::now();
    let width = block.inner(area).width as usize;
    let query = app.filter.as_ref().map(|f| f.query.as_str());
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(app.tree_offset)
        .take(height)
        .map(|(i, row)| {
            let flash = app.is_flashing(row.id, now);
            let line = Line::from(row_spans(&app.tree, row, query, width, flash));
            if Some(i) == selected {
                line.style(theme::selection(focused))
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// The tree pane with no rows: waiting for the first message, or a filter
/// that matches nothing.
fn render_tree_empty(f: &mut Frame, app: &App, area: Rect) {
    let muted = Style::new().fg(theme::MUTED);
    let mut lines = match &app.filter {
        Some(flt) => vec![Line::styled(
            format!("no topics match \"{}\"", single_line(&flt.query)),
            muted,
        )],
        None => vec![Line::styled("waiting for messages…", muted)],
    };
    if let (None, Some(server)) = (&app.filter, &app.server) {
        lines.push(Line::styled(
            format!(
                "subscribed to {}",
                single_line(&server.subscriptions.join(", "))
            ),
            muted,
        ));
    }
    let top = area.height.saturating_sub(lines.len() as u16) / 2;
    let [_, body] = Layout::vertical([Constraint::Length(top), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        body,
    );
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

/// `▸ name  preview ……… count`, indented by depth, the count right-aligned
/// to `width` (dropped when it does not fit). `query` matches in the name and
/// the preview are highlighted; `flash` colours the name.
fn row_spans<'a>(
    tree: &TopicTree,
    row: &Row,
    query: Option<&str>,
    width: usize,
    flash: bool,
) -> Vec<Span<'a>> {
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
    let mut spans = vec![
        Span::raw("  ".repeat(row.depth as usize)),
        Span::styled(format!("{marker} "), Style::new().fg(theme::ACCENT)),
    ];
    let mut name_style = Style::new();
    if node.latest.is_some() {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    if flash {
        name_style = name_style.fg(FLASH_FG).add_modifier(FLASH_MODIFIER);
    }
    highlight(&mut spans, name, query, name_style);
    if let Some(m) = &node.latest {
        spans.push(Span::raw("  "));
        highlight(
            &mut spans,
            preview(&m.payload, PREVIEW_CHARS),
            query,
            Style::new().fg(theme::MUTED),
        );
    }
    let count = if node.topic_count > 0 {
        format!("{} · {}t", node.msg_count, node.topic_count)
    } else {
        node.msg_count.to_string()
    };
    let used = spans_width(&spans);
    let count_w = count.chars().count();
    if used + 1 + count_w <= width {
        spans.push(Span::raw(" ".repeat(width - used - count_w)));
        spans.push(Span::styled(count, Style::new().fg(theme::MUTED)));
    }
    spans
}

/// Push `text` as spans, styling the byte ranges where `query` matches.
fn highlight<'a>(out: &mut Vec<Span<'a>>, text: String, query: Option<&str>, base: Style) {
    let ranges = query.map(|q| match_ranges(&text, q)).unwrap_or_default();
    let mut at = 0;
    for r in ranges {
        if r.start > at {
            out.push(Span::styled(text[at..r.start].to_owned(), base));
        }
        out.push(Span::styled(
            text[r.clone()].to_owned(),
            base.add_modifier(MATCH_MODIFIER),
        ));
        at = r.end;
    }
    if at < text.len() {
        out.push(Span::styled(text[at..].to_owned(), base));
    }
}

fn single_line(s: &str) -> String {
    sanitize(s).replace('\n', "\u{23ce}")
}

/// Shortest key bound to `action` (`j` over `down`), or `?` when unbound.
pub(super) fn key_for(app: &App, action: Action) -> String {
    app.keymap
        .bindings()
        .into_iter()
        .find(|(a, _)| *a == action)
        .and_then(|(_, keys)| {
            keys.iter()
                .map(ToString::to_string)
                .min_by_key(|k| k.chars().count())
        })
        .unwrap_or_else(|| "?".into())
}

/// The context's key hints as `(key, label)`, most useful first. Keys come
/// from the keymap, so remaps show up here.
fn hints(app: &App) -> Vec<(String, &'static str)> {
    let k = |a| key_for(app, a);
    let jk = format!("{}/{}", k(Action::Down), k(Action::Up));
    match (app.mode, app.focus) {
        (Mode::Browse, Focus::Tree) => vec![
            (k(Action::Search), "search"),
            (k(Action::Publish), "publish"),
            (k(Action::Select), "expand"),
            (k(Action::FocusNext), "focus"),
            (k(Action::Help), "help"),
            (k(Action::Quit), "quit"),
        ],
        (Mode::Browse, Focus::Payload) => vec![
            (jk, "scroll"),
            (k(Action::ToggleDiff), "diff"),
            (k(Action::FocusNext), "focus"),
            (k(Action::Help), "help"),
            (k(Action::Quit), "quit"),
        ],
        (Mode::Browse, Focus::History) => vec![
            (jk, "history"),
            (k(Action::ToggleDiff), "diff"),
            (k(Action::FocusNext), "focus"),
            (k(Action::Help), "help"),
            (k(Action::Quit), "quit"),
        ],
        (Mode::Picker, _) => vec![
            (k(Action::Select), "connect"),
            (jk, "move"),
            (k(Action::Quit), "quit"),
        ],
        (Mode::Password, _) if app.picker.profiles.is_empty() => {
            vec![("enter".into(), "connect"), ("esc".into(), "quit")]
        }
        (Mode::Password, _) => vec![("enter".into(), "connect"), ("esc".into(), "back")],
        _ => vec![],
    }
}

/// Append whole hints while they fit in `width` columns.
fn push_hints(spans: &mut Vec<Span<'static>>, hints: &[(String, &str)], width: usize) {
    for (key, label) in hints {
        let hint = theme::key_hint(key, label);
        // The trailing gap of the last hint may be cut.
        if spans_width(spans) + spans_width(&hint) - 2 > width {
            break;
        }
        spans.extend(hint);
    }
}

/// The footer: the search prompt while typing a search; otherwise the filter
/// chip, then a note (red for errors) or the context's key hints.
fn footer_line(app: &App, width: u16) -> Line<'static> {
    let width = usize::from(width);
    if app.mode == Mode::SearchInput {
        let mut spans = vec![
            Span::styled(
                "/",
                Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{}▏", single_line(&app.search))),
        ];
        let mut right = Vec::new();
        push_hints(
            &mut right,
            &[("enter".into(), "keep"), ("esc".into(), "clear")],
            width,
        );
        let right_w = spans_width(&right).saturating_sub(2);
        let pad = width.saturating_sub(spans_width(&spans) + right_w);
        if !right.is_empty() && pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
            spans.extend(right);
        }
        return Line::from(spans);
    }
    let mut spans = Vec::new();
    if let Some(f) = app
        .filter
        .as_ref()
        .filter(|_| !matches!(app.mode, Mode::Picker | Mode::Password))
    {
        let chip = format!(" filter: {} ({}) ", single_line(&f.query), f.matches.len());
        spans.push(Span::styled(
            ellipsize(&chip, width),
            Style::new().fg(Color::Black).bg(theme::WARN),
        ));
        spans.push(Span::raw(" "));
        push_hints(
            &mut spans,
            &[(key_for(app, Action::Cancel), "clear")],
            width,
        );
    }
    if let Some(note) = app.note_at(Instant::now()) {
        let room = width
            .saturating_sub(spans_width(&spans))
            .min(NOTE_MAX_CHARS);
        let color = if note.error { theme::ERR } else { theme::OK };
        spans.push(Span::styled(
            ellipsize(&single_line(&note.text), room),
            Style::new().fg(color),
        ));
    } else {
        push_hints(&mut spans, &hints(app), width);
    }
    Line::from(spans)
}

/// Width of `spans` in terminal columns.
fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Cut `s` to at most `max` columns, ending in `…` when it was cut.
fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    if max > 0 {
        out.push('…');
    }
    out
}

/// ` mqtt-zapp  ● connected  name  url ……… rate · topics`. When the line is
/// too wide the url goes first, then the stats, then the name; the state
/// pill is cut last.
fn header_line(app: &App, width: u16) -> Line<'static> {
    let width = usize::from(width);
    let badge = Span::styled(
        " mqtt-zapp ",
        Style::new()
            .fg(Color::Black)
            .bg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    );
    let (state, color) = match &app.conn_state {
        ConnState::Connected => ("● connected".to_owned(), theme::OK),
        ConnState::Connecting => ("◌ connecting".to_owned(), theme::WARN),
        ConnState::Disconnected {
            reason,
            retry_in_ms: 0,
        } => (format!("✕ {}", single_line(reason)), theme::ERR),
        ConnState::Disconnected {
            reason,
            retry_in_ms,
        } => (
            format!(
                "✕ {} · retry {}s",
                single_line(reason),
                retry_in_ms.div_ceil(1000)
            ),
            theme::ERR,
        ),
    };
    let pill_style = Style::new().fg(color).add_modifier(Modifier::BOLD);
    let muted = Style::new().fg(theme::MUTED);
    let mut name = app.server.as_ref().map(|s| {
        vec![
            Span::raw("  "),
            Span::styled(
                single_line(&s.name),
                Style::new().add_modifier(Modifier::BOLD),
            ),
        ]
    });
    let mut url = app
        .server
        .as_ref()
        .map(|s| vec![Span::raw("  "), Span::styled(single_line(&s.url), muted)]);
    let mut stats = Some(Span::styled(
        format!(
            "{:.1} msg/s · {} topics ",
            app.msg_rate.per_sec(Instant::now()),
            app.tree.node(TopicTree::ROOT).topic_count
        ),
        muted,
    ));
    let fixed = badge.width() + 1;
    let pill_w = state.chars().count();
    let total = |name: &Option<Vec<Span>>, url: &Option<Vec<Span>>, stats: &Option<Span>| {
        fixed
            + pill_w
            + name.as_deref().map_or(0, spans_width)
            + url.as_deref().map_or(0, spans_width)
            + stats.as_ref().map_or(0, |s| s.width() + 2)
    };
    if total(&name, &url, &stats) > width {
        url = None;
    }
    if total(&name, &url, &stats) > width {
        stats = None;
    }
    if total(&name, &url, &stats) > width {
        name = None;
    }
    let state = ellipsize(&state, width.saturating_sub(fixed));
    let mut spans = vec![badge, Span::raw(" "), Span::styled(state, pill_style)];
    spans.extend(name.into_iter().flatten());
    spans.extend(url.into_iter().flatten());
    if let Some(stats) = stats {
        let pad = width.saturating_sub(spans_width(&spans) + stats.width());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(stats);
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Launch;
    use crate::tui::app::{App, Mode, Note, ServerInfo};
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
    fn narrow_terminal_stacks_panes() {
        let names: Vec<String> = (0..40).map(|i| format!("t{i:02}")).collect();
        let items: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"1"[..])).collect();
        let mut a = app_with(&items);
        a.handle_action(Action::Down); // select something first
        a.handle_action(Action::Bottom);
        let b = draw(&mut a, 60, 30);
        assert_eq!(b[(0, 1)].symbol(), "┏");
        assert_eq!(b[(59, 1)].symbol(), "┓");
        let s = text(&b);
        let second = s
            .lines()
            .skip(2)
            .position(|l| l.starts_with('╭'))
            .expect("message pane below the tree");
        assert!(second > 5, "{s}");
        assert!(s.contains("t39"), "selected row visible: {s}");
    }

    #[test]
    fn wide_terminal_splits_40_60() {
        let mut a = app_with(&[("a", b"1")]);
        let b = draw(&mut a, 100, 24);
        assert_eq!(b[(39, 1)].symbol(), "┓");
        assert_eq!(b[(40, 1)].symbol(), "╭");
    }

    #[test]
    fn tab_moves_thick_border_to_history() {
        let mut a = app_with(&[("a", b"1"), ("a", b"2")]);
        a.handle_action(Action::Down);
        let first_thick = |b: &Buffer| {
            let w = b.area.width;
            (0..b.content.len())
                .find(|&i| b.content[i].symbol() == "┏")
                .map(|i| (i as u16 % w, i as u16 / w))
                .unwrap()
        };
        let b = draw(&mut a, 100, 24);
        let (x, y) = first_thick(&b);
        assert!(x < 40, "tree focused: {x}");
        assert_eq!(b[(x, y)].symbol(), "┏");
        a.handle_action(Action::FocusNext);
        a.handle_action(Action::FocusNext);
        let b = draw(&mut a, 100, 24);
        let (x, _) = first_thick(&b);
        assert!(x >= 40, "history focused: {x}");
        let tree_corner = (0..b.content.len())
            .find(|&i| b.content[i].symbol() == "╭")
            .map(|i| i as u16 % 100)
            .unwrap();
        assert!(tree_corner < 40);
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
        let line = |needle: &str| {
            s.lines()
                .find(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle}: {s}"))
                .chars()
                .take(32) // the tree pane at 80 columns
                .collect::<String>()
        };
        // Counts are right-aligned against the pane's right border.
        assert!(line("▾ a").ends_with(" 3 · 2t┃"), "{s}");
        assert!(line("  ▸ ∅").ends_with(" 1 · 1t┃"), "{s}");
        assert!(line("b  world").ends_with(" 2┃"), "{s}");
        assert!(s.contains("message"), "right pane placeholder: {s}");
    }

    #[test]
    fn fresh_row_flashes_and_selection_has_background() {
        let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
        a.handle_action(Action::Down);
        let b = draw(&mut a, 80, 24);
        let s = text(&b);
        let row_of = |needle: &str| s.lines().position(|l| l.contains(needle)).unwrap() as u16;
        let sel = &b[(3, row_of("a  1"))];
        let other = &b[(3, row_of("b  2"))];
        assert_eq!(sel.bg, theme::SEL_BG);
        assert_ne!(other.bg, theme::SEL_BG);
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
        let top = s.lines().next().unwrap();
        assert!(top.contains("✕ refused · retry 4s"), "{top}");
        assert!(top.contains("msg/s"), "{top}");
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
        let last = s.lines().last().unwrap().trim_end().to_owned();
        assert!(last.starts_with("/wzr▏"), "{last}");
        assert!(last.ends_with("enter keep  esc clear"), "{last}");
        assert!(!s.contains("  b  1"), "non-matching row hidden: {s}");
        let tree_part = |l: &str| l.chars().take(32).collect::<String>();
        let row = s
            .lines()
            .position(|l| tree_part(l).contains("esp-WZR1  "))
            .unwrap();
        let line = s.lines().nth(row).unwrap();
        let line = &line[..line.char_indices().nth(32).unwrap().0]; // tree pane
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
        assert!(s.lines().last().unwrap().contains("filter: wzr (1)"), "{s}");
    }

    #[test]
    fn filter_segment_coexists_with_state_and_note() {
        let mut a = app_with(&[("a/wzr", b"1")]);
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Connected,
        ));
        a.filter = mqtt_zapp_core::search::filter(&a.tree, "wzr");
        a.set_note(Note::info("published ok"), Instant::now());
        let b = draw(&mut a, 100, 24);
        let last = last_line(&b);
        let top = first_line(&b);
        assert!(top.contains("● connected"), "{top}");
        assert!(top.contains("msg/s"), "{top}");
        assert!(last.contains("filter: wzr (1)"), "{last}");
        assert!(last.contains("published ok"), "{last}");
        assert!(!last.contains("p publish"), "note beats hint: {last}");
    }

    #[test]
    fn browse_hint_follows_keymap_remaps() {
        let mut a = app_with(&[("a", b"1")]);
        let last = last_line(&draw(&mut a, 100, 24));
        assert!(
            last.contains("/ search  p publish  enter expand  tab focus  ? help"),
            "{last}"
        );
        let mut s = Settings::default();
        s.keys.insert("publish".into(), "P".into());
        let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
        a.mode = Mode::Browse;
        let last = last_line(&draw(&mut a, 100, 24));
        assert!(last.contains("P publish"), "{last}");
        assert!(!last.contains("p publish"), "{last}");
    }

    #[test]
    fn password_hint_says_quit_without_profiles() {
        let mut a = picker_app(vec![]);
        a.mode = Mode::Password;
        assert!(last_line(&draw(&mut a, 100, 24)).contains("esc quit"));
        let mut a = picker_app(vec![profile(
            "x",
            mqtt_zapp_core::config::PasswordSource::Prompt,
        )]);
        a.mode = Mode::Password;
        assert!(last_line(&draw(&mut a, 100, 24)).contains("esc back"));
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

    fn first_line(b: &Buffer) -> String {
        text(b).lines().next().unwrap().trim_end().to_owned()
    }

    fn home() -> ServerInfo {
        ServerInfo {
            name: "home".into(),
            url: "mqtts://alice@broker.lan:8883".into(),
            subscriptions: vec!["#".into(), "$SYS/#".into()],
        }
    }

    #[test]
    fn header_shows_broker_and_state() {
        let mut a = app_with(&[("a", b"1")]);
        a.server = Some(home());
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Connected,
        ));
        let b = draw(&mut a, 120, 24);
        let top = first_line(&b);
        for want in [
            "mqtt-zapp",
            "● connected",
            "home",
            "mqtts://alice@broker.lan:8883",
        ] {
            assert!(top.contains(want), "{want}: {top}");
        }
        assert!(top.ends_with("topics"), "{top}");
    }

    #[test]
    fn header_fits_40_cols() {
        let mut a = app_with(&[("a", b"1")]);
        a.server = Some(home());
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Disconnected {
                reason: "a very long reason that does not fit anywhere at all".into(),
                retry_in_ms: 1000,
            },
        ));
        let b = draw(&mut a, 40, 10);
        let top = first_line(&b);
        assert!(top.starts_with(" mqtt-zapp "), "{top}");
        assert!(top.contains('✕'), "{top}");
        assert!(top.ends_with('…'), "{top}");
        let second = text(&b).lines().nth(1).unwrap().to_owned();
        assert!(
            second.starts_with('┏') || second.starts_with('╭'),
            "{second}"
        );
    }

    #[test]
    fn header_sanitizes_profile_and_reason() {
        let mut a = app_with(&[("a", b"1")]);
        a.server = Some(ServerInfo {
            name: "h\x1b[31m".into(),
            url: "mqtt://\x1b]0;x\x07:1".into(),
            subscriptions: vec![],
        });
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Disconnected {
                reason: "\x1b[2J".into(),
                retry_in_ms: 0,
            },
        ));
        let b = draw(&mut a, 120, 24);
        assert!(b.content.iter().all(|c| !c.symbol().contains('\x1b')));
        assert!(b.content.iter().all(|c| !c.symbol().contains('\x07')));
    }

    #[test]
    fn search_footer_survives_long_query_at_40_cols() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut a = app_with(&[("a", b"1")]);
        a.handle_action(Action::Search);
        for c in "x".repeat(60).chars() {
            a.on_search_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(last_line(&draw(&mut a, 40, 10)).starts_with("/xxx"));
    }

    #[test]
    fn tree_empty_state_lists_subscriptions() {
        let mut a = app_with(&[]);
        a.server = Some(home());
        let s = text(&draw(&mut a, 100, 24));
        assert!(s.contains("waiting for messages…"), "{s}");
        assert!(s.contains("subscribed to #, $SYS/#"), "{s}");
        a.server = None;
        let s = text(&draw(&mut a, 100, 24));
        assert!(!s.contains("subscribed to"), "{s}");
    }

    #[test]
    fn tree_filter_no_match_message() {
        let mut a = app_with(&[("a", b"1")]);
        a.filter = mqtt_zapp_core::search::filter(&a.tree, "zzz");
        let s = text(&draw(&mut a, 100, 24));
        assert!(s.contains("no topics match \"zzz\""), "{s}");
    }

    #[test]
    fn footer_hints_follow_focus() {
        let mut a = app_with(&[("a", b"1")]);
        let last = last_line(&draw(&mut a, 100, 24));
        assert!(
            last.contains("/ search") && last.contains("p publish"),
            "{last}"
        );
        a.handle_action(Action::FocusNext);
        let last = last_line(&draw(&mut a, 100, 24));
        assert!(
            last.contains("j/k scroll") && last.contains("d diff"),
            "{last}"
        );
        assert!(!last.contains("p publish"), "{last}");
    }

    #[test]
    fn footer_drops_hints_when_narrow() {
        let mut a = app_with(&[("a", b"1")]);
        let last = last_line(&draw(&mut a, 40, 10));
        assert!(last.chars().count() <= 40, "{last}");
        assert!(last.starts_with("/ search"), "{last}");
        // Whole hints only: every key is followed by its label.
        let words: Vec<&str> = last.split("  ").collect();
        assert!(words.iter().all(|w| w.contains(' ')), "{last}");
    }

    #[test]
    fn error_note_is_red() {
        let mut a = app_with(&[("a", b"1")]);
        a.set_note(Note::error("publish failed: x"), Instant::now());
        let b = draw(&mut a, 100, 24);
        let last = last_line(&b);
        let col = last.find("publish failed").unwrap();
        let col = last[..col].chars().count() as u16;
        assert_eq!(b[(col, 23)].fg, theme::ERR);
        a.set_note(Note::info("published a"), Instant::now());
        let b = draw(&mut a, 100, 24);
        let last = last_line(&b);
        let col = last[..last.find("published").unwrap()].chars().count() as u16;
        assert_eq!(b[(col, 23)].fg, theme::OK);
    }

    #[test]
    fn filter_chip_is_highlighted() {
        let mut a = app_with(&[("a/wzr", b"1")]);
        a.filter = mqtt_zapp_core::search::filter(&a.tree, "wzr");
        let b = draw(&mut a, 100, 24);
        let last = last_line(&b);
        assert!(last.starts_with(" filter: wzr (1) "), "{last}");
        assert_eq!(b[(1, 23)].bg, theme::WARN);
        assert!(last.contains("esc clear"), "{last}");
    }

    #[test]
    fn password_modal_shows_server() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use mqtt_zapp_core::config::PasswordSource::Prompt;
        let mut a = picker_app(vec![profile("home", Prompt)]);
        a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let s = text(&draw(&mut a, 80, 24));
        assert!(s.contains("password · home"), "{s}");
        assert!(s.contains("mqtts://alice@broker.lan:8883"), "{s}");
    }

    #[test]
    fn help_groups_actions() {
        let mut a = app_with(&[("a", b"1")]);
        a.handle_action(Action::Help);
        let s = text(&draw(&mut a, 100, 30));
        for want in ["navigate", "view", "actions"] {
            assert!(s.contains(want), "{want}: {s}");
        }
    }

    #[test]
    fn overlay_fits_tiny_terminal() {
        let mut a = app_with(&[("a", b"1")]);
        a.handle_action(Action::Help);
        let b = draw(&mut a, 40, 10);
        assert!(text(&b).contains("help"));
        a.mode = Mode::Browse;
        a.handle_action(Action::Publish);
        let s = text(&draw(&mut a, 40, 10));
        assert!(s.contains("publish"), "{s}");
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
        let row = s.lines().position(|l| l.contains("▶ lab")).unwrap();
        let line = s.lines().nth(row).unwrap();
        assert!(line.contains("mqtts://alice@broker.lan:8883"), "{s}");
        let col = line[..line.find("lab").unwrap()].chars().count() as u16;
        assert_eq!(b[(col, row as u16)].bg, theme::SEL_BG);
        let other = s.lines().position(|l| l.contains("  home ")).unwrap() as u16;
        assert_ne!(b[(col, other)].bg, theme::SEL_BG);
        assert!(
            last_line(&b).contains("enter connect  j/k move  q quit"),
            "{s}"
        );
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
        a.server = Some(home());
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Disconnected {
                reason: "connection refused".into(),
                retry_in_ms: 3200,
            },
        ));
        let b = draw(&mut a, 120, 24);
        let top = first_line(&b);
        assert!(top.contains("✕ connection refused · retry 4s"), "{top}");
        assert!(top.contains("home"), "{top}");
        assert!(top.ends_with("0.0 msg/s · 0 topics"), "{top}");
        let col = top.chars().position(|c| c == '✕').unwrap() as u16;
        assert_eq!(b[(col, 0)].fg, theme::ERR);
    }

    #[test]
    fn connected_and_connecting_markers() {
        let mut a = app_with(&[("a", b"1")]);
        a.server = Some(home());
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Connected,
        ));
        let b = draw(&mut a, 100, 24);
        let top = first_line(&b);
        assert!(top.contains("● connected"), "{top}");
        assert!(top.contains("home"), "{top}");
        assert!(top.contains("1 topics"), "{top}");
        let col = top.chars().position(|c| c == '●').unwrap() as u16;
        assert_eq!(b[(col, 0)].fg, theme::OK);
        a.apply_core_event(CoreEvent::State(
            mqtt_zapp_core::client::ConnState::Connecting,
        ));
        let b = draw(&mut a, 100, 24);
        let top = first_line(&b);
        assert!(top.contains("◌ connecting"), "{top}");
        let col = top.chars().position(|c| c == '◌').unwrap() as u16;
        assert_eq!(b[(col, 0)].fg, theme::WARN);
    }

    #[test]
    fn publish_overlay_renders_fields_and_error_title() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut a = app_with(&[("a/b", b"1")]);
        a.handle_action(Action::Publish);
        let s = text(&draw(&mut a, 80, 24));
        for want in [
            "publish",
            "topic: ",
            "payload",
            "QoS: [0] 1 2   Retain: [ ]",
            "ctrl-s send  ctrl-f format json  tab next  esc close",
            "topics",
        ] {
            assert!(s.contains(want), "{want}: {s}");
        }
        a.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let s = text(&draw(&mut a, 80, 24));
        assert!(s.contains("publish - invalid topic"), "{s}");
    }

    #[test]
    fn status_note_is_sanitized_and_expires() {
        let mut a = app_with(&[]);
        a.set_note(Note::error("publish failed: \x1b[2Jboom"), Instant::now());
        let b = draw(&mut a, 100, 24);
        assert!(b.content.iter().all(|c| !c.symbol().contains('\x1b')));
        assert!(last_line(&b).contains("publish failed: \u{fffd}[2Jboom"));
        a.status_note = Some((
            Note::info("old"),
            Instant::now() - std::time::Duration::from_secs(6),
        ));
        assert!(!last_line(&draw(&mut a, 100, 24)).contains("old"));
    }

    #[test]
    fn help_overlay_lists_effective_keymap() {
        let mut st = Settings::default();
        st.keys.insert("quit".into(), "x".into());
        let mut a = App::new(Launch::Picker, ProfileStore::default(), st).unwrap();
        a.mode = Mode::Browse;
        a.handle_action(Action::Help);
        assert_eq!(a.mode, Mode::Help);
        let s = text(&draw(&mut a, 100, 30));
        let line = s.lines().find(|l| l.contains("quit")).expect("quit row");
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|w| w.contains("quit")).unwrap();
        assert_eq!(words[at + 1], "x", "{line}");
        assert!(!words.contains(&"q"), "{line}");
        for want in [
            "toggle_diff",
            "d",
            "ctrl-d",
            "k, up",
            "ctrl-s",
            "publish form",
        ] {
            assert!(s.contains(want), "{want}: {s}");
        }
    }
}
