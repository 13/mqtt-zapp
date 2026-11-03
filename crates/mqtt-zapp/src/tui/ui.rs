//! Drawing. Every string that came from the broker (topic levels, payloads,
//! disconnect reasons) goes through `sanitize`/`preview` before it reaches
//! the terminal.

use super::app::{App, Focus, HitAreas, Mode};
use super::keymap::Action;
use super::rows::Row;
use super::theme;
use mqtt_zapp_core::TopicTree;
use mqtt_zapp_core::client::ConnState;
use mqtt_zapp_core::payload::{preview, sanitize};
use mqtt_zapp_core::search::{Filter, QueryMode};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::time::Instant;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;
/// Narrower terminals show the tree above the message pane.
const STACK_BELOW: u16 = 80;
/// Topic names that got a message within `app::FLASH` are drawn bold yellow.
pub const FLASH_MODIFIER: Modifier = Modifier::BOLD;
const FLASH_FG: Color = Color::Yellow;
const PREVIEW_CHARS: usize = 60;
/// Longest footer note; broker error text can be long.
const NOTE_MAX_CHARS: usize = 120;
/// Search matches in tree rows.
const MATCH_MODIFIER: Modifier = Modifier::BOLD.union(Modifier::UNDERLINED);

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.hit.set(Default::default());
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        f.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }
    match app.mode {
        Mode::Picker | Mode::Password | Mode::ProfileForm => render_front(f, app, area),
        Mode::Confirm if on_front(app) => render_front(f, app, area),
        _ => render_browse(f, app, area),
    }
    theme::apply(f.buffer_mut(), area, app.theme);
}

/// Whether a confirm question belongs to the picker (deleting a profile).
fn on_front(app: &App) -> bool {
    matches!(
        app.confirm,
        Some(super::app::Confirm {
            action: super::app::ConfirmAction::DeleteProfile(_),
            ..
        })
    )
}

/// The picker and the password prompt: a centred card above the footer.
fn render_front(f: &mut Frame, app: &App, area: Rect) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    match (app.mode, &app.password) {
        (Mode::ProfileForm, _) => {
            let _ =
                super::picker::render_picker(f, main, &app.picker, None, app.connected.as_deref());
            if let Some(form) = &app.profile_form {
                super::profile_form::render(f, main, form);
            }
        }
        (Mode::Confirm, _) => {
            let _ =
                super::picker::render_picker(f, main, &app.picker, None, app.connected.as_deref());
            if let Some(c) = &app.confirm {
                render_confirm(f, main, c);
            }
        }
        (Mode::Password, Some(st)) => {
            if !app.picker.profiles.is_empty() {
                let _ = super::picker::render_picker(
                    f,
                    main,
                    &app.picker,
                    None,
                    app.connected.as_deref(),
                );
                theme::dim_backdrop(f, main);
            }
            super::picker::render_password(f, main, st)
        }
        _ => {
            let (card, skip) = super::picker::render_picker(
                f,
                main,
                &app.picker,
                app.store_error.as_deref(),
                app.connected.as_deref(),
            );
            app.hit.set(HitAreas {
                picker: card,
                picker_skip: skip,
                ..app.hit.get()
            });
        }
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
    let (top, broker) = header_line(app, area.width);
    f.render_widget(Paragraph::new(top), header);
    app.hit.set(HitAreas {
        header_broker: broker.map_or(Rect::default(), |(x, w)| Rect {
            x: header.x + x,
            width: w,
            ..header
        }),
        ..app.hit.get()
    });
    // Payloads need the width; below `STACK_BELOW` columns the panes stack.
    let stacked = main.width < STACK_BELOW;
    let [tree, message] = if stacked {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main)
    } else {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main)
    };
    render_tree(f, app, tree);
    super::message_view::render_message(f, message, app, stacked);
    if let (Mode::Publish, Some(form)) = (app.mode, &app.publish) {
        super::publish::render(f, main, form);
    }
    if app.mode == Mode::Help {
        render_help(f, main, app);
    }
    if app.mode == Mode::Stats {
        render_stats(f, main, app);
    }
    if let (Mode::Confirm, Some(c)) = (app.mode, &app.confirm) {
        render_confirm(f, main, c);
    }
    f.render_widget(Paragraph::new(footer_line(app, area.width)), status);
}

