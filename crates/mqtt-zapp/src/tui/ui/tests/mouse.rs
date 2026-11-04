use super::*;

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
fn click_closes_help_and_beside_publish_leaves_it_open() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let _ = draw(&mut a, 100, 24);
    click(&mut a, 50, 10);
    assert_eq!(a.mode, Mode::Browse);
    a.handle_action(Action::Publish);
    let _ = draw(&mut a, 100, 24);
    click(&mut a, 10, 2);
    assert_eq!(a.mode, Mode::Browse, "the tree click goes to the tree");
    assert!(a.publish.is_some(), "the form stays open");
    assert!(a.selected.is_some());
}

#[test]
fn tab_skips_hidden_history_box() {
    let mut a = two_msgs();
    let b = draw(&mut a, 40, 10);
    assert!(!text(&b).contains("history"), "{}", text(&b));
    assert!(!a.history_shown.get());
    assert_eq!(a.focus, Focus::Tree);
    a.handle_action(Action::FocusNext);
    assert_eq!(a.focus, Focus::Payload);
    a.handle_action(Action::FocusNext);
    assert_eq!(a.focus, Focus::Tree);
}

#[test]
fn tab_reaches_history_when_shown() {
    let mut a = two_msgs();
    let b = draw(&mut a, 100, 30);
    assert!(text(&b).contains("history"), "{}", text(&b));
    assert!(a.history_shown.get());
    a.handle_action(Action::FocusNext);
    assert_eq!(a.focus, Focus::Payload);
    a.handle_action(Action::FocusNext);
    assert_eq!(a.focus, Focus::History);
    a.handle_action(Action::FocusNext);
    assert_eq!(a.focus, Focus::Tree);
}

#[test]
fn hidden_history_has_no_hit_area() {
    let mut a = two_msgs();
    draw(&mut a, 100, 30);
    assert_ne!(a.hit.get().history, Rect::default());
    draw(&mut a, 40, 10);
    assert_eq!(a.hit.get().history, Rect::default());
}

#[test]
fn tree_click_uses_drawn_rows() {
    let mut a = app_with(&[("b", b"2"), ("c", b"3")]);
    let b = draw(&mut a, 100, 24);
    let y = tree_row(&b, "c  3");
    // A topic sorting first lands after the draw, shifting every row.
    a.apply_core_event(CoreEvent::Message {
        topic: "a".into(),
        msg: Message {
            payload: Bytes::copy_from_slice(b"1"),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        },
    });
    click(&mut a, 10, y);
    assert_eq!(a.selected, a.tree.find("c"));
}

#[test]
fn tree_click_after_reset_tree_does_not_panic() {
    let names: Vec<String> = (0..20).map(|i| format!("t{i:02}")).collect();
    let items: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"1"[..])).collect();
    let mut a = app_with(&items);
    let b = draw(&mut a, 100, 30);
    let y = tree_row(&b, "t19");
    // A broker switch: the picker frames that follow never draw the tree.
    a.reset_tree();
    a.apply_core_event(CoreEvent::Message {
        topic: "x".into(),
        msg: Message {
            payload: Bytes::copy_from_slice(b"1"),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        },
    });
    click(&mut a, 10, y);
    use crossterm::event::{MouseButton, MouseEventKind};
    let _ = mouse(&mut a, MouseEventKind::Down(MouseButton::Right), 10, y);
    assert!(
        a.selected
            .is_none_or(|s| a.tree.is_alive(s) && s < a.tree.len()),
        "{:?}",
        a.selected
    );
    draw(&mut a, 100, 30);
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

#[test]
fn chart_click_with_active_filter_hits_same_line() {
    let mut a = app_with(&[
        ("dev/x", br#"{"t":21.5,"h":40}"#),
        ("dev/x", br#"{"t":22,"h":41}"#),
    ]);
    a.selected = a.tree.find("dev/x");
    let f = mqtt_zapp_core::search::filter(&a.tree, "h", mqtt_zapp_core::search::QueryMode::Plain)
        .unwrap();
    a.set_filter(f);
    let b = draw(&mut a, 100, 30);
    click_payload_row(&mut a, &b, "\"h\": 41");
    assert_eq!(a.chart, Some(Some("h".into())));
}
