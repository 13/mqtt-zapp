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

/// The card's badge row: the line just below the path row.
fn badge_row(l: &[String]) -> (usize, &String) {
    let y = l.iter().position(|r| r.contains("dev/")).unwrap() + 1;
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
    assert!(l[0].starts_with('╭'), "{}", l[0]);
    assert!(l[3].starts_with('╰'), "{}", l[3]);
    assert!(
        l[1].contains("tasmota/tele/tasmota_A1B2C3/ SENSOR "),
        "{}",
        l[1]
    );
    let x = l[1][..l[1].find("SENSOR").unwrap()].chars().count() as u16;
    assert_eq!(b[(x, 1)].bg, theme::ACCENT);
    let px = l[1][..l[1].find("tasmota").unwrap()].chars().count() as u16;
    assert_eq!(b[(px, 1)].fg, theme::MUTED);
    assert!(l[2].contains(" JSON "), "{}", l[2]);
    let jx = l[2][..l[2].find("JSON").unwrap()].chars().count() as u16;
    assert_eq!(b[(jx, 2)].bg, theme::ACCENT);
    assert!(
        l[2].contains(" QoS 1 ") && l[2].contains(" RETAINED "),
        "{}",
        l[2]
    );
}

#[test]
fn header_no_retained_badge_when_not_retained() {
    let a = app_with(&[b"x"]);
    let l = lines(&draw(&a));
    assert!(!l[2].contains("RETAINED"), "{}", l[2]);
}

#[test]
fn header_long_cjk_topic_keeps_leaf() {
    let (parents, leaf) = fit_topic("设备/房间/很长很长很长很长很长很长很长很长/温度", 30);
    assert_eq!(leaf, "温度");
    assert!(parents.width() + leaf.width() + 2 <= 30, "{parents}");
    assert!(parents.contains('…') && parents.ends_with('/'), "{parents}");
    let (p, leaf) = fit_topic("a/b/c", 40);
    assert_eq!((p.as_str(), leaf.as_str()), ("a/b/", "c"));
    let (p, leaf) = fit_topic(&format!("a/{}", "x".repeat(50)), 12);
    assert!(p.is_empty(), "{p}");
    assert!(leaf.starts_with('…') && leaf.width() + 2 <= 12, "{leaf}");
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
    assert!(l[1].ends_with('│'), "{:?}", l[1]);
    assert!(l[1].contains("温 度"), "leaf kept: {:?}", l[1]);
    let x = (0..40).find(|&x| b[(x, 1)].symbol() == "温").unwrap();
    assert_eq!(b[(x, 1)].bg, theme::ACCENT);
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
            l[0].contains("dev/ x ") && l[0].contains(" TEXT "),
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
