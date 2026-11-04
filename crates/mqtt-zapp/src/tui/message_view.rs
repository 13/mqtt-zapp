//! Right-hand pane: metadata line, payload (pretty, coloured, or diffed
//! against the previous message) and the history list. Everything that came
//! from the broker is sanitized before it is drawn.

use super::app::{App, Focus, HitAreas, Mode};
use super::keymap::Action;
use super::theme;
use chrono::{DateTime, Local};
use mqtt_zapp_core::diff::{ChangeKind, Diff, LineTag, diff};
use mqtt_zapp_core::payload::{PayloadKind, Props, detect, pretty_lines, preview, sanitize};
use mqtt_zapp_core::{Message, TopicNode};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Payloads above this are not diffed (the diff is quadratic-ish in lines).
const MAX_DIFF_BYTES: usize = 256 * 1024;

/// A payload above this size is rebuilt at most this often while its topic
/// keeps receiving messages (the header still shows the newest one).
pub const BODY_MIN_INTERVAL: Duration = Duration::from_millis(250);
/// Payloads above this are subject to `BODY_MIN_INTERVAL`.
const BODY_THROTTLE_BYTES: usize = 64 * 1024;
const HINT: &str = "select a topic with a message";
const HISTORY_ROWS: usize = 6;
/// Payloads above this are shown as plain text, never parsed as JSON.
const PRETTY_MAX: usize = 1 << 20;

