//! Right-hand pane: metadata line, payload (pretty, coloured, or diffed
//! against the previous message) and the history list. Everything that came
//! from the broker is sanitized before it is drawn.

use super::app::{App, Focus, HitAreas};
use super::keymap::Action;
use super::theme;
use chrono::{DateTime, Local};
use mqtt_zapp_core::diff::{ChangeKind, Diff, LineTag, diff};
use mqtt_zapp_core::payload::{PayloadKind, detect, pretty_lines, preview, sanitize};
use mqtt_zapp_core::{Message, TopicNode};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Payloads above this are not diffed (the diff is quadratic-ish in lines).
const MAX_DIFF_BYTES: usize = 256 * 1024;

const HINT: &str = "select a topic with a message";
const HISTORY_PREVIEW: usize = 40;
const HISTORY_ROWS: usize = 6;

type Stamp = (SystemTime, usize);
type CacheKey = (usize, Stamp, Option<Stamp>, bool);
/// The built payload area plus the shown message's kind label (`JSON`, ...).
type Built = (Rc<Body>, &'static str);

/// The last computed `Body`, reused until the shown message(s) or the diff
/// toggle change, so a frame costs nothing for big payloads.
#[derive(Default)]
pub struct BodyCache {
    slot: RefCell<Option<(CacheKey, Built)>>,
    #[cfg(test)]
    builds: std::cell::Cell<u32>,
}

impl BodyCache {
    pub fn clear(&self) {
        *self.slot.borrow_mut() = None;
    }

    #[cfg(test)]
    pub fn is_cached(&self) -> bool {
        self.slot.borrow().is_some()
    }
}

/// What the payload area shows.
enum Body {
    Json(Vec<String>),
    Plain(Vec<String>),
    Diff(Vec<(Color, String)>),
    Note(String),
}

impl Body {
    fn len(&self) -> usize {
        match self {
            Body::Json(l) | Body::Plain(l) => l.len(),
            Body::Diff(l) => l.len(),
            Body::Note(_) => 1,
        }
    }
}

/// The message to show and the one before it, if any.
struct Shown<'a> {
    node: &'a TopicNode,
    msg: &'a Message,
    prev: Option<&'a Message>,
    /// Index into `node.history` of `msg`, when there is a history.
    idx: Option<usize>,
    /// Index into `node.history` of the marked diff base (`m`), if any.
    base: Option<usize>,
}

fn shown(app: &App) -> Option<Shown<'_>> {
    let node = app.tree.node(app.selected?);
    let h = &node.history;
    if h.is_empty() {
        let msg = node.latest.as_ref()?;
        return Some(Shown {
            node,
            msg,
            prev: None,
            idx: None,
            base: None,
        });
    }
    // The index may be stale; never trust it.
    let i = app.history_sel.map_or(h.len() - 1, |i| i.min(h.len() - 1));
    let stamp = |m: &Message| (m.received_at, m.payload.len());
    let base = app
        .diff_base
        .and_then(|b| h.iter().position(|m| stamp(m) == b));
    Some(Shown {
        node,
        msg: &h[i],
        prev: match base {
            Some(b) => Some(&h[b]),
            None => i.checked_sub(1).map(|p| &h[p]),
        },
        idx: Some(i),
        base,
    })
}

/// The message the payload area shows, if any.
pub fn shown_message(app: &App) -> Option<&Message> {
    shown(app).map(|s| s.msg)
}

fn body(app: &App) -> Option<Built> {
    let s = shown(app)?;
    let stamp = |m: &Message| (m.received_at, m.payload.len());
    let key = (
        app.selected?,
        stamp(s.msg),
        s.prev.map(stamp),
        app.show_diff,
    );
    if let Some((k, (b, kind))) = &*app.body_cache.slot.borrow()
        && *k == key
    {
        return Some((Rc::clone(b), kind));
    }
    #[cfg(test)]
    app.body_cache.builds.set(app.body_cache.builds.get() + 1);
    let kind = detect(&s.msg.payload);
    let label = match kind {
        PayloadKind::Json(_) => "JSON",
        PayloadKind::Text(_) => "TEXT",
        PayloadKind::Binary => "BINARY",
    };
    let b = Rc::new(build_body(&s, kind, app.show_diff));
    *app.body_cache.slot.borrow_mut() = Some((key, (Rc::clone(&b), label)));
    Some((b, label))
}

