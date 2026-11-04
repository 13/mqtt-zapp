use super::*;

#[test]
fn stacked_payload_keeps_room_at_79x24() {
    let body = (0..20)
        .map(|i| format!("l{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let items: Vec<(&str, &[u8])> = (0..6).map(|_| ("a", body.as_bytes())).collect();
    let mut a = app_with(&items);
    a.handle_action(Action::Down);
    let s = text(&draw(&mut a, 79, 24));
    // Payload lines sit right after a border; history rows start with a time.
    for want in ["│l0 ", "│l1 ", "│l2 ", "│l3 "] {
        assert!(s.contains(want), "{want}: {s}");
    }
}

#[test]
fn history_focus_visible_at_40x10() {
    let mut a = app_with(&[("a", b"1"), ("a", b"2")]);
    a.handle_action(Action::Down);
    a.focus = Focus::History;
    let b = draw(&mut a, 40, 10);
    assert!(b.content.iter().any(|c| c.symbol() == "┏"), "{}", text(&b));
}

#[test]
fn history_focus_falls_back_when_resized_away() {
    let mut a = two_msgs();
    draw(&mut a, 100, 30);
    a.focus = Focus::History;
    let b = draw(&mut a, 40, 10);
    assert_eq!(a.focus, Focus::Payload);
    assert!(!text(&b).contains("history"), "{}", text(&b));
}

#[test]
fn narrow_terminal_stacks_panes() {
    let names: Vec<String> = (0..40).map(|i| format!("t{i:02}")).collect();
    let items: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"1"[..])).collect();
    let mut a = app_with(&items);
    a.handle_action(Action::Down); // select something first
    a.handle_action(Action::Bottom);
    let b = draw(&mut a, 60, 30);
    assert_eq!(b[(0, 1)].symbol(), "┏");
    assert_eq!(b[(59, 1)].symbol(), "┓");
    let s = text(&b);
    let second = s
        .lines()
        .skip(2)
        .position(|l| l.starts_with('╭'))
        .expect("message pane below the tree");
    assert!(second > 5, "{s}");
    assert!(s.contains("t39"), "selected row visible: {s}");
}

#[test]
fn wide_terminal_splits_40_60() {
    let mut a = app_with(&[("a", b"1")]);
    let b = draw(&mut a, 100, 24);
    assert_eq!(b[(39, 1)].symbol(), "┓");
    assert_eq!(b[(40, 1)].symbol(), "╭");
}

#[test]
fn too_small_terminal_renders_notice() {
    let mut a = app_with(&[("a", b"1")]);
    assert!(text(&draw(&mut a, 30, 8)).contains("terminal too small"));
    assert!(text(&draw(&mut a, 39, 24)).contains("terminal too small"));
    assert!(!text(&draw(&mut a, 40, 10)).contains("terminal too small"));
}

#[test]
fn escape_sequence_in_payload_is_not_rendered_raw() {
    let mut a = app_with(&[("x\x1b[31m", b"\x1b[2J")]);
    let b = draw(&mut a, 80, 24);
    assert!(b.content.iter().all(|c| !c.symbol().contains('\x1b')));
}

#[test]
fn tree_row_shows_marker_name_preview_and_count() {
    let mut a = app_with(&[("a/b", b"hello"), ("a/b", b"world"), ("a//c", b"1")]);
    a.handle_action(Action::Down);
    a.handle_action(Action::Expand);
    let s = text(&draw(&mut a, 80, 24));
    let line = |needle: &str| {
        s.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle}: {s}"))
            .chars()
            .take(32) // the tree pane at 80 columns
            .collect::<String>()
    };
    // Counts are right-aligned against the pane's right border.
    assert!(line("▾ a").ends_with(" 3 · 2t┃"), "{s}");
    assert!(line("  ▸ ∅").ends_with(" 1 · 1t┃"), "{s}");
    assert!(line("b  world").ends_with(" 2┃"), "{s}");
    assert!(s.contains("message"), "right pane placeholder: {s}");
}

#[test]
fn fresh_row_flashes_and_selection_has_background() {
    let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
    a.handle_action(Action::Down);
    let b = draw(&mut a, 80, 24);
    let s = text(&b);
    let row_of = |needle: &str| s.lines().position(|l| l.contains(needle)).unwrap() as u16;
    let sel = &b[(3, row_of("a  1"))];
    let other = &b[(3, row_of("b  2"))];
    assert_eq!(sel.bg, theme::SEL_BG);
    assert_ne!(other.bg, theme::SEL_BG);
    assert!(other.modifier.contains(FLASH_MODIFIER));
}

