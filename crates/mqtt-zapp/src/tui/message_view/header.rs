//! The message pane header: topic title, badges, time and copy button.

use super::*;

/// Keep `max` display columns of `s`, from the end (`true`) or the start.
pub(super) fn take_width(s: &str, max: usize, from_end: bool) -> String {
    let mut out = Vec::new();
    let mut w = 0;
    let chars: Vec<char> = if from_end {
        s.chars().rev().collect()
    } else {
        s.chars().collect()
    };
    for c in chars {
        let cw = c.width().unwrap_or(0);
        if w + cw > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    if from_end {
        out.reverse();
    }
    out.into_iter().collect()
}

/// Split `topic` into (parents with their trailing `/`, leaf), fitted so
/// that both take at most `width` columns. Whole middle levels go first
/// (`a/…/`), then all but `…/`; the leaf is cut (from the left) only once
/// there is no room for any parent.
pub(super) fn fit_topic(topic: &str, width: usize) -> (String, String) {
    let topic = one_line(topic);
    let mut levels: Vec<&str> = topic.split('/').collect();
    let leaf = levels.pop().unwrap_or_default().to_owned();
    let leaf_room = width;
    if width == 0 {
        return (String::new(), String::new());
    }
    if leaf.width() > leaf_room {
        return (
            String::new(),
            format!("…{}", take_width(&leaf, leaf_room.saturating_sub(1), true)),
        );
    }
    let room = leaf_room - leaf.width();
    let spaced = |ls: &[&str]| ls.iter().map(|l| format!("{l}{SEP}")).collect::<String>();
    let full = spaced(&levels);
    if full.width() <= room {
        return (full, leaf);
    }
    // `first / … / tail / `: whole middle levels go, the first stays and as
    // many of the last ones as fit.
    if levels.len() >= 2 {
        let head = format!("{}{SEP}…{SEP}", levels[0]);
        if head.width() <= room {
            let mut tail = String::new();
            for l in levels[2..].iter().rev() {
                let t = format!("{l}{SEP}{tail}");
                if head.width() + t.width() > room {
                    break;
                }
                tail = t;
            }
            return (head + &tail, leaf);
        }
    }
    let ellipsis = format!("…{SEP}");
    if ellipsis.width() <= room {
        return (ellipsis, leaf);
    }
    (String::new(), leaf)
}

/// The copy button for a `width`-column border whose left title needs
/// `title_w` columns and can shrink to `min_title_w`: `⧉ copy` when the
/// whole title fits beside it (corners and one `─` between), else `⧉` when
/// the shortest title does, else nothing (`""`): the title wins.
pub(super) fn copy_label(title_w: usize, min_title_w: usize, width: u16) -> &'static str {
    let fits = |title: usize, label: &str| 2 + title + 1 + label.width() <= usize::from(width);
    if fits(title_w, COPY_LABEL) {
        COPY_LABEL
    } else if fits(min_title_w, COMPACT_COPY) {
        COMPACT_COPY
    } else {
        ""
    }
}

/// Where a right-aligned `copy_w`-column label sits on the top border of
/// `area`; nothing when there is no label.
pub(super) fn copy_rect(area: Rect, copy_w: u16) -> Rect {
    if copy_w == 0 {
        return Rect::default();
    }
    Rect {
        x: (area.x + area.width).saturating_sub(1 + copy_w),
        y: area.y,
        width: copy_w.min(area.width.saturating_sub(2)),
        height: 1,
    }
}

/// The last topic level, right after its `/`: bold, in the accent colour.
pub(super) fn leaf_span(leaf: &str) -> Span<'static> {
    Span::styled(
        leaf.to_owned(),
        Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
    )
}

/// `JSON` `5 B` `QoS 1` [`RETAINED`] badges.
pub(super) fn badges(s: &Shown, kind: &str) -> Vec<Span<'static>> {
    let chip = Style::new()
        .fg(theme::MUTED)
        .add_modifier(Modifier::REVERSED);
    let mut v = vec![
        Span::styled(
            format!(" {kind} "),
            Style::new().fg(Color::Black).bg(theme::ACCENT),
        ),
        Span::raw(" "),
        Span::styled(
            format!(
                " {} ",
                human_bytes(s.msg.wire_len.unwrap_or(s.msg.payload.len()))
            ),
            chip,
        ),
        Span::raw(" "),
        Span::styled(format!(" QoS {} ", s.msg.qos), chip),
    ];
    if s.msg.retain {
        v.push(Span::raw(" "));
        v.push(Span::styled(
            " RETAINED ",
            Style::new().fg(Color::Black).bg(Color::Magenta),
        ));
    }
    v
}