fn build_body(s: &Shown, kind: PayloadKind, show_diff: bool) -> Body {
    if !show_diff {
        let lines = pretty_lines(&kind, &s.msg.payload);
        return match kind {
            PayloadKind::Json(_) => Body::Json(lines),
            _ => Body::Plain(lines),
        };
    }
    let Some(prev) = s.prev else {
        return Body::Note("no previous message".into());
    };
    let size = prev.payload.len().max(s.msg.payload.len());
    if size > MAX_DIFF_BYTES {
        return Body::Note(format!("payload too large to diff ({size} bytes)"));
    }
    match diff(&prev.payload, &s.msg.payload) {
        Diff::Binary { equal: true } => Body::Note("binary payloads are identical".into()),
        Diff::Binary { equal: false } => Body::Note("binary payloads differ".into()),
        Diff::Json(changes) if changes.is_empty() => Body::Note("no changes".into()),
        Diff::Json(changes) => Body::Diff(
            changes
                .iter()
                .map(|c| {
                    let (old, new) = (
                        c.old.as_deref().unwrap_or(""),
                        c.new.as_deref().unwrap_or(""),
                    );
                    match c.kind {
                        ChangeKind::Changed => {
                            (Color::Yellow, format!("~ {}: {old} → {new}", c.path))
                        }
                        ChangeKind::Added => (Color::Green, format!("+ {}: {new}", c.path)),
                        ChangeKind::Removed => (Color::Red, format!("- {}: {old}", c.path)),
                    }
                })
                .map(|(c, t)| (c, one_line(&t)))
                .collect(),
        ),
        Diff::Text(changes) => Body::Diff(
            changes
                .iter()
                .map(|c| match c.tag {
                    LineTag::Same => (Color::Reset, format!("  {}", one_line(&c.text))),
                    LineTag::Added => (Color::Green, format!("+ {}", one_line(&c.text))),
                    LineTag::Removed => (Color::Red, format!("- {}", one_line(&c.text))),
                })
                .collect(),
        ),
    }
}

/// Number of lines the payload area scrolls over (for clamping).
pub fn content_len(app: &App) -> usize {
    body(app).map_or(0, |(b, _)| b.len())
}

fn one_line(s: &str) -> String {
    sanitize(s).replace('\n', "\u{23ce}")
}

fn clock(t: SystemTime, fmt: &str) -> String {
    DateTime::<Local>::from(t).format(fmt).to_string()
}

/// `512 B`, `1.5 KiB`, `5.0 MiB`.
fn human_bytes(n: usize) -> String {
    const KIB: f64 = 1024.0;
    match n as f64 {
        x if x < KIB => format!("{n} B"),
        x if x < KIB * KIB => format!("{:.1} KiB", x / KIB),
        x => format!("{:.1} MiB", x / (KIB * KIB)),
    }
}

/// Below this many rows (or with the panes stacked) the header is one row.
pub const HEADER_CARD_MIN_HEIGHT: u16 = 14;

