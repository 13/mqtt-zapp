use super::*;
use crate::cli::Launch;
use crate::tui::app::{App, Focus, Mode};
use crate::tui::keymap::Action;
use mqtt_zapp_core::client::CoreEvent;
use mqtt_zapp_core::config::{ProfileStore, Settings};
use mqtt_zapp_core::payload::Props;
use mqtt_zapp_core::{Bytes, Message};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use std::time::Duration;
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
            payload: Bytes::copy_from_slice(payload),
            qos: 1,
            retain,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
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
    t.draw(|f| render_message(f, f.area(), a, false)).unwrap();
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

/// The card's badge row: the line just below the title border.
fn badge_row(l: &[String]) -> (usize, &String) {
    let y = l.iter().position(|r| r.contains("dev/x")).unwrap() + 1;
    (y, &l[y])
}

/// The clock on the badge row: the word before ` · <age>`.
fn clock_of(row: &str) -> &str {
    let before = &row[..row.rfind(" · ").unwrap()];
    before.split(' ').rfind(|w| !w.is_empty()).unwrap()
}

#[test]
fn metadata_line() {
    let mut a = app_depth(10);
    send(&mut a, T, b"hello", true);
    a.selected = a.tree.find(T);
    let b = draw(&a);
    let l = lines(&b);
    let (y, m) = badge_row(&l);
    assert!(m.contains(" QoS 1 "), "{m}");
    assert!(m.contains(" RETAINED "), "{m}");
    assert!(m.contains(" TEXT "), "{m}");
    assert!(m.contains("5 B"), "{m}");
    let x = m[..m.find("RETAINED").unwrap()].chars().count() as u16;
    assert_eq!(b[(x, y as u16)].bg, Color::Magenta);
    let t = clock_of(m);
    assert_eq!(t.len(), 12, "HH:MM:SS.mmm in {t:?}");
    assert_eq!(&t[2..3], ":");
    assert_eq!(&t[8..9], ".");
    assert!(
        m.trim_end()
            .trim_end_matches('│')
            .trim_end()
            .ends_with("· now"),
        "{m}"
    );
}

#[test]
fn age_formats() {
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let at = |s: u64| age(t0, t0 + Duration::from_secs(s));
    assert_eq!(at(0), "now");
    assert_eq!(at(12), "12s ago");
    assert_eq!(at(240), "4m ago");
    assert_eq!(at(3 * 3600 + 5), "3h ago");
    assert_eq!(at(2 * 86400 + 7), "2d ago");
}

#[test]
fn age_future_is_now() {
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    assert_eq!(age(t0 + Duration::from_secs(30), t0), "now");
}

#[test]
fn header_card_draws_box_path_and_badges() {
    let topic = "tasmota/tele/tasmota_A1B2C3/SENSOR";
    let mut a = app_depth(10);
    send(&mut a, topic, br#"{"a":1}"#, true);
    a.selected = a.tree.find(topic);
    let b = draw(&a);
    let l = lines(&b);
    assert!(
        l[0].starts_with("╭ tasmota/tele/tasmota_A1B2C3/SENSOR ─"),
        "{}",
        l[0]
    );
    assert!(l[0].contains("⧉ copy ╮"), "{}", l[0]);
    let x = l[0][..l[0].find("SENSOR").unwrap()].chars().count() as u16;
    assert_eq!(b[(x, 0)].fg, theme::ACCENT);
    assert!(b[(x, 0)].modifier.contains(Modifier::BOLD));
    // The path reads in the text colour; only the leaf is accented.
    assert_eq!(b[(x - 1, 0)].fg, Color::Reset);
    let copy = l[0][..l[0].find("⧉ copy").unwrap()].chars().count() as u16;
    assert_eq!(b[(copy, 0)].fg, theme::MUTED, "copy keeps its colour");
    assert!(l[1].contains(" JSON "), "{}", l[1]);
    let jx = l[1][..l[1].find("JSON").unwrap()].chars().count() as u16;
    assert_eq!(b[(jx, 1)].bg, theme::ACCENT);
    assert!(
        l[1].contains(" QoS 1 ") && l[1].contains(" RETAINED "),
        "{}",
        l[1]
    );
    assert!(l[2].starts_with('╰'), "{}", l[2]);
    assert!(l[3].contains("payload"), "payload right below: {}", l[3]);
}

#[test]
fn header_no_retained_badge_when_not_retained() {
    let a = app_with(&[b"x"]);
    let l = lines(&draw(&a));
    assert!(!l[2].contains("RETAINED"), "{}", l[2]);
}

#[test]
fn header_long_cjk_topic_keeps_leaf() {
    let (parents, leaf) = fit_topic("设备/房间/很长很长很长很长很长很长很长很长/温度", 26);
    assert_eq!(leaf, "温度");
    assert!(parents.width() + leaf.width() <= 26, "{parents}");
    assert_eq!(parents, "设备/…/");
    let (p, leaf) = fit_topic("a/b/c", 40);
    assert_eq!((p.as_str(), leaf.as_str()), ("a/b/", "c"));
    let (p, leaf) = fit_topic(&format!("a/{}", "x".repeat(50)), 12);
    assert!(p.is_empty(), "{p}");
    assert!(leaf.starts_with('…') && leaf.width() <= 12, "{leaf}");
}

#[test]
fn header_row_never_overflows() {
    let topic = "设备/房间/很长很长很长很长很长很长很长很长/温度";
    let mut a = app_depth(10);
    send(&mut a, topic, b"21.5", true);
    a.selected = a.tree.find(topic);
    let mut t = Terminal::new(TestBackend::new(40, 30)).unwrap();
    t.draw(|f| render_message(f, f.area(), &a, false)).unwrap();
    let b = t.backend().buffer();
    let l = lines(b);
    // Wide chars occupy two cells (the second reads as a blank here).
    assert!(l[0].ends_with('╮'), "{:?}", l[0]);
    assert!(l[0].contains("温 度"), "leaf kept: {:?}", l[0]);
    let x = (0..40).find(|&x| b[(x, 0)].symbol() == "温").unwrap();
    assert_eq!(b[(x, 0)].fg, theme::ACCENT);
}

#[test]
fn header_compact_when_short_or_stacked() {
    let a = app_with(&[b"x"]);
    for (w, h, stacked) in [(79, 24, true), (100, 12, false)] {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render_message(f, f.area(), &a, stacked))
            .unwrap();
        let l = lines(t.backend().buffer());
        assert!(!l[0].contains('╭'), "{w}x{h}: {}", l[0]);
        assert!(
            l[0].contains("dev/x") && l[0].contains(" TEXT "),
            "{}",
            l[0]
        );
    }
}

