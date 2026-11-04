use super::*;

#[test]
fn typing_filters_each_keystroke() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    assert_eq!(a.mode, Mode::SearchInput);
    type_str(&mut a, "wz");
    assert!(a.filter.is_some());
    assert!(row_names(&a).contains(&"muh/esp/esp32-WZR1".to_string()));
    assert!(!row_names(&a).contains(&"other".to_string()));
    type_str(&mut a, "r");
    assert!(row_names(&a).contains(&"muh/esp/esp32-WZR1".to_string()));
    a.handle_key(key(KeyCode::Backspace));
    assert_eq!(a.search, "wz");
}

#[test]
fn payload_match_shows_topic() {
    let mut a = search_app();
    a.apply_core_event(msg("tasmota/tele/x/STATE", r#"{"DeviceName":"wzr-lamp"}"#));
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    assert!(row_names(&a).contains(&"tasmota/tele/x/STATE".to_string()));
}

#[test]
fn new_matching_topic_appears_while_filtered() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.mode, Mode::Browse);
    assert!(a.filter.is_some(), "enter keeps the filter");
    a.apply_core_event(msg("new/wzr2", "v"));
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    assert!(row_names(&a).contains(&"new/wzr2".to_string()));
}

#[test]
fn esc_clears_filter() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Browse);
    assert!(a.filter.is_none() && a.search.is_empty());
    // In Browse, esc clears a kept filter too.
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Enter));
    a.handle_key(key(KeyCode::Esc));
    assert!(a.filter.is_none() && a.search.is_empty());
}

#[test]
fn clearing_filter_keeps_selected_deep_node_visible() {
    for keep in [false, true] {
        let mut a = search_app();
        a.handle_key(key(KeyCode::Char('/')));
        type_str(&mut a, "wzr");
        let deep = a.tree.find("muh/esp/esp32-WZR1/LWT").unwrap();
        a.selected = Some(deep);
        if keep {
            a.handle_key(key(KeyCode::Enter));
            a.handle_key(key(KeyCode::Esc));
        } else {
            a.handle_key(key(KeyCode::Esc));
        }
        let rows = a.rows();
        let i = rows.iter().position(|r| r.id == deep).expect("visible");
        a.handle_action(Action::Down);
        assert_eq!(a.selected, Some(rows[i + 1].id), "keep={keep}");
        assert_eq!(sel(&a), "other");
    }
}

#[test]
fn enter_keeping_filter_reveals_selection() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    let deep = a.tree.find("muh/esp/esp32-WZR1/LWT").unwrap();
    a.selected = Some(deep);
    a.handle_key(key(KeyCode::Enter));
    a.set_filter(None);
    assert!(a.rows().iter().any(|r| r.id == deep));
}

#[test]
fn selection_jumps_to_first_match_when_hidden() {
    let mut a = search_app();
    let other = a.tree.find("other").unwrap();
    a.selected = Some(other);
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    assert_eq!(sel(&a), "muh/esp/esp32-WZR1");
}

#[test]
fn typing_q_in_search_does_not_quit() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('q'))),
        Effect::None
    ));
    assert_eq!(a.search, "q");
}

#[test]
fn on_tick_rebuilds_active_filter() {
    let mut a = browse(&["a/wzr"]);
    a.search = "wzr".into();
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    a.apply_core_event(msg("b/wzr2", "v"));
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
}

#[test]
fn on_tick_throttles_filter_rebuild() {
    let mut a = browse(&["a/wzr"]);
    a.search = "wzr".into();
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    let n = a.filter_builds.get();
    a.apply_core_event(msg("b/wzr2", "v"));
    a.on_tick();
    assert_eq!(a.filter_builds.get(), n, "second tick is throttled");
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    assert_eq!(
        a.filter_builds.get(),
        n + 1,
        "the pending change still lands"
    );
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
}

#[test]
fn keystroke_rebuilds_filter_immediately() {
    let mut a = browse(&["a/wzr"]);
    a.mode = Mode::SearchInput;
    a.last_filter_rebuild = Instant::now();
    let n = a.filter_builds.get();
    a.on_search_key(key(KeyCode::Char('w')));
    assert_eq!(a.filter_builds.get(), n + 1);
}

#[test]
fn ctrl_r_toggles_regex() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "esp32-wzr\\d");
    assert!(
        a.filter.as_ref().unwrap().matches.is_empty(),
        "plain: literal \\d"
    );
    a.handle_key(ctrl_r());
    assert!(a.search_regex);
    let f = a.filter.as_ref().unwrap();
    assert_eq!(f.mode, mqtt_zapp_core::search::QueryMode::Regex);
    // esp32-WZR1 and its LWT child both have the match in their path.
    assert_eq!(f.matches.len(), 2);
    a.handle_key(ctrl_r());
    assert!(!a.search_regex);
}