/// `now`, `12s ago`, `4m ago`, `3h ago`, `2d ago`. A time in the future
/// (clock skew) is `now`.
pub fn age(received: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(received).map_or(0, |d| d.as_secs());
    match secs {
        0 => "now".into(),
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// Keep `max` display columns of `s`, from the end (`true`) or the start.
fn take_width(s: &str, max: usize, from_end: bool) -> String {
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
fn fit_topic(topic: &str, width: usize) -> (String, String) {
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

/// The copy buttons on the header card and the payload pane's title.
const COPY_LABEL: &str = " ⧉ copy ";
/// The one-row header's copy button.
const COMPACT_COPY: &str = " ⧉ ";

/// Between topic levels in the header: the topic as-is, `muh/wst/status`.
const SEP: &str = "/";

/// The last topic level, right after its `/`: bold, in the accent colour.
fn leaf_span(leaf: &str) -> Span<'static> {
    Span::styled(
        leaf.to_owned(),
        Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
    )
}

/// `JSON` `5 B` `QoS 1` [`RETAINED`] badges.
fn badges(s: &Shown, kind: &str) -> Vec<Span<'static>> {
    let chip = Style::new()
        .fg(theme::MUTED)
        .add_modifier(Modifier::REVERSED);
    let mut v = vec![
        Span::styled(
            format!(" {kind} "),
            Style::new().fg(Color::Black).bg(theme::ACCENT),
        ),
        Span::raw(" "),
        Span::styled(format!(" {} ", human_bytes(s.msg.payload.len())), chip),
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

fn width_of(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// The card's title, ` muh/sensors/200/json `, fitted into `room` columns
/// (padding included); empty when there is no room for any of it.
fn card_title(topic: &str, room: usize) -> Line<'static> {
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
fn header_lines(
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
fn time_spans(clock: &str, age: &str) -> Vec<Span<'static>> {
    vec![
        Span::raw(clock.to_owned()),
        Span::styled(" · ", Style::new().fg(theme::MUTED)),
        Span::raw(age.to_owned()),
    ]
}

/// One row: path with the leaf highlighted, the badges, `time · age`.
fn header_compact(
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

pub fn render_message(f: &mut Frame, area: Rect, app: &App, stacked: bool) {
    let Some(s) = shown(app) else {
        // Payload and history focus both land here while there is nothing
        // to show, so the focus stays visible.
        let block = theme::pane("message", app.focus != Focus::Tree);
        app.hit.set(HitAreas {
            payload: area,
            header_topic: Rect::default(),
            header_copy: Rect::default(),
            payload_copy: Rect::default(),
            ..app.hit.get()
        });
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [_, mid] = Layout::vertical([
            Constraint::Length(inner.height.saturating_sub(1) / 2),
            Constraint::Min(0),
        ])
        .areas(inner);
        let hint = Paragraph::new(HINT)
            .style(Style::new().fg(theme::MUTED))
            .alignment(Alignment::Center);
        f.render_widget(hint, mid);
        return;
    };
    // History takes at most half of what the meta line leaves, so the
    // payload keeps room in short (stacked) layouts; with history focused it
    // keeps at least one row so the focus stays visible.
    let card = !stacked && area.height >= HEADER_CARD_MIN_HEIGHT;
    let header_rows = if card { 3 } else { 1 };
    let half = area.height.saturating_sub(header_rows) / 2;
    let mut hist_rows = (s.node.history.len().min(HISTORY_ROWS) as u16).min(half.saturating_sub(2));
    if app.focus == Focus::History {
        hist_rows = hist_rows.max(1);
    }
    let hist_height = if hist_rows == 0 { 0 } else { hist_rows + 2 };
    let [meta, payload, history] = Layout::vertical([
        Constraint::Length(header_rows),
        Constraint::Min(0),
        Constraint::Length(hist_height),
    ])
    .areas(area);

    let topic = app.tree.full_topic(app.selected.unwrap_or_default());
    let kind = body(app).map_or("", |(_, k)| k);
    let now = SystemTime::now();
    if card {
        let copy_w = COPY_LABEL.width() as u16;
        // Corners, one `─` between the two titles, the copy label.
        let room = usize::from(meta.width).saturating_sub(2 + 1 + usize::from(copy_w));
        let title = card_title(&topic, room);
        let title_w = title.width() as u16;
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::MUTED))
            .title(title)
            .title(Line::styled(COPY_LABEL, Style::new().fg(theme::MUTED)).right_aligned());
        let inner = block.inner(meta);
        let row = header_lines(&s, kind, &app.timestamp_format, now, inner.width);
        app.hit.set(HitAreas {
            header_topic: Rect {
                x: meta.x + 1,
                y: meta.y,
                width: title_w,
                height: 1,
            },
            header_copy: Rect {
                x: (meta.x + meta.width).saturating_sub(1 + copy_w),
                y: meta.y,
                width: copy_w.min(meta.width.saturating_sub(2)),
                height: 1,
            },
            ..app.hit.get()
        });
        f.render_widget(Paragraph::new(row).block(block), meta);
    } else {
        // ` ⧉ ` at the right end; the row is cut short of it.
        let glyph_w = COMPACT_COPY.width() as u16;
        let [row_area, glyph] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(glyph_w)]).areas(meta);
        let row = header_compact(&s, &topic, kind, &app.timestamp_format, now, row_area.width);
        // The path is the row's first two spans (parents, leaf).
        let path_w: usize = row.spans.iter().take(2).map(|s| s.content.width()).sum();
        app.hit.set(HitAreas {
            header_topic: Rect {
                width: (path_w as u16).min(row_area.width),
                ..row_area
            },
            header_copy: glyph,
            ..app.hit.get()
        });
        f.render_widget(Paragraph::new(row), row_area);
        f.render_widget(
            Paragraph::new(Span::styled(COMPACT_COPY, Style::new().fg(theme::MUTED))),
            glyph,
        );
    }
    let payload = match &app.chart {
        Some(series) => {
            let [chart, rest] =
                Layout::vertical([Constraint::Min(5), Constraint::Percentage(50)]).areas(payload);
            render_chart(f, chart, app, &s, series.as_deref());
            rest
        }
        None => payload,
    };
    render_payload(f, payload, app);
    render_history(f, history, app, &s);
}

type ChartKey = (usize, usize, Option<Stamp>, Option<String>);

/// Chart points, shared between the cache and the frame being drawn.
type Points = Rc<Vec<(f64, f64)>>;

/// The last computed chart points, reused until the topic, its history or
/// the series change: parsing every history entry each frame is costly for
/// big JSON payloads.
#[derive(Default)]
pub struct ChartCache {
    slot: RefCell<Option<(ChartKey, Points)>>,
    #[cfg(test)]
    pub builds: std::cell::Cell<u32>,
}

/// (history index, value) for each history entry with a number.
fn chart_points(app: &App, s: &Shown, path: Option<&str>) -> Points {
    use mqtt_zapp_core::payload::numeric_at;
    let h = &s.node.history;
    let key = (
        app.selected.unwrap_or_default(),
        h.len(),
        h.back().map(|m| (m.received_at, m.payload.len())),
        path.map(str::to_owned),
    );
    if let Some((k, p)) = &*app.chart_cache.slot.borrow()
        && *k == key
    {
        return Rc::clone(p);
    }
    #[cfg(test)]
    app.chart_cache.builds.set(app.chart_cache.builds.get() + 1);
    let points: Points = Rc::new(
        h.iter()
            .enumerate()
            .filter_map(|(i, m)| numeric_at(&m.payload, path).map(|y| (i as f64, y)))
            .collect(),
    );
    *app.chart_cache.slot.borrow_mut() = Some((key, Rc::clone(&points)));
    points
}

/// `12.50` → `12.5`, `3.00` → `3`.
fn fmt_num(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A braille line chart of the series over the topic's history (oldest
/// left); entries without a number are skipped.
fn render_chart(f: &mut Frame, area: Rect, app: &App, s: &Shown, path: Option<&str>) {
    use ratatui::symbols::Marker;
    use ratatui::widgets::{Axis, Chart, Dataset, GraphType};
    let name = path.unwrap_or("value");
    let block = theme::pane(&format!("chart · {name}"), false);
    let points = chart_points(app, s, path);
    let points = &*points;
    if points.len() < 2 {
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new("not enough numeric values").style(Style::new().fg(theme::MUTED)),
            inner,
        );
        return;
    }
    let (lo, hi) = points
        .iter()
        .fold((f64::MAX, f64::MIN), |(lo, hi), &(_, y)| {
            (lo.min(y), hi.max(y))
        });
    let pad = if hi > lo { (hi - lo) * 0.05 } else { 1.0 };
    let x_max = points.last().map_or(1.0, |p| p.0).max(1.0);
    let data = [Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(theme::ACCENT))
        .data(points)];
    let muted = Style::new().fg(theme::MUTED);
    let chart = Chart::new(data.to_vec())
        .block(block)
        .x_axis(Axis::default().bounds([0.0, x_max]))
        .y_axis(Axis::default().bounds([lo - pad, hi + pad]).labels([
            Span::styled(fmt_num(lo), muted),
            Span::styled(fmt_num(hi), muted),
        ]));
    f.render_widget(chart, area);
}

fn render_payload(f: &mut Frame, area: Rect, app: &App) {
    let base_time = shown(app)
        .and_then(|s| s.base.map(|b| s.node.history[b].received_at))
        .map(|t| clock(t, &app.timestamp_format));
    let title = match (app.show_diff, base_time) {
        (true, Some(t)) => format!("diff vs {t}"),
        (true, None) => "diff".to_owned(),
        (false, _) => "payload".to_owned(),
    };
    let title = title.as_str();
    app.hit.set(HitAreas {
        payload: area,
        ..app.hit.get()
    });
    let mut block = theme::pane(title, app.focus == Focus::Payload);
    let height = block.inner(area).height;
    app.payload_height.set(height);
    let Some((body, _)) = body(app) else {
        app.hit.set(HitAreas {
            payload_copy: Rect::default(),
            ..app.hit.get()
        });
        return;
    };
    let body = &*body;
    let focused = app.focus == Focus::Payload;
    let copy_w = COPY_LABEL.width() as u16;
    block = block.title(
        Line::styled(
            COPY_LABEL,
            Style::new().fg(if focused { theme::ACCENT } else { theme::MUTED }),
        )
        .right_aligned(),
    );
    app.hit.set(HitAreas {
        payload_copy: Rect {
            x: (area.x + area.width).saturating_sub(1 + copy_w),
            y: area.y,
            width: copy_w.min(area.width.saturating_sub(2)),
            height: 1,
        },
        ..app.hit.get()
    });
    // The stored scroll may predate a resize or a new message.
    let max = body.len().saturating_sub(height as usize);
    let skip = app.payload_scroll.min(max);
    let take = height as usize;
    if body.len() > take {
        let last = (skip + take).min(body.len());
        block = block.title_bottom(
            Line::styled(
                format!(" {}-{last}/{} ", skip + 1, body.len()),
                Style::new().fg(theme::MUTED),
            )
            .right_aligned(),
        );
    }
    let lines: Vec<Line> = match body {
        Body::Json(l) => l
            .iter()
            .skip(skip)
            .take(take)
            .map(|l| json_line(l))
            .collect(),
        Body::Plain(l) => l
            .iter()
            .skip(skip)
            .take(take)
            .map(|l| Line::raw(l.as_str()))
            .collect(),
        Body::Diff(l) => l
            .iter()
            .skip(skip)
            .take(take)
            .map(|(c, t)| Line::styled(t.as_str(), Style::new().fg(*c)))
            .collect(),
        Body::Note(n) => vec![Line::styled(
            n.as_str(),
            Style::new().add_modifier(Modifier::DIM),
        )],
    };
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_history(f: &mut Frame, area: Rect, app: &App, s: &Shown) {
    let title = match app.history_sel {
        None => "history · live".to_owned(),
        Some(_) => format!(
            "history · paused ({} to newest)",
            super::ui::key_for(app, Action::Down)
        ),
    };
    let focused = app.focus == Focus::History;
    let block = theme::pane(&title, focused);
    let height = block.inner(area).height as usize;
    let h = &s.node.history;
    // Newest first: row `r` is history index `len - 1 - r`.
    let sel_row = s.idx.map(|i| h.len() - 1 - i);
    let offset = sel_row.map_or(0, |r| (r + 1).saturating_sub(height));
    app.hit.set(HitAreas {
        history: area,
        history_offset: offset,
        ..app.hit.get()
    });
    let lines: Vec<Line> = h
        .iter()
        .rev()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(row, m)| {
            let marked = s.base == Some(h.len() - 1 - row);
            let line = Line::from(vec![
                Span::styled(
                    clock(m.received_at, "%H:%M:%S"),
                    Style::new().fg(theme::MUTED),
                ),
                Span::styled(
                    if marked { " ◆ " } else { "  " },
                    Style::new().fg(theme::ACCENT),
                ),
                Span::raw(preview(&m.payload, HISTORY_PREVIEW)),
            ]);
            if Some(row) == sel_row && app.history_sel.is_some() {
                theme::select(line, focused)
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Colour one line of pretty-printed JSON: keys cyan, strings green, numbers
/// yellow, booleans and null magenta.
fn json_line(line: &str) -> Line<'static> {
    let chars: Vec<char> = line.chars().collect();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let color = if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i = (i + 1).min(chars.len());
            let is_key = chars[i..].iter().find(|c| !c.is_whitespace()) == Some(&':');
            Some(if is_key { Color::Cyan } else { Color::Green })
        } else if c == '-' || c.is_ascii_digit() {
            while i < chars.len() && matches!(chars[i], '-' | '+' | '.' | 'e' | 'E' | '0'..='9') {
                i += 1;
            }
            Some(Color::Yellow)
        } else if c.is_ascii_alphabetic() {
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                i += 1;
            }
            Some(Color::Magenta)
        } else {
            i += 1;
            None
        };
        let text: String = chars[start..i].iter().collect();
        spans.push(match color {
            Some(c) => Span::styled(text, Style::new().fg(c)),
            None => Span::raw(text),
        });
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests;
