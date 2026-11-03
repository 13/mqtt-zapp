use super::*;
use crate::cli::Launch;
use crate::tui::app::{App, Effect, Mode, Note, ServerInfo};
use crate::tui::keymap::Action;
use mqtt_zapp_core::Message;
use mqtt_zapp_core::client::CoreEvent;
use mqtt_zapp_core::config::ThemeName;
use mqtt_zapp_core::config::{ProfileStore, Settings};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use std::time::SystemTime;

fn app_with(items: &[(&str, &[u8])]) -> App {
    let mut a = App::new(Launch::Picker, ProfileStore::default(), Settings::default()).unwrap();
    a.mode = Mode::Browse;
    for (topic, payload) in items {
        a.apply_core_event(CoreEvent::Message {
            topic: (*topic).into(),
            msg: Message {
                payload: payload.to_vec(),
                qos: 0,
                retain: false,
                received_at: SystemTime::now(),
            },
        });
    }
    a
}

fn draw(a: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render(f, a)).unwrap();
    t.backend().buffer().clone()
}

fn text(b: &Buffer) -> String {
    let w = b.area.width as usize;
    b.content
        .chunks(w)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

// --- mouse ---

fn mouse(a: &mut App, kind: crossterm::event::MouseEventKind, x: u16, y: u16) -> Effect {
    a.handle_mouse(crossterm::event::MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    })
}

fn click(a: &mut App, x: u16, y: u16) -> Effect {
    use crossterm::event::{MouseButton, MouseEventKind};
    mouse(a, MouseEventKind::Down(MouseButton::Left), x, y)
}

/// Screen row of the first line containing `needle` in the tree pane.
fn tree_row(b: &Buffer, needle: &str) -> u16 {
    text(b)
        .lines()
        .position(|l| l.chars().take(40).collect::<String>().contains(needle))
        .unwrap_or_else(|| panic!("{needle}: {}", text(b))) as u16
}

/// Screen position of the first occurrence of `needle`.
fn find_at(b: &Buffer, needle: &str) -> (u16, u16) {
    let t = text(b);
    for (y, l) in t.lines().enumerate() {
        if let Some(i) = l.find(needle) {
            return (l[..i].chars().count() as u16, y as u16);
        }
    }
    panic!("{needle} not on screen: {t}");
}

/// Screen positions of every occurrence of `needle`, top to bottom.
fn find_all(b: &Buffer, needle: &str) -> Vec<(u16, u16)> {
    let t = text(b);
    let mut v = vec![];
    for (y, l) in t.lines().enumerate() {
        for (i, _) in l.match_indices(needle) {
            v.push((l[..i].chars().count() as u16, y as u16));
        }
    }
    v
}

/// The payload pane's `⧉ copy` (the header's comes first).
fn payload_copy_at(b: &Buffer) -> (u16, u16) {
    *find_all(b, "⧉ copy").last().expect("payload copy button")
}

fn copied(e: Effect) -> (&'static str, String) {
    match e {
        Effect::Copy { what, text } => (what, text),
        e => panic!("expected a copy, got {e:?}"),
    }
}

#[test]
fn click_header_path_copies_topic() {
    let mut a = app_with(&[("dev/x", b"hello")]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 100, 24);
    // The topic is the card's title, on its top border (first message row).
    let (x, y) = find_at(&b, "╭ dev/x ");
    assert_eq!(y, 1);
    // `d` and the leaf `x` both copy the topic.
    assert_eq!(copied(click(&mut a, x + 2, y)), ("topic", "dev/x".into()));
    assert_eq!(copied(click(&mut a, x + 6, y)), ("topic", "dev/x".into()));
    // The bare border between the title and `⧉ copy` does nothing.
    let (cx, _) = find_all(&b, "⧉ copy")[0];
    let mid = (x + 8 + cx) / 2;
    assert!(matches!(click(&mut a, mid, y), Effect::None));
}

#[test]
fn click_copy_button_copies_payload() {
    let mut a = app_with(&[("dev/x", b"hello")]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 100, 24);
    let (x, y) = payload_copy_at(&b);
    assert_eq!(copied(click(&mut a, x + 2, y)), ("payload", "hello".into()));
}

#[test]
fn click_copy_button_copies_pinned_history_entry() {
    let mut a = app_with(&[("dev/x", b"first"), ("dev/x", b"second")]);
    a.selected = a.tree.find("dev/x");
    a.history_sel = Some(0);
    let b = draw(&mut a, 100, 24);
    let (x, y) = payload_copy_at(&b);
    assert_eq!(copied(click(&mut a, x, y)).1, "first");
}

#[test]
fn right_click_tree_row_copies_that_topic_without_selecting() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
    a.selected = a.tree.find("a");
    let b = draw(&mut a, 100, 24);
    let e = mouse(
        &mut a,
        MouseEventKind::Down(MouseButton::Right),
        10,
        tree_row(&b, "b  2"),
    );
    assert_eq!(copied(e), ("topic", "b".into()));
    assert_eq!(a.selected, a.tree.find("a"));
}

#[test]
fn click_header_with_nothing_shown_is_none() {
    let mut a = app_with(&[("dev/x", b"hello")]);
    let _ = draw(&mut a, 100, 24);
    assert!(matches!(click(&mut a, 60, 2), Effect::None));
}