/// Live broker statistics from `$SYS`; redrawn every frame from the tree.
fn render_stats(f: &mut Frame, area: Rect, app: &App) {
    let stats = mqtt_zapp_core::stats::broker_stats(&app.tree);
    theme::dim_backdrop(f, area);
    let card = theme::centered(area, 64, stats.len().max(1) as u16 + 4);
    f.render_widget(Clear, card);
    let muted = Style::new().fg(theme::MUTED);
    let mut lines = vec![Line::default()];
    if stats.is_empty() {
        lines.push(Line::styled(
            " no $SYS data – subscribe to $SYS/# (the default)",
            muted,
        ));
    }
    for (label, value) in stats {
        lines.push(Line::from(vec![
            Span::styled(format!(" {label:<20}"), muted),
            Span::styled(value, Style::new().add_modifier(Modifier::BOLD)),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines).block(theme::pane("broker stats · live", true)),
        card,
    );
}

/// A yes/no card for a destructive action.
fn render_confirm(f: &mut Frame, area: Rect, c: &super::app::Confirm) {
    theme::dim_backdrop(f, area);
    let card = theme::centered(area, 64, 6);
    f.render_widget(Clear, card);
    let mut hint = vec![Span::raw(" ")];
    hint.extend(theme::key_hint("y", "yes"));
    hint.push(Span::styled(
        "any other key: no",
        Style::new().fg(theme::MUTED),
    ));
    let lines = vec![
        Line::raw(format!(" {}", single_line(&c.prompt))),
        Line::default(),
        Line::from(hint),
    ];
    f.render_widget(
        Paragraph::new(lines).block(theme::pane("confirm", true)),
        card,
    );
}

/// The help overlay's groups and the column (0–2) each goes in; every
/// action is in exactly one group.
const HELP_GROUPS: &[(&str, usize, &[Action])] = &[
    (
        "navigate",
        0,
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
        0,
        &[
            Action::FocusNext,
            Action::ScrollDown,
            Action::ScrollUp,
            Action::ToggleDiff,
            Action::MarkDiffBase,
            Action::Chart,
            Action::Stats,
        ],
    ),
    (
        "actions",
        1,
        &[
            Action::Search,
            Action::Publish,
            Action::CopyTopic,
            Action::CopyPayload,
            Action::Brokers,
            Action::Help,
            Action::Cancel,
            Action::Quit,
        ],
    ),
    (
        "retained",
        1,
        &[Action::DeleteRetained, Action::ClearRetained],
    ),
    (
        "tree",
        1,
        &[Action::CycleSort, Action::ExpandAll, Action::CollapseAll],
    ),
    (
        "picker",
        2,
        &[
            Action::AddProfile,
            Action::EditProfile,
            Action::DeleteProfile,
            Action::MoveUp,
            Action::MoveDown,
        ],
    ),
];

/// Keys that bypass the keymap, shown in the third help column.
const HELP_FIXED: &[(&str, &[(&str, &str)])] = &[
    (
        "publish form",
        &[
            ("ctrl-s", "send"),
            ("ctrl-e", "$EDITOR"),
            ("ctrl-p/n", "history"),
            ("ctrl-f", "format json"),
            ("tab", "next field"),
            ("esc", "close"),
        ],
    ),
    (
        "search",
        &[("ctrl-r", "regex on/off"), ("enter", "keep filter")],
    ),
];

/// Effective keymap, grouped, in two columns; keys that bypass the keymap
/// in a third.
fn render_help(f: &mut Frame, area: Rect, app: &App) {
    let bindings = app.keymap.bindings();
    debug_assert!(
        bindings
            .iter()
            .all(|(a, _)| { HELP_GROUPS.iter().filter(|(_, _, g)| g.contains(a)).count() == 1 })
    );
    theme::dim_backdrop(f, area);
    let area = theme::centered(area, 78, 22);
    let block = theme::pane("help · any key closes", true).title_bottom(
        Line::styled(
            " shift-drag selects text · remap in settings.toml [keys] ",
            Style::new().fg(theme::MUTED),
        )
        .right_aligned(),
    );
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let cols: [Rect; 3] = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .areas(inner);
    let heading = Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD);
    let key_style = Style::new().add_modifier(Modifier::BOLD);
    let row = |name: &str, keys: String, pad: usize| {
        Line::from(vec![
            Span::styled(format!(" {name:<pad$}"), Style::new().fg(theme::MUTED)),
            Span::styled(keys, key_style),
        ])
    };
    // Longest action name plus a space, so names never run into keys.
    let name_w = bindings
        .iter()
        .map(|(a, _)| a.name().len() + 1)
        .max()
        .unwrap_or(15);
    let mut columns: [Vec<Line>; 3] = Default::default();
    for (title, col, actions) in HELP_GROUPS {
        let lines = &mut columns[*col];
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(format!(" {title}"), heading));
        for action in *actions {
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
            lines.push(row(action.name(), keys, name_w));
        }
    }
    for (title, keys) in HELP_FIXED {
        let lines = &mut columns[2];
        if !lines.is_empty() {
            lines.push(Line::default());
        }
        lines.push(Line::styled(format!(" {title}"), heading));
        for (k, what) in *keys {
            lines.push(row(k, (*what).into(), 9));
        }
    }
    for (lines, col) in columns.into_iter().zip(cols) {
        f.render_widget(Paragraph::new(lines), col);
    }
}