type Stamp = (SystemTime, usize);
/// What a body is built from: the shown messages, and the `Shape` of the view.
type CacheKey = (Shape, Stamp, Option<Stamp>);
/// Everything in the key except the messages: the topic, the diff toggle, the
/// history selection and the marked diff base. A change of any of these
/// rebuilds at once; only a new message on an unchanged shape may be throttled.
type Shape = (usize, bool, Option<usize>, Option<Stamp>);
/// The built payload area plus the shown message's kind label (`JSON`, ...).
type Built = (Rc<Body>, &'static str);

/// The last computed `Body`, reused until the shown message(s) or the diff
/// toggle change, so a frame costs nothing for big payloads.
#[derive(Default)]
pub struct BodyCache {
    slot: RefCell<Option<(CacheKey, Built, Instant)>>,
    #[cfg(test)]
    builds: std::cell::Cell<u32>,
}

impl BodyCache {
    #[cfg(test)]
    pub fn age(&self, by: Duration) {
        if let Some((_, _, at)) = self.slot.borrow_mut().as_mut() {
            *at = at.checked_sub(by).unwrap();
        }
    }

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
        let msg = node.latest()?;
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
    let shape: Shape = (app.selected?, app.show_diff, app.history_sel, app.diff_base);
    let key = (shape, stamp(s.msg), s.prev.map(stamp));
    if let Some((k, (b, kind), built)) = &*app.body_cache.slot.borrow()
        && (*k == key
            // Following the newest message of a busy topic: keep the body
            // of an earlier one for a moment. Same shape, so same topic.
            || (k.0 == shape
                && shape.2.is_none()
                // The chart and its clicks read the newest message.
                && app.chart.is_none()
                && s.msg.payload.len() > BODY_THROTTLE_BYTES
                && built.elapsed() < BODY_MIN_INTERVAL))
    {
        return Some((Rc::clone(b), kind));
    }
    #[cfg(test)]
    app.body_cache.builds.set(app.body_cache.builds.get() + 1);
    let kind = match detect_capped(&s.msg.payload) {
        // A cut payload shows as text even when its prefix parses as JSON.
        PayloadKind::Json(_) if s.msg.truncated_from.is_some() => {
            PayloadKind::Text(String::from_utf8_lossy(&s.msg.payload).into_owned())
        }
        k => k,
    };
    let label = match kind {
        _ if s.msg.sparkplug => "SPARKPLUG",
        PayloadKind::Json(_) => "JSON",
        PayloadKind::Text(_) => "TEXT",
        PayloadKind::Binary => "BINARY",
    };
    let b = Rc::new(build_body(&s, kind, app.show_diff));
    *app.body_cache.slot.borrow_mut() = Some((key, (Rc::clone(&b), label), Instant::now()));
    Some((b, label))
}

/// `detect`, except that a payload above `PRETTY_MAX` is never parsed as
/// JSON (a parse plus pretty-print of a huge payload would stall the UI):
/// it is text, or binary when it is not UTF-8 or holds a NUL.
fn detect_capped(payload: &[u8]) -> PayloadKind {
    if payload.len() <= PRETTY_MAX {
        return detect(payload);
    }
    match std::str::from_utf8(payload) {
        Ok(t) if !payload.contains(&0) => PayloadKind::Text(t.to_owned()),
        _ => PayloadKind::Binary,
    }
}

fn build_body(s: &Shown, kind: PayloadKind, show_diff: bool) -> Body {
    if !show_diff {
        let payload = &s.msg.payload;
        // A hex dump is one string per 16 bytes: bound it like the rest.
        let shown_len = match kind {
            PayloadKind::Binary => payload.len().min(PRETTY_MAX),
            _ => payload.len(),
        };
        let mut lines = pretty_lines(&kind, &payload[..shown_len]);
        if shown_len < payload.len() {
            lines.push(format!(
                "… {} more bytes not shown",
                payload.len() - shown_len
            ));
        }
        // `body` already made a cut JSON payload text; the marker is not JSON.
        return match (kind, s.msg.truncated_from) {
            (_, Some(n)) => {
                let what = if s.msg.sparkplug { " decoded" } else { "" };
                lines.push(format!("… truncated ({n} bytes{what})"));
                Body::Plain(lines)
            }
            (PayloadKind::Json(_), None) => Body::Json(lines),
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

/// The copy buttons on the header card and the payload pane's title.
const COPY_LABEL: &str = " ⧉ copy ";
/// The one-row header's copy button, and the borders' when the text does
/// not fit.
const COMPACT_COPY: &str = " ⧉ ";

/// The one-row header drops its ` ⧉ ` below this many columns.
const COMPACT_COPY_MIN_WIDTH: u16 = 19;

/// Between topic levels in the header: the topic as-is, `muh/wst/status`.
const SEP: &str = "/";

fn width_of(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Whether the header has a props row: when `msg` or any history entry has
/// properties. Keyed on the whole history so the boxes below do not move
/// while browsing it.
fn props_row_shown(node: &TopicNode, msg: &Message) -> bool {
    msg.props.is_some() || node.history.iter().any(|m| m.props.is_some())
}

/// The props row for `msg`: its chips, or a muted filler without any.
fn props_row(msg: &Message, width: u16) -> Line<'static> {
    match msg.props.as_deref() {
        Some(p) => props_line(p, width),
        None => Line::styled(" no properties", Style::new().fg(theme::MUTED)),
    }
}

/// Rows the history box gets in a message pane of `area`, before any
/// focus override: 0 means it is not drawn. History takes at most half of
/// what the meta line leaves, so the payload keeps room in short (stacked)
/// layouts.
fn history_rows(area: Rect, stacked: bool, entries: usize, props: bool) -> u16 {
    let card = !stacked && area.height >= HEADER_CARD_MIN_HEIGHT;
    let header_rows = if card { 3 } else { 1 } + u16::from(props);
    let half = area.height.saturating_sub(header_rows) / 2;
    (entries.min(HISTORY_ROWS) as u16).min(half.saturating_sub(2))
}

/// Whether the history box fits in the message pane `area`. If history has
/// focus but the box is not shown, focus moves to the payload.
pub fn settle_history_focus(app: &mut App, area: Rect, stacked: bool) {
    let fits = shown(app).is_some_and(|s| {
        history_rows(
            area,
            stacked,
            s.node.history.len(),
            props_row_shown(s.node, s.msg),
        ) > 0
    });
    app.history_shown.set(fits);
    if !fits && app.focus == Focus::History {
        app.focus = Focus::Payload;
    }
}

pub fn render_message(f: &mut Frame, area: Rect, app: &App, stacked: bool) {
    let Some(s) = shown(app) else {
        app.history_shown.set(false);
        app.hit.set(HitAreas {
            history: Rect::default(),
            ..app.hit.get()
        });
        // Payload and history focus both land here while there is nothing
        // to show, so the focus stays visible.
        let block = theme::pane(
            "message",
            !app.pane_focused(Focus::Tree) && app.mode != Mode::Publish,
        );
        app.hit.set(HitAreas {
            payload: area,
            header_topic: Rect::default(),
            header_copy: Rect::default(),
            payload_copy: Rect::default(),
            payload_title: Rect::default(),
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
    let card = !stacked && area.height >= HEADER_CARD_MIN_HEIGHT;
    let props = props_row_shown(s.node, s.msg);
    let header_rows = if card { 3 } else { 1 } + u16::from(props);
    let hist_rows = history_rows(area, stacked, s.node.history.len(), props);
    app.history_shown.set(hist_rows > 0);
    if hist_rows == 0 {
        app.hit.set(HitAreas {
            history: Rect::default(),
            ..app.hit.get()
        });
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
        let full = one_line(&topic);
        let leaf_w = full.rsplit('/').next().unwrap_or_default().width();
        let label = copy_label(full.width() + 2, leaf_w + 2, meta.width);
        let copy_w = label.width() as u16;
        // Corners, one `─` between the two titles, the copy label.
        let room = usize::from(meta.width).saturating_sub(2 + 1 + usize::from(copy_w));
        let title = card_title(&topic, room);
        let title_w = title.width() as u16;
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::MUTED))
            .title(title)
            .title(Line::styled(label, Style::new().fg(theme::MUTED)).right_aligned());
        let inner = block.inner(meta);
        let row = header_lines(&s, kind, &app.timestamp_format, now, inner.width);
        app.hit.set(HitAreas {
            header_topic: Rect {
                x: meta.x + 1,
                y: meta.y,
                width: title_w,
                height: 1,
            },
            header_copy: copy_rect(meta, copy_w),
            ..app.hit.get()
        });
        let mut text = vec![row];
        text.extend(props.then(|| props_row(s.msg, inner.width)));
        f.render_widget(Paragraph::new(text).block(block), meta);
    } else {
        // ` ⧉ ` at the right end; the row is cut short of it. Too narrow
        // a row keeps all its columns for the path.
        let glyph_w = if meta.width >= COMPACT_COPY_MIN_WIDTH {
            COMPACT_COPY.width() as u16
        } else {
            0
        };
        let (meta, props_area) = match props {
            true => {
                let [m, p] =
                    Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(meta);
                (m, Some(p))
            }
            false => (meta, None),
        };
        if let Some(area) = props_area {
            f.render_widget(Paragraph::new(props_row(s.msg, area.width)), area);
        }
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
            header_copy: if glyph_w == 0 { Rect::default() } else { glyph },
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
            let [chart, rest] = chart_split(payload);
            render_chart(f, chart, app, &s, series.as_deref());
            rest
        }
        None => payload,
    };
    render_payload(f, payload, app);
    if hist_rows > 0 {
        render_history(f, history, app, &s);
    }
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
    let mut block = theme::pane(title, app.pane_focused(Focus::Payload));
    let height = block.inner(area).height;
    app.payload_height.set(height);
    let Some((body, kind)) = body(app) else {
        app.hit.set(HitAreas {
            payload_copy: Rect::default(),
            payload_title: Rect::default(),
            ..app.hit.get()
        });
        return;
    };
    let body = &*body;
    let focused = app.pane_focused(Focus::Payload);
    let title_w = theme::pane_title_width(title, focused);
    let label = copy_label(title_w, title_w, area.width);
    let copy_w = label.width() as u16;
    block = block.title(
        Line::styled(
            label,
            Style::new().fg(if focused { theme::ACCENT } else { theme::MUTED }),
        )
        .right_aligned(),
    );
    app.hit.set(HitAreas {
        payload_copy: copy_rect(area, copy_w),
        payload_title: Rect {
            x: area.x + 1,
            y: area.y,
            width: (title_w as u16).min(area.width.saturating_sub(2 + 1 + copy_w)),
            height: 1,
        },
        ..app.hit.get()
    });
    // The stored scroll may predate a resize or a new message.
    let max = body.len().saturating_sub(height as usize);
    let skip = app.payload_scroll.min(max);
    let take = height as usize;
    app.hit.set(HitAreas {
        payload_skip: skip,
        ..app.hit.get()
    });
    // The charted values' lines, highlighted.
    let hl = app.chart_lines();
    let mark = |i: usize, line: Line<'static>| {
        if hl.contains(&(skip + i)) {
            theme::select(line, false)
        } else {
            line
        }
    };
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
    // Search matches, underlined; not in the diff, whose lines are not payload.
    let filter = app
        .filter
        .as_ref()
        .filter(|_| !app.show_diff && kind != "BINARY");
    let matched = |text: &str, line: Line<'static>| match filter {
        Some(f) => mark_matches(line, &f.ranges(text)),
        None => line,
    };
    let lines: Vec<Line> = match body {
        Body::Json(l) => l
            .iter()
            .skip(skip)
            .take(take)
            .enumerate()
            .map(|(i, l)| mark(i, matched(l, json_line(l))))
            .collect(),
        Body::Plain(l) => l
            .iter()
            .skip(skip)
            .take(take)
            .enumerate()
            .map(|(i, l)| mark(i, matched(l, Line::raw(l.clone()))))
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

/// Add `MATCH_MODIFIER` to the parts of `line` inside `ranges` (byte offsets
/// into its concatenated text), splitting spans at the boundaries. Colours are
/// kept; empty ranges are skipped.
fn mark_matches(line: Line<'static>, ranges: &[Range<usize>]) -> Line<'static> {
    if ranges.iter().all(Range::is_empty) {
        return line;
    }
    let mut out = Vec::new();
    let mut at = 0;
    for span in line.spans {
        let (end, style) = (at + span.content.len(), span.style);
        let mut pos = 0;
        for r in ranges {
            let (s, e) = (r.start.max(at), r.end.min(end));
            if s >= e {
                continue;
            }
            let (s, e) = (s - at, e - at);
            if s > pos {
                out.push(Span::styled(span.content[pos..s].to_owned(), style));
            }
            out.push(Span::styled(
                span.content[s..e].to_owned(),
                style.add_modifier(super::ui::MATCH_MODIFIER),
            ));
            pos = e;
        }
        if pos < span.content.len() {
            out.push(Span::styled(span.content[pos..].to_owned(), style));
        }
        at = end;
    }
    Line { spans: out, ..line }
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

mod chart;
mod header;
mod history;
pub use chart::*;
use header::*;
use history::*;

#[cfg(test)]
mod tests;