#[test]
fn click_tree_row_selects_and_focuses() {
    let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
    a.focus = Focus::History;
    let b = draw(&mut a, 100, 24);
    click(&mut a, 10, tree_row(&b, "b  2"));
    assert_eq!(a.focus, Focus::Tree);
    assert_eq!(a.selected, a.tree.find("b"));
}

#[test]
fn click_selected_row_or_marker_toggles_expand() {
    let mut a = app_with(&[("a/x", b"1"), ("c/y", b"2")]);
    let b = draw(&mut a, 100, 24);
    let row = tree_row(&b, "▸ a");
    click(&mut a, 10, row); // selects
    assert!(a.expanded.is_empty());
    click(&mut a, 10, row); // selected again: expands
    assert!(a.expanded.contains(&a.tree.find("a").unwrap()));
    let b = draw(&mut a, 100, 24);
    click(&mut a, 1, tree_row(&b, "▸ c")); // marker of an unselected row
    assert!(a.expanded.contains(&a.tree.find("c").unwrap()));
    assert_eq!(a.selected, a.tree.find("c"));
}

#[test]
fn click_below_last_row_only_focuses() {
    let mut a = app_with(&[("a", b"1")]);
    a.focus = Focus::Payload;
    let _ = draw(&mut a, 100, 24);
    click(&mut a, 10, 20);
    assert_eq!(a.focus, Focus::Tree);
    assert_eq!(a.selected, None);
}

#[test]
fn wheel_over_tree_moves_selection() {
    use crossterm::event::MouseEventKind;
    let mut a = app_with(&[("a", b"1"), ("b", b"2"), ("c", b"3")]);
    let _ = draw(&mut a, 100, 24);
    mouse(&mut a, MouseEventKind::ScrollDown, 10, 5);
    mouse(&mut a, MouseEventKind::ScrollDown, 10, 5);
    assert_eq!(a.selected, a.tree.find("b"));
    mouse(&mut a, MouseEventKind::ScrollUp, 10, 5);
    assert_eq!(a.selected, a.tree.find("a"));
    assert_eq!(a.focus, Focus::Tree, "wheel does not move focus");
}

#[test]
fn click_and_wheel_on_payload() {
    use crossterm::event::MouseEventKind;
    let body = (0..60)
        .map(|i| format!("l{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut a = app_with(&[("a", body.as_bytes())]);
    a.handle_action(Action::Down);
    let _ = draw(&mut a, 100, 24);
    click(&mut a, 70, 5);
    assert_eq!(a.focus, Focus::Payload);
    mouse(&mut a, MouseEventKind::ScrollDown, 70, 5);
    assert_eq!(a.payload_scroll, 3);
    mouse(&mut a, MouseEventKind::ScrollUp, 70, 5);
    assert_eq!(a.payload_scroll, 0);
    assert_eq!(a.selected, a.tree.find("a"), "tree untouched");
}

#[test]
fn click_history_row_pins_that_message() {
    let mut a = app_with(&[("a", b"one"), ("a", b"two"), ("a", b"three")]);
    a.handle_action(Action::Down);
    let b = draw(&mut a, 100, 24);
    let s: Vec<String> = text(&b).lines().map(str::to_owned).collect();
    let y = s.iter().rposition(|l| l.contains("  one")).unwrap() as u16;
    click(&mut a, 70, y);
    assert_eq!(a.focus, Focus::History);
    assert_eq!(a.history_sel, Some(0));
    let y = s.iter().rposition(|l| l.contains("  three")).unwrap() as u16;
    click(&mut a, 70, y);
    assert_eq!(a.history_sel, None, "newest is live");
}

#[test]
fn picker_click_selects_then_connects() {
    use mqtt_zapp_core::config::PasswordSource::None as N;
    let mut a = picker_app(vec![profile("home", N), profile("lab", N)]);
    let b = draw(&mut a, 80, 24);
    let y = text(&b).lines().position(|l| l.contains("  lab ")).unwrap() as u16;
    assert!(matches!(click(&mut a, 20, y), Effect::None));
    assert_eq!(a.picker.sel, 1);
    assert!(matches!(click(&mut a, 20, y), Effect::Connect(p, None) if p.name == "lab"));
}

#[test]
fn click_closes_help_and_is_ignored_in_publish() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let _ = draw(&mut a, 100, 24);
    click(&mut a, 50, 10);
    assert_eq!(a.mode, Mode::Browse);
    a.handle_action(Action::Publish);
    let _ = draw(&mut a, 100, 24);
    click(&mut a, 10, 3);
    assert_eq!(a.mode, Mode::Publish);
    assert_eq!(a.selected, None);
}

#[test]
fn help_shows_every_action_at_80x24() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let s = text(&draw(&mut a, 80, 24));
    for want in [
        "quit",
        "cancel",
        "up",
        "toggle_diff",
        "ctrl-s",
        "keep filter",
    ] {
        assert!(s.contains(want), "{want}: {s}");
    }
    // Every grouped action fits, none is cut off at the bottom border.
    for (_, _, group) in HELP_GROUPS {
        for action in *group {
            assert!(s.contains(action.name()), "{}: {s}", action.name());
        }
    }
}

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
fn publish_keys_in_footer() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Publish);
    let last = last_line(&draw(&mut a, 40, 10));
    assert!(last.starts_with("ctrl-s send"), "{last}");
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
fn tab_moves_thick_border_to_history() {
    let mut a = app_with(&[("a", b"1"), ("a", b"2")]);
    a.handle_action(Action::Down);
    let first_thick = |b: &Buffer| {
        let w = b.area.width;
        (0..b.content.len())
            .find(|&i| b.content[i].symbol() == "┏")
            .map(|i| (i as u16 % w, i as u16 / w))
            .unwrap()
    };
    let b = draw(&mut a, 100, 24);
    let (x, y) = first_thick(&b);
    assert!(x < 40, "tree focused: {x}");
    assert_eq!(b[(x, y)].symbol(), "┏");
    a.handle_action(Action::FocusNext);
    a.handle_action(Action::FocusNext);
    let b = draw(&mut a, 100, 24);
    let (x, _) = first_thick(&b);
    assert!(x >= 40, "history focused: {x}");
    let tree_corner = (0..b.content.len())
        .find(|&i| b.content[i].symbol() == "╭")
        .map(|i| i as u16 % 100)
        .unwrap();
    assert!(tree_corner < 40);
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
fn status_bar_shows_state_and_rate() {
    let mut a = app_with(&[("a", b"1")]);
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Disconnected {
            reason: "refused".into(),
            retry_in_ms: 4000,
        },
    ));
    let s = text(&draw(&mut a, 80, 24));
    let top = s.lines().next().unwrap();
    assert!(top.contains("✕ refused · retry 4s"), "{top}");
    assert!(top.contains("msg/s"), "{top}");
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
fn filter_segment_coexists_with_state_and_note() {
    let mut a = app_with(&[("a/wzr", b"1")]);
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Connected,
    ));
    a.filter =
        mqtt_zapp_core::search::filter(&a.tree, "wzr", mqtt_zapp_core::search::QueryMode::Plain)
            .unwrap();
    a.set_note(Note::info("published ok"), Instant::now());
    let b = draw(&mut a, 100, 24);
    let last = last_line(&b);
    let top = first_line(&b);
    assert!(top.contains("● connected"), "{top}");
    assert!(top.contains("msg/s"), "{top}");
    assert!(last.contains("filter: wzr (1)"), "{last}");
    assert!(last.contains("published ok"), "{last}");
    assert!(!last.contains("p publish"), "note beats hint: {last}");
}