fn render_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Tree;
    app.hit.set(HitAreas {
        tree: area,
        ..app.hit.get()
    });
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
    let query = app.filter.as_ref();
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(app.tree_offset)
        .take(height)
        .map(|(i, row)| {
            let flash = app.is_flashing(row.id, now);
            let line = Line::from(row_spans(&app.tree, row, query, width, flash));
            if Some(i) == selected {
                theme::select(line, focused)
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
    query: Option<&Filter>,
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
    if node.retained {
        spans.push(Span::styled(" ®", Style::new().fg(theme::MUTED)));
    }
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
fn highlight<'a>(out: &mut Vec<Span<'a>>, text: String, query: Option<&Filter>, base: Style) {
    let ranges = query.map(|f| f.ranges(&text)).unwrap_or_default();
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
            (k(Action::Help), "help"),
            (k(Action::Quit), "quit"),
            (k(Action::FocusNext), "focus"),
            (k(Action::Select), "expand"),
        ],
        (Mode::Browse, Focus::Payload) => vec![
            (jk, "scroll"),
            (k(Action::ToggleDiff), "diff"),
            (k(Action::FocusNext), "focus"),
            (k(Action::Help), "help"),
            (k(Action::Quit), "quit"),
        ],
        (Mode::Browse, Focus::History) => vec![
            (jk, "older/newer"),
            (k(Action::ToggleDiff), "diff"),
            (k(Action::MarkDiffBase), "mark"),
            (k(Action::FocusNext), "focus"),
            (k(Action::Help), "help"),
            (k(Action::Quit), "quit"),
        ],
        (Mode::Picker, _) => vec![
            (k(Action::Select), "connect"),
            (k(Action::AddProfile), "add"),
            (k(Action::EditProfile), "edit"),
            (k(Action::DeleteProfile), "delete"),
            (jk, "move"),
            (
                format!("{}/{}", k(Action::MoveDown), k(Action::MoveUp)),
                "reorder",
            ),
            (k(Action::CycleSort), "sort"),
        ]
        .into_iter()
        .chain(app.in_session().then(|| (k(Action::Cancel), "back")))
        .chain([(k(Action::Quit), "quit")])
        .collect(),
        (Mode::Password, _) if app.picker.profiles.is_empty() => {
            vec![("enter".into(), "connect"), ("esc".into(), "quit")]
        }
        (Mode::Password, _) => vec![("enter".into(), "connect"), ("esc".into(), "back")],
        (Mode::ProfileForm, _) => vec![
            ("ctrl-s".into(), "save"),
            ("tab".into(), "next"),
            ("space".into(), "change"),
            ("esc".into(), "cancel"),
        ],
        (Mode::Confirm, _) => vec![("y".into(), "confirm"), ("esc".into(), "cancel")],
        (Mode::Publish, _) => vec![
            ("ctrl-s".into(), "send"),
            ("ctrl-e".into(), "editor"),
            ("ctrl-p/n".into(), "history"),
            ("ctrl-f".into(), "format json"),
            ("tab".into(), "next"),
            ("esc".into(), "close"),
        ],
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
                if app.search_regex { "regex/" } else { "/" },
                Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{}▏", single_line(&app.search))),
        ];
        let mut right = Vec::new();
        push_hints(
            &mut right,
            &[
                (
                    "ctrl-r".into(),
                    if app.search_regex { "plain" } else { "regex" },
                ),
                ("enter".into(), "keep"),
                ("esc".into(), "clear"),
            ],
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
    let front = matches!(app.mode, Mode::Picker | Mode::Password);
    if let Some(e) = app.search_error.as_ref().filter(|_| !front) {
        let chip = format!(" regex error: {} ", single_line(e));
        spans.push(Span::styled(
            ellipsize(&chip, width),
            Style::new().fg(Color::Black).bg(theme::ERR),
        ));
        spans.push(Span::raw(" "));
    } else if let Some(f) = app.filter.as_ref().filter(|_| !front) {
        let label = match f.mode {
            QueryMode::Plain => "filter",
            QueryMode::Regex => "regex",
        };
        let chip = format!(" {label}: {} ({}) ", single_line(&f.query), f.matches.len());
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

/// Cut `s` to at most `max` display columns, ending in `…` when it was cut.
fn ellipsize(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// ` mqtt-zapp  ● connected  name  url ……… rate · topics`. When the line is
/// too wide the url goes first, then the stats, then the name; the state
/// pill is cut last. Also returns where the broker name was drawn (x, width),
/// a click target for the picker.
fn header_line(app: &App, width: u16) -> (Line<'static>, Option<(u16, u16)>) {
    let width = usize::from(width);
    let badge = Span::styled(
        " mqtt-zapp ",
        Style::new()
            .fg(Color::Black)
            .bg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    );
    // (prefix, reason, suffix): only the reason is ever cut.
    let (prefix, reason, suffix, color) = match &app.conn_state {
        ConnState::Connected => ("● connected", String::new(), String::new(), theme::OK),
        ConnState::Connecting => ("◌ connecting", String::new(), String::new(), theme::WARN),
        ConnState::Disconnected {
            reason,
            retry_in_ms,
        } => (
            "✕ ",
            single_line(reason),
            if *retry_in_ms == 0 {
                String::new()
            } else {
                format!(" · retry {}s", retry_in_ms.div_ceil(1000))
            },
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
    let pill_w = prefix.width() + reason.width() + suffix.width();
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
    let room = width.saturating_sub(fixed + prefix.width() + suffix.width());
    let state = format!("{prefix}{}{suffix}", ellipsize(&reason, room));
    let mut spans = vec![badge, Span::raw(" "), Span::styled(state, pill_style)];
    let broker = name.as_ref().map(|n| {
        let x = spans_width(&spans) + n[0].width();
        (x as u16, n[1].width() as u16)
    });
    spans.extend(name.into_iter().flatten());
    spans.extend(url.into_iter().flatten());
    if let Some(stats) = stats {
        let pad = width.saturating_sub(spans_width(&spans) + stats.width());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(stats);
    }
    (Line::from(spans), broker)
}

#[cfg(test)]
mod tests;