#[test]
fn invalid_regex_keeps_last_filter() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    a.handle_key(ctrl_r());
    type_str(&mut a, "LWT");
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    type_str(&mut a, "(");
    assert!(a.search_error.is_some());
    assert_eq!(a.filter.as_ref().unwrap().query, "LWT");
    a.handle_key(key(KeyCode::Backspace));
    assert!(a.search_error.is_none());
    a.handle_key(key(KeyCode::Esc));
    assert!(a.search_error.is_none() && a.filter.is_none());
}

#[test]
fn multi_term_search_filters_rows() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "muh LWT");
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
}

#[test]
fn frozen_tick_does_not_rebuild_filter() {
    let mut a = browse(&["a/live", "c/other"]);
    a.search = "live".into();
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    a.selected = a.tree.find("a/live");
    a.handle_action(Action::Freeze);
    let n = a.filter_builds.get();
    a.apply_core_event(msg("b/live", "new"));
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    assert_eq!(a.filter_builds.get(), n, "no tick rebuild while frozen");
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    assert_eq!(a.selected, a.tree.find("a/live"));
}

#[test]
fn unfreeze_rebuilds_pending_filter() {
    let mut a = browse(&["a/live"]);
    a.search = "live".into();
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    a.handle_action(Action::Freeze);
    a.apply_core_event(msg("b/live", "new"));
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    a.handle_action(Action::Freeze);
    assert!(!a.frozen);
    a.on_tick();
    assert_eq!(
        a.filter.as_ref().unwrap().matches.len(),
        2,
        "the change seen while frozen lands once live"
    );
}

#[test]
fn f_types_in_search_input_while_frozen() {
    let mut a = search_app();
    a.frozen = true;
    a.handle_key(key(KeyCode::Char('/')));
    a.handle_key(key(KeyCode::Char('f')));
    assert_eq!(a.search, "f");
    assert!(a.frozen, "f in the search input is text, not the toggle");
}

/// A tree past `DEBOUNCE_MIN_NODES`.
fn big_app() -> App {
    let mut a = browse(&["muh/wzr/LWT", "other/thing"]);
    for i in 0..DEBOUNCE_MIN_NODES {
        a.apply_core_event(plain(&format!("dev_{i}"), "v"));
    }
    a.mode = Mode::SearchInput;
    a
}

fn builds(a: &App) -> (u32, u32, u32) {
    (
        a.filter_full.get(),
        a.filter_narrow.get(),
        a.filter_incr.get(),
    )
}

#[test]
fn search_debounces_on_big_trees() {
    let mut a = big_app();
    type_str(&mut a, "wzr");
    assert_eq!(a.filter_builds.get(), 0, "no filter call per keystroke");
    assert!(a.filter.is_none());
    let t0 = Instant::now();
    a.on_tick_at(t0 + Duration::from_millis(50));
    assert_eq!(a.filter_builds.get(), 0, "still inside the debounce");
    a.on_tick_at(t0 + SEARCH_DEBOUNCE + Duration::from_millis(50));
    assert_eq!(a.filter_builds.get(), 1, "one call for the whole burst");
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
    a.on_tick_at(t0 + Duration::from_secs(5));
    assert_eq!(a.filter_builds.get(), 1, "nothing pending any more");
}

#[test]
fn enter_and_esc_apply_the_pending_search_at_once() {
    let mut a = big_app();
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.filter_builds.get(), 1);
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
    let mut a = big_app();
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Esc));
    assert!(a.filter.is_none() && a.search.is_empty());
    a.on_tick_at(Instant::now() + Duration::from_secs(5));
    assert_eq!(a.filter_builds.get(), 0, "esc cancels the pending rebuild");
    assert!(a.filter.is_none());
}

#[test]
fn narrowing_reuses_previous_matches() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "w");
    assert_eq!(builds(&a), (1, 0, 0), "the first key has nothing to reuse");
    type_str(&mut a, "zr");
    assert_eq!(builds(&a), (1, 2, 0), "extending the query narrows");
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
    a.handle_key(key(KeyCode::Backspace));
    assert_eq!(builds(&a), (2, 2, 0), "shortening needs a full scan");
    a.handle_key(key(KeyCode::Char('r')));
    a.handle_key(key(KeyCode::Char('r')));
    a.handle_key(ctrl_key('r'));
    let full = a.filter_full.get();
    type_str(&mut a, "1");
    assert_eq!(a.filter_full.get(), full + 1, "regex never narrows");
}

