use super::*;

#[test]
fn publish_keys_in_footer() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Publish);
    let last = last_line(&draw(&mut a, 40, 10));
    assert!(last.starts_with("ctrl-s send"), "{last}");
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
fn header_shows_topics_refused_badge() {
    let settings = Settings {
        max_topics: 2,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), settings).unwrap();
    a.mode = Mode::Browse;
    a.server = Some(home());
    for t in ["a", "b", "c", "d"] {
        a.apply_core_event(CoreEvent::Message {
            topic: t.into(),
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
    }
    a.dropped = 5;
    let b = draw(&mut a, 160, 24);
    let top = first_line(&b);
    assert!(top.contains("⚠ 5 dropped · ⚠ 2 topics refused"), "{top}");
    let col = top.find("2 topics refused").unwrap();
    let col = top[..col].chars().count() as u16;
    assert_eq!(b[(col, 0)].fg, theme::WARN);
    // A new connection starts a fresh tree, and a fresh count.
    a.reset_tree();
    let top = first_line(&draw(&mut a, 160, 24));
    assert!(!top.contains("refused"), "{top}");
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
    a.set_filter(mqtt_zapp_core::search::filter(&a.tree, "w.r", QueryMode::Regex).unwrap());
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(last.starts_with(" regex: w.r (1) "), "{last}");
    a.search_error = Some("unclosed group".into());
    let b = draw(&mut a, 100, 24);
    let last = last_line(&b);
    assert!(last.starts_with(" regex error: unclosed group "), "{last}");
    assert_eq!(b[(1, 23)].bg, theme::ERR);
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

#[test]
fn security_warning_survives_long_disconnect_reason() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(ServerInfo {
        warning: Some("password in cleartext"),
        ..home()
    });
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Disconnected {
            reason: "r".repeat(100),
            retry_in_ms: 2000,
        },
    ));
    let top = first_line(&draw(&mut a, 80, 24));
    assert!(top.contains("⚠ password in cleartext"), "{top}");
    assert!(top.contains('…'), "the reason is ellipsized: {top}");
    assert!(top.contains("retry 2s"), "{top}");
    assert!(
        !top.contains("mqtts://"),
        "the url goes before the warning: {top}"
    );
}

#[test]
fn header_shows_frozen_badge() {
    let mut a = app_with(&[("a", b"1")]);
    a.server = Some(home());
    let top = first_line(&draw(&mut a, 120, 24));
    assert!(!top.contains("frozen"), "{top}");
    a.frozen = true;
    a.dropped = 42;
    let b = draw(&mut a, 160, 24);
    let top = first_line(&b);
    assert!(top.contains("❄ frozen"), "{top}");
    let col = top[..top.find("❄ frozen").unwrap()].chars().count() as u16;
    assert_eq!(b[(col, 0)].fg, theme::WARN);
    // Narrow: the dropped badge goes, the frozen one stays.
    a.apply_core_event(CoreEvent::State(
        mqtt_zapp_core::client::ConnState::Connected,
    ));
    let top = first_line(&draw(&mut a, 40, 10));
    assert!(!top.contains("dropped"), "{top}");
    assert!(top.contains("❄ frozen"), "{top}");
}

#[test]
fn filter_hint_hidden_while_chart_open() {
    let mut a = app_with(&[("a/wzr", br#"{"v":1}"#), ("a/wzr", br#"{"v":2}"#)]);
    a.filter =
        mqtt_zapp_core::search::filter(&a.tree, "wzr", mqtt_zapp_core::search::QueryMode::Plain)
            .unwrap();
    a.selected = a.tree.find("a/wzr");
    a.handle_action(Action::Chart);
    a.status_note = None;
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(last.contains("filter: wzr"), "{last}");
    assert!(!last.contains("esc clear"), "chart open: {last}");
    a.handle_action(Action::Chart);
    a.status_note = None;
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(last.contains("esc clear"), "chart off: {last}");
}

#[test]
fn starred_chip_in_footer() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Down);
    a.handle_action(Action::ToggleStar);
    a.handle_action(Action::StarredOnly);
    let last = last_line(&draw(&mut a, 100, 24));
    assert!(last.contains(" ★ starred (1) "), "{last}");
}