/// The card's title, ` muh/sensors/200/json `, fitted into `room` columns
/// (padding included); empty when there is no room for any of it.
pub(super) fn card_title(topic: &str, room: usize) -> Line<'static> {
    if room < 3 {
        return Line::default();
    }
    let (parents, leaf) = fit_topic(topic, room - 2);
    Line::from(vec![
        Span::raw(" "),
        Span::styled(parents, theme::TEXT),
        leaf_span(&leaf),
        Span::raw(" "),
    ])
}

/// The card's inner row for a `width`-column interior: badges, `time · age`.
pub(super) fn header_lines(
    s: &Shown,
    kind: &str,
    time_fmt: &str,
    now: SystemTime,
    width: u16,
) -> Line<'static> {
    let width = width as usize;
    let mut left = vec![Span::raw(" ")];
    left.extend(badges(s, kind));
    let clock = clock(s.msg.received_at, time_fmt);
    let age = age(s.msg.received_at, now);
    // Right side: `time · age`, else just the age, else nothing.
    let used = width_of(&left);
    let right = [time_spans(&clock, &age), vec![Span::raw(age.clone())]]
        .into_iter()
        // A gap of at least one before it and the trailing space after it.
        .find(|r| used + 1 + width_of(r) < width)
        .unwrap_or_default();
    let gap = width.saturating_sub(used + width_of(&right) + 1);
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    left.push(Span::raw(" "));
    Line::from(left)
}

/// `clock · age`: both in the text colour, the dot muted.
pub(super) fn time_spans(clock: &str, age: &str) -> Vec<Span<'static>> {
    vec![
        Span::raw(clock.to_owned()),
        Span::styled(" · ", Style::new().fg(theme::MUTED)),
        Span::raw(age.to_owned()),
    ]
}

/// One row: path with the leaf highlighted, the badges, `time · age`.
pub(super) fn header_compact(
    s: &Shown,
    topic: &str,
    kind: &str,
    time_fmt: &str,
    now: SystemTime,
    width: u16,
) -> Line<'static> {
    let muted = Style::new().fg(theme::MUTED);
    let (parents, leaf) = fit_topic(topic, (width as usize / 2).max(8));
    let mut spans = vec![
        Span::styled(parents, muted),
        leaf_span(&leaf),
        Span::raw(" "),
    ];
    spans.extend(badges(s, kind));
    spans.push(Span::raw("  "));
    spans.extend(time_spans(
        &clock(s.msg.received_at, time_fmt),
        &age(s.msg.received_at, now),
    ));
    Line::from(spans)
}

/// The props row: chips joined by two spaces in the muted colour, filled
/// greedily into `width` columns; chips that do not fit become ` +N more`.
pub(super) fn props_line(props: &Props, width: u16) -> Line<'static> {
    let chips: Vec<String> = props.summary().iter().map(|c| one_line(c)).collect();
    let width = usize::from(width);
    // Leading and trailing space frame the row.
    let room = width.saturating_sub(2);
    let tail = |k: usize| {
        let more = chips.len() - k + props.dropped_user;
        if more > 0 {
            format!(" +{more} more")
        } else {
            String::new()
        }
    };
    let mut used = 0;
    let mut fit = 0;
    let mut best = 0;
    for (i, c) in chips.iter().enumerate() {
        used += c.width() + if i > 0 { 2 } else { 0 };
        if used > room {
            break;
        }
        fit = i + 1;
        if used + tail(fit).width() <= room {
            best = fit;
        }
    }
    // All chips fit with no tail; otherwise the most that leaves room for it.
    let k = if fit == chips.len() && props.dropped_user == 0 {
        fit
    } else {
        best
    };
    let mut text = format!(" {}{}", chips[..k].join("  "), tail(k));
    if k == 0 && !chips.is_empty() {
        // Not even the first chip fits beside the tail: cut it to fill the
        // room, counting it as shown.
        let rest = tail(1);
        let cut = take_width(&chips[0], room.saturating_sub(rest.width() + 1), false);
        text = format!(" {cut}…{rest}");
    } else if k == 0 && text.width() > width {
        text = tail(0).chars().take(width).collect();
    }
    Line::styled(text, Style::new().fg(theme::MUTED))
}