#[test]
fn selected_row_text_is_readable_on_selection_bg() {
    // `a` is fresh (flashing), has a muted preview, count and accent marker.
    let mut a = app_with(&[("a/x", b"1"), ("b", b"2")]);
    a.handle_action(Action::Down);
    let b = draw(&mut a, 80, 24);
    let s = text(&b);
    let y = s.lines().position(|l| l.contains("▸ a")).unwrap() as u16;
    let cells: Vec<_> = (1..79)
        .map(|x| &b[(x, y)])
        .filter(|c| c.bg == theme::SEL_BG)
        .collect();
    assert!(cells.len() > 10, "selection spans the row");
    assert!(cells.iter().all(|c| c.fg == theme::SEL_FG), "{s}");
}

#[test]
fn search_highlights_matches_and_shows_status() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("a/esp-WZR1", b"hello wzr"), ("b", b"1")]);
    a.handle_action(Action::Search);
    for c in "wzr".chars() {
        a.on_search_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let b = draw(&mut a, 80, 24);
    let s = text(&b);
    let last = s.lines().last().unwrap().trim_end().to_owned();
    assert!(last.starts_with("/wzr▏"), "{last}");
    assert!(last.ends_with("enter keep  esc clear"), "{last}");
    assert!(!s.contains("  b  1"), "non-matching row hidden: {s}");
    let tree_part = |l: &str| l.chars().take(32).collect::<String>();
    let row = s
        .lines()
        .position(|l| tree_part(l).contains("esp-WZR1  "))
        .unwrap();
    let line = s.lines().nth(row).unwrap();
    let line = &line[..line.char_indices().nth(32).unwrap().0]; // tree pane
    let col_of = |byte: usize| line[..byte].chars().count() as u16;
    let (name, prev) = (line.find("WZR").unwrap(), line.rfind("wzr").unwrap());
    let row = row as u16;
    assert!(b[(col_of(name), row)].modifier.contains(MATCH_MODIFIER));
    assert!(
        !b[(col_of(name) - 1, row)]
            .modifier
            .contains(Modifier::UNDERLINED)
    );
    assert!(
        b[(col_of(prev), row)].modifier.contains(MATCH_MODIFIER),
        "preview"
    );
    a.on_search_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.lines().last().unwrap().contains("filter: wzr (1)"), "{s}");
}

#[test]
fn tree_empty_state_lists_subscriptions() {
    let mut a = app_with(&[]);
    a.server = Some(home());
    let s = text(&draw(&mut a, 100, 24));
    assert!(s.contains("waiting for messages…"), "{s}");
    assert!(s.contains("subscribed to #, $SYS/#"), "{s}");
    a.server = None;
    let s = text(&draw(&mut a, 100, 24));
    assert!(!s.contains("subscribed to"), "{s}");
}

#[test]
fn tree_filter_no_match_message() {
    let mut a = app_with(&[("a", b"1")]);
    a.filter =
        mqtt_zapp_core::search::filter(&a.tree, "zzz", mqtt_zapp_core::search::QueryMode::Plain)
            .unwrap();
    let s = text(&draw(&mut a, 100, 24));
    assert!(s.contains("no topics match \"zzz\""), "{s}");
}

#[test]
fn retained_marker_on_row() {
    let mut a = app_with(&[("plain", b"1")]);
    retained(&mut a, "kept", b"2");
    retained(&mut a, "gone", b"");
    // A live (RETAIN=0) update keeps the marker.
    a.apply_core_event(CoreEvent::Message {
        topic: "kept".into(),
        msg: Message {
            payload: Bytes::from_static(b"2"),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        },
    });
    let s = text(&draw(&mut a, 100, 24));
    assert!(s.contains("kept ®  2"), "{s}");
    assert!(!s.contains("plain ®"), "{s}");
    assert!(!s.contains("gone ®"), "{s}");
}

#[test]
fn mono_strips_colours_keeps_selection_and_focus() {
    let (_, b) = themed(ThemeName::Mono);
    assert!(
        b.content
            .iter()
            .all(|c| c.fg == Color::Reset && c.bg == Color::Reset),
        "colour left"
    );
    let s = text(&b);
    let y = s.lines().position(|l| l.contains("a  1")).unwrap() as u16;
    assert!(b[(3, y)].modifier.contains(Modifier::REVERSED));
    assert!(s.contains('┏'));
}

