use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn help_search_highlights_matching_row() {
    let mut a = help_app("payload");
    let b = draw(&mut a, 100, 30);
    // copy_payload row: keys in accent bold, the match marked like search.
    let keys = style_on_row(&b, "copy_payload", "Y");
    assert_eq!(keys.fg, Some(theme::ACCENT), "{keys:?}");
    assert!(keys.add_modifier.contains(Modifier::BOLD));
    let m = style_on_row(&b, "copy_payload", "payload");
    assert!(m.add_modifier.contains(Modifier::UNDERLINED), "{m:?}");
    // A non-matching row is muted, keys included.
    let quit = style_on_row(&b, "quit", "quit");
    assert_eq!(quit.fg, Some(theme::MUTED));
    let qk = style_on_row(&b, "quit", "q");
    assert_eq!(qk.fg, Some(theme::MUTED));
    // Without a query nothing is dimmed.
    let mut a = help_app("");
    let b = draw(&mut a, 100, 30);
    assert_ne!(style_on_row(&b, "copy_payload", "Y").fg, Some(theme::MUTED));
}

#[test]
fn help_search_matches_key_text() {
    let mut a = help_app("ctrl-d");
    let b = draw(&mut a, 100, 30);
    let hit = style_on_row(&b, "scroll_down", "ctrl-d");
    assert_eq!(hit.fg, Some(theme::ACCENT));
    assert!(hit.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(
        style_on_row(&b, "scroll_up", "scroll_up").fg,
        Some(theme::MUTED)
    );
}

#[test]
fn help_title_shows_count() {
    let s = text(&draw(&mut help_app(""), 100, 30));
    assert!(s.contains("help · type to search · esc closes"), "{s}");
    assert!(s.contains("f freezes") && s.contains("shift-drag"), "{s}");
    let s = text(&draw(&mut help_app("ctrl-d"), 100, 30));
    assert!(s.contains("help · /ctrl-d · 1 found · esc clears"), "{s}");
    let s = text(&draw(&mut help_app("zzzz"), 100, 30));
    assert!(s.contains("0 found"), "{s}");
}

#[test]
fn help_search_no_match_does_not_panic_at_40x10() {
    for q in ["zzzz", "é世界", "a", ""] {
        let mut a = help_app(q);
        let _ = draw(&mut a, 40, 10);
        let _ = draw(&mut a, 80, 24);
    }
}

#[test]
fn help_reaches_every_action_at_80x24() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let mut seen = String::new();
    for _ in 0..40 {
        seen += &text(&draw(&mut a, 80, 24));
        a.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    }
    for want in [
        "quit",
        "cancel",
        "up",
        "toggle_diff",
        "ctrl-s",
        "keep filter",
    ] {
        assert!(seen.contains(want), "{want}: {seen}");
    }
    for (_, _, group) in HELP_GROUPS {
        for action in *group {
            assert!(seen.contains(action.name()), "{}", action.name());
        }
    }
}

#[test]
fn help_scrolls_with_arrows() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let s = text(&draw(&mut a, 80, 14));
    assert!(s.contains("1-"), "{s}");
    assert_eq!(a.help_scroll, 0);
    a.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(a.help_scroll, 1);
    let s = text(&draw(&mut a, 80, 14));
    assert!(s.contains(" 2-"), "{s}");
    a.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    a.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(a.help_scroll, 0);
    a.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(a.help_scroll > 1);
    // j types into the filter instead of scrolling.
    a.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert_eq!(a.help_query, "j");
}

#[test]
fn help_filter_resets_scroll() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let _ = draw(&mut a, 80, 14);
    a.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert!(a.help_scroll > 0);
    a.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert_eq!(a.help_scroll, 0);
    let _ = draw(&mut a, 80, 14);
    a.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    a.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(a.help_scroll, 0);
    a.handle_action(Action::Cancel);
    a.handle_action(Action::Help);
    assert_eq!(a.help_scroll, 0);
}

#[test]
fn help_rows_keep_name_and_key_apart() {
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.contains("delete_retained x"), "{s}");
}

