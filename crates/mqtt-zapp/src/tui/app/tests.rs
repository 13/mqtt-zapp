use super::*;
use crate::cli::Launch;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::Message;
use mqtt_zapp_core::client::{ConnState, CoreEvent};
use mqtt_zapp_core::config::{
    PasswordSource, Profile, ProfileStore, Protocol, Settings, TlsConfig,
};
use mqtt_zapp_core::state::{AppState, PickerSort};
use std::time::{Duration, Instant, SystemTime};

fn msg(topic: &str, p: &str) -> CoreEvent {
    CoreEvent::Message {
        topic: topic.into(),
        msg: Message {
            payload: p.into(),
            qos: 0,
            retain: true,
            received_at: SystemTime::now(),
        },
    }
}

fn app() -> App {
    App::new(Launch::Picker, ProfileStore::default(), Settings::default()).unwrap()
}

fn browse(topics: &[&str]) -> App {
    let mut a = app();
    a.mode = Mode::Browse;
    for t in topics {
        a.apply_core_event(msg(t, "v"));
    }
    a
}

#[test]
#[allow(non_snake_case)]
fn y_copies_topic_and_Y_copies_shown_payload() {
    let mut a = browse(&[]);
    assert!(matches!(a.handle_action(Action::CopyTopic), Effect::None));
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("nothing to copy"))
    );
    a.apply_core_event(msg("x/y", "old"));
    a.apply_core_event(msg("x/y", "new"));
    a.selected = a.tree.find("x/y");
    match a.handle_action(Action::CopyTopic) {
        Effect::Copy { what, text } => assert_eq!((what, text.as_str()), ("topic", "x/y")),
        e => panic!("{e:?}"),
    }
    a.history_sel = Some(0);
    match a.handle_action(Action::CopyPayload) {
        Effect::Copy { what, text } => assert_eq!((what, text.as_str()), ("payload", "old")),
        e => panic!("{e:?}"),
    }
}

fn plain(topic: &str, p: &str) -> CoreEvent {
    match msg(topic, p) {
        CoreEvent::Message { topic, mut msg } => {
            msg.retain = false;
            CoreEvent::Message { topic, msg }
        }
        e => e,
    }
}

#[test]
fn delete_retained_needs_confirmation() {
    let mut a = browse(&["a/b"]);
    a.selected = a.tree.find("a/b");
    assert!(matches!(
        a.handle_action(Action::DeleteRetained),
        Effect::None
    ));
    assert_eq!(a.mode, Mode::Confirm);
    assert_eq!(
        a.confirm.as_ref().unwrap().prompt,
        "delete retained message on a/b?"
    );
    match a.handle_key(key(KeyCode::Char('y'))) {
        Effect::ClearRetained(t) => assert_eq!(t, vec!["a/b".to_owned()]),
        e => panic!("{e:?}"),
    }
    assert_eq!(a.mode, Mode::Browse);
    a.handle_action(Action::DeleteRetained);
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('n'))),
        Effect::None
    ));
    assert_eq!(a.mode, Mode::Browse);
    assert!(a.confirm.is_none());
    assert_eq!(a.note_at(Instant::now()), Some(&Note::info("cancelled")));
    a.handle_action(Action::DeleteRetained);
    assert!(matches!(
        a.handle_key(key(KeyCode::Enter)),
        Effect::ClearRetained(_)
    ));
}

#[test]
fn clear_retained_lists_every_retained_topic() {
    let mut a = browse(&["a/x", "a/y/z", "a/y", "b"]);
    a.apply_core_event(plain("a/w", "v"));
    a.apply_core_event(msg("a/x", "")); // already cleared on the broker
    a.selected = a.tree.find("a");
    a.handle_action(Action::ClearRetained);
    let c = a.confirm.as_ref().unwrap();
    assert_eq!(c.prompt, "clear 2 retained messages under a?");
    let ConfirmAction::ClearRetained(t) = &c.action else {
        panic!("{c:?}");
    };
    let mut t = t.clone();
    t.sort();
    assert_eq!(t, vec!["a/y", "a/y/z"]);
}