#[test]
fn browse_hint_follows_keymap_remaps() {
    let mut a = app_with(&[("a", b"1")]);
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(
        last.contains("/ search  p publish  ? help  q quit  tab focus  enter expand"),
        "{last}"
    );
    let mut s = Settings::default();
    s.keys.insert("publish".into(), "P".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(last.contains("P publish"), "{last}");
    assert!(!last.contains("p publish"), "{last}");
}

#[test]
fn password_hint_says_quit_without_profiles() {
    let mut a = picker_app(vec![]);
    a.mode = Mode::Password;
    assert!(last_line(&draw(&mut a, 100, 24)).contains("esc quit"));
    let mut a = picker_app(vec![profile(
        "x",
        mqtt_zapp_core::config::PasswordSource::Prompt,
    )]);
    a.mode = Mode::Password;
    assert!(last_line(&draw(&mut a, 100, 24)).contains("esc back"));
}

fn picker_app(profiles: Vec<mqtt_zapp_core::config::Profile>) -> App {
    let store = ProfileStore { profiles };
    App::new(Launch::Picker, store, Settings::default()).unwrap()
}

fn profile(
    name: &str,
    pw: mqtt_zapp_core::config::PasswordSource,
) -> mqtt_zapp_core::config::Profile {
    use mqtt_zapp_core::config::{Protocol, TlsConfig};
    mqtt_zapp_core::config::Profile {
        name: name.into(),
        host: "broker.lan".into(),
        port: 8883,
        protocol: Protocol::Mqtts,
        mqtt_version: Default::default(),
        ws_path: "/".into(),
        client_id: None,
        username: Some("alice".into()),
        password: pw,
        tls: TlsConfig::default(),
        subscriptions: vec!["#".into()],
    }
}

fn first_line(b: &Buffer) -> String {
    text(b).lines().next().unwrap().trim_end().to_owned()
}

fn home() -> ServerInfo {
    ServerInfo {
        name: "home".into(),
        url: "mqtts://alice@broker.lan:8883".into(),
        subscriptions: vec!["#".into(), "$SYS/#".into()],
        warning: None,
    }
}

#[test]
fn header_shows_broker_and_state() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Connected,
    ));
    let b = draw(&mut a, 120, 24);
    let top = first_line(&b);
    for want in [
        "mqtt-zapp",
        "● connected",
        "home",
        "mqtts://alice@broker.lan:8883",
    ] {
        assert!(top.contains(want), "{want}: {top}");
    }
    assert!(top.ends_with("topics"), "{top}");
}

#[test]
fn header_shows_dropped_badge() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    a.dropped = 42;
    let b = draw(&mut a, 120, 24);
    let top = first_line(&b);
    assert!(top.contains("42 dropped"), "{top}");
}

#[test]
fn header_drops_dropped_badge_first_when_narrow() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    a.dropped = 42;
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Connected,
    ));
    let b = draw(&mut a, 40, 10);
    let top = first_line(&b);
    assert!(!top.contains("dropped"), "{top}");
    assert!(top.contains("● connected"), "{top}");
}

