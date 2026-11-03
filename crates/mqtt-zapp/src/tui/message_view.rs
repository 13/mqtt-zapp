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
use ratatui::widgets::Paragraph;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

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
        });
    }
    // The index may be stale; never trust it.
    let i = app.history_sel.map_or(h.len() - 1, |i| i.min(h.len() - 1));
    Some(Shown {
        node,
        msg: &h[i],
        prev: i.checked_sub(1).map(|p| &h[p]),
        idx: Some(i),
    })
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

/// `topic  QoS 1  RETAINED  JSON  5 B  12:00:00.000` as styled chips.
fn metadata(s: &Shown, topic: &str, kind: &str) -> Line<'static> {
    let muted = Style::new().fg(theme::MUTED);
    let chip = Style::new()
        .fg(theme::MUTED)
        .add_modifier(Modifier::REVERSED);
    let mut spans = vec![
        Span::styled(
            one_line(topic),
            Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(format!(" QoS {} ", s.msg.qos), chip),
    ];
    if s.msg.retain {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            " RETAINED ",
            Style::new().fg(Color::Black).bg(Color::Magenta),
        ));
    }
    spans.extend([
        Span::raw(" "),
        Span::styled(format!(" {kind} "), chip),
        Span::raw(" "),
        Span::styled(human_bytes(s.msg.payload.len()), muted),
        Span::raw(" "),
        Span::styled(clock(s.msg.received_at, "%H:%M:%S%.3f"), muted),
    ]);
    Line::from(spans)
}