#[test]
fn live_update_keeps_topic_clearable() {
    let mut a = browse(&["a/b"]);
    a.apply_core_event(plain("a/b", "live"));
    a.selected = a.tree.find("a/b");
    a.handle_action(Action::DeleteRetained);
    assert_eq!(a.mode, Mode::Confirm);
    a.handle_key(key(KeyCode::Esc));
    a.selected = a.tree.find("a");
    a.handle_action(Action::ClearRetained);
    assert_eq!(
        a.confirm.as_ref().unwrap().prompt,
        "clear 1 retained message under a?"
    );
}

#[test]
fn sys_topics_are_never_cleared() {
    let mut a = browse(&["$SYS/broker/version", "$SYS/broker/uptime"]);
    a.selected = a.tree.find("$SYS");
    a.handle_action(Action::ClearRetained);
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("no retained messages here"))
    );
}

#[test]
fn nothing_retained_notes_without_confirm() {
    let mut a = browse(&[]);
    a.apply_core_event(plain("p", "v"));
    a.selected = a.tree.find("p");
    a.handle_action(Action::DeleteRetained);
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("no retained messages here"))
    );
    a.selected = None;
    a.handle_action(Action::ClearRetained);
    assert_eq!(a.mode, Mode::Browse);
}

#[test]
fn s_cycles_sort_and_notes() {
    use mqtt_zapp_core::config::SortOrder;
    let mut a = browse(&["a"]);
    assert_eq!(a.sort, SortOrder::Name);
    a.handle_action(Action::CycleSort);
    assert_eq!(a.sort, SortOrder::Count);
    assert_eq!(a.note_at(Instant::now()), Some(&Note::info("sort: count")));
    a.handle_action(Action::CycleSort);
    assert_eq!(a.sort, SortOrder::Recent);
    a.handle_action(Action::CycleSort);
    assert_eq!(a.sort, SortOrder::Name);
}

#[test]
fn expand_all_and_collapse_all_subtree() {
    let mut a = browse(&["a/b/c", "a/d/e", "a/f", "z/y"]);
    a.selected = a.tree.find("a");
    a.handle_action(Action::ExpandAll);
    let ids = |a: &App, ts: &[&str]| {
        ts.iter()
            .map(|t| a.tree.find(t).unwrap())
            .collect::<HashSet<_>>()
    };
    assert_eq!(a.expanded, ids(&a, &["a", "a/b", "a/d"]));
    a.expanded.insert(a.tree.find("z").unwrap());
    a.handle_action(Action::CollapseAll);
    assert_eq!(a.expanded, ids(&a, &["z"]));
}

fn ctrl_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn payload_of(a: &App) -> String {
    a.publish.as_ref().unwrap().payload_text()
}

/// Open the form on `topic`, focus the payload, type `p`, send.
fn publish_one(a: &mut App, topic: &str, p: &str) {
    a.selected = a.tree.find(topic);
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(a, p);
    assert!(matches!(
        a.handle_key(ctrl_key('s')),
        Effect::Publish { .. }
    ));
    a.handle_key(key(KeyCode::Esc));
}

#[test]
fn set_publish_payload_replaces_text() {
    let mut a = browse(&["t"]);
    a.handle_action(Action::Publish);
    match a.handle_key(ctrl_key('e')) {
        Effect::EditPayload(t) => assert_eq!(t, ""),
        e => panic!("{e:?}"),
    }
    a.set_publish_payload("from\neditor");
    assert_eq!(payload_of(&a), "from\neditor");
}

#[test]
fn payload_history_walk_and_restore_draft() {
    let mut a = browse(&["t"]);
    publish_one(&mut a, "t", "one");
    publish_one(&mut a, "t", "two");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "draft");
    a.handle_key(ctrl_key('p'));
    assert_eq!(payload_of(&a), "two");
    a.handle_key(ctrl_key('p'));
    assert_eq!(payload_of(&a), "one");
    a.handle_key(ctrl_key('p'));
    assert_eq!(payload_of(&a), "one", "stops at the oldest");
    a.handle_key(ctrl_key('n'));
    assert_eq!(payload_of(&a), "two");
    a.handle_key(ctrl_key('n'));
    assert_eq!(payload_of(&a), "draft", "past the newest: the draft");
}