#[test]
fn header_fits_40_cols() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Disconnected {
            reason: "a very long reason that does not fit anywhere at all".into(),
            retry_in_ms: 1000,
        },
    ));
    let b = draw(&mut a, 40, 10);
    let top = first_line(&b);
    assert!(top.starts_with(" mqtt-zapp "), "{top}");
    assert!(top.contains('✕'), "{top}");
    assert!(top.ends_with("… · retry 1s"), "{top}");
    let second = text(&b).lines().nth(1).unwrap().to_owned();
    assert!(
        second.starts_with('┏') || second.starts_with('╭'),
        "{second}"
    );
}

#[test]
fn header_sanitizes_profile_and_reason() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(ServerInfo {
        name: "h\x1b[31m".into(),
        url: "mqtt://\x1b]0;x\x07:1".into(),
        subscriptions: vec![],
        warning: None,
    });
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Disconnected {
            reason: "\x1b[2J".into(),
            retry_in_ms: 0,
        },
    ));
    let b = draw(&mut a, 120, 24);
    assert!(b.content.iter().all(|c| !c.symbol().contains('\x1b')));
    assert!(b.content.iter().all(|c| !c.symbol().contains('\x07')));
}

#[test]
fn search_footer_survives_long_query_at_40_cols() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Search);
    for c in "x".repeat(60).chars() {
        a.on_search_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert!(last_line(&draw(&mut a, 40, 10)).starts_with("/xxx"));
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

fn retained(a: &mut App, topic: &str, payload: &[u8]) {
    a.apply_core_event(CoreEvent::Message {
        topic: topic.into(),
        msg: Message {
            payload: payload.to_vec(),
            qos: 0,
            retain: true,
            received_at: SystemTime::now(),
        },
    });
}

#[test]
fn help_rows_keep_name_and_key_apart() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.contains("delete_retained x"), "{s}");
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
            payload: b"2".to_vec(),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
        },
    });
    let s = text(&draw(&mut a, 100, 24));
    assert!(s.contains("kept ®  2"), "{s}");
    assert!(!s.contains("plain ®"), "{s}");
    assert!(!s.contains("gone ®"), "{s}");
}

#[test]
fn confirm_overlay_renders_at_40x10() {
    let mut a = app_with(&[]);
    retained(&mut a, "a/b", b"1");
    a.selected = a.tree.find("a");
    a.handle_action(Action::ClearRetained);
    let b = draw(&mut a, 40, 10);
    let s = text(&b);
    assert!(s.contains("confirm"), "{s}");
    assert!(last_line(&b).starts_with("y confirm"), "{s}");
    let s = text(&draw(&mut a, 100, 24));
    assert!(s.contains("clear 1 retained message under a?"), "{s}");
}

#[test]
fn stats_overlay_shows_values_and_closes_on_key() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("$SYS/broker/clients/connected", b"7")]);
    a.handle_action(Action::Stats);
    assert_eq!(a.mode, Mode::Stats);
    let s = text(&draw(&mut a, 100, 24));
    assert!(s.contains("broker stats"), "{s}");
    assert!(s.contains("clients connected"), "{s}");
    let line = s.lines().find(|l| l.contains("clients connected")).unwrap();
    assert!(line.contains('7'), "{line}");
    a.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert_eq!(a.mode, Mode::Browse);
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Stats);
    assert!(text(&draw(&mut a, 100, 24)).contains("no $SYS data"));
    let _ = draw(&mut a, 40, 10);
}

fn themed(theme: ThemeName) -> (App, Buffer) {
    let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
    a.theme = theme;
    a.handle_action(Action::Down);
    a.history_sel = None;
    let b = draw(&mut a, 100, 24);
    (a, b)
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
fn ellipsize_counts_columns() {
    use unicode_width::UnicodeWidthStr;
    let e = ellipsize("日本語テキスト", 5);
    assert!(e.width() <= 5, "{e}");
    assert!(e.ends_with('…'), "{e}");
    assert_eq!(ellipsize("abc", 3), "abc");
}

#[test]
fn header_keeps_retry_suffix() {
    let mut a = app_with(&[("a", b"1")]);
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Disconnected {
            reason: "connection refused by the remote broker".into(),
            retry_in_ms: 3200,
        },
    ));
    let top = first_line(&draw(&mut a, 40, 10));
    assert!(top.ends_with("· retry 4s"), "{top}");
    assert!(top.contains('…'), "{top}");
}

#[test]
fn payload_focus_ignores_tree_keys() {
    let mut a = app_with(&[("a/b", b"1"), ("c", b"2")]);
    a.handle_action(Action::Down);
    let sel = a.selected;
    a.focus = Focus::Payload;
    for x in [
        Action::Expand,
        Action::ToggleExpand,
        Action::Select,
        Action::Collapse,
    ] {
        a.handle_action(x);
        assert!(a.expanded.is_empty(), "{x:?}");
        assert_eq!(a.selected, sel, "{x:?}");
    }
}

#[test]
fn footer_hints_follow_focus() {
    let mut a = app_with(&[("a", b"1")]);
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(
        last.contains("/ search") && last.contains("p publish"),
        "{last}"
    );
    a.handle_action(Action::FocusNext);
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(
        last.contains("j/k scroll") && last.contains("d diff"),
        "{last}"
    );
    assert!(!last.contains("p publish"), "{last}");
}

#[test]
fn footer_drops_hints_when_narrow() {
    let mut a = app_with(&[("a", b"1")]);
    let last = last_line(&draw(&mut a, 40, 10));
    assert!(last.chars().count() <= 40, "{last}");
    assert!(last.starts_with("/ search"), "{last}");
    assert!(last.contains("? help"), "{last}");
    // Whole hints only: every key is followed by its label.
    let words: Vec<&str> = last.split("  ").collect();
    assert!(words.iter().all(|w| w.contains(' ')), "{last}");
}

