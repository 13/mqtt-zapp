//! The footer hint bar and the header line.

use super::*;

/// Shortest key bound to `action` (`j` over `down`), or `?` when unbound.
pub(in crate::tui) fn key_for(app: &App, action: Action) -> String {
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
pub(super) fn hints(app: &App) -> Vec<(String, &'static str)> {
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
pub(super) fn push_hints(spans: &mut Vec<Span<'static>>, hints: &[(String, &str)], width: usize) {
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
pub(super) fn footer_line(app: &App, width: u16) -> Line<'static> {
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
pub(super) fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Cut `s` to at most `max` display columns, ending in `…` when it was cut.
pub(super) fn ellipsize(s: &str, max: usize) -> String {
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

/// Header cells a disconnect reason keeps before anything else is cut.
pub(super) const REASON_MIN: usize = 12;

/// ` mqtt-zapp  ● connected  name  url ……… rate · topics`. When the line is
/// too wide the url goes first, then the stats, then the name; the state
/// pill is cut last. Also returns where the broker name was drawn (x, width),
/// a click target for the picker.
pub(super) fn header_line(app: &App, width: u16) -> (Line<'static>, Option<(u16, u16)>) {
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
    // Security warning: right after the URL, cut only after everything else.
    let mut security = app
        .server
        .as_ref()
        .and_then(|s| s.warning)
        .map(|w| Span::styled(format!("  ⚠ {w}"), Style::new().fg(theme::ERR)));
    let mut stats = Some(Span::styled(
        format!(
            "{:.1} msg/s · {} topics ",
            app.msg_rate.per_sec(Instant::now()),
            app.tree.node(TopicTree::ROOT).topic_count
        ),
        muted,
    ));
    // Warnings after the pill, joined with " · "; the first part cut.
    let mut warnings = Vec::new();
    if app.dropped > 0 {
        warnings.push(format!("⚠ {} dropped", app.dropped));
    }
    if app.tree.dropped_topics() > 0 {
        warnings.push(format!("⚠ {} topics refused", app.tree.dropped_topics()));
    }
    let mut warn = (!warnings.is_empty()).then(|| {
        Span::styled(
            format!(" · {}", warnings.join(" · ")),
            Style::new().fg(theme::WARN),
        )
    });
    // Frozen: never cut, the user must see why nothing moves.
    let frozen = app
        .frozen
        .then(|| Span::styled(" · ❄ frozen", Style::new().fg(theme::WARN)));
    let fixed = badge.width() + 1 + frozen.as_ref().map_or(0, Span::width);
    // The cuts count the reason at most `REASON_MIN` wide: it is ellipsized
    // into whatever is left, so a long one must not push the rest out.
    let reason_w = reason.width().min(REASON_MIN);
    let pill_w = prefix.width() + reason_w + suffix.width();
    let total = |name: &Option<Vec<Span>>,
                 url: &Option<Vec<Span>>,
                 stats: &Option<Span>,
                 warn: &Option<Span>,
                 security: &Option<Span>| {
        fixed
            + pill_w
            + security.as_ref().map_or(0, Span::width)
            + warn.as_ref().map_or(0, Span::width)
            + name.as_deref().map_or(0, spans_width)
            + url.as_deref().map_or(0, spans_width)
            + stats.as_ref().map_or(0, |s| s.width() + 2)
    };
    if total(&name, &url, &stats, &warn, &security) > width {
        warn = None;
    }
    if total(&name, &url, &stats, &warn, &security) > width {
        url = None;
    }
    if total(&name, &url, &stats, &warn, &security) > width {
        stats = None;
    }
    if total(&name, &url, &stats, &warn, &security) > width {
        name = None;
    }
    if total(&name, &url, &stats, &warn, &security) > width {
        security = None;
    }
    let room = width.saturating_sub(total(&name, &url, &stats, &warn, &security) - reason_w);
    let state = format!("{prefix}{}{suffix}", ellipsize(&reason, room));
    let mut spans = vec![badge, Span::raw(" "), Span::styled(state, pill_style)];
    spans.extend(frozen);
    spans.extend(warn);
    let broker = name.as_ref().map(|n| {
        let x = spans_width(&spans) + n[0].width();
        (x as u16, n[1].width() as u16)
    });
    spans.extend(name.into_iter().flatten());
    spans.extend(url.into_iter().flatten());
    spans.extend(security);
    if let Some(stats) = stats {
        let pad = width.saturating_sub(spans_width(&spans) + stats.width());
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(stats);
    }
    (Line::from(spans), broker)
}