#[test]
fn history_dedups_and_caps_at_20() {
    let mut a = browse(&["t"]);
    for i in 0..25 {
        publish_one(&mut a, "t", &format!("p{i}"));
    }
    publish_one(&mut a, "t", "p10");
    let h = &a.sent["t"];
    assert_eq!(h.len(), 20);
    assert_eq!(h[0], "p10");
    assert_eq!(h.iter().filter(|p| *p == "p10").count(), 1);
}

#[test]
fn bad_timestamp_format_falls_back() {
    let s = Settings {
        timestamp_format: "%Q nope".into(),
        ..Settings::default()
    };
    let a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    assert_eq!(a.timestamp_format, "%H:%M:%S%.3f");
    let s = Settings {
        timestamp_format: "%H:%M".into(),
        ..Settings::default()
    };
    let a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    assert_eq!(a.timestamp_format, "%H:%M");
}

fn sel(a: &App) -> String {
    a.tree.full_topic(a.selected.unwrap())
}

#[test]
fn burst_of_retained_messages_keeps_selection_stable() {
    let mut a = browse(&["m/a"]);
    a.handle_action(Action::Down);
    a.handle_action(Action::Expand);
    a.handle_action(Action::Down);
    let id = a.tree.find("m/a").unwrap();
    assert_eq!(a.selected, Some(id));
    for i in 0..2000 {
        a.apply_core_event(msg(&format!("m/{i:04}"), "x"));
    }
    assert_eq!(a.selected, Some(id));
    assert_eq!(sel(&a), "m/a");
}

#[test]
fn down_up_moves_selection_over_visible_rows() {
    let mut a = browse(&["a/x", "b", "c"]);
    a.handle_action(Action::Down);
    assert_eq!(sel(&a), "a");
    a.handle_action(Action::Down);
    assert_eq!(sel(&a), "b", "collapsed a/x is skipped");
    a.handle_action(Action::Down);
    a.handle_action(Action::Down);
    assert_eq!(sel(&a), "c", "stops at the last row");
    a.handle_action(Action::Up);
    a.handle_action(Action::Up);
    a.handle_action(Action::Up);
    assert_eq!(sel(&a), "a", "stops at the first row");
    a.handle_action(Action::Expand);
    a.handle_action(Action::Down);
    assert_eq!(sel(&a), "a/x");
    a.handle_action(Action::Bottom);
    assert_eq!(sel(&a), "c");
    a.handle_action(Action::Top);
    assert_eq!(sel(&a), "a");
}

#[test]
fn collapse_on_child_moves_to_parent_then_collapses() {
    let mut a = browse(&["a/x"]);
    a.handle_action(Action::Down);
    a.handle_action(Action::ToggleExpand);
    a.handle_action(Action::Down);
    assert_eq!(sel(&a), "a/x");
    a.handle_action(Action::Collapse);
    assert_eq!(sel(&a), "a");
    assert!(a.expanded.contains(&a.selected.unwrap()));
    a.handle_action(Action::Collapse);
    assert!(a.expanded.is_empty());
}

#[test]
fn quit_and_ctrl_c() {
    let mut a = browse(&[]);
    assert!(matches!(a.handle_action(Action::Quit), Effect::Quit));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(matches!(a.handle_key(ctrl_c), Effect::Quit));
    a.mode = Mode::Picker;
    assert!(matches!(a.handle_key(ctrl_c), Effect::Quit));
    let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    a.mode = Mode::Browse;
    assert!(matches!(a.handle_key(q), Effect::Quit));
}