#[test]
fn error_note_is_red() {
    let mut a = app_with(&[("a", b"1")]);
    a.set_note(Note::error("publish failed: x"), Instant::now());
    let b = draw(&mut a, 100, 24);
    let last = last_line(&b);
    let col = last.find("publish failed").unwrap();
    let col = last[..col].chars().count() as u16;
    assert_eq!(b[(col, 23)].fg, theme::ERR);
    a.set_note(Note::info("published a"), Instant::now());
    let b = draw(&mut a, 100, 24);
    let last = last_line(&b);
    let col = last[..last.find("published").unwrap()].chars().count() as u16;
    assert_eq!(b[(col, 23)].fg, theme::OK);
}

#[test]
fn filter_chip_is_highlighted() {
    let mut a = app_with(&[("a/wzr", b"1")]);
    a.filter =
        mqtt_zapp_core::search::filter(&a.tree, "wzr", mqtt_zapp_core::search::QueryMode::Plain)
            .unwrap();
    let b = draw(&mut a, 100, 24);
    let last = last_line(&b);
    assert!(last.starts_with(" filter: wzr (1) "), "{last}");
    assert_eq!(b[(1, 23)].bg, theme::WARN);
    assert!(last.contains("esc clear"), "{last}");
}

#[test]
fn regex_chip_and_error_chip() {
    use mqtt_zapp_core::search::QueryMode;
    let mut a = app_with(&[("a/wzr", b"1")]);
    a.filter = mqtt_zapp_core::search::filter(&a.tree, "w.r", QueryMode::Regex).unwrap();
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(last.starts_with(" regex: w.r (1) "), "{last}");
    a.search_error = Some("unclosed group".into());
    let b = draw(&mut a, 100, 24);
    let last = last_line(&b);
    assert!(last.starts_with(" regex error: unclosed group "), "{last}");
    assert_eq!(b[(1, 23)].bg, theme::ERR);
}

#[test]
fn password_modal_shows_server() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mqtt_zapp_core::config::PasswordSource::Prompt;
    let mut a = picker_app(vec![profile("home", Prompt)]);
    a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.contains("password · home"), "{s}");
    assert!(s.contains("mqtts://alice@broker.lan:8883"), "{s}");
}

#[test]
fn help_groups_actions() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let s = text(&draw(&mut a, 100, 30));
    for want in ["navigate", "view", "actions"] {
        assert!(s.contains(want), "{want}: {s}");
    }
}

#[test]
fn overlay_fits_tiny_terminal() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let b = draw(&mut a, 40, 10);
    assert!(text(&b).contains("help"));
    a.mode = Mode::Browse;
    a.handle_action(Action::Publish);
    let s = text(&draw(&mut a, 40, 10));
    assert!(s.contains("publish"), "{s}");
}

fn last_line(b: &Buffer) -> String {
    text(b).lines().last().unwrap().trim_end().to_owned()
}

#[test]
fn empty_picker_hint() {
    let mut a = picker_app(vec![]);
    let s = text(&draw(&mut a, 90, 24));
    assert!(s.contains("no profiles – press a to add one"), "{s}");
    assert!(!s.contains("load error"));
    a.store_error = Some("load error: bad toml".into());
    let s = text(&draw(&mut a, 90, 24));
    let err = s
        .lines()
        .position(|l| l.contains("load error: bad toml"))
        .unwrap();
    let hint = s.lines().position(|l| l.contains("no profiles")).unwrap();
    assert!(err < hint, "error above hint: {s}");
}

#[test]
fn picker_lists_profiles_with_selection_and_hint() {
    use mqtt_zapp_core::config::PasswordSource::None as N;
    let mut a = picker_app(vec![profile("home", N), profile("lab", N)]);
    a.picker.sel = 1;
    let b = draw(&mut a, 80, 24);
    let s = text(&b);
    let row = s.lines().position(|l| l.contains("▶ lab")).unwrap();
    let line = s.lines().nth(row).unwrap();
    assert!(line.contains("mqtts://alice@broker.lan:8883"), "{s}");
    let col = line[..line.find("lab").unwrap()].chars().count() as u16;
    assert_eq!(b[(col, row as u16)].bg, theme::SEL_BG);
    let other = s.lines().position(|l| l.contains("  home ")).unwrap() as u16;
    assert_ne!(b[(col, other)].bg, theme::SEL_BG);
    assert!(
        last_line(&b).contains(
            "enter connect  a add  e edit  D delete  j/k move  J/K reorder  s sort  q quit"
        ),
        "{s}"
    );
}

#[test]
fn profile_form_drawn_over_picker() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mqtt_zapp_core::config::PasswordSource::None as N;
    let mut a = picker_app(vec![profile("home", N)]);
    a.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    let b = draw(&mut a, 80, 24);
    let s = text(&b);
    assert!(s.contains("add profile"), "{s}");
    assert!(last_line(&b).contains("ctrl-s save"), "{s}");
    // Deleting asks on the front screen too.
    a.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    a.handle_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE));
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.contains("delete profile home?"), "{s}");
    assert!(s.contains("home"), "picker behind it: {s}");
}