#[test]
fn light_theme_swaps_inactive_selection_bg() {
    let mut a = app_with(&[("a", b"1"), ("a", b"2")]);
    a.handle_action(Action::Down);
    a.history_sel = Some(0);
    a.theme = ThemeName::Light;
    let b = draw(&mut a, 100, 24);
    assert!(b.content.iter().all(|c| c.bg != Color::DarkGray));
    assert!(b.content.iter().any(|c| c.bg == Color::Gray));
    a.theme = ThemeName::Dark;
    let b = draw(&mut a, 100, 24);
    assert!(b.content.iter().any(|c| c.bg == Color::DarkGray));
}

#[test]
fn no_color_env_forces_mono() {
    assert_eq!(theme::effective(ThemeName::Dark, None), ThemeName::Dark);
    assert_eq!(
        theme::effective(ThemeName::Light, Some("")),
        ThemeName::Light
    );
    assert_eq!(
        theme::effective(ThemeName::Dark, Some("1")),
        ThemeName::Mono
    );
}

#[test]
fn history_j_moves_highlight_down_the_screen() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    let mut a = app_with(&[
        ("dev/x", b"p1"),
        ("dev/x", b"p2"),
        ("dev/x", b"p3"),
        ("dev/x", b"p4"),
    ]);
    a.selected = a.tree.find("dev/x");
    a.focus = Focus::History;
    a.handle_key(k('j'));
    let first = history_highlight_y(&draw(&mut a, 100, 30)).expect("a pinned row");
    a.handle_key(k('j'));
    let second = history_highlight_y(&draw(&mut a, 100, 30)).unwrap();
    assert_eq!(second, first + 1, "j goes down the screen");
    a.handle_key(k('k'));
    assert_eq!(history_highlight_y(&draw(&mut a, 100, 30)), Some(first));
}

fn aged_app(stale_after_s: u64) -> App {
    let settings = Settings {
        stale_after_s,
        auto_expand: 5,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), settings).unwrap();
    a.mode = Mode::Browse;
    a.apply_core_event(CoreEvent::Message {
        topic: "zzz/jjj".into(),
        msg: Message {
            payload: Bytes::from_static(b"1"),
            qos: 0,
            retain: false,
            received_at: SystemTime::now() - std::time::Duration::from_secs(3600),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        },
    });
    a
}

fn dimmed_cell(b: &Buffer, ch: &str) -> Option<bool> {
    b.content
        .iter()
        .find(|c| c.symbol() == ch)
        .map(|c| c.modifier.contains(ratatui::style::Modifier::DIM))
}

#[test]
fn device_row_marked_offline() {
    let mut a = aged_app(0);
    a.apply_core_event(CoreEvent::Message {
        topic: "tasmota/x/LWT".into(),
        msg: Message {
            payload: Bytes::from_static(b"Offline"),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        },
    });
    let s = text(&draw(&mut a, 80, 12));
    let x = s.lines().find(|l| l.contains(" x")).unwrap();
    assert!(x.contains(" offline"), "{s}");
}

#[test]
fn stale_rows_dimmed() {
    let mut a = aged_app(300);
    let b = draw(&mut a, 80, 12);
    let s = text(&b);
    assert!(s.contains(" stale"), "{s}");
    assert_eq!(dimmed_cell(&b, "j"), Some(true), "{s}");
}

#[test]
fn stale_off_by_default() {
    let mut a = aged_app(0);
    let b = draw(&mut a, 80, 12);
    let s = text(&b);
    assert!(!s.contains(" stale"), "{s}");
    assert_eq!(dimmed_cell(&b, "j"), Some(false), "{s}");
}

#[test]
fn offline_marker_clears_on_online() {
    let mut a = aged_app(0);
    for p in [&b"Offline"[..], &b"Online"[..]] {
        a.apply_core_event(CoreEvent::Message {
            topic: "tasmota/x/LWT".into(),
            msg: Message {
                payload: Bytes::copy_from_slice(p),
                qos: 0,
                retain: false,
                received_at: SystemTime::now(),
                truncated_from: None,
                sparkplug: false,
                wire_len: None,
                props: None,
            },
        });
        let s = text(&draw(&mut a, 80, 12));
        assert_eq!(s.contains(" offline"), p == b"Offline", "{s}");
    }
}
