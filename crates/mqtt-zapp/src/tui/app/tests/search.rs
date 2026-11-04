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