#[test]
fn password_render_is_masked() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mqtt_zapp_core::config::PasswordSource::Prompt;
    let mut a = picker_app(vec![profile("home", Prompt)]);
    a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for c in "pw".chars() {
        a.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let b = draw(&mut a, 80, 24);
    let s = text(&b);
    assert!(s.contains("••"), "{s}");
    assert!(!s.contains("pw"), "{s}");
    assert!(!s.contains("•••"), "one dot per char: {s}");
    assert!(last_line(&b).contains("enter connect  esc back"), "{s}");
}

#[test]
fn disconnected_state_in_status_bar() {
    let mut a = app_with(&[]);
    a.server = Some(home());
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Disconnected {
            reason: "connection refused".into(),
            retry_in_ms: 3200,
        },
    ));
    let b = draw(&mut a, 120, 24);
    let top = first_line(&b);
    assert!(top.contains("✕ connection refused · retry 4s"), "{top}");
    assert!(top.contains("home"), "{top}");
    assert!(top.ends_with("0.0 msg/s · 0 topics"), "{top}");
    let col = top.chars().position(|c| c == '✕').unwrap() as u16;
    assert_eq!(b[(col, 0)].fg, theme::ERR);
}

#[test]
fn connected_and_connecting_markers() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Connected,
    ));
    let b = draw(&mut a, 100, 24);
    let top = first_line(&b);
    assert!(top.contains("● connected"), "{top}");
    assert!(top.contains("home"), "{top}");
    assert!(top.contains("1 topics"), "{top}");
    let col = top.chars().position(|c| c == '●').unwrap() as u16;
    assert_eq!(b[(col, 0)].fg, theme::OK);
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Connecting,
    ));
    let b = draw(&mut a, 100, 24);
    let top = first_line(&b);
    assert!(top.contains("◌ connecting"), "{top}");
    let col = top.chars().position(|c| c == '◌').unwrap() as u16;
    assert_eq!(b[(col, 0)].fg, theme::WARN);
}

#[test]
fn publish_overlay_renders_fields_and_error_title() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("a/b", b"1")]);
    a.handle_action(Action::Publish);
    let s = text(&draw(&mut a, 80, 24));
    for want in [
        "publish",
        "topic: ",
        "payload",
        "QoS: [0] 1 2   Retain: [ ]",
        "ctrl-s send  ctrl-e editor  ctrl-p/n history",
        "topics",
    ] {
        assert!(s.contains(want), "{want}: {s}");
    }
    a.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.contains("publish - invalid topic"), "{s}");
}

#[test]
fn status_note_is_sanitized_and_expires() {
    let mut a = app_with(&[]);
    a.set_note(Note::error("publish failed: \x1b[2Jboom"), Instant::now());
    let b = draw(&mut a, 100, 24);
    assert!(b.content.iter().all(|c| !c.symbol().contains('\x1b')));
    assert!(last_line(&b).contains("publish failed: \u{fffd}[2Jboom"));
    a.status_note = Some((
        Note::info("old"),
        Instant::now() - std::time::Duration::from_secs(6),
    ));
    assert!(!last_line(&draw(&mut a, 100, 24)).contains("old"));
}

#[test]
fn help_overlay_lists_effective_keymap() {
    let mut st = Settings::default();
    st.keys.insert("quit".into(), "x".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), st).unwrap();
    a.mode = Mode::Browse;
    a.handle_action(Action::Help);
    assert_eq!(a.mode, Mode::Help);
    let s = text(&draw(&mut a, 100, 30));
    let line = s.lines().find(|l| l.contains("quit")).expect("quit row");
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| w.contains("quit")).unwrap();
    assert_eq!(words[at + 1], "x", "{line}");
    assert!(!words.contains(&"q"), "{line}");
    for want in [
        "toggle_diff",
        "d",
        "ctrl-d",
        "k, up",
        "ctrl-s",
        "publish form",
    ] {
        assert!(s.contains(want), "{want}: {s}");
    }
}

#[test]
fn picker_title_and_marker_show_connected() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mqtt_zapp_core::config::PasswordSource::None as N;
    let mut a = picker_app(vec![profile("home", N), profile("lab", N)]);
    a.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    a.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.contains("pick a broker · custom · connected: lab"), "{s}");
    let lab = s
        .lines()
        .find(|l| l.contains("lab ") && l.contains("://"))
        .unwrap();
    assert!(lab.contains('●'), "{lab}");
    let home = s
        .lines()
        .find(|l| l.contains("home ") && l.contains("://"))
        .unwrap();
    assert!(!home.contains('●'), "{home}");
    assert!(s.contains("esc back"), "{s}");
    a.profile_deleted("lab");
    let s = text(&draw(&mut a, 80, 24));
    let lab = s
        .lines()
        .find(|l| l.contains("lab ") && l.contains("://"))
        .unwrap();
    assert!(!lab.contains('●') && !s.contains("connected:"), "{s}");
}

#[test]
fn click_broker_name_opens_picker() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    let b = draw(&mut a, 120, 24);
    let (x, y) = find_at(&b, "home");
    assert_eq!(y, 0);
    assert!(matches!(click(&mut a, x + 1, y), Effect::None));
    assert_eq!(a.mode, Mode::Picker);
    // The badge left of it does nothing.
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    let _ = draw(&mut a, 120, 24);
    click(&mut a, 2, 0);
    assert_eq!(a.mode, Mode::Browse);
}