#[test]
fn confirm_overlay_renders_at_40x10() {
    let mut a = app_with(&[]);
    a.conn_state = mqtt_zapp_core::client::ConnState::Connected;
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

#[test]
fn empty_picker_hint() {
    let mut a = picker_app(vec![]);
    let s = text(&draw(&mut a, 90, 24));
    assert!(s.contains(&crate::tui::picker::empty_hint("a")), "{s}");
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
fn publish_overlay_renders_fields_and_error_title() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("a/b", b"1")]);
    a.handle_action(Action::Publish);
    let s = text(&draw(&mut a, 80, 24));
    for want in [
        "publish",
        "topic: ",
        "payload",
        "QoS [0] 1 2  Retain [ ]  Format [raw] json xml",
        "topics",
    ] {
        assert!(s.contains(want), "{want}: {s}");
    }
    assert_eq!(s.matches("ctrl-s send").count(), 1, "hints once: {s}");
    assert!(last_line(&draw(&mut a, 80, 24)).starts_with("ctrl-s send  enter next"));
    a.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    for _ in 0..5 {
        a.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
    }
    a.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    let s = text(&draw(&mut a, 80, 24));
    assert!(s.contains("✕ invalid topic"), "{s}");
}

#[test]
fn help_overlay_lists_effective_keymap() {
    let mut st = Settings::default();
    st.keys.insert("quit".into(), "Q".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), st).unwrap();
    a.mode = Mode::Browse;
    a.handle_action(Action::Help);
    assert_eq!(a.mode, Mode::Help);
    let s = text(&draw(&mut a, 100, 30));
    let line = s.lines().find(|l| l.contains("quit")).expect("quit row");
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| w.contains("quit")).unwrap();
    assert_eq!(words[at + 1], "Q", "{line}");
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
fn picker_renders_values_and_underlines() {
    let fields: Vec<String> = (0..50).map(|i| format!("\"f{i}\":{i}")).collect();
    let p = format!("{{{}}}", fields.join(","));
    let mut a = app_with(&[("dev/x", p.as_bytes())]);
    a.selected = a.tree.find("dev/x");
    a.handle_action(Action::ChartPick);
    for c in "f4".chars() {
        a.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let b = draw(&mut a, 100, 30);
    let t = text(&b);
    let row = t.lines().find(|l| l.contains("f40")).expect(&t);
    assert!(row.contains("40"), "{row}");
    let m = style_on_row(&b, "f40", "f4");
    assert!(m.add_modifier.contains(MATCH_MODIFIER), "{m:?}");
    let rest = style_on_row(&b, "f40", "0 ");
    assert!(
        !rest.add_modifier.contains(Modifier::UNDERLINED),
        "{rest:?}"
    );
}

#[test]
fn pending_lookup_shows_persistently() {
    use mqtt_zapp_core::config::PasswordSource::Keyring as K;
    let mut a = picker_app(vec![profile("home", K)]);
    a.begin_connect(profile("home", K));
    let s = text(&draw(&mut a, 90, 24));
    assert!(s.contains("connecting… (waiting for password)"), "{s}");
    // Long past the note TTL: still drawn, from state.
    assert!(
        a.note_at(std::time::Instant::now() + 2 * crate::tui::app::NOTE_TTL)
            .is_none()
    );
    let s = text(&draw(&mut a, 90, 24));
    assert!(s.contains("connecting… (waiting for password)"), "{s}");
}

#[test]
fn publish_form_docks_in_message_pane_under_history() {
    let mut a = app_with(&[("a", b"1"), ("a", b"2"), ("a", b"3")]);
    a.handle_action(Action::Down);
    a.handle_action(Action::Publish);
    let s = text(&draw(&mut a, 120, 40));
    let rows: Vec<&str> = s.lines().collect();
    let row_of = |needle: &str| rows.iter().position(|r| r.contains(needle));
    let hist = row_of("history · ").expect("history box shown");
    let form = row_of("● publish").expect("publish form shown");
    assert!(form > hist, "form below history: {s}");
    // Right pane only: the tree keeps its column, undimmed beside the form.
    let col = rows[form]
        .chars()
        .position(|c| c == '╭' || c == '┏')
        .unwrap();
    assert!(
        col >= 120 * 40 / 100,
        "form starts in the message pane: {s}"
    );
    // The form has focus; the tree beside it is drawn unfocused.
    assert!(
        rows[form].starts_with('│'),
        "tree unfocused beside the form: {s}"
    );
    let form_h = crate::tui::publish::height(a.publish.as_ref().unwrap(), 72);
    assert_eq!(
        form + usize::from(form_h),
        rows.len() - 1,
        "form ends above the footer"
    );
}

#[test]
fn left_form_stays_drawn_unfocused_and_a_click_returns() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
    let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
    a.handle_action(Action::Down);
    a.handle_action(Action::Publish);
    a.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    let s = text(&draw(&mut a, 120, 40));
    let rows: Vec<&str> = s.lines().collect();
    let form = rows
        .iter()
        .position(|r| r.contains("publish ─"))
        .expect("thin border");
    assert!(rows[1].starts_with('┏'), "tree focused: {s}");
    assert!(last_line(&draw(&mut a, 120, 40)).contains("p back to form"));
    // A click on the tree while in the form leaves it; on the form, enters.
    mouse(
        &mut a,
        MouseEventKind::Down(MouseButton::Left),
        60,
        form as u16 + 1,
    );
    assert_eq!(a.mode, Mode::Publish);
    draw(&mut a, 120, 40);
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), 2, 2);
    assert_eq!(a.mode, Mode::Browse);
    assert!(a.publish.is_some());
}