#[test]
fn incremental_filter_adds_new_match() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Enter));
    let before = builds(&a);
    a.apply_core_event(msg("new/deep/wzr2", "v"));
    a.apply_core_event(msg("new/other", "v"));
    a.on_tick_at(Instant::now() + Duration::from_secs(1));
    assert_eq!(builds(&a), (before.0, before.1, before.2 + 1));
    let f = a.filter.as_ref().unwrap();
    assert_eq!(f.matches.len(), 3, "WZR1, its LWT child, wzr2");
    let deep = a.tree.find("new/deep").unwrap();
    assert!(f.visible.contains(&deep) && f.visible.contains(&a.tree.find("new").unwrap()));
    assert!(row_names(&a).contains(&"new/deep/wzr2".to_string()));
    assert!(!row_names(&a).contains(&"new/other".to_string()));
}

#[test]
fn incremental_filter_drops_unmatched() {
    let mut a = browse(&[]);
    a.apply_core_event(msg("a/x", "wzr"));
    a.apply_core_event(msg("b/wzr", "v"));
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
    let before = builds(&a);
    a.apply_core_event(msg("a/x", "changed"));
    a.on_tick_at(Instant::now() + Duration::from_secs(1));
    assert_eq!(
        builds(&a),
        (before.0 + 1, before.1, before.2),
        "full rebuild"
    );
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    assert!(!row_names(&a).contains(&"a/x".to_string()));
}

#[test]
fn touched_overflow_forces_full_rebuild() {
    let mut a = search_app();
    a.handle_key(key(KeyCode::Char('/')));
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Enter));
    let before = builds(&a);
    for _ in 0..=mqtt_zapp_core::tree::TOUCHED_MAX {
        a.apply_core_event(msg("other/thing", "v"));
    }
    a.apply_core_event(msg("n/wzr9", "v"));
    a.on_tick_at(Instant::now() + Duration::from_secs(1));
    assert_eq!(builds(&a), (before.0 + 1, before.1, before.2));
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 3);
}

#[test]
fn tick_respects_pending_debounce() {
    let mut a = big_app();
    type_str(&mut a, "wz");
    let t0 = Instant::now();
    a.on_tick_at(t0 + SEARCH_DEBOUNCE + Duration::from_millis(50));
    let n = a.filter_builds.get();
    a.apply_core_event(msg("new/wzr9", "v"));
    type_str(&mut a, "r");
    // Throttle has long elapsed, but the new key's debounce has not.
    a.last_filter_rebuild -= Duration::from_secs(5);
    a.on_tick_at(Instant::now() + Duration::from_millis(10));
    assert_eq!(a.filter_builds.get(), n, "tick must not rebuild mid-typing");
    assert!(a.filter_due.is_some());
    a.on_tick_at(Instant::now() + SEARCH_DEBOUNCE + Duration::from_millis(50));
    assert!(a.filter_builds.get() > n);
}

#[test]
fn debounce_starts_exactly_at_min_nodes() {
    let mut a = browse(&[]);
    while a.tree.len() < DEBOUNCE_MIN_NODES - 1 {
        let i = a.tree.len();
        a.apply_core_event(plain(&format!("dev_{i}"), "v"));
    }
    a.mode = Mode::SearchInput;
    assert_eq!(a.tree.len(), DEBOUNCE_MIN_NODES - 1);
    a.on_search_key(key(KeyCode::Char('d')));
    assert_eq!(a.filter_builds.get(), 1, "below the limit: immediate");
    a.apply_core_event(plain("one_more", "v"));
    assert_eq!(a.tree.len(), DEBOUNCE_MIN_NODES);
    a.on_search_key(key(KeyCode::Char('e')));
    assert_eq!(a.filter_builds.get(), 1, "at the limit: debounced");
    assert!(a.filter_due.is_some());
}

/// CI does not run this; see `rows_fast_at_50k`.
#[test]
#[ignore = "timing: run with --release -- --ignored"]
fn perf_search_200k() {
    let mut a = browse(&[]);
    for i in 0..200_000 {
        a.apply_core_event(plain(&format!("dev_{}/s{}/x", i / 10, i % 10), "v"));
    }
    a.mode = Mode::SearchInput;
    let mut worst = Duration::ZERO;
    for c in "dev_12".chars() {
        let s = Instant::now();
        a.on_search_key(key(KeyCode::Char(c)));
        worst = worst.max(s.elapsed());
    }
    eprintln!("perf_search_200k: worst on_search_key {worst:?}");
    assert!(worst < Duration::from_millis(2), "worst {worst:?}");
}

#[test]
fn paste_appends_first_line_to_search() {
    let mut a = search_app();
    a.handle_action(Action::Search);
    a.handle_paste("ab?\ndef");
    assert_eq!(a.search, "ab?");
    assert_eq!(a.mode, Mode::SearchInput);
}