#[test]
fn keymap_comes_from_settings() {
    let mut s = Settings::default();
    s.keys.insert("quit".into(), "x".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    let x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(matches!(a.handle_key(x), Effect::Quit));
    s = Settings::default();
    s.keys.insert("nope".into(), "x".into());
    assert!(App::new(Launch::Picker, ProfileStore::default(), s).is_err());
}

#[test]
fn launch_decides_mode_and_startup_effect() {
    let mut a = app();
    assert_eq!(a.mode, Mode::Picker);
    assert!(matches!(a.startup_effect(), Effect::None));
    let store = ProfileStore::default();
    let p = crate::cli::resolve_launch(
        &<crate::cli::Args as clap::Parser>::parse_from(["mqtt-zapp", "--host", "h"]),
        &store,
    )
    .unwrap();
    let mut a = App::new(p, store, Settings::default()).unwrap();
    assert_eq!(a.mode, Mode::Browse);
    match a.startup_effect() {
        Effect::Connect(p, None) => assert_eq!(p.host, "h"),
        _ => panic!("expected connect"),
    }
    assert!(matches!(a.startup_effect(), Effect::None), "only once");
}

#[test]
fn state_event_updates_status() {
    let mut a = browse(&[]);
    a.dirty = false;
    a.apply_core_event(CoreEvent::State(ConnState::Connected));
    assert!(matches!(a.conn_state, ConnState::Connected));
    assert!(a.dirty);
}

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn type_str(a: &mut App, s: &str) {
    for c in s.chars() {
        a.handle_key(key(KeyCode::Char(c)));
    }
}

fn row_names(a: &App) -> Vec<String> {
    a.rows().iter().map(|r| a.tree.full_topic(r.id)).collect()
}

fn search_app() -> App {
    browse(&[
        "muh/esp/esp32-WZR1/LWT",
        "tasmota/tele/x/STATE",
        "other/thing",
    ])
}

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
    a.filter = None;
    assert!(a.rows().iter().any(|r| r.id == deep));
}

#[test]
fn select_in_browse_toggles_expand() {
    let mut a = browse(&["a/b"]);
    a.handle_action(Action::Down);
    let n = a.rows().len();
    a.handle_action(Action::Select);
    assert_eq!(a.rows().len(), n + 1);
    a.handle_action(Action::Select);
    assert_eq!(a.rows().len(), n);
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
    a.on_tick();
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    a.apply_core_event(msg("b/wzr2", "v"));
    a.on_tick();
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
}

#[test]
fn on_tick_marks_dirty_while_flashing() {
    let mut a = browse(&["a"]);
    a.dirty = false;
    a.on_tick();
    assert!(a.dirty, "fresh message is still flashing");
}

fn profile(name: &str, pw: PasswordSource) -> Profile {
    Profile {
        name: name.into(),
        host: "h".into(),
        port: 1883,
        protocol: Protocol::Mqtt,
        mqtt_version: Default::default(),
        ws_path: "/".into(),
        client_id: None,
        username: Some("u".into()),
        password: pw,
        tls: TlsConfig::default(),
        subscriptions: vec!["#".into()],
    }
}

fn picker_app(profiles: Vec<Profile>) -> App {
    let store = ProfileStore { profiles };
    App::new(Launch::Picker, store, Settings::default()).unwrap()
}

fn press(a: &mut App, c: char) -> Effect {
    a.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

#[test]
fn add_key_opens_form_and_save_emits_effect() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    press(&mut a, 'a');
    assert_eq!(a.mode, Mode::ProfileForm);
    press(&mut a, 'n');
    a.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    press(&mut a, 'h');
    let e = a.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(
        matches!(e, Effect::SaveProfile(ref ed) if ed.profile.name == "n" && ed.original.is_none()),
        "{e:?}"
    );
    assert_eq!(a.mode, Mode::ProfileForm, "open until the save is done");
}

#[test]
fn add_refuses_existing_name() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    press(&mut a, 'a');
    press(&mut a, 'a');
    a.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    press(&mut a, 'h');
    let e = a.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(matches!(e, Effect::None));
    assert_eq!(
        a.profile_form.as_ref().unwrap().error.as_deref(),
        Some("profile 'a' already exists")
    );
}