#[test]
fn tab_skips_hidden_message_pane() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Down);
    a.handle_action(Action::Publish);
    a.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    draw(&mut a, 79, 24);
    assert!(
        !a.message_shown.get(),
        "stacked 79x24: the form fills the pane"
    );
    a.handle_action(Action::FocusNext);
    assert_eq!(a.mode, Mode::Publish);
}

#[test]
fn inactive_form_error_has_no_thick_border() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Publish);
    // Empty topic: the send fails and the form shows its error.
    a.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    a.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    a.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(a.publish.as_ref().unwrap().error.is_some());
    draw(&mut a, 120, 40);
    // A click on the tree leaves the form without a key reaching it.
    mouse(
        &mut a,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        2,
        2,
    );
    assert_eq!(a.mode, Mode::Browse);
    let s = text(&draw(&mut a, 120, 40));
    let row = s.lines().find(|l| l.contains(" publish ")).unwrap();
    assert!(row.contains('╭') && !row.contains('┏'), "{row}");
}

#[test]
fn help_draws_over_the_picker() {
    use mqtt_zapp_core::config::PasswordSource::None as N;
    let mut a = picker_app(vec![profile("home", N)]);
    a.handle_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('?'),
        crossterm::event::KeyModifiers::NONE,
    ));
    let s = text(&draw(&mut a, 100, 30));
    assert!(s.contains("picker"), "the help's picker column: {s}");
    assert!(
        !s.contains("waiting for messages"),
        "not the browse screen: {s}"
    );
}

#[test]
fn help_footer_and_empty_picker_use_remapped_keys() {
    let mut s = mqtt_zapp_core::config::Settings::default();
    s.keys.insert("freeze".into(), "F".into());
    s.keys.insert("add_profile".into(), "+".into());
    let mut a = App::new(crate::cli::Launch::Picker, ProfileStore::default(), s).unwrap();
    let picker = text(&draw(&mut a, 80, 24));
    assert!(picker.contains("press + to add one"), "{picker}");
    a.mode = Mode::Browse;
    a.handle_action(Action::Help);
    let help = text(&draw(&mut a, 120, 40));
    assert!(help.contains("F freezes"), "{help}");
}

#[test]
fn field_picker_cuts_wide_labels_by_width() {
    let key = "温".repeat(30);
    let payload = format!("{{\"{key}\":42,\"b\":1}}");
    let mut a = app_with(&[("t", payload.as_bytes())]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::ChartPick);
    assert_eq!(a.mode, Mode::FieldPicker);
    let s = text(&draw(&mut a, 100, 30));
    // The card's row: the label cut with `…`, the value against the border.
    let row = s.lines().find(|l| l.contains("…") && l.contains('温'));
    let row = row.unwrap_or_else(|| panic!("{s}"));
    assert!(row.contains(" 42 ┃"), "{row}");
}

#[test]
fn long_field_filter_keeps_the_cursor_in_view() {
    let mut a = app_with(&[("t", br#"{"a":1,"b":2}"#)]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::ChartPick);
    a.field_picker.as_mut().unwrap().query = "q".repeat(70);
    let s = text(&draw(&mut a, 100, 30));
    assert!(s.contains("q▏"), "{s}");
}