#[test]
fn on_tick_redraws_once_a_second_for_the_age() {
    let mut a = app_with(&[b"x"]);
    a.on_tick();
    a.dirty = false;
    a.age_second = 0;
    a.on_tick();
    assert!(a.dirty);
}

#[test]
fn c_cycles_series() {
    let mut a = app_with(&[br#"{"ENERGY":{"Power":12},"t":1}"#]);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(Some("ENERGY.Power".to_owned())));
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(Some("t".to_owned())));
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    let mut a = app_with(&[b"42"]);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(None));
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    let mut a = app_with(&[b"hello"]);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    assert!(a.note_at(std::time::Instant::now()).is_some());
}

#[test]
fn chart_renders_braille_series() {
    let ps: Vec<String> = (0..10)
        .map(|i| format!(r#"{{"ENERGY":{{"Power":{}}}}}"#, i * i))
        .collect();
    let refs: Vec<&[u8]> = ps.iter().map(|p| p.as_bytes()).collect();
    let mut a = app_with(&refs);
    a.handle_action(Action::Chart);
    let b = draw(&a);
    let s = text(&b);
    assert!(s.contains("chart · ENERGY.Power"), "{s}");
    assert!(
        b.content.iter().any(|c| c
            .symbol()
            .chars()
            .any(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))),
        "{s}"
    );
    assert!(s.contains("81") && s.contains('0'), "y labels: {s}");
    // Another topic turns the chart off.
    send(&mut a, "other", b"1", false);
    a.selected = None;
    a.handle_action(Action::Down);
    assert_eq!(a.chart, None);
}

#[test]
fn chart_points_are_cached() {
    let mut a = app_with(&[b"1", b"2", b"3"]);
    a.handle_action(Action::Chart);
    let _ = draw(&a);
    let _ = draw(&a);
    assert_eq!(a.chart_cache.builds.get(), 1);
    send(&mut a, T, b"4", false);
    let _ = draw(&a);
    assert_eq!(a.chart_cache.builds.get(), 2);
}

#[test]
fn chart_not_enough_values() {
    let mut a = app_with(&[b"1", b"x"]);
    a.history_sel = Some(0);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(None));
    assert!(text(&draw(&a)).contains("not enough numeric values"));
}

#[test]
fn meta_uses_timestamp_format() {
    let mut a = app_with(&[b"x"]);
    a.timestamp_format = "%H:%M".into();
    let l = lines(&draw(&a));
    let (_, m) = badge_row(&l);
    let t = clock_of(m);
    assert_eq!(t.len(), 5, "{m}");
    assert_eq!(&t[2..3], ":");
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
    assert!(text(&draw(&a)).contains("history · paused (k to newest)"));
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
    draw(&a); // history_shown is set by a frame
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
    hist(&mut a, &[Action::Down]);
    assert_eq!(a.history_sel, Some(2));
    a.handle_action(Action::Down);
    a.handle_action(Action::Down);
    a.handle_action(Action::Down);
    assert_eq!(a.history_sel, Some(0));
    a.handle_action(Action::Up);
    assert_eq!(a.history_sel, Some(1));
    a.handle_action(Action::Up);
    assert_eq!(a.history_sel, Some(2));
    // The newest entry is live, never a frozen selection.
    a.handle_action(Action::Up);
    assert_eq!(a.history_sel, None);
    a.handle_action(Action::Up);
    assert_eq!(a.history_sel, None);
}