#[test]
fn click_header_copy_copies_topic() {
    let mut a = app_with(&[("dev/x", b"hello")]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 100, 24);
    let (x, y) = find_all(&b, "⧉ copy")[0];
    // On the card's top border, the first row of the message area.
    assert_eq!(y, 1);
    assert!(text(&b).lines().nth(1).unwrap().contains('╭'));
    assert_eq!(copied(click(&mut a, x + 2, y)), ("topic", "dev/x".into()));
}

#[test]
fn header_and_payload_copy_buttons_are_distinct() {
    let mut a = app_with(&[("dev/x", b"hello")]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 100, 24);
    let all = find_all(&b, "⧉ copy");
    assert_eq!(all.len(), 2, "{}", text(&b));
    assert_ne!(all[0].1, all[1].1);
    assert_eq!(copied(click(&mut a, all[0].0, all[0].1)).0, "topic");
    assert_eq!(copied(click(&mut a, all[1].0, all[1].1)).0, "payload");
    // Nothing shown: neither old position copies.
    a.selected = None;
    let _ = draw(&mut a, 100, 24);
    for (x, y) in all {
        assert!(matches!(click(&mut a, x + 1, y), Effect::None));
    }
}

#[test]
fn compact_header_has_copy_glyph() {
    let mut a = app_with(&[("dev/x", b"hello")]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 79, 24);
    let row = text(&b).lines().position(|l| l.contains("dev/x")).unwrap() as u16;
    let line = text(&b)
        .lines()
        .nth(row as usize)
        .unwrap()
        .trim_end()
        .to_owned();
    assert!(line.ends_with('⧉'), "{line:?}");
    let x = line.chars().count() as u16 - 1;
    assert_eq!(copied(click(&mut a, x, row)), ("topic", "dev/x".into()));
}

/// The screen row of the highlighted history entry (focused selection bg).
fn history_highlight_y(b: &Buffer) -> Option<u16> {
    let area = b.area;
    (0..area.height).rev().find(|&y| {
        let l: String = (0..area.width).map(|x| b[(x, y)].symbol()).collect();
        l.contains("  p") && (0..area.width).any(|x| b[(x, y)].bg == theme::SEL_BG)
    })
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

#[test]
fn history_wheel_and_jk_agree() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind};
    let items: &[(&str, &[u8])] = &[("dev/x", b"p1"), ("dev/x", b"p2"), ("dev/x", b"p3")];
    let mut wheel = app_with(items);
    wheel.selected = wheel.tree.find("dev/x");
    let b = draw(&mut wheel, 100, 30);
    let y = text(&b)
        .lines()
        .collect::<Vec<_>>()
        .iter()
        .rposition(|l| l.contains("  p1"))
        .unwrap() as u16;
    mouse(&mut wheel, MouseEventKind::ScrollDown, 70, y);
    let mut keys = app_with(items);
    keys.selected = keys.tree.find("dev/x");
    keys.focus = Focus::History;
    keys.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(wheel.history_sel.is_some());
    assert_eq!(wheel.history_sel, keys.history_sel);
}

#[test]
fn click_payload_title_copies_payload() {
    let mut a = app_with(&[("dev/x", b"hello")]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 100, 24);
    let (x, y) = find_at(&b, " payload ");
    assert_eq!(copied(click(&mut a, x + 1, y)), ("payload", "hello".into()));
    assert_eq!(copied(click(&mut a, x + 7, y)), ("payload", "hello".into()));
    // The bare border right of the title does not copy.
    assert!(!matches!(click(&mut a, x + 12, y), Effect::Copy { .. }));
    // Focused (` ● payload `) the title is wider and still copies.
    a.focus = crate::tui::app::Focus::Payload;
    let b = draw(&mut a, 100, 24);
    let (x, y) = find_at(&b, "● payload ");
    assert_eq!(copied(click(&mut a, x + 8, y)), ("payload", "hello".into()));
}

/// Click the payload row containing `needle` (the last one on screen).
fn click_payload_row(a: &mut App, b: &Buffer, needle: &str) -> Effect {
    let l: Vec<String> = text(b).lines().map(str::to_owned).collect();
    let y = l
        .iter()
        .rposition(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("{needle}"));
    click(a, 70, y as u16)
}