pub fn render_message(f: &mut Frame, area: Rect, app: &App) {
    let Some(s) = shown(app) else {
        // Payload and history focus both land here while there is nothing
        // to show, so the focus stays visible.
        let block = theme::pane("message", app.focus != Focus::Tree);
        app.hit.set(HitAreas {
            payload: area,
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
    let half = area.height.saturating_sub(1) / 2;
    let mut hist_rows = (s.node.history.len().min(HISTORY_ROWS) as u16).min(half.saturating_sub(2));
    if app.focus == Focus::History {
        hist_rows = hist_rows.max(1);
    }
    let hist_height = if hist_rows == 0 { 0 } else { hist_rows + 2 };
    let [meta, payload, history] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(hist_height),
    ])
    .areas(area);

    let topic = app.tree.full_topic(app.selected.unwrap_or_default());
    let kind = body(app).map_or("", |(_, k)| k);
    f.render_widget(Paragraph::new(metadata(&s, &topic, kind)), meta);
    render_payload(f, payload, app);
    render_history(f, history, app, &s);
}

fn render_payload(f: &mut Frame, area: Rect, app: &App) {
    let title = if app.show_diff { "diff" } else { "payload" };
    app.hit.set(HitAreas {
        payload: area,
        ..app.hit.get()
    });
    let mut block = theme::pane(title, app.focus == Focus::Payload);
    let height = block.inner(area).height;
    app.payload_height.set(height);
    let Some((body, _)) = body(app) else { return };
    let body = &*body;
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
            let line = Line::from(vec![
                Span::styled(
                    clock(m.received_at, "%H:%M:%S"),
                    Style::new().fg(theme::MUTED),
                ),
                Span::raw(format!("  {}", preview(&m.payload, HISTORY_PREVIEW))),
            ]);
            if Some(row) == sel_row && app.history_sel.is_some() {
                line.style(theme::selection(focused))
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
mod tests {
    use super::*;
    use crate::cli::Launch;
    use crate::tui::app::{App, Focus, Mode};
    use crate::tui::keymap::Action;
    use mqtt_zapp_core::Message;
    use mqtt_zapp_core::client::CoreEvent;
    use mqtt_zapp_core::config::{ProfileStore, Settings};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use std::time::SystemTime;

    const T: &str = "dev/x";

    fn app_depth(depth: usize) -> App {
        let settings = Settings {
            history_depth: depth,
            ..Settings::default()
        };
        let mut a = App::new(Launch::Picker, ProfileStore::default(), settings).unwrap();
        a.mode = Mode::Browse;
        a
    }

    fn send(a: &mut App, topic: &str, payload: &[u8], retain: bool) {
        a.apply_core_event(CoreEvent::Message {
            topic: topic.into(),
            msg: Message {
                payload: payload.to_vec(),
                qos: 1,
                retain,
                received_at: SystemTime::now(),
            },
        });
    }

    /// App with `T` selected after sending each payload to it.
    fn app_with(payloads: &[&[u8]]) -> App {
        let mut a = app_depth(100);
        for p in payloads {
            send(&mut a, T, p, false);
        }
        a.selected = a.tree.find(T);
        a
    }

    fn draw(a: &App) -> Buffer {
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| render_message(f, f.area(), a)).unwrap();
        t.backend().buffer().clone()
    }

    fn lines(b: &Buffer) -> Vec<String> {
        let w = b.area.width as usize;
        b.content
            .chunks(w)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect()
    }

    fn text(b: &Buffer) -> String {
        lines(b).join("\n")
    }

    #[test]
    fn shows_multiline_pretty_json_as_multiple_lines() {
        let a = app_with(&[b"{\n  \"UPSNAME\": \"apc-ups\",\n  \"WATTS\": 120\n}"]);
        let l = lines(&draw(&a));
        let row = |n: &str| l.iter().position(|r| r.contains(n));
        let (u, w) = (
            row("\"UPSNAME\": \"apc-ups\"").unwrap(),
            row("\"WATTS\": 120").unwrap(),
        );
        assert_ne!(u, w);
    }

    #[test]
    fn json_tokens_are_coloured() {
        let a = app_with(&[br#"{"k":"s","n":12,"b":true,"z":null}"#]);
        let b = draw(&a);
        let l = lines(&b);
        // `ctx` locates the line; the token is its tail.
        let fg_at = |ctx: &str, tok: &str| {
            let y = l.iter().position(|r| r.contains(ctx)).unwrap();
            // Byte offsets, converted to columns (the border is multi-byte).
            let byte = l[y].find(ctx).unwrap() + ctx.find(tok).unwrap();
            let x = l[y][..byte].chars().count();
            b[(x as u16, y as u16)].fg
        };
        assert_eq!(fg_at("\"k\": ", "\"k\""), Color::Cyan);
        assert_eq!(fg_at("\"k\": \"s\"", "\"s\""), Color::Green);
        assert_eq!(fg_at("\"n\": 12", "12"), Color::Yellow);
        assert_eq!(fg_at("\"b\": true", "true"), Color::Magenta);
        assert_eq!(fg_at("\"z\": null", "null"), Color::Magenta);
    }

    #[test]
    fn binary_shows_hex() {
        let a = app_with(&[&[0xff, 0x00, 0x41]]);
        assert!(text(&draw(&a)).contains("ff 00 41"));
    }

    #[test]
    fn diff_shows_changed_key() {
        let mut a = app_with(&[br#"{"ENERGY":{"Power":10}}"#, br#"{"ENERGY":{"Power":12}}"#]);
        a.show_diff = true;
        assert!(text(&draw(&a)).contains("~ ENERGY.Power: 10 → 12"));
    }

    #[test]
    fn diff_added_removed_and_text() {
        let mut a = app_with(&[br#"{"a":1,"b":2}"#, br#"{"a":1,"c":3}"#]);
        a.show_diff = true;
        let s = text(&draw(&a));
        assert!(s.contains("+ c: 3"), "{s}");
        assert!(s.contains("- b: 2"), "{s}");
        let mut a = app_with(&[b"one\ntwo", b"one\nthree"]);
        a.show_diff = true;
        let s = text(&draw(&a));
        assert!(s.contains("- two"), "{s}");
        assert!(s.contains("+ three"), "{s}");
    }

    #[test]
    fn diff_without_previous() {
        let mut a = app_with(&[b"{\"a\":1}"]);
        a.show_diff = true;
        assert!(text(&draw(&a)).contains("no previous message"));
    }

    #[test]
    fn history_selection_freezes_view() {
        let mut a = app_with(&[b"first-payload", b"second-payload"]);
        a.history_sel = Some(0);
        send(&mut a, T, b"third-payload", false);
        let s = text(&draw(&a));
        assert!(s.contains("first-payload"), "{s}");
        // "third-payload" appears only in the history list, not the payload block.
        assert_eq!(s.matches("third-payload").count(), 1, "{s}");
        assert_eq!(s.matches("first-payload").count(), 2, "{s}");
    }

    #[test]
    fn live_view_follows_latest() {
        let mut a = app_with(&[b"first-payload"]);
        send(&mut a, T, b"second-payload", false);
        let s = text(&draw(&a));
        assert_eq!(s.matches("second-payload").count(), 2, "{s}");
    }

    #[test]
    fn metadata_line() {
        let mut a = app_depth(10);
        send(&mut a, T, b"hello", true);
        a.selected = a.tree.find(T);
        let b = draw(&a);
        let l = lines(&b);
        let y = l.iter().position(|r| r.contains("dev/x")).unwrap();
        let m = &l[y];
        assert!(m.contains(" QoS 1 "), "{m}");
        assert!(m.contains(" RETAINED "), "{m}");
        assert!(m.contains(" TEXT "), "{m}");
        assert!(m.contains("5 B"), "{m}");
        let x = m[..m.find("RETAINED").unwrap()].chars().count() as u16;
        assert_eq!(b[(x, y as u16)].bg, Color::Magenta);
        let t = m.split(' ').rfind(|w| !w.is_empty()).unwrap();
        assert_eq!(t.len(), 12, "HH:MM:SS.mmm in {t:?}");
        assert_eq!(&t[2..3], ":");
        assert_eq!(&t[8..9], ".");
    }

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn meta_shows_json_kind() {
        let a = app_with(&[br#"{"a":1}"#]);
        let s = text(&draw(&a));
        assert!(s.contains(" JSON "), "{s}");
        let a = app_with(&[&[0xff, 0x00]]);
        assert!(text(&draw(&a)).contains(" BINARY "));
    }

    #[test]
    fn payload_scroll_indicator() {
        let body = (0..50)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut a = app_with(&[body.as_bytes()]);
        let _ = draw(&a);
        let s = text(&draw(&a));
        assert!(s.contains("/50"), "{s}");
        a.handle_action(Action::ScrollDown);
        let h = usize::from(a.payload_height.get());
        let s = text(&draw(&a));
        let want = format!("{}-{}/50", h / 2 + 1, h / 2 + h);
        assert!(s.contains(&want), "{want}: {s}");
        let a = app_with(&[b"short"]);
        assert!(!text(&draw(&a)).contains("/1"));
    }

    #[test]
    fn history_title_says_live_or_paused() {
        let mut a = app_with(&[b"1", b"2"]);
        assert!(text(&draw(&a)).contains("history · live"));
        a.history_sel = Some(0);
        assert!(text(&draw(&a)).contains("history · paused (j to newest)"));
    }

    #[test]
    fn not_retained_is_not_marked() {
        let a = app_with(&[b"hello"]);
        assert!(!text(&draw(&a)).contains("RETAINED"));
    }

    #[test]
    fn placeholder_pane_shows_focus() {
        let mut a = app_depth(10);
        assert_eq!(draw(&a)[(0, 0)].symbol(), "╭");
        a.focus = Focus::Payload;
        assert_eq!(draw(&a)[(0, 0)].symbol(), "┏");
        a.focus = Focus::History;
        assert_eq!(draw(&a)[(0, 0)].symbol(), "┏");
    }

    #[test]
    fn hint_when_nothing_to_show() {
        let mut a = app_depth(10);
        assert!(text(&draw(&a)).contains("select a topic with a message"));
        send(&mut a, "a/b", b"x", false);
        a.selected = a.tree.find("a"); // parent without a message
        assert!(text(&draw(&a)).contains("select a topic with a message"));
    }

    #[test]
    fn hostile_payload_is_not_rendered_raw() {
        let a = app_with(&[b"\x1b[2J\x1b]0;pwn\x07 text"]);
        let b = draw(&a);
        assert!(b.content.iter().all(|c| !c.symbol().contains('\x1b')));
        assert!(b.content.iter().all(|c| !c.symbol().contains('\x07')));
        let mut a = app_with(&[br#"{"a\u001b[2J":"x"}"#, br#"{"a\u001b[2J":"y\u001b"}"#]);
        a.show_diff = true;
        assert!(
            draw(&a)
                .content
                .iter()
                .all(|c| !c.symbol().contains('\x1b'))
        );
    }

    #[test]
    fn huge_payload_does_not_panic() {
        let big = vec![b'a'; 1_000_000];
        let mut a = app_with(&[&big]);
        a.payload_scroll = 40_000; // way past the end
        assert!(draw(&a).area.width > 0);
    }

    #[test]
    fn history_list_is_newest_first_and_marks_selection() {
        let mut a = app_with(&[b"aaa", b"bbb", b"ccc"]);
        a.history_sel = Some(1);
        let b = draw(&a);
        let l = lines(&b);
        let pos = |n: &str| l.iter().rposition(|r| r.contains(n)).unwrap();
        assert!(pos("ccc") < pos("bbb") && pos("bbb") < pos("aaa"));
        let y = pos("bbb") as u16;
        assert!((0..100).any(|x| b[(x, y)].bg == theme::SEL_BG_INACTIVE));
    }

    #[test]
    fn out_of_range_selection_does_not_panic() {
        let mut a = app_with(&[b"aaa", b"bbb"]);
        a.history_sel = Some(99);
        assert!(text(&draw(&a)).contains("aaa") || text(&draw(&a)).contains("bbb"));
    }

    #[test]
    fn focused_pane_border_is_highlighted() {
        let mut a = app_with(&[b"aaa", b"bbb"]);
        let corner = |a: &App, needle: &str| {
            let b = draw(a);
            let y = lines(&b).iter().position(|r| r.contains(needle)).unwrap();
            b[(0, y as u16)].fg
        };
        assert_ne!(corner(&a, "history"), theme::ACCENT);
        a.focus = Focus::History;
        assert_eq!(corner(&a, "history"), theme::ACCENT);
    }

    // --- key handling ---

    fn hist(a: &mut App, actions: &[Action]) {
        a.focus = Focus::History;
        for x in actions {
            a.handle_action(*x);
        }
    }

    #[test]
    fn focus_next_toggles_and_diff_toggles() {
        let mut a = app_with(&[b"a"]);
        a.handle_action(Action::ToggleDiff);
        assert!(a.show_diff);
        a.handle_action(Action::ToggleDiff);
        assert!(!a.show_diff);
    }

    #[test]
    fn focus_cycles_tree_payload_history() {
        let mut a = app_with(&[b"a"]);
        a.handle_action(Action::FocusNext);
        assert_eq!(a.focus, Focus::Payload);
        a.handle_action(Action::FocusNext);
        assert_eq!(a.focus, Focus::History);
        a.handle_action(Action::FocusNext);
        assert_eq!(a.focus, Focus::Tree);
    }

    #[test]
    fn payload_focus_jk_scrolls_one_line() {
        let body = (0..50)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut a = app_with(&[body.as_bytes()]);
        let sel = a.selected;
        a.payload_height.set(10);
        a.focus = Focus::Payload;
        a.handle_action(Action::Down);
        a.handle_action(Action::Down);
        assert_eq!(a.payload_scroll, 2);
        a.handle_action(Action::Up);
        assert_eq!(a.payload_scroll, 1);
        a.handle_action(Action::Bottom);
        assert_eq!(a.payload_scroll, 40);
        a.handle_action(Action::Down);
        assert_eq!(a.payload_scroll, 40);
        a.handle_action(Action::Top);
        assert_eq!(a.payload_scroll, 0);
        assert_eq!(a.selected, sel);
    }

    #[test]
    fn payload_focus_keys_noop_without_message() {
        let mut a = app_depth(10);
        a.payload_height.set(10);
        a.focus = Focus::Payload;
        for x in [Action::Down, Action::Up, Action::Top, Action::Bottom] {
            a.handle_action(x);
            assert_eq!(a.payload_scroll, 0);
        }
        assert_eq!(a.selected, None);
    }

    #[test]
    fn history_navigation_up_down() {
        let mut a = app_with(&[b"1", b"2", b"3", b"4"]);
        hist(&mut a, &[Action::Up]);
        assert_eq!(a.history_sel, Some(2));
        a.handle_action(Action::Up);
        a.handle_action(Action::Up);
        a.handle_action(Action::Up);
        assert_eq!(a.history_sel, Some(0));
        a.handle_action(Action::Down);
        assert_eq!(a.history_sel, Some(1));
        a.handle_action(Action::Down);
        assert_eq!(a.history_sel, Some(2));
        // The newest entry is live, never a frozen selection.
        a.handle_action(Action::Down);
        assert_eq!(a.history_sel, None);
        a.handle_action(Action::Down);
        assert_eq!(a.history_sel, None);
    }

    #[test]
    fn history_up_with_single_entry_stays_live() {
        let mut a = app_with(&[b"1"]);
        hist(&mut a, &[Action::Up]);
        assert_eq!(a.history_sel, None);
    }

    #[test]
    fn tree_focus_jk_moves_tree_not_history() {
        let mut a = app_with(&[b"1", b"2"]);
        a.handle_action(Action::Up);
        assert_eq!(a.history_sel, None);
    }

    #[test]
    fn selecting_other_node_resets_history_and_scroll() {
        let mut a = app_with(&[b"1", b"2", b"3"]);
        send(&mut a, "zzz", b"x", false);
        hist(&mut a, &[Action::Up]);
        a.payload_scroll = 3;
        a.focus = Focus::Tree;
        a.handle_action(Action::Down);
        assert_eq!(a.history_sel, None);
        assert_eq!(a.payload_scroll, 0);
    }

    #[test]
    fn frozen_entry_tracks_eviction_and_falls_back_to_oldest() {
        let mut a = app_depth(3);
        for p in [b"1", b"2", b"3"] {
            send(&mut a, T, p, false);
        }
        a.selected = a.tree.find(T);
        a.history_sel = Some(1);
        send(&mut a, T, b"4", false); // evicts "1"; the frozen "2" is now index 0
        assert_eq!(a.history_sel, Some(0));
        send(&mut a, T, b"5", false); // evicts "2" itself; fall back to oldest
        assert_eq!(a.history_sel, Some(0));
        // Other topics do not shift the selection.
        a.history_sel = Some(2);
        send(&mut a, "other", b"x", false);
        assert_eq!(a.history_sel, Some(2));
    }

    #[test]
    fn scroll_is_clamped() {
        let body = (0..50)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut a = app_with(&[body.as_bytes()]);
        let _ = draw(&a); // learn the payload height
        a.handle_action(Action::ScrollUp);
        assert_eq!(a.payload_scroll, 0);
        let h = usize::from(a.payload_height.get());
        assert!(h > 2);
        a.handle_action(Action::ScrollDown);
        assert_eq!(a.payload_scroll, h / 2);
        for _ in 0..100 {
            a.handle_action(Action::ScrollDown);
        }
        assert_eq!(a.payload_scroll, 50 - h);
        let s = text(&draw(&a));
        assert!(s.contains("l49"), "{s}");
    }

    #[test]
    fn scroll_works_from_history_focus_too() {
        let body = (0..50).map(|i| format!("l{i}\n")).collect::<String>();
        let mut a = app_with(&[body.as_bytes()]);
        let _ = draw(&a);
        a.focus = Focus::History;
        a.handle_action(Action::ScrollDown);
        assert!(a.payload_scroll > 0);
    }

    #[test]
    fn big_binary_renders_and_body_is_cached() {
        let big: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
        let mut a = app_with(&[&big]);
        let first = text(&draw(&a));
        assert!(first.contains("00000000"));
        assert_eq!(a.body_cache.builds.get(), 1);
        let _ = draw(&a);
        a.handle_action(Action::ScrollDown); // content_len shares the cache
        assert_eq!(a.body_cache.builds.get(), 1);
        send(&mut a, T, b"new", false); // a new message invalidates it
        let _ = draw(&a);
        assert_eq!(a.body_cache.builds.get(), 2);
        a.show_diff = true;
        let _ = draw(&a);
        assert_eq!(a.body_cache.builds.get(), 3);
    }

    #[test]
    fn oversized_text_diff_is_refused() {
        let big = "x\n".repeat(150_000); // 300 kB
        let mut a = app_with(&[big.as_bytes(), big.as_bytes()]);
        a.show_diff = true;
        let s = text(&draw(&a));
        assert!(
            s.contains("payload too large to diff (300000 bytes)"),
            "{s}"
        );
    }

    #[test]
    fn key_with_escaped_quote_is_coloured_as_key() {
        let a = app_with(&[br#"{"a\"b": "c\\"}"#]);
        let b = draw(&a);
        let l = lines(&b);
        let y = l.iter().position(|r| r.contains(r#""a\"b": "#)).unwrap();
        let byte = l[y].find(r#""a\"b""#).unwrap();
        let x = l[y][..byte].chars().count() as u16;
        assert_eq!(b[(x, y as u16)].fg, Color::Cyan);
        assert_eq!(b[(x + 4, y as u16)].fg, Color::Cyan); // the b after the escape
        let vx = l[y][..l[y].find(r#""c\\""#).unwrap()].chars().count() as u16;
        assert_eq!(b[(vx, y as u16)].fg, Color::Green);
    }
}
