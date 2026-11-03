//! Right-hand pane: metadata line, payload (pretty, coloured, or diffed
//! against the previous message) and the history list. Everything that came
//! from the broker is sanitized before it is drawn.

use super::app::{App, Focus};
use super::ui::{SELECTED_BG, border_style};
use chrono::{DateTime, Local};
use mqtt_zapp_core::diff::{ChangeKind, Diff, LineTag, diff};
use mqtt_zapp_core::payload::{detect, pretty_lines, preview, sanitize};
use mqtt_zapp_core::{Message, TopicNode};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use std::time::SystemTime;

const HINT: &str = "select a topic with a message";
const HISTORY_PREVIEW: usize = 40;
const HISTORY_ROWS: usize = 6;

/// What the payload area shows.
enum Body {
    Json(Vec<String>),
    Plain(Vec<String>),
    Diff(Vec<(Color, String)>),
    Note(&'static str),
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

fn body(app: &App) -> Option<Body> {
    let s = shown(app)?;
    if !app.show_diff {
        let kind = detect(&s.msg.payload);
        let lines = pretty_lines(&kind, &s.msg.payload);
        return Some(match kind {
            mqtt_zapp_core::payload::PayloadKind::Json(_) => Body::Json(lines),
            _ => Body::Plain(lines),
        });
    }
    let Some(prev) = s.prev else {
        return Some(Body::Note("no previous message"));
    };
    Some(match diff(&prev.payload, &s.msg.payload) {
        Diff::Binary { equal: true } => Body::Note("binary payloads are identical"),
        Diff::Binary { equal: false } => Body::Note("binary payloads differ"),
        Diff::Json(changes) if changes.is_empty() => Body::Note("no changes"),
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
    })
}

/// Number of lines the payload area scrolls over (for clamping).
pub fn content_len(app: &App) -> usize {
    body(app).map_or(0, |b| b.len())
}

fn one_line(s: &str) -> String {
    sanitize(s).replace('\n', "\u{23ce}")
}

fn clock(t: SystemTime, fmt: &str) -> String {
    DateTime::<Local>::from(t).format(fmt).to_string()
}

fn metadata(s: &Shown, topic: &str) -> String {
    let mut out = format!("{} · qos {}", one_line(topic), s.msg.qos);
    if s.msg.retain {
        out.push_str(" · retained");
    }
    out.push_str(&format!(
        " · {} B · {}",
        s.msg.payload.len(),
        clock(s.msg.received_at, "%H:%M:%S%.3f")
    ));
    out
}

pub fn render_message(f: &mut Frame, area: Rect, app: &App) {
    let Some(s) = shown(app) else {
        let hint = Paragraph::new(HINT)
            .style(Style::new().add_modifier(Modifier::DIM))
            .block(Block::bordered().title("message"));
        f.render_widget(hint, area);
        return;
    };
    let hist_rows = s.node.history.len().min(HISTORY_ROWS) as u16;
    let [meta, payload, history] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(hist_rows + 2),
    ])
    .areas(area);

    let topic = app.tree.full_topic(app.selected.unwrap_or_default());
    f.render_widget(Paragraph::new(metadata(&s, &topic)), meta);
    render_payload(f, payload, app);
    render_history(f, history, app, &s);
}

fn render_payload(f: &mut Frame, area: Rect, app: &App) {
    let title = if app.show_diff { "diff" } else { "payload" };
    let block = Block::bordered().title(title);
    let height = block.inner(area).height;
    app.payload_height.set(height);
    let Some(body) = body(app) else { return };
    // The stored scroll may predate a resize or a new message.
    let max = body.len().saturating_sub(height as usize);
    let skip = (app.payload_scroll as usize).min(max);
    let take = height as usize;
    let lines: Vec<Line> = match &body {
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
        Body::Note(n) => vec![Line::styled(*n, Style::new().add_modifier(Modifier::DIM))],
    };
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_history(f: &mut Frame, area: Rect, app: &App, s: &Shown) {
    let title = match app.history_sel {
        None => "history (live)",
        Some(_) => "history (frozen)",
    };
    let block = Block::bordered()
        .title(title)
        .border_style(border_style(app.focus == Focus::History));
    let height = block.inner(area).height as usize;
    let h = &s.node.history;
    // Newest first: row `r` is history index `len - 1 - r`.
    let sel_row = s.idx.map(|i| h.len() - 1 - i);
    let offset = sel_row.map_or(0, |r| (r + 1).saturating_sub(height));
    let lines: Vec<Line> = h
        .iter()
        .rev()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(row, m)| {
            let text = format!(
                "{}  {}",
                clock(m.received_at, "%H:%M:%S"),
                preview(&m.payload, HISTORY_PREVIEW)
            );
            let style = if Some(row) == sel_row && app.history_sel.is_some() {
                Style::new().bg(SELECTED_BG)
            } else {
                Style::new()
            };
            Line::styled(text, style)
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
    use crate::tui::ui;
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
        let l = lines(&draw(&a));
        let m = l.iter().find(|r| r.contains("dev/x")).unwrap();
        assert!(m.contains("qos 1"), "{m}");
        assert!(m.contains("retained"), "{m}");
        assert!(m.contains("5 B"), "{m}");
        let t = m.split(" · ").last().unwrap().trim();
        assert_eq!(t.len(), 12, "HH:MM:SS.mmm in {t:?}");
        assert_eq!(&t[2..3], ":");
        assert_eq!(&t[8..9], ".");
    }

    #[test]
    fn not_retained_is_not_marked() {
        let a = app_with(&[b"hello"]);
        assert!(!text(&draw(&a)).contains("retained"));
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
        assert!((0..100).any(|x| b[(x, y)].bg == ui::SELECTED_BG));
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
        assert_ne!(corner(&a, "history"), ui::border_style(true).fg.unwrap());
        a.focus = Focus::History;
        assert_eq!(corner(&a, "history"), ui::border_style(true).fg.unwrap());
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
        a.handle_action(Action::FocusNext);
        assert_eq!(a.focus, Focus::History);
        a.handle_action(Action::FocusNext);
        assert_eq!(a.focus, Focus::Tree);
        a.handle_action(Action::ToggleDiff);
        assert!(a.show_diff);
        a.handle_action(Action::ToggleDiff);
        assert!(!a.show_diff);
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
        a.handle_action(Action::Down);
        assert_eq!(a.history_sel, Some(3));
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
        let h = a.payload_height.get();
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
}