#[test]
fn history_down_with_single_entry_stays_live() {
    let mut a = app_with(&[b"1"]);
    hist(&mut a, &[Action::Down]);
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
    hist(&mut a, &[Action::Down]);
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

#[test]
fn diff_uses_marked_base() {
    let mut a = app_with(&[br#"{"v":1}"#, br#"{"v":2}"#, br#"{"v":3}"#]);
    a.history_sel = Some(0);
    a.handle_action(Action::MarkDiffBase);
    a.history_sel = None; // live: shows v3
    let s = text(&draw(&a));
    assert!(s.contains("~ v: 1 → 3"), "{s}");
}

#[test]
fn title_names_base_time_and_marks_row() {
    let mut a = app_with(&[b"one", b"two"]);
    a.timestamp_format = "%H:%M".into();
    a.history_sel = Some(0);
    a.handle_action(Action::MarkDiffBase);
    let s = text(&draw(&a));
    assert!(s.contains("diff vs "), "{s}");
    assert_eq!(s.matches('◆').count(), 1, "{s}");
}

#[test]
fn diff_without_base_still_uses_previous() {
    let mut a = app_with(&[br#"{"v":1}"#, br#"{"v":2}"#, br#"{"v":3}"#]);
    a.show_diff = true;
    assert!(text(&draw(&a)).contains("~ v: 2 → 3"));
}

fn fit(topic: &str, width: usize) -> (String, String) {
    let (p, l) = fit_topic(topic, width);
    assert!(
        p.width() + l.width() <= width,
        "{topic} @ {width}: {p:?} {l:?}"
    );
    (p, l)
}

#[test]
fn plain_path_basic() {
    assert_eq!(
        fit("muh/wst/status", 40),
        ("muh/wst/".into(), "status".into())
    );
}

#[test]
fn plain_path_shortens_middle_levels() {
    let t = "tasmota/tele/tasmota_A1B2C3/SENSOR";
    assert_eq!(fit(t, 18), ("tasmota/…/".into(), "SENSOR".into()));
    // Room for the first level and the last parent: only the middle goes.
    assert_eq!(fit("a/b/c/d/leaf", 11), ("a/…/d/".into(), "leaf".into()));
    assert_eq!(fit(t, 10), ("…/".into(), "SENSOR".into()));
    assert_eq!(fit(t, 7), ("".into(), "SENSOR".into()));
    assert_eq!(fit(t, 4), ("".into(), "…SOR".into()));
}

#[test]
fn plain_path_edge_topics() {
    assert_eq!(fit("a", 10), ("".into(), "a".into()));
    assert_eq!(fit("a//b", 20), ("a//".into(), "b".into()));
    assert_eq!(fit("/", 10), ("/".into(), "".into()));
    assert_eq!(fit("", 10), ("".into(), "".into()));
    for w in 1..30 {
        for t in ["/", "a//b", "///", "x/yyyyyyyyyyyy/z", "设备/房间/温度"] {
            fit(t, w);
        }
    }
}

fn card_row0(topic: &str, w: u16) -> String {
    let mut a = app_depth(10);
    send(&mut a, topic, b"1", false);
    a.selected = a.tree.find(topic);
    let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
    t.draw(|f| render_message(f, f.area(), &a, false)).unwrap();
    lines(t.backend().buffer())[0].clone()
}

#[test]
fn card_title_never_reaches_copy_button() {
    let topic = "a/very/long/topic/path/with/many/levels/leaf";
    for w in 20..80u16 {
        let r = card_row0(topic, w);
        assert!(r.ends_with('╮'), "{w}: {r}");
        assert_eq!(r.matches('⧉').count(), 1, "{w}: {r}");
        let copy = r.find('⧉').unwrap();
        if let Some(leaf) = r.find("leaf") {
            assert!(leaf + 4 < copy, "{w}: {r}");
        }
        if usize::from(w) >= 2 + 1 + 3 + 6 {
            assert!(r.contains("leaf "), "{w}: {r}");
        }
    }
}

#[test]
fn narrow_card_keeps_copy_button() {
    let r = card_row0("a/b/c", 12);
    assert!(r.contains('⧉'), "{r}");
}

/// Cells of row `y` whose text starts at the first occurrence of `needle`.
fn cell_at<'a>(b: &'a ratatui::buffer::Buffer, y: u16, needle: &str) -> &'a ratatui::buffer::Cell {
    let row: String = (0..b.area.width).map(|x| b[(x, y)].symbol()).collect();
    let x = row[..row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle}: {row}"))]
        .chars()
        .count() as u16;
    &b[(x, y)]
}

#[test]
fn clock_and_age_read_in_the_text_colour() {
    let mut a = app_depth(10);
    send(&mut a, T, b"x", false);
    a.selected = a.tree.find(T);
    // Card (row 1 is the badge row) and compact (row 0).
    let b = draw(&a);
    let row = lines(&b)[1].clone();
    let clock = clock_of(&row).to_owned();
    assert_eq!(cell_at(&b, 1, &clock).fg, Color::Reset, "{row}");
    assert_eq!(cell_at(&b, 1, "now").fg, Color::Reset, "{row}");
    assert_eq!(cell_at(&b, 1, "·").fg, theme::MUTED, "{row}");
    let mut t = Terminal::new(TestBackend::new(79, 24)).unwrap();
    t.draw(|f| render_message(f, f.area(), &a, true)).unwrap();
    let b = t.backend().buffer();
    assert_eq!(cell_at(b, 0, &clock).fg, Color::Reset);
    assert_eq!(cell_at(b, 0, "now").fg, Color::Reset);
}

#[test]
fn history_k_from_live_stays_live() {
    let mut a = app_with(&[b"1", b"2", b"3", b"4"]);
    hist(&mut a, &[Action::Up]);
    assert_eq!(a.history_sel, None);
    a.handle_action(Action::Down);
    assert_eq!(a.history_sel, Some(2));
}

#[test]
fn history_times_are_bold_in_the_text_colour() {
    let a = app_with(&[b"first-payload", b"second-payload"]);
    let b = draw(&a);
    let l = lines(&b);
    let y = l.iter().rposition(|r| r.contains("first-payload")).unwrap() as u16;
    let x = l[y as usize].find(|c: char| c.is_ascii_digit()).unwrap();
    let x = l[y as usize][..x].chars().count() as u16;
    let c = &b[(x, y)];
    assert_eq!(c.fg, Color::Reset, "{}", l[y as usize]);
    assert!(c.modifier.contains(Modifier::BOLD));
}

#[test]
fn copy_text_only_when_the_whole_title_fits() {
    // `╭ a/b/c ─ ⧉ copy ╮` needs 2 + 7 + 1 + 8 = 18 columns.
    assert!(card_row0("a/b/c", 18).contains("⧉ copy"));
    let r = card_row0("a/b/c", 17);
    assert!(r.contains(" ⧉ ") && !r.contains("copy"), "{r}");
    assert!(r.contains("a/b/c"), "the topic keeps the room: {r}");
    // A topic too long for the card: only the glyph, the topic gets the rest.
    let r = card_row0("a/very/long/topic/path/with/many/levels/leaf", 40);
    assert!(!r.contains("copy") && r.contains("leaf"), "{r}");
}

#[test]
fn payload_copy_text_only_when_it_fits() {
    let mut a = app_depth(10);
    send(&mut a, T, b"x", false);
    a.selected = a.tree.find(T);
    // `╭ payload ─ ⧉ copy ╮` needs 2 + 9 + 1 + 8 = 20 columns.
    for (w, full) in [(20u16, true), (19, false)] {
        let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
        t.draw(|f| render_message(f, f.area(), &a, false)).unwrap();
        let l = lines(t.backend().buffer());
        let r = l.iter().find(|r| r.contains("payload")).unwrap();
        assert_eq!(r.contains("⧉ copy"), full, "{w}: {r}");
        assert!(r.contains('⧉'), "{w}: {r}");
    }
}

#[test]
fn no_copy_glyph_when_not_even_it_fits() {
    // Card: `╭ …/leafname ─ ⧉ ╮` needs 2 + (8 + 2) + 1 + 3 = 16 columns.
    assert!(card_row0("a/b/leafname", 16).contains('⧉'));
    let r = card_row0("a/b/leafname", 15);
    assert!(!r.contains('⧉') && r.contains("leafname"), "{r}");
    // Payload: `╭ payload ─ ⧉ ╮` needs 2 + 9 + 1 + 3 = 15 columns.
    let mut a = app_depth(10);
    send(&mut a, T, b"x", false);
    a.selected = a.tree.find(T);
    for (w, glyph) in [(15u16, true), (14, false)] {
        let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
        t.draw(|f| render_message(f, f.area(), &a, false)).unwrap();
        let l = lines(t.backend().buffer());
        let r = l.iter().find(|r| r.contains("payload")).unwrap();
        assert_eq!(r.contains('⧉'), glyph, "{w}: {r}");
        if !glyph {
            assert_eq!(a.hit.get().payload_copy, Rect::default());
        }
    }
    // One-row header: the glyph needs a row of at least 19 columns.
    for (w, glyph) in [(19u16, true), (18, false)] {
        let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
        t.draw(|f| render_message(f, f.area(), &a, true)).unwrap();
        let l = lines(t.backend().buffer());
        assert_eq!(l[0].contains('⧉'), glyph, "{w}: {}", l[0]);
        if !glyph {
            assert_eq!(a.hit.get().header_copy, Rect::default());
        }
    }
}

/// The last history row showing `needle`, without the borders.
fn history_row(b: &Buffer, needle: &str) -> String {
    let l = lines(b);
    let r = l.iter().rev().find(|r| r.contains(needle)).unwrap();
    r.trim_start_matches('│').trim_end_matches('│').to_owned()
}

#[test]
fn history_preview_uses_the_whole_row() {
    let long = "a".repeat(200);
    let a = app_with(&[long.as_bytes(), long.as_bytes()]);
    // 100 columns: 98 inside, `HH:MM:SS` + 2 spaces leaves 88.
    let row = history_row(&draw(&a), "aaa");
    assert_eq!(row.chars().count(), 98, "{row:?}");
    assert!(row.ends_with("a…"), "{row:?}");
    assert_eq!(row.matches('a').count(), 87);
    // Narrow pane: still fills the row, never past the border.
    for w in [20u16, 30, 45] {
        let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
        t.draw(|f| render_message(f, f.area(), &a, false)).unwrap();
        let l = lines(t.backend().buffer());
        let r = l.iter().rev().find(|r| r.contains("aaa")).unwrap();
        assert!(r.ends_with("a…│"), "{w}: {r:?}");
    }
    // A short payload is not padded or cut.
    let a = app_with(&[b"short", b"short"]);
    assert!(history_row(&draw(&a), "short").contains("  short"));
}

#[test]
fn history_preview_of_wide_chars_never_overflows() {
    let wide = "温度".repeat(100);
    let a = app_with(&[wide.as_bytes(), wide.as_bytes()]);
    for w in [20u16, 31, 45, 100] {
        let mut t = Terminal::new(TestBackend::new(w, 30)).unwrap();
        t.draw(|f| render_message(f, f.area(), &a, false)).unwrap();
        let l = lines(t.backend().buffer());
        let r = l.iter().rev().find(|r| r.contains('温')).unwrap();
        assert!(r.ends_with('│') && r.contains('…'), "{w}: {r:?}");
    }
}

/// The row of `b` containing `needle` (the last one, below the chart).
fn row_with(b: &Buffer, needle: &str) -> u16 {
    lines(b)
        .iter()
        .rposition(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("{needle}")) as u16
}

fn highlighted(b: &Buffer, y: u16) -> bool {
    (0..b.area.width).any(|x| b[(x, y)].bg == theme::SEL_BG_INACTIVE)
}

#[test]
fn chart_highlights_active_line() {
    let mut a = app_with(&[br#"{"t":21.5,"h":40}"#, br#"{"t":22,"h":41}"#]);
    a.handle_action(Action::Chart);
    let b = draw(&a);
    assert!(highlighted(&b, row_with(&b, "\"t\": 22")));
    assert!(!highlighted(&b, row_with(&b, "\"h\": 41")));
    a.handle_action(Action::Chart);
    let b = draw(&a);
    assert!(highlighted(&b, row_with(&b, "\"h\": 41")));
    assert!(!highlighted(&b, row_with(&b, "\"t\": 22")));
    a.handle_action(Action::Chart);
    let b = draw(&a);
    assert!(!highlighted(&b, row_with(&b, "\"h\": 41")), "chart off");
}

#[test]
fn chart_title_shows_position() {
    let mut a = app_with(&[br#"{"t":21.5,"h":40}"#, br#"{"t":22,"h":41}"#]);
    a.handle_action(Action::Chart);
    assert!(text(&draw(&a)).contains("chart · t · 1/2"));
    a.handle_action(Action::Chart);
    assert!(text(&draw(&a)).contains("chart · h · 2/2"));
}

#[test]
fn cycle_scrolls_highlight_into_view() {
    let body: String = (0..60)
        .map(|i| format!("\"k{i}\":{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let p = format!("{{{body}}}");
    let mut a = app_with(&[p.as_bytes(), p.as_bytes()]);
    for _ in 0..30 {
        let _ = draw(&a);
        a.handle_action(Action::Chart);
        // Check against the pane as drawn after the press.
        let _ = draw(&a);
        let l = a.chart_line().unwrap();
        let (s, h) = (a.payload_scroll, usize::from(a.payload_height.get()));
        assert!(l >= s && l < s + h, "line {l} outside {s}..{}", s + h);
    }
}

#[test]
fn first_c_shows_line_in_shrunk_pane() {
    // Line 9 sits in the top half of the tall pane, but below the pane the
    // chart leaves once it opens.
    let p = br#"{"a":"x","b":"x","c":"x","d":"x","e":"x","f":"x","g":"x","i":"x","n":1}"#;
    let mut a = app_with(&[p, p]);
    let _ = draw(&a);
    a.handle_action(Action::Chart);
    let _ = draw(&a);
    let l = a.chart_line().unwrap();
    let (s, h) = (a.payload_scroll, usize::from(a.payload_height.get()));
    assert!(l >= s && l < s + h, "line {l} outside {s}..{}", s + h);
}

#[test]
fn highlight_follows_path_across_messages() {
    let mut a = app_with(&[br#"{"t":1,"h":2}"#, br#"{"t":1,"h":3}"#]);
    a.handle_action(Action::Chart);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart_line(), Some(2));
    send(&mut a, T, br#"{"x":1,"y":5,"h":2}"#, false);
    assert_eq!(a.chart_line(), Some(3));
    let b = draw(&a);
    assert!(highlighted(&b, row_with(&b, "\"h\": 2")));
    send(&mut a, T, br#"{"x":1}"#, false);
    assert_eq!(a.chart_line(), None);
    let b = draw(&a);
    assert!(!highlighted(&b, row_with(&b, "\"x\": 1")));
}

#[test]
fn diff_view_has_no_highlight() {
    let mut a = app_with(&[br#"{"t":1}"#, br#"{"t":2}"#]);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ToggleDiff);
    assert_eq!(a.chart_line(), None);
    let b = draw(&a);
    assert!(!(0..b.area.height).any(|y| highlighted(&b, y)));
}

#[test]
fn truncated_json_renders_as_text_with_marker() {
    let settings = Settings {
        max_payload_kb: 1,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), settings).unwrap();
    a.mode = Mode::Browse;
    // 2048 bytes of valid JSON: cut at 1 KiB it no longer parses.
    let fill = "1,".repeat(1019);
    let json = format!(r#"{{"vv":[{fill}1]}}"#);
    assert_eq!(json.len(), 2048);
    send(&mut a, T, json.as_bytes(), false);
    a.selected = a.tree.find(T);
    let m = a.tree.node(a.selected.unwrap()).latest.as_ref().unwrap();
    assert_eq!((m.payload.len(), m.truncated_from), (1024, Some(2048)));
    let (body, kind) = body(&a).unwrap();
    assert_eq!(kind, "TEXT");
    let Body::Plain(l) = &*body else {
        panic!("not plain text");
    };
    assert_eq!(l.last().unwrap(), "… truncated (2048 bytes)");
    // The marker is part of what the pane scrolls over.
    assert_eq!(content_len(&a), l.len());
    for _ in 0..3 {
        a.handle_action(Action::Chart);
        let _ = draw(&a);
    }
}

#[test]
fn truncated_prefix_that_parses_as_json_is_labelled_text() {
    let settings = Settings {
        max_payload_kb: 1,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), settings).unwrap();
    a.mode = Mode::Browse;
    // Valid JSON in the first 1 KiB, padding after it.
    let json = format!("{{\"a\":1}}{}", " ".repeat(2000));
    send(&mut a, T, json.as_bytes(), false);
    a.selected = a.tree.find(T);
    let (body, kind) = body(&a).unwrap();
    assert_eq!(kind, "TEXT");
    let Body::Plain(l) = &*body else {
        panic!("not plain text");
    };
    assert!(
        l[0].starts_with("{\"a\":1}"),
        "raw text, not pretty JSON: {l:?}"
    );
    assert_eq!(l.last().unwrap(), "… truncated (2007 bytes)");
}

/// A JSON array just over `PRETTY_MAX`, on one line.
fn big_json() -> Vec<u8> {
    let mut p = b"[".to_vec();
    while p.len() < PRETTY_MAX * 3 / 2 {
        p.extend_from_slice(b"1,");
    }
    p.extend_from_slice(b"1]");
    p
}

#[test]
fn large_json_skips_pretty_print() {
    let p = big_json();
    let a = app_with(&[&p]);
    let (b, label) = body(&a).unwrap();
    assert_eq!(label, "TEXT");
    match &*b {
        Body::Plain(l) => assert_eq!(l, &[String::from_utf8(p).unwrap()]),
        _ => panic!("expected plain text"),
    }
}

#[test]
fn json_at_the_cap_is_still_pretty_printed() {
    let mut p = b"[".to_vec();
    p.extend(b"1,".repeat((PRETTY_MAX - 4) / 2));
    p.extend_from_slice(b"11]");
    assert_eq!(p.len(), PRETTY_MAX);
    let a = app_with(&[&p]);
    let (_, label) = body(&a).unwrap();
    assert_eq!(label, "JSON");
}

#[test]
fn chart_series_parsed_once_per_message() {
    let p = br#"{"a":1,"b":2}"#;
    let mut a = app_with(&[p, p]);
    a.chart = Some(Some("a".into()));
    for _ in 0..5 {
        let _ = draw(&a);
    }
    assert_eq!(a.chart_cache.builds.get(), 1);
    assert_eq!(a.chart_cache.point_builds.get(), 1);
    assert_eq!(a.chart_series(), vec![Some("a".into()), Some("b".into())]);
    assert_eq!(a.chart_line(), Some(1));
    assert_eq!(a.chart_cache.builds.get(), 1);
    send(&mut a, T, br#"{"b":1,"a":2}"#, false);
    assert_eq!(a.chart_line(), Some(2));
    assert_eq!(a.chart_cache.builds.get(), 2);
}

#[test]
fn oversize_payload_is_not_charted_and_does_not_panic() {
    let p = big_json();
    let mut a = app_with(&[&p, &p]);
    assert!(a.chart_series().is_empty());
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    // A stale path chosen on an earlier, small message.
    a.chart = Some(Some("0".into()));
    assert_eq!(a.chart_line(), None);
    let s = text(&draw(&a));
    assert!(s.contains("not enough numeric values"), "{s}");
}

#[test]
fn large_binary_hex_dump_is_capped() {
    let mut p = vec![7u8; PRETTY_MAX * 3 / 2];
    p[10] = 0;
    let a = app_with(&[&p]);
    let (b, label) = body(&a).unwrap();
    assert_eq!(label, "BINARY");
    let Body::Plain(l) = &*b else {
        panic!("expected plain")
    };
    assert!(l.len() <= PRETTY_MAX / 16 + 2, "{}", l.len());
    assert_eq!(
        l.last().unwrap(),
        &format!("… {} more bytes not shown", p.len() - PRETTY_MAX)
    );
}

fn app_with_props(props: Props) -> App {
    let mut a = app_depth(100);
    a.apply_core_event(CoreEvent::Message {
        topic: T.into(),
        msg: Message {
            payload: Bytes::from_static(b"x"),
            qos: 1,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: Some(std::sync::Arc::new(props)),
        },
    });
    a.selected = a.tree.find(T);
    a
}

fn draw_sized(a: &App, w: u16, h: u16, stacked: bool) -> Vec<String> {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render_message(f, f.area(), a, stacked)).unwrap();
    lines(t.backend().buffer())
}

#[test]
fn props_row_shown_in_card() {
    let a = app_with_props(Props {
        content_type: Some("text/plain".into()),
        user: vec![("dev".into(), "42".into())],
        ..Props::default()
    });
    let l = draw_sized(&a, 100, 30, false);
    let row = l
        .iter()
        .position(|r| r.contains("type text/plain") && r.contains("dev=42"))
        .expect("props row");
    // Card: border, badges, props, then the payload pane starts at row 4.
    assert_eq!(row, 2, "{l:#?}");
    assert!(l[4].contains("payload"), "{:?}", l[4]);
    // Without props the card is 3 rows tall.
    let plain = draw_sized(&app_with(&[b"x"]), 100, 30, false);
    assert!(plain[3].contains("payload"), "{:?}", plain[3]);
}

#[test]
fn props_row_is_one_line_and_ellipsized() {
    let a = app_with_props(Props {
        user: (0..300)
            .map(|i| (format!("k{i}"), "v".to_owned()))
            .collect(),
        ..Props::default()
    });
    let l = draw_sized(&a, 80, 30, false);
    let row = &l[2];
    assert!(row.contains("k0=v"), "{row:?}");
    let at = row.find(" +").expect("more marker");
    let rest = row[at..].trim_end().trim_end_matches('│').trim_end();
    let n: usize = rest
        .strip_prefix(" +")
        .and_then(|r| r.strip_suffix(" more"))
        .and_then(|r| r.parse().ok())
        .unwrap_or_else(|| panic!("tail {rest:?}"));
    let shown = row.matches("=v").count();
    assert_eq!(shown + n, 300);
    assert!(l[3].starts_with('╰'), "{:?}", l[3]);
}

#[test]
fn props_row_in_compact_header() {
    let a = app_with_props(Props {
        content_type: Some("text/plain".into()),
        ..Props::default()
    });
    let l = draw_sized(&a, 79, 24, true);
    assert!(l[0].contains(" TEXT "), "{:?}", l[0]);
    assert!(l[1].contains("type text/plain"), "{:?}", l[1]);
}

#[test]
fn props_row_counts_capped_pairs_in_more() {
    let props = Props {
        user: (0..300)
            .map(|i| (format!("k{i}"), "v".to_owned()))
            .collect(),
        ..Props::default()
    }
    .capped();
    let a = app_with_props(props);
    let row = draw_sized(&a, 400, 30, false)[2].clone();
    // All 32 kept pairs fit at this width; the tail counts the 268 dropped.
    assert!(row.contains("k31=v"), "{row:?}");
    assert!(row.contains("+268 more"), "{row:?}");
}

fn set_query(a: &mut App, q: &str, regex: bool) {
    use mqtt_zapp_core::search::{QueryMode, filter};
    let mode = if regex {
        QueryMode::Regex
    } else {
        QueryMode::Plain
    };
    let f = filter(&a.tree, q, mode).unwrap();
    a.set_filter(f);
}

/// Row of the first line containing `needle`.
fn row_of(b: &Buffer, needle: &str) -> u16 {
    lines(b)
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle}: {}", text(b))) as u16
}

#[test]
fn payload_highlights_filter_terms() {
    let mut a = app_with(&[br#"{"id":"WZR-7"}"#]);
    set_query(&mut a, "wzr", false);
    let b = draw(&a);
    let y = row_of(&b, "WZR");
    let c = cell_at(&b, y, "WZR");
    assert_eq!(c.symbol(), "W");
    let x = (0..b.area.width)
        .find(|&x| b[(x, y)].symbol() == "W")
        .unwrap();
    for (i, want) in [(0, true), (1, true), (2, true), (3, false)] {
        let m = b[(x + i, y)]
            .modifier
            .contains(super::super::ui::MATCH_MODIFIER);
        assert_eq!(m, want, "cell +{i}");
    }
    assert!(
        !b[(x - 1, y)]
            .modifier
            .contains(super::super::ui::MATCH_MODIFIER)
    );
}

#[test]
fn payload_highlight_keeps_json_colours() {
    let mut a = app_with(&[br#"{"id":"WZR-7"}"#]);
    set_query(&mut a, "wzr", false);
    let b = draw(&a);
    let y = row_of(&b, "WZR");
    let x = (0..b.area.width)
        .find(|&x| b[(x, y)].symbol() == "W")
        .unwrap();
    assert_eq!(b[(x, y)].fg, Color::Green);
    assert!(
        b[(x, y)]
            .modifier
            .contains(super::super::ui::MATCH_MODIFIER)
    );
}

#[test]
fn payload_highlight_skips_empty_matches() {
    let mut a = app_with(&[br#"{"k":"bab"}"#]);
    set_query(&mut a, "a*", true);
    let b = draw(&a);
    let y = row_of(&b, "bab");
    let x = (0..b.area.width)
        .find(|&x| b[(x, y)].symbol() == "b")
        .unwrap();
    let m = |i: u16| {
        b[(x + i, y)]
            .modifier
            .contains(super::super::ui::MATCH_MODIFIER)
    };
    assert!(!m(0) && m(1) && !m(2));
}

#[test]
fn payload_highlight_off_in_diff_view() {
    let mut a = app_with(&[br#"{"id":"WZR-7"}"#, br#"{"id":"WZR-8"}"#]);
    set_query(&mut a, "wzr", false);
    a.show_diff = true;
    let b = draw(&a);
    let m = super::super::ui::MATCH_MODIFIER;
    for y in 0..b.area.height {
        for x in 0..b.area.width {
            assert!(!b[(x, y)].modifier.contains(m), "({x},{y})");
        }
    }
}

fn has_fg(b: &Buffer, c: Color) -> bool {
    b.content.iter().any(|x| {
        x.fg == c
            && x.symbol()
                .chars()
                .any(|ch| ('\u{2801}'..='\u{28ff}').contains(&ch))
    })
}

#[test]
fn overlay_draws_legend_and_colours() {
    let mut a = app_with(&[
        br#"{"a":1,"b":9}"#,
        br#"{"a":5,"b":2}"#,
        br#"{"a":2,"b":7}"#,
    ]);
    a.chart = Some(Some("a".into()));
    a.chart_pinned = vec![Some("a".into())];
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(Some("b".into())));
    let b = draw(&a);
    let s = text(&b);
    assert!(s.contains("chart · b + a"), "{s}");
    // The legend box lists both names, below the title row.
    let ls = lines(&b);
    let boxed = |n: &str| ls.iter().skip(1).any(|l| l.contains(&format!("│{n}│")));
    assert!(boxed("b") && boxed("a"), "{s}");
    assert!(ls.iter().any(|l| l.contains("┌─┐")), "{s}");
    // One series: no legend.
    a.chart_pinned.clear();
    let ls = lines(&draw(&a));
    assert!(!ls.iter().any(|l| l.contains("┌─┐")), "legend hidden");
    assert!(has_fg(&b, SERIES_COLORS[0]), "current colour");
    assert!(has_fg(&b, SERIES_COLORS[1]), "pinned colour");
}

#[test]
fn overlay_missing_points_are_skipped() {
    let mut a = app_with(&[
        br#"{"b":1,"a":1}"#,
        br#"{"b":2}"#,
        br#"{"b":3,"a":2}"#,
        br#"{"b":4}"#,
        br#"{"b":5,"a":3}"#,
    ]);
    a.chart = Some(Some("b".into()));
    a.chart_pinned = vec![Some("a".into())];
    let _ = draw(&a);
    let s = shown(&a).unwrap();
    assert_eq!(chart_points(&a, &s, Some("a")).len(), 3);
    assert_eq!(chart_points(&a, &s, Some("b")).len(), 5);
}

#[test]
fn overlay_highlights_all_charted_lines() {
    let mut a = app_with(&[br#"{"a":1,"b":2,"c":3}"#, br#"{"a":2,"b":3,"c":4}"#]);
    a.chart = Some(Some("b".into()));
    a.chart_pinned = vec![Some("a".into())];
    let b = draw(&a);
    assert!(highlighted(&b, row_with(&b, "\"a\": 2")));
    assert!(highlighted(&b, row_with(&b, "\"b\": 3")));
    assert!(!highlighted(&b, row_with(&b, "\"c\": 4")));
}

fn sparkplug_app(truncated_from: Option<usize>) -> App {
    let mut a = app_depth(100);
    let json = format!(r#"{{"metrics":{{"T":1}},"pad":"{}"}}"#, "x".repeat(572));
    assert_eq!(json.len(), 600, "fixture");
    a.apply_core_event(CoreEvent::Message {
        topic: T.into(),
        msg: Message {
            payload: Bytes::from(json.into_bytes()),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from,
            props: None,
            sparkplug: true,
            wire_len: Some(120),
        },
    });
    a.selected = a.tree.find(T);
    a
}

#[test]
fn sparkplug_badge_shows_wire_size() {
    let t = text(&draw(&sparkplug_app(None)));
    assert!(t.contains(" 120 B "), "{t}");
    assert!(!t.contains(" 600 B "), "{t}");
}

#[test]
fn cut_sparkplug_keeps_badge() {
    let t = text(&draw(&sparkplug_app(Some(5000))));
    assert!(t.contains(" SPARKPLUG "), "{t}");
    assert!(t.contains(" 120 B "), "{t}");
}

#[test]
fn cut_sparkplug_note_says_decoded() {
    let t = text(&draw(&sparkplug_app(Some(5000))));
    assert!(t.contains("truncated (5000 bytes decoded)"), "{t}");
    let t = text(&draw(&app_with(&[b"plain"])));
    assert!(!t.contains("decoded"), "{t}");
}

#[test]
fn sparkplug_badge_and_chart_path() {
    let mut a = app_depth(100);
    a.apply_core_event(CoreEvent::Message {
        topic: T.into(),
        msg: Message {
            payload: Bytes::from_static(br#"{"metrics":{"Temperature":21.5},"timestamp":1}"#),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            props: None,
            sparkplug: true,
            wire_len: None,
        },
    });
    a.selected = a.tree.find(T);
    let t = text(&draw(&a));
    assert!(t.contains(" SPARKPLUG "), "{t}");
    assert!(!t.contains(" JSON "));
    assert!(
        a.chart_series()
            .contains(&Some("metrics.Temperature".into()))
    );
    assert_eq!(
        a.chart_series().first(),
        Some(&Some("metrics.Temperature".into()))
    );
}

/// The chart's top border row: the first row containing `chart ·`.
fn chart_border(b: &Buffer) -> String {
    lines(b)
        .into_iter()
        .find(|l| l.contains("chart ·"))
        .unwrap_or_default()
}

#[test]
fn chart_title_shows_close_hint() {
    let mut a = app_with(&[br#"{"a":1}"#, br#"{"a":2}"#]);
    a.handle_action(Action::Chart);
    let row = chart_border(&draw(&a));
    assert!(row.contains("esc close"), "{row}");
}

#[test]
fn chart_hint_uses_cancel_key() {
    let mut s = Settings::default();
    s.keys.insert("cancel".into(), "ctrl-q".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    send(&mut a, T, br#"{"a":1}"#, false);
    send(&mut a, T, br#"{"a":2}"#, false);
    a.selected = a.tree.find(T);
    a.handle_action(Action::Chart);
    let row = chart_border(&draw(&a));
    assert!(row.contains("ctrl-q close"), "{row}");
    assert!(!row.contains("esc close"), "{row}");
}

#[test]
fn close_hint_never_garbles_a_long_title() {
    let p: &[u8] = br#"{"temperature_outdoor":1,"humidity_outdoor":2,"pressure_sea":3}"#;
    let q: &[u8] = br#"{"temperature_outdoor":4,"humidity_outdoor":5,"pressure_sea":6}"#;
    let mut a = app_with(&[p, q]);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Chart);
    for w in 40..=100 {
        let ls = draw_sized(&a, w, 30, false);
        let row = ls.iter().find(|l| l.contains("chart ·")).unwrap();
        // Either the whole hint, or none of it: what is left of the title
        // is a clean prefix of it, never followed by a cut-off fragment.
        let full = "chart · pressure_sea + temperature_outdoor + humidity_outdoor";
        let inner = row
            .trim_start_matches(['╭', ' '])
            .trim_end_matches(['╮', ' ', '─']);
        assert!(
            inner.contains("esc close") || full.starts_with(inner),
            "w={w}: {row}"
        );
        if w == 100 {
            assert!(row.contains("esc close"), "room enough at 100: {row}");
        }
    }
}

fn send_props(a: &mut App, props: Option<Props>) {
    a.apply_core_event(CoreEvent::Message {
        topic: T.into(),
        msg: Message {
            payload: Bytes::from_static(b"x"),
            qos: 1,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: props.map(std::sync::Arc::new),
        },
    });
}

fn history_with_one_props() -> App {
    let mut a = app_depth(100);
    send_props(&mut a, None);
    send_props(
        &mut a,
        Some(Props {
            content_type: Some("text/plain".into()),
            ..Props::default()
        }),
    );
    send_props(&mut a, None);
    a.selected = a.tree.find(T);
    a
}

#[test]
fn header_height_stable_across_history() {
    let mut a = history_with_one_props();
    let top = |a: &App| {
        draw_sized(a, 100, 30, false)
            .iter()
            .position(|r| r.contains("payload"))
            .expect("payload box")
    };
    let live = top(&a);
    for i in 0..3 {
        a.history_sel = Some(i);
        assert_eq!(top(&a), live, "history_sel {i}");
    }
}

#[test]
fn entry_without_props_says_so() {
    let mut a = history_with_one_props();
    a.history_sel = Some(0);
    let l = draw_sized(&a, 100, 30, false);
    assert!(l[2].contains(" no properties"), "{:?}", l[2]);
    a.history_sel = Some(1);
    let l = draw_sized(&a, 100, 30, false);
    assert!(l[2].contains("type text/plain"), "{:?}", l[2]);
}

#[test]
fn oversized_first_chip_is_cut() {
    let a = app_with_props(Props {
        content_type: Some("a".repeat(1024)),
        ..Props::default()
    });
    let row = draw_sized(&a, 60, 30, false)[2].clone();
    assert!(row.contains("type "), "{row:?}");
    assert!(row.contains('…'), "{row:?}");
    assert!(row.width() <= 60, "{row:?}");
}

#[test]
fn hex_dump_not_underlined() {
    let mut a = app_with(&[&[0xff, 0x00, 0xfe, 0x01]]);
    set_query(&mut a, "ff", false);
    let b = draw(&a);
    assert!(text(&b).contains(" BINARY "));
    assert!(
        !b.content
            .iter()
            .any(|c| c.modifier.contains(super::super::ui::MATCH_MODIFIER)),
        "hex dump underlined"
    );
}

#[test]
fn overlay_title_names_coloured() {
    let mut a = app_with(&[
        br#"{"a":1,"b":9}"#,
        br#"{"a":5,"b":2}"#,
        br#"{"a":2,"b":7}"#,
    ]);
    a.chart = Some(Some("a".into()));
    a.chart_pinned = vec![Some("a".into())];
    a.handle_action(Action::Chart);
    let b = draw(&a);
    let y = row_of(&b, "chart · b + a");
    assert_eq!(cell_at(&b, y, "b + a").fg, SERIES_COLORS[0]);
    let x = (0..b.area.width)
        .find(|&x| b[(x, y)].symbol() == "b" && b[(x + 2, y)].symbol() == "+")
        .unwrap();
    assert_eq!(b[(x, y)].fg, SERIES_COLORS[0]);
    assert_eq!(b[(x + 2, y)].fg, theme::MUTED);
    assert_eq!(b[(x + 4, y)].fg, SERIES_COLORS[1]);
}

fn json_of(n: usize) -> Vec<u8> {
    let mut p = b"[".to_vec();
    p.extend(b"1,".repeat(n / 2));
    p.extend_from_slice(b"1]");
    p
}

#[test]
fn hot_big_topic_body_rebuild_is_throttled() {
    let big = json_of(100_000);
    let mut a = app_with(&[&big]);
    let first = draw(&a);
    assert_eq!(a.body_cache.builds.get(), 1);
    // Newer messages within BODY_MIN_INTERVAL keep the cached body ...
    send(&mut a, T, &json_of(300_000), false);
    let b = draw(&a);
    assert_eq!(a.body_cache.builds.get(), 1, "throttled");
    // ... but the header follows the newest message.
    assert_ne!(lines(&b)[..3], lines(&first)[..3], "header is current");
    // After the interval it rebuilds.
    a.body_cache
        .age(BODY_MIN_INTERVAL + std::time::Duration::from_millis(1));
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), 2);
    // Small payloads are never throttled.
    send(&mut a, T, b"small", false);
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), 3);
}

#[test]
fn throttled_body_never_belongs_to_another_topic() {
    let big = json_of(100_000);
    let mut a = app_with(&[&big]);
    let _ = draw(&a);
    send(&mut a, "other", &big, false);
    a.selected = a.tree.find("other");
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), 2);
}

#[test]
fn history_change_rebuilds_body_at_once() {
    let big = json_of(100_000);
    let mut a = app_with(&[&big]);
    let mut bigger = big.clone();
    bigger.extend_from_slice(b" ");
    send(&mut a, T, &bigger, false);
    let _ = draw(&a);
    let n = a.body_cache.builds.get();
    a.history_sel = Some(0);
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), n + 1);
    a.show_diff = true;
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), n + 2);
    a.on_resize();
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), n + 3);
}

#[test]
fn chart_disables_body_throttle() {
    let big = json_of(100_000);
    let mut a = app_with(&[&big]);
    a.chart = Some(None);
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), 1);
    send(&mut a, T, &json_of(300_000), false);
    let _ = draw(&a);
    assert_eq!(a.body_cache.builds.get(), 2, "chart open: no throttle");
}