#[test]
fn click_numeric_line_charts_it() {
    let mut a = app_with(&[
        ("dev/x", br#"{"t":21.5,"h":40}"#),
        ("dev/x", br#"{"t":22,"h":41}"#),
    ]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 100, 30);
    assert!(matches!(
        click_payload_row(&mut a, &b, "\"h\": 41"),
        Effect::None
    ));
    assert_eq!(a.chart, Some(Some("h".into())));
    assert_eq!(a.focus, crate::tui::app::Focus::Payload);
    let b = draw(&mut a, 100, 30);
    click_payload_row(&mut a, &b, "\"h\": 41");
    assert_eq!(a.chart, None, "clicking the charted line turns it off");
    let b = draw(&mut a, 100, 30);
    click_payload_row(&mut a, &b, "\"t\": 22");
    let b = draw(&mut a, 100, 30);
    // A line without a number leaves the chart alone.
    let y = text(&b)
        .lines()
        .collect::<Vec<_>>()
        .iter()
        .rposition(|r| r.contains("┃}"))
        .unwrap_or_else(|| panic!("{}", text(&b)));
    click(&mut a, 70, y as u16);
    assert_eq!(a.chart, Some(Some("t".into())));
}

#[test]
fn click_same_spot_twice_toggles() {
    // Opening the chart shrinks the payload pane; the clicked line must stay
    // under the pointer so a second click there turns the chart off.
    let body: String = (0..20)
        .map(|i| format!("\"k{i}\":{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let p = format!("{{{body}}}");
    let mut a = app_with(&[("dev/x", p.as_bytes()), ("dev/x", p.as_bytes())]);
    a.selected = a.tree.find("dev/x");
    let b = draw(&mut a, 100, 30);
    let y = text(&b)
        .lines()
        .collect::<Vec<_>>()
        .iter()
        .rposition(|r| r.contains("\"k15\": 15"))
        .unwrap() as u16;
    click(&mut a, 70, y);
    assert_eq!(a.chart, Some(Some("k15".into())));
    let b = draw(&mut a, 100, 30);
    let row = text(&b).lines().nth(usize::from(y)).unwrap().to_owned();
    assert!(row.contains("\"k15\": 15"), "row {y} is now {row:?}");
    click(&mut a, 70, y);
    assert_eq!(a.chart, None);
}

#[test]
fn click_numeric_line_when_scrolled() {
    let body: String = (0..60)
        .map(|i| format!("\"k{i}\":{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let p = format!("{{{body}}}");
    let mut a = app_with(&[("dev/x", p.as_bytes())]);
    a.selected = a.tree.find("dev/x");
    let _ = draw(&mut a, 100, 30);
    a.payload_scroll = 20;
    let b = draw(&mut a, 100, 30);
    click_payload_row(&mut a, &b, "\"k22\": 22");
    assert_eq!(a.chart, Some(Some("k22".into())));
}

#[test]
fn click_uses_pinned_payload() {
    let mut a = app_with(&[("dev/x", br#"{"a":1}"#), ("dev/x", br#"{"b":1,"a":2}"#)]);
    a.selected = a.tree.find("dev/x");
    a.history_sel = Some(0);
    let b = draw(&mut a, 100, 30);
    click_payload_row(&mut a, &b, "\"a\": 1");
    assert_eq!(a.chart, Some(Some("a".into())));
}

/// The confirm card alone on a `w`×`h` screen; its inner rows, trimmed.
fn confirm_rows(prompt: &str, w: u16, h: u16) -> Vec<String> {
    use crate::tui::app::{Confirm, ConfirmAction};
    let c = Confirm {
        prompt: prompt.to_owned(),
        action: ConfirmAction::DeleteTopic("t".into()),
    };
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render_confirm(f, f.area(), &c)).unwrap();
    text(t.backend().buffer())
        .lines()
        .filter_map(|r| {
            let (a, b) = (r.find('┃')?, r.rfind('┃')?);
            (b > a).then(|| r[a + '┃'.len_utf8()..b].trim().to_owned())
        })
        .collect()
}

#[test]
fn confirm_wraps_long_prompt() {
    let topic =
        "home/livingroom/sensors/temperature/very/long/path/that/goes/on/and/on/forever/leaf";
    let prompt = format!("delete {topic} and clear 12 retained on the broker?");
    for w in [30, 50, 70, 120] {
        let rows = confirm_rows(&prompt, w, 30);
        let joined: String = rows.concat().split_whitespace().collect();
        let want: String = prompt.split_whitespace().collect();
        assert!(joined.contains(&want), "w={w}: {rows:#?}");
        assert!(
            rows.last().unwrap().starts_with("y yes"),
            "w={w}: {rows:#?}"
        );
    }
}

#[test]
fn confirm_short_prompt_stays_one_line() {
    let rows = confirm_rows("delete retained message on a/b?", 100, 24);
    assert!(
        rows.iter().any(|r| r == "delete retained message on a/b?"),
        "{rows:#?}"
    );
}

#[test]
fn confirm_tiny_screen_does_not_panic() {
    let p = "x".repeat(300);
    for (w, h) in [(1, 1), (5, 3), (12, 6), (20, 4)] {
        let _ = confirm_rows(&p, w, h);
    }
}

#[test]
fn wrap_words_fits_width_and_keeps_text() {
    use unicode_width::UnicodeWidthStr;
    assert_eq!(
        wrap_words("delete aa/bbbb/cc now", 9),
        ["delete", "aa/bbbb/", "cc now"]
    );
    for s in [
        "delete 设备/房间/温度 and clear 3?",
        "x".repeat(50).as_str(),
        "a/b c",
    ] {
        for w in 1..30 {
            let out = wrap_words(s, w);
            assert!(
                out.iter().all(|l| l.width() <= w.max(2)),
                "{s} w={w}: {out:?}"
            );
            let joined: String = out.concat().split_whitespace().collect();
            assert_eq!(joined, s.split_whitespace().collect::<String>(), "w={w}");
        }
    }
}

#[test]
fn header_shows_security_warning() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(ServerInfo {
        warning: Some("unverified tls"),
        ..home()
    });
    let b = draw(&mut a, 120, 24);
    let top = first_line(&b);
    let url = top.find("mqtts://").unwrap();
    let warn = top.find("⚠ unverified tls").expect(&top);
    assert!(warn > url, "after the URL: {top}");
    let col = top[..warn].chars().count() as u16;
    assert_eq!(b[(col, 0)].fg, theme::ERR);

    // Narrow: the drop badge and stats go first, the warning outlasts the URL.
    a.dropped = 3;
    let b = draw(&mut a, 70, 24);
    let top = first_line(&b);
    assert!(top.contains("⚠ unverified tls"), "{top}");
    assert!(!top.contains("dropped"), "{top}");
}