#[test]
fn edit_key_opens_selected_profile() {
    let mut a = picker_app(vec![
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    a.picker.down();
    press(&mut a, 'e');
    assert_eq!(a.mode, Mode::ProfileForm);
    let e = a.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(
        matches!(e, Effect::SaveProfile(ref ed) if ed.original.as_deref() == Some("b")),
        "{e:?}"
    );
    a.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(a.mode, Mode::Picker);
    assert!(a.profile_form.is_none());
}

#[test]
fn profile_saved_closes_form_and_selects() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    press(&mut a, 'a');
    a.profile_saved(
        vec![
            profile("a", PasswordSource::None),
            profile("n", PasswordSource::None),
        ],
        Some("n"),
        "saved n",
    );
    assert_eq!(a.mode, Mode::Picker);
    assert!(a.profile_form.is_none());
    assert_eq!(a.picker.sel, 1);
    assert_eq!(a.note_at(Instant::now()).unwrap().text, "saved n");
    // A delete leaves the selection clamped to the shorter list.
    a.profile_saved(vec![profile("a", PasswordSource::None)], None, "deleted n");
    assert_eq!(a.picker.sel, 0);
}

fn names(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

#[test]
fn picker_j_returns_save_order() {
    let mut a = picker_app(vec![
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    match press(&mut a, 'J') {
        Effect::SaveOrder(order) => assert_eq!(names(&order), ["b", "a"]),
        e => panic!("{e:?}"),
    }
    assert_eq!(a.picker.selected().unwrap().name, "a");
    assert!(matches!(press(&mut a, 'K'), Effect::SaveOrder(_)));
    assert_eq!(names(&a.picker.order()), ["a", "b"]);
}

#[test]
fn picker_j_at_bottom_returns_none() {
    let mut a = picker_app(vec![
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    press(&mut a, 'j');
    assert!(matches!(press(&mut a, 'J'), Effect::None));
}

#[test]
fn picker_s_cycles_and_saves_state() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    match press(&mut a, 's') {
        Effect::SaveState(st) => assert_eq!(st.picker_sort, PickerSort::Name),
        e => panic!("{e:?}"),
    }
}

#[test]
fn set_app_state_applies_sort_and_last_used() {
    let mut a = picker_app(vec![
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    let mut st = AppState {
        picker_sort: PickerSort::LastUsed,
        ..Default::default()
    };
    st.touch("b", 5);
    a.set_app_state(st.clone());
    assert_eq!(a.picker.selected().unwrap().name, "b");
    assert_eq!(a.app_state(), st);
}

#[test]
fn connect_from_picker_records_last_used() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(a.app_state().last_used.get("a").is_some_and(|&t| t > 0));
}

#[test]
fn save_state_failure_is_a_note() {
    let mut a = picker_app(vec![]);
    a.state_save_failed("x");
    let n = a.note_at(Instant::now()).unwrap();
    assert!(n.text.contains("could not save state: x"), "{}", n.text);
}

#[test]
fn last_used_follows_rename_and_delete() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    let mut st = AppState::default();
    st.touch("a", 5);
    a.set_app_state(st);
    a.profile_renamed("a", "b");
    assert_eq!(a.app_state().last_used, [("b".to_string(), 5)].into());
    a.profile_deleted("b");
    assert!(a.app_state().last_used.is_empty());
}

#[test]
fn profile_save_failed_keeps_form() {
    let mut a = picker_app(vec![]);
    press(&mut a, 'a');
    a.profile_save_failed("disk full".into());
    assert_eq!(a.mode, Mode::ProfileForm);
    assert_eq!(
        a.profile_form.as_ref().unwrap().error.as_deref(),
        Some("disk full")
    );
    // Without a form (a failed delete) it is a note.
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    a.profile_save_failed("disk full".into());
    let n = a.note_at(Instant::now()).unwrap();
    assert!(n.error && n.text == "disk full");
}

#[test]
fn edit_and_delete_on_empty_picker_do_nothing() {
    let mut a = picker_app(vec![]);
    press(&mut a, 'e');
    assert_eq!(a.mode, Mode::Picker);
    press(&mut a, 'D');
    assert_eq!(a.mode, Mode::Picker);
    assert!(a.confirm.is_none());
}

#[test]
fn delete_confirms_then_returns_to_picker() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    press(&mut a, 'D');
    assert_eq!(a.mode, Mode::Confirm);
    assert_eq!(a.confirm.as_ref().unwrap().prompt, "delete profile a?");
    let e = press(&mut a, 'y');
    assert!(
        matches!(e, Effect::DeleteProfile(ref n) if n == "a"),
        "{e:?}"
    );
    assert_eq!(a.mode, Mode::Picker);
    press(&mut a, 'D');
    let e = press(&mut a, 'n');
    assert!(matches!(e, Effect::None));
    assert_eq!(a.mode, Mode::Picker);
    assert_eq!(a.note_at(Instant::now()).unwrap().text, "cancelled");
}

#[test]
fn profile_keys_refused_when_store_broken() {
    let mut a = picker_app(vec![]);
    a.store_error = Some("parse error".into());
    for c in ['a', 'e', 'D'] {
        press(&mut a, c);
        assert_eq!(a.mode, Mode::Picker, "{c}");
        assert!(
            a.note_at(Instant::now())
                .unwrap()
                .text
                .contains("profiles.toml has errors"),
            "{c}"
        );
    }
}

#[test]
fn picker_enter_on_none_password_connects() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    match a.handle_key(key(KeyCode::Enter)) {
        Effect::Connect(p, None) => assert_eq!(p.name, "a"),
        e => panic!("expected connect, got {e:?}"),
    }
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(a.server.as_ref().map(|s| s.name.as_str()), Some("a"));
}

#[test]
fn picker_moves_selection_and_connects_to_it() {
    let mut a = picker_app(vec![
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    a.handle_key(key(KeyCode::Down));
    a.handle_key(key(KeyCode::Down));
    assert_eq!(a.picker.sel, 1, "stops at the last");
    a.handle_key(key(KeyCode::Up));
    a.handle_key(key(KeyCode::Down));
    assert!(matches!(a.handle_key(key(KeyCode::Enter)),
        Effect::Connect(p, None) if p.name == "b"));
    let mut a = picker_app(vec![]);
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
    assert_eq!(a.mode, Mode::Picker);
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('q'))),
        Effect::Quit
    ));
}

#[test]
fn picker_prompt_goes_to_password_mode_then_connects() {
    let mut a = picker_app(vec![profile("a", PasswordSource::Prompt)]);
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
    assert_eq!(a.mode, Mode::Password);
    // Keys bypass the keymap: q and j are characters.
    type_str(&mut a, "pqj");
    a.handle_key(key(KeyCode::Backspace));
    type_str(&mut a, "w");
    match a.handle_key(key(KeyCode::Enter)) {
        Effect::Connect(p, Some(pw)) => {
            assert_eq!((p.name.as_str(), pw.as_str()), ("a", "pqw"))
        }
        e => panic!("expected connect, got {e:?}"),
    }
    assert_eq!(a.mode, Mode::Browse);
    assert!(a.password.is_none(), "input cleared");
    assert_eq!(a.server.as_ref().map(|s| s.name.as_str()), Some("a"));
}

#[test]
fn password_esc_returns_to_picker() {
    let mut a = picker_app(vec![profile("a", PasswordSource::Prompt)]);
    a.handle_key(key(KeyCode::Enter));
    type_str(&mut a, "secret");
    assert!(matches!(a.handle_key(key(KeyCode::Esc)), Effect::None));
    assert_eq!(a.mode, Mode::Picker);
    assert!(a.password.is_none(), "input dropped");
    // Without any profile (ad-hoc launch) there is no picker to go back to.
    let mut a = App::new(
        Launch::Connect(profile("adhoc", PasswordSource::Prompt)),
        ProfileStore::default(),
        Settings::default(),
    )
    .unwrap();
    assert!(matches!(a.startup_effect(), Effect::None));
    assert_eq!(a.mode, Mode::Password);
    assert!(matches!(a.handle_key(key(KeyCode::Esc)), Effect::Quit));
}

#[test]
fn keyring_failure_falls_back_to_password_prompt() {
    let lookup = |r: Result<Option<String>>| {
        let mut a = picker_app(vec![]);
        let e = a.begin_connect_with(profile("k", PasswordSource::Keyring), |_| r);
        (a.mode, e)
    };
    let (mode, e) = lookup(Err(anyhow::anyhow!("no secret service")));
    assert_eq!(mode, Mode::Password);
    assert!(matches!(e, Effect::None));
    let (mode, e) = lookup(Ok(None));
    assert_eq!(mode, Mode::Password);
    assert!(matches!(e, Effect::None));
    let (mode, e) = lookup(Ok(Some("pw".into())));
    assert_ne!(mode, Mode::Password);
    assert!(matches!(e, Effect::Connect(_, Some(pw)) if pw == "pw"));
}

#[test]
fn startup_resolves_password_like_the_picker() {
    let mut a = App::new(
        Launch::Connect(profile("p", PasswordSource::Prompt)),
        ProfileStore::default(),
        Settings::default(),
    )
    .unwrap();
    assert!(matches!(a.startup_effect(), Effect::None));
    assert_eq!(a.mode, Mode::Password);
    assert_eq!(a.password.as_ref().unwrap().profile.name, "p");
}

#[test]
fn password_debug_does_not_leak() {
    let mut a = picker_app(vec![profile("a", PasswordSource::Prompt)]);
    a.handle_key(key(KeyCode::Enter));
    type_str(&mut a, "hunter2");
    assert!(!format!("{:?}", a.password).contains("hunter2"));
}

#[test]
fn reset_tree_clears_body_cache() {
    let mut a = browse(&["a"]);
    a.handle_action(Action::Down);
    super::super::message_view::content_len(&a);
    assert!(a.body_cache.is_cached());
    a.reset_tree();
    assert!(!a.body_cache.is_cached());
}

#[test]
fn msg_rate_is_a_ten_second_window() {
    let mut r = MsgRate::default();
    let t0 = Instant::now();
    for _ in 0..50 {
        r.record(t0);
    }
    assert_eq!(r.per_sec(t0), 5.0);
    assert_eq!(r.per_sec(t0 + Duration::from_secs(9)), 5.0);
    assert_eq!(r.per_sec(t0 + Duration::from_secs(11)), 0.0);
}

#[test]
fn effect_debug_redacts_password() {
    let e = Effect::Connect(
        profile("home", PasswordSource::Prompt),
        Some("hunter2".into()),
    );
    let s = format!("{e:?}");
    assert!(!s.contains("hunter2"), "{s}");
    assert_eq!(s, "Connect(home, <redacted>)");
    let e = Effect::Connect(profile("home", PasswordSource::None), None);
    assert!(format!("{e:?}").contains("home"));
}

#[test]
fn p_opens_prefilled_form_and_ctrl_s_publishes_keeping_it_open() {
    let mut a = browse(&["dev/cmnd/POWER"]);
    for _ in 0..2 {
        a.handle_action(Action::Down);
        a.handle_action(Action::Expand);
    }
    a.handle_action(Action::Down);
    assert_eq!(sel(&a), "dev/cmnd/POWER");
    let k = |c, m| KeyEvent::new(c, m);
    assert!(matches!(
        a.handle_key(k(KeyCode::Char('p'), KeyModifiers::NONE)),
        Effect::None
    ));
    assert_eq!(a.mode, Mode::Publish);
    assert_eq!(
        a.publish.as_ref().unwrap().topic.lines()[0],
        "dev/cmnd/POWER"
    );
    // keys go to the form, not the keymap: 'q' is not quit
    a.handle_key(k(KeyCode::Tab, KeyModifiers::NONE));
    assert!(matches!(
        a.handle_key(k(KeyCode::Char('q'), KeyModifiers::NONE)),
        Effect::None
    ));
    match a.handle_key(k(KeyCode::Char('s'), KeyModifiers::CONTROL)) {
        Effect::Publish {
            topic,
            payload,
            qos,
            retain,
        } => {
            assert_eq!(topic, "dev/cmnd/POWER");
            assert_eq!(payload, b"q");
            assert_eq!((qos, retain), (0, false));
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(a.mode, Mode::Publish);
    assert!(a.publish.is_some());
    assert!(matches!(
        a.handle_key(k(KeyCode::Esc, KeyModifiers::NONE)),
        Effect::None
    ));
    assert_eq!(a.mode, Mode::Browse);
    assert!(a.publish.is_none());
    // ctrl-c still quits from the form
    a.handle_action(Action::Publish);
    assert!(matches!(
        a.handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Effect::Quit
    ));
}

#[test]
fn p_without_selection_opens_empty_form() {
    let mut a = browse(&[]);
    a.handle_action(Action::Publish);
    assert_eq!(a.mode, Mode::Publish);
    assert_eq!(a.publish.as_ref().unwrap().topic.lines(), [""]);
}

#[test]
fn status_note_is_set_and_expires() {
    let mut a = browse(&[]);
    let t0 = Instant::now();
    a.set_note(Note::info("published a/b"), t0);
    assert_eq!(
        a.note_at(t0 + Duration::from_secs(4)),
        Some(&Note::info("published a/b"))
    );
    assert_eq!(a.note_at(t0 + Duration::from_secs(6)), None);
}

#[test]
fn help_opens_on_question_mark_and_any_key_closes() {
    let mut a = app();
    a.mode = Mode::Browse;
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    a.handle_key(k('?'));
    assert_eq!(a.mode, Mode::Help);
    // A key that is bound (q = quit) only closes the overlay.
    assert!(matches!(a.handle_key(k('q')), Effect::None));
    assert_eq!(a.mode, Mode::Browse);
    a.handle_key(k('?'));
    assert!(matches!(
        a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Effect::Quit
    ));
}

// --- diff base ---

fn history_app(depth: usize, payloads: &[&str]) -> App {
    let s = Settings {
        history_depth: depth,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    for p in payloads {
        a.apply_core_event(msg("t", p));
    }
    a.selected = a.tree.find("t");
    a
}

#[test]
fn m_marks_shown_entry_and_turns_diff_on() {
    let mut a = history_app(10, &["1", "2", "3"]);
    a.history_sel = Some(0);
    a.handle_action(Action::MarkDiffBase);
    let first = &a.tree.node(a.selected.unwrap()).history[0];
    assert_eq!(a.diff_base, Some((first.received_at, first.payload.len())));
    assert!(a.show_diff);
}

#[test]
fn m_twice_unmarks() {
    let mut a = history_app(10, &["1", "2"]);
    a.handle_action(Action::MarkDiffBase);
    a.handle_action(Action::MarkDiffBase);
    assert_eq!(a.diff_base, None);
}

#[test]
fn m_without_message_notes() {
    let mut a = browse(&[]);
    a.handle_action(Action::MarkDiffBase);
    assert_eq!(a.diff_base, None);
    assert!(a.note_at(Instant::now()).is_some());
}

#[test]
fn base_cleared_on_topic_change() {
    let mut a = history_app(10, &["1", "2"]);
    a.apply_core_event(msg("u", "x"));
    a.handle_action(Action::MarkDiffBase);
    assert!(a.diff_base.is_some());
    let u = a.tree.find("u").unwrap();
    a.select(u);
    assert_eq!(a.diff_base, None);
}

#[test]
fn base_cleared_when_evicted() {
    let mut a = history_app(3, &["1", "2", "3"]);
    a.history_sel = Some(0);
    a.handle_action(Action::MarkDiffBase);
    a.history_sel = None;
    a.apply_core_event(msg("t", "4"));
    assert_eq!(a.diff_base, None);
}

#[test]
fn base_kept_when_other_topics_arrive() {
    let mut a = history_app(3, &["1", "2", "3"]);
    a.handle_action(Action::MarkDiffBase);
    a.apply_core_event(msg("other", "x"));
    assert!(a.diff_base.is_some());
}

// --- search modes ---

fn ctrl_r() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
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

// --- auto_expand / memory cap ---

fn expand_app(levels: usize) -> App {
    let s = Settings {
        auto_expand: levels,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    a
}

#[test]
fn auto_expand_opens_new_nodes_to_depth() {
    let mut a = expand_app(1);
    a.apply_core_event(msg("a/b/c", "v"));
    assert!(a.expanded.contains(&a.tree.find("a").unwrap()));
    assert!(!a.expanded.contains(&a.tree.find("a/b").unwrap()));
    let mut a = expand_app(2);
    a.apply_core_event(msg("a/b/c", "v"));
    assert!(a.expanded.contains(&a.tree.find("a/b").unwrap()));
    assert!(!a.expanded.contains(&a.tree.find("a/b/c").unwrap()));
    let mut a = expand_app(0);
    a.apply_core_event(msg("a/b/c", "v"));
    assert!(a.expanded.is_empty());
}

#[test]
fn auto_expand_respects_manual_collapse() {
    let mut a = expand_app(1);
    a.apply_core_event(msg("a/b", "v"));
    let id = a.tree.find("a").unwrap();
    a.expanded.remove(&id);
    a.apply_core_event(msg("a/c", "v"));
    assert!(!a.expanded.contains(&id));
}

#[test]
fn memory_cap_comes_from_settings() {
    let s = Settings {
        max_history_mb: 1,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    let big = "x".repeat(600 * 1024);
    for _ in 0..3 {
        a.apply_core_event(msg("t", &big));
    }
    assert!(
        a.tree.history_bytes() <= 1024 * 1024,
        "{}",
        a.tree.history_bytes()
    );
    a.reset_tree();
    for _ in 0..3 {
        a.apply_core_event(msg("t", &big));
    }
    assert!(
        a.tree.history_bytes() <= 1024 * 1024,
        "cap survives reset_tree"
    );
}
