//! Overlays: the stats card, the confirm card and the help screen.

use super::*;

/// Live broker statistics from `$SYS`; redrawn every frame from the tree.
pub(super) fn render_stats(f: &mut Frame, area: Rect, app: &App) {
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

/// A yes/no card for a destructive action. The prompt wraps (a long topic
/// breaks after a `/`), and the card grows to fit it; on a short screen the
/// prompt is cut with `…` so the yes/no hint always shows.
pub(super) fn render_confirm(f: &mut Frame, area: Rect, c: &super::super::app::Confirm) {
    theme::dim_backdrop(f, area);
    let prompt = single_line(&c.prompt);
    // Borders and a space either side of the text.
    let want = u16::try_from(prompt.width() + 4).unwrap_or(u16::MAX);
    let width = want.clamp(CONFIRM_MIN_WIDTH, CONFIRM_MAX_WIDTH);
    let room = usize::from(theme::centered(area, width, 1).width.saturating_sub(4)).max(1);
    let mut text = wrap_words(&prompt, room);
    let want_h = u16::try_from(text.len() + 4).unwrap_or(u16::MAX);
    let card = theme::centered(area, width, want_h);
    // Inner rows: the prompt, a blank row if there is room, the hint.
    let inner = usize::from(card.height.saturating_sub(2));
    let fit = inner.saturating_sub(1).max(1);
    let gap = inner > text.len() + 1;
    if text.len() > fit {
        text.truncate(fit);
        if let Some(last) = text.last_mut() {
            *last = format!("{}…", fit_width(last, room.saturating_sub(1)));
        }
    }
    f.render_widget(Clear, card);
    let mut hint = vec![Span::raw(" ")];
    hint.extend(theme::key_hint("y", "yes"));
    hint.push(Span::styled(
        "any other key: no",
        Style::new().fg(theme::MUTED),
    ));
    let mut lines: Vec<Line> = text
        .into_iter()
        .map(|l| Line::raw(format!(" {l}")))
        .collect();
    if gap {
        lines.push(Line::default());
    }
    lines.push(Line::from(hint));
    f.render_widget(
        Paragraph::new(lines).block(theme::pane("confirm", true)),
        card,
    );
}

pub(super) const CONFIRM_MIN_WIDTH: u16 = 40;

pub(super) const CONFIRM_MAX_WIDTH: u16 = 80;

/// The longest prefix of `s` at most `width` columns wide.
pub(super) fn fit_width(s: &str, width: usize) -> &str {
    let mut w = 0;
    for (i, ch) in s.char_indices() {
        w += ch.width().unwrap_or(0);
        if w > width {
            return &s[..i];
        }
    }
    s
}

/// Wrap `s` into lines at most `width` columns wide, breaking at spaces; a
/// word too long for a line breaks after its last `/` that fits, else at
/// the width.
pub(super) fn wrap_words(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split(' ').filter(|w| !w.is_empty()) {
        let sep = usize::from(!line.is_empty());
        if line.width() + sep + word.width() <= width {
            if sep == 1 {
                line.push(' ');
            }
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        let mut rest = word;
        while rest.width() > width {
            let head = fit_width(rest, width);
            let cut = match head.rfind('/') {
                Some(i) if i > 0 => i + 1,
                _ if head.is_empty() => rest.chars().next().map_or(rest.len(), char::len_utf8),
                _ => head.len(),
            };
            out.push(rest[..cut].to_owned());
            rest = &rest[cut..];
        }
        line.push_str(rest);
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// The help overlay's groups and the column (0–2) each goes in; every
/// action is in exactly one group.
pub(super) const HELP_GROUPS: &[(&str, usize, &[Action])] = &[
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
            Action::ChartPin,
            Action::ChartPrev,
            Action::ChartPick,
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
            Action::Freeze,
            Action::Help,
            Action::Cancel,
            Action::Quit,
            Action::DeleteRetained,
            Action::ClearRetained,
            Action::DeleteTopic,
            Action::CycleSort,
            Action::ExpandAll,
            Action::CollapseAll,
            Action::Stats,
        ],
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
pub(super) const HELP_FIXED: &[(&str, &[(&str, &str)])] = &[
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
pub(super) fn render_help(f: &mut Frame, area: Rect, app: &App) {
    let bindings = app.keymap.bindings();
    debug_assert!(
        bindings
            .iter()
            .all(|(a, _)| { HELP_GROUPS.iter().filter(|(_, _, g)| g.contains(a)).count() == 1 })
    );
    theme::dim_backdrop(f, area);
    let area = theme::centered(area, 78, 22);
    let query = app.help_query.as_str();
    let key_text = |action: &Action| -> String {
        let keys = bindings
            .iter()
            .find(|(a, _)| a == action)
            .map(|(_, k)| k.iter().map(|k| k.to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        if keys.is_empty() {
            "(unbound)".to_owned()
        } else {
            keys.join(", ")
        }
    };
    let rows = HELP_GROUPS
        .iter()
        .flat_map(|(_, _, g)| g.iter().map(|a| (a.name(), key_text(a))))
        .chain(
            HELP_FIXED
                .iter()
                .flat_map(|(_, r)| r.iter().map(|(k, w)| (*k, (*w).to_owned()))),
        );
    let found = if query.is_empty() {
        0
    } else {
        rows.filter(|(n, k)| !help_matches(query, n, k).is_empty())
            .count()
    };
    let title = if query.is_empty() {
        "help · type to search · esc closes".to_owned()
    } else {
        format!(
            "help · /{} · {found} found · esc clears",
            single_line(query)
        )
    };
    let inner = theme::pane(&title, true).inner(area);
    let cols: [Rect; 3] = Layout::horizontal([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .areas(inner);
    let heading = Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD);
    let row = |name: &str, keys: String, pad: usize| {
        let muted = Style::new().fg(theme::MUTED);
        let mut spans = Vec::new();
        if query.is_empty() {
            spans.push(Span::styled(format!(" {name:<pad$}"), muted));
            spans.push(Span::styled(
                keys,
                Style::new().add_modifier(Modifier::BOLD),
            ));
        } else if help_matches(query, name, &keys).is_empty() {
            // Dimmed, but where it was.
            spans.push(Span::styled(format!(" {name:<pad$}"), muted));
            spans.push(Span::styled(keys, muted));
        } else {
            let accent = Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD);
            spans.push(Span::styled(" ", muted));
            highlight_ranges(&mut spans, name, &match_in(query, name), muted);
            spans.push(Span::styled(
                " ".repeat(pad.saturating_sub(name.chars().count())),
                muted,
            ));
            highlight_ranges(&mut spans, &keys, &match_in(query, &keys), accent);
        }
        Line::from(spans)
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
            lines.push(row(action.name(), key_text(action), name_w));
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
    let total = columns.iter().map(Vec::len).max().unwrap_or(0);
    let page = usize::from(inner.height);
    let max = total.saturating_sub(page);
    let skip = app.help_scroll.min(max);
    app.help_view.set((max, page.max(1)));
    let mut block = theme::pane(&title, true).title_bottom(
        Line::styled(
            " f freezes · shift-drag selects · remap in settings.toml ",
            Style::new().fg(theme::MUTED),
        )
        .right_aligned(),
    );
    if max > 0 {
        let last = (skip + page).min(total);
        block = block.title_bottom(
            Line::styled(
                format!(" {}-{last}/{total} ", skip + 1),
                Style::new().fg(theme::MUTED),
            )
            .left_aligned(),
        );
    }
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    for (lines, col) in columns.into_iter().zip(cols) {
        f.render_widget(Paragraph::new(lines).scroll((skip as u16, 0)), col);
    }
}

/// Non-overlapping byte ranges where `query` occurs in `text`, ignoring
/// ASCII case. One literal substring: spaces are part of the query, unlike
/// the multi-term tree search.
pub(in crate::tui) fn match_in(query: &str, text: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let (t, q) = (text.to_ascii_lowercase(), query.to_ascii_lowercase());
    t.match_indices(&q).map(|(i, m)| i..i + m.len()).collect()
}

/// Ranges of a help row's name and keys that match; empty when neither does.
pub(super) fn help_matches(query: &str, name: &str, keys: &str) -> Vec<std::ops::Range<usize>> {
    let mut r = match_in(query, name);
    r.extend(match_in(query, keys));
    r
}

/// Push `text` as spans, marking `ranges` with the search-match modifier.
pub(in crate::tui) fn highlight_ranges<'a>(
    out: &mut Vec<Span<'a>>,
    text: &str,
    ranges: &[std::ops::Range<usize>],
    base: Style,
) {
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
