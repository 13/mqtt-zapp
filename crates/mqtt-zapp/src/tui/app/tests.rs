use super::*;
use crate::cli::Launch;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::client::{ConnState, CoreEvent};
use mqtt_zapp_core::config::{
    PasswordSource, Profile, ProfileStore, Protocol, Settings, TlsConfig,
};
use mqtt_zapp_core::state::{AppState, PickerSort};
use mqtt_zapp_core::{Bytes, Message};
use std::time::{Duration, Instant, SystemTime};

fn msg(topic: &str, p: &str) -> CoreEvent {
    CoreEvent::Message {
        topic: topic.into(),
        msg: Message {
            payload: Bytes::copy_from_slice(p.as_bytes()),
            qos: 0,
            retain: true,
            received_at: SystemTime::now(),
            truncated_from: None,
            props: None,
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

#[test]
fn copied_payload_is_sanitized() {
    let mut a = browse(&[]);
    a.apply_core_event(msg("x/y", "ok\x1b]52;c;x\x07"));
    a.selected = a.tree.find("x/y");
    match a.handle_action(Action::CopyPayload) {
        Effect::Copy { text, .. } => assert_eq!(text, "ok]52;c;x"),
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
        a.handle_key(key(KeyCode::Char('y'))),
        Effect::ClearRetained(_)
    ));
}

#[test]
fn enter_does_not_confirm_a_destructive_prompt() {
    let mut a = browse(&["a/b"]);
    a.selected = a.tree.find("a/b");
    a.handle_action(Action::DeleteRetained);
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
    assert_eq!(a.note_at(Instant::now()), Some(&Note::info("cancelled")));
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
    assert_eq!(*a.expanded.as_set(), ids(&a, &["a", "a/b", "a/d"]));
    a.expanded.insert(a.tree.find("z").unwrap());
    a.handle_action(Action::CollapseAll);
    assert_eq!(*a.expanded.as_set(), ids(&a, &["z"]));
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
fn reset_tree_clears_chart_cache() {
    let mut a = browse(&["a"]);
    a.handle_action(Action::Down);
    let _ = a.chart_series();
    assert!(a.chart_cache.is_cached());
    a.reset_tree();
    assert!(!a.chart_cache.is_cached());
}

#[test]
fn dropped_events_accumulate() {
    let mut a = browse(&["a"]);
    a.apply_core_event(CoreEvent::Dropped(3));
    a.apply_core_event(CoreEvent::Dropped(3));
    assert_eq!(a.dropped, 6);
}

#[test]
fn reset_tree_clears_dropped() {
    let mut a = browse(&["a"]);
    a.apply_core_event(CoreEvent::Dropped(3));
    a.reset_tree();
    assert_eq!(a.dropped, 0);
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
fn help_opens_on_question_mark_and_enter_closes() {
    let mut a = app();
    a.mode = Mode::Browse;
    let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    a.handle_key(k('?'));
    assert_eq!(a.mode, Mode::Help);
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
    assert_eq!(a.mode, Mode::Browse);
    a.handle_key(k('?'));
    assert!(matches!(
        a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Effect::Quit
    ));
}

#[test]
fn help_typing_builds_query_and_esc_clears_then_closes() {
    let mut a = app();
    a.mode = Mode::Browse;
    a.handle_action(Action::Help);
    // Bound keys (q quit, f freeze) are typed, never dispatched.
    type_str(&mut a, "qf");
    assert_eq!(a.help_query, "qf");
    assert_eq!(a.mode, Mode::Help);
    assert!(!a.frozen);
    a.handle_key(key(KeyCode::Backspace));
    assert_eq!(a.help_query, "q");
    a.handle_key(key(KeyCode::Esc));
    assert_eq!(a.help_query, "");
    assert_eq!(a.mode, Mode::Help);
    a.handle_key(key(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Browse);
    // A new session starts empty.
    a.handle_action(Action::Help);
    type_str(&mut a, "x");
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.mode, Mode::Browse);
    a.handle_action(Action::Help);
    assert_eq!(a.help_query, "");
}

#[test]
fn help_question_mark_closes_only_when_query_empty() {
    let mut a = app();
    a.mode = Mode::Browse;
    a.handle_action(Action::Help);
    type_str(&mut a, "a?");
    assert_eq!(a.mode, Mode::Help);
    assert_eq!(a.help_query, "a?");
    a.handle_key(key(KeyCode::Esc));
    a.handle_key(key(KeyCode::Char('?')));
    assert_eq!(a.mode, Mode::Browse);
}

#[test]
fn stats_any_key_still_closes() {
    let mut a = app();
    a.mode = Mode::Browse;
    a.handle_action(Action::Stats);
    assert_eq!(a.mode, Mode::Stats);
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('x'))),
        Effect::None
    ));
    assert_eq!(a.mode, Mode::Browse);
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

// --- final review fixes ---

fn capped_app() -> App {
    let s = Settings {
        max_history_mb: 1,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    a
}

fn kb(tag: &str) -> String {
    format!("{tag}{}", "x".repeat(100 * 1024))
}

#[test]
fn cap_eviction_elsewhere_keeps_paused_entry() {
    let mut a = capped_app();
    for i in 0..5 {
        a.apply_core_event(msg("a", &kb(&i.to_string())));
    }
    a.selected = a.tree.find("a");
    a.history_sel = Some(3);
    let paused = |a: &App| {
        String::from_utf8_lossy(&crate::tui::message_view::shown_message(a).unwrap().payload)[..1]
            .to_owned()
    };
    assert_eq!(paused(&a), "3");
    // The cap evicts a0..a2 (older); the paused a3 survives and stays shown.
    for i in 0..8 {
        a.apply_core_event(msg("b", &kb(&format!("b{i}"))));
    }
    assert_eq!(paused(&a), "3", "the paused entry must not change");
}

#[test]
fn cap_eviction_elsewhere_clears_gone_diff_base() {
    let mut a = capped_app();
    for i in 0..5 {
        a.apply_core_event(msg("a", &kb(&i.to_string())));
    }
    a.selected = a.tree.find("a");
    a.history_sel = Some(0);
    a.handle_action(Action::MarkDiffBase);
    a.history_sel = None;
    for i in 0..8 {
        a.apply_core_event(msg("b", &kb(&format!("b{i}"))));
    }
    assert_eq!(a.diff_base, None);
}

#[test]
fn rename_in_last_used_sort_keeps_renamed_selected() {
    let mut a = picker_app(vec![
        profile("c", PasswordSource::None),
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    let mut st = AppState {
        picker_sort: PickerSort::LastUsed,
        ..Default::default()
    };
    st.touch("a", 10);
    st.touch("b", 5);
    a.set_app_state(st);
    let renamed = vec![
        profile("c", PasswordSource::None),
        profile("a", PasswordSource::None),
        profile("bb", PasswordSource::None),
    ];
    a.profile_edit_saved(renamed, Some("b"), "bb");
    assert_eq!(a.picker.selected().unwrap().name, "bb");
}

#[test]
fn move_at_edge_in_name_sort_changes_nothing() {
    let mut a = picker_app(vec![
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    press(&mut a, 's'); // name sort; "a" selected, first in file order
    assert!(matches!(press(&mut a, 'K'), Effect::None));
    assert_eq!(a.picker.sort, PickerSort::Name);
}

/// Connected to `lab` (second of home/lab/vault) through the picker.
fn session_on_lab() -> App {
    let mut a = picker_app(vec![
        profile("home", PasswordSource::None),
        profile("lab", PasswordSource::None),
        profile("vault", PasswordSource::Prompt),
    ]);
    a.handle_key(key(KeyCode::Down));
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::Connect(p, _) if p.name == "lab"));
    a.picker.sel = 0;
    a
}

#[test]
fn b_opens_picker_on_connected_profile() {
    let mut a = session_on_lab();
    assert!(matches!(press(&mut a, 'b'), Effect::None));
    assert_eq!(a.mode, Mode::Picker);
    assert_eq!(a.picker.selected().unwrap().name, "lab");
    assert!(a.in_session());
}

#[test]
fn esc_in_picker_returns_to_session() {
    let mut a = session_on_lab();
    press(&mut a, 'b');
    assert!(matches!(a.handle_key(key(KeyCode::Esc)), Effect::None));
    assert_eq!(a.mode, Mode::Browse);
    press(&mut a, 'b');
    assert!(matches!(press(&mut a, 'b'), Effect::None));
    assert_eq!(a.mode, Mode::Browse);
}

#[test]
fn esc_in_picker_without_session_does_nothing() {
    let mut a = picker_app(vec![profile("home", PasswordSource::None)]);
    assert!(!a.in_session());
    assert!(matches!(a.handle_key(key(KeyCode::Esc)), Effect::None));
    assert!(matches!(press(&mut a, 'b'), Effect::None));
    assert_eq!(a.mode, Mode::Picker);
}

#[test]
fn enter_on_connected_profile_returns_without_reconnect() {
    let mut a = session_on_lab();
    press(&mut a, 'b');
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("already connected to lab"))
    );
}

#[test]
fn enter_on_other_profile_switches() {
    let mut a = session_on_lab();
    press(&mut a, 'b');
    a.handle_key(key(KeyCode::Up));
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::Connect(p, _) if p.name == "home"));
    assert_eq!(a.server.as_ref().unwrap().name, "home");
}

#[test]
fn switch_password_esc_returns_to_picker_session_intact() {
    let mut a = session_on_lab();
    press(&mut a, 'b');
    a.handle_key(key(KeyCode::Down));
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
    assert_eq!(a.mode, Mode::Password);
    assert!(matches!(a.handle_key(key(KeyCode::Esc)), Effect::None));
    assert_eq!(a.mode, Mode::Picker);
    assert!(a.in_session());
    assert_eq!(a.server.as_ref().unwrap().name, "lab");
    assert_eq!(a.connected.as_deref(), Some("lab"));
}

#[test]
fn messages_keep_arriving_while_picking() {
    let mut a = session_on_lab();
    press(&mut a, 'b');
    a.apply_core_event(msg("still/here", "1"));
    a.handle_key(key(KeyCode::Esc));
    assert!(a.tree.find("still/here").is_some());
}

#[test]
fn connected_marker_follows_rename_and_delete() {
    let mut a = session_on_lab();
    a.profile_renamed("lab", "lab2");
    assert_eq!(a.connected.as_deref(), Some("lab2"));
    a.profile_deleted("lab2");
    assert_eq!(a.connected, None);
    assert!(a.in_session(), "deleting the profile keeps the session");
}

#[test]
fn adhoc_session_marks_nothing() {
    let mut a = App::new(
        Launch::Connect(profile("adhoc", PasswordSource::None)),
        ProfileStore {
            profiles: vec![Profile {
                host: "other".into(),
                ..profile("adhoc", PasswordSource::None)
            }],
        },
        Settings::default(),
    )
    .unwrap();
    assert!(matches!(a.startup_effect(), Effect::Connect(..)));
    assert_eq!(a.connected, None);
    press(&mut a, 'b');
    assert_eq!(a.mode, Mode::Picker);
    // A saved profile that happens to share the ad-hoc name still connects.
    assert!(matches!(
        a.handle_key(key(KeyCode::Enter)),
        Effect::Connect(..)
    ));
}

#[test]
fn enter_on_edited_connected_profile_reconnects() {
    let mut a = session_on_lab();
    press(&mut a, 'b');
    let mut profiles = a.picker.profiles.clone();
    profiles[1].port = 1884;
    a.profile_edit_saved(profiles, Some("lab"), "lab");
    assert_eq!(a.connected.as_deref(), Some("lab"), "still marked");
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::Connect(p, _) if p.port == 1884));
}

#[test]
fn enter_on_connected_profile_after_hard_failure_reconnects() {
    let mut a = session_on_lab();
    a.apply_core_event(CoreEvent::State(ConnState::Disconnected {
        reason: "bad ca".into(),
        retry_in_ms: 0,
    }));
    press(&mut a, 'b');
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::Connect(p, _) if p.name == "lab"));
}

fn del_key(a: &mut App) -> Effect {
    a.handle_key(key(KeyCode::Delete))
}

#[test]
fn delete_topic_asks_then_clears_retained_and_removes_subtree() {
    let mut a = browse(&["a/b", "a/b/c", "a/x"]);
    a.selected = a.tree.find("a/b");
    assert!(matches!(del_key(&mut a), Effect::None));
    assert_eq!(a.mode, Mode::Confirm);
    assert_eq!(
        a.confirm.as_ref().unwrap().prompt,
        "delete a/b and clear 2 retained on the broker?"
    );
    match a.handle_key(key(KeyCode::Char('y'))) {
        Effect::ClearRetained(mut t) => {
            t.sort();
            assert_eq!(t, ["a/b", "a/b/c"]);
        }
        e => panic!("{e:?}"),
    }
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(a.tree.find("a/b"), None);
    assert!(a.tree.find("a/x").is_some());
    assert_eq!(
        a.selected,
        a.tree.find("a"),
        "selection moves to the parent"
    );
    let parent = a.tree.node(a.tree.find("a").unwrap());
    assert_eq!(parent.children.keys().collect::<Vec<_>>(), ["x"]);
}

#[test]
fn broker_echo_of_the_clear_does_not_bring_the_topic_back() {
    let mut a = browse(&["a/b", "a/x"]);
    a.selected = a.tree.find("a/b");
    del_key(&mut a);
    a.handle_key(key(KeyCode::Char('y')));
    a.apply_core_event(plain("a/b", ""));
    assert_eq!(a.tree.find("a/b"), None, "empty echo dropped");
    // A real new message brings it back.
    a.apply_core_event(plain("a/b", "again"));
    assert!(a.tree.find("a/b").is_some());
    // The echo is swallowed once only.
    a.apply_core_event(plain("a/x", ""));
    assert!(a.tree.find("a/x").is_some());
}

#[test]
fn delete_topic_without_retained_only_removes_from_list() {
    let mut a = browse(&[]);
    a.apply_core_event(plain("live/t", "1"));
    a.apply_core_event(msg("$SYS/broker/uptime", "5"));
    a.selected = a.tree.find("live/t");
    del_key(&mut a);
    assert_eq!(
        a.confirm.as_ref().unwrap().prompt,
        "remove live/t from the list? (nothing retained on the broker)"
    );
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('y'))),
        Effect::None
    ));
    assert_eq!(a.tree.find("live"), None, "empty parent pruned");
    assert_eq!(a.selected, None);
    // `$` topics are the broker's: only the list.
    a.selected = a.tree.find("$SYS/broker/uptime");
    del_key(&mut a);
    assert!(
        a.confirm
            .as_ref()
            .unwrap()
            .prompt
            .starts_with("remove $SYS/broker/uptime")
    );
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('y'))),
        Effect::None
    ));
    assert_eq!(a.tree.find("$SYS"), None);
}

#[test]
fn delete_topic_cancel_and_nothing_selected() {
    let mut a = browse(&["a/b"]);
    assert!(matches!(del_key(&mut a), Effect::None));
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("no topic selected"))
    );
    a.selected = a.tree.find("a/b");
    del_key(&mut a);
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('n'))),
        Effect::None
    ));
    assert!(a.tree.find("a/b").is_some());
}

#[test]
fn delete_topic_clears_pinned_history_and_diff_base() {
    let mut a = browse(&["a/b", "a/b", "a/x"]);
    a.selected = a.tree.find("a/b");
    a.history_sel = Some(0);
    a.diff_base = Some((SystemTime::now(), 1));
    del_key(&mut a);
    a.handle_key(key(KeyCode::Char('y')));
    assert_eq!((a.history_sel, a.diff_base), (None, None));
}

#[test]
fn delete_topic_updates_an_active_filter() {
    let mut a = browse(&["a/wzr1", "a/foo", "b/wzr2"]);
    a.handle_action(Action::Search);
    type_str(&mut a, "wzr");
    a.handle_key(key(KeyCode::Enter));
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
    a.selected = a.tree.find("a/wzr1");
    del_key(&mut a);
    a.handle_key(key(KeyCode::Char('y')));
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
    assert!(
        !row_names(&a).contains(&"a".to_string()),
        "{:?}",
        row_names(&a)
    );
    // And stays right after the next message triggers a rebuild.
    a.apply_core_event(msg("c/other", "1"));
    a.on_tick();
    assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
}

#[test]
fn retained_arriving_during_the_confirm_is_cleared_too() {
    let mut a = browse(&["a/b"]);
    a.selected = a.tree.find("a/b");
    del_key(&mut a);
    a.apply_core_event(msg("a/b/new", "v"));
    match a.handle_key(key(KeyCode::Char('y'))) {
        Effect::ClearRetained(mut t) => {
            t.sort();
            assert_eq!(t, ["a/b", "a/b/new"]);
        }
        e => panic!("{e:?}"),
    }
}

fn warn_profile(protocol: Protocol, insecure: bool, user: bool, pw: PasswordSource) -> Profile {
    let mut p = profile("w", pw);
    p.protocol = protocol;
    p.tls.insecure = insecure;
    p.username = user.then(|| "alice".to_string());
    p
}

#[test]
fn security_warning_unverified_tls() {
    use super::front::security_warning as w;
    for proto in [Protocol::Mqtts, Protocol::Wss] {
        let p = warn_profile(proto, true, false, PasswordSource::None);
        assert_eq!(w(&p), Some("unverified tls"));
    }
    // Insecure on a plain transport has no certificate to skip.
    let p = warn_profile(Protocol::Mqtt, true, false, PasswordSource::None);
    assert_eq!(w(&p), None);
}

#[test]
fn security_warning_cleartext_password() {
    use super::front::security_warning as w;
    for proto in [Protocol::Mqtt, Protocol::Ws] {
        for pw in [PasswordSource::Keyring, PasswordSource::Prompt] {
            let p = warn_profile(proto, false, true, pw);
            assert_eq!(w(&p), Some("password in cleartext"));
        }
        let p = warn_profile(proto, false, true, PasswordSource::None);
        assert_eq!(w(&p), None, "no password source");
        let p = warn_profile(proto, false, false, PasswordSource::Prompt);
        assert_eq!(w(&p), None, "no username");
    }
    let p = warn_profile(Protocol::Mqtts, false, true, PasswordSource::Prompt);
    assert_eq!(w(&p), None, "verified tls is fine");
}

#[test]
fn zero_payload_cap_means_unlimited() {
    let settings = Settings {
        max_payload_kb: 0,
        ..Settings::default()
    };
    let mut a = App::new(Launch::Picker, ProfileStore::default(), settings).unwrap();
    a.mode = Mode::Browse;
    let big = "x".repeat(3000);
    a.apply_core_event(msg("t", &big));
    let n = a.tree.node(a.tree.find("t").unwrap());
    let m = n.latest.as_ref().unwrap();
    assert_eq!((m.payload.len(), m.truncated_from), (3000, None));
    assert!(n.retained, "a non-empty retained payload stays retained");
}

#[test]
fn rows_are_cached_between_calls() {
    let mut a = browse(&["a/b", "c"]);
    a.handle_action(Action::Down);
    let first = a.rows();
    a.on_tick();
    assert!(std::rc::Rc::ptr_eq(&first, &a.rows()));
    // A message to a known topic keeps the shape (Name sort).
    a.apply_core_event(msg("c", "again"));
    assert!(std::rc::Rc::ptr_eq(&first, &a.rows()));
}

#[test]
fn rows_rebuild_on_expand_filter_sort_and_new_topic() {
    let mut a = browse(&["a/b", "c/wzr"]);
    a.handle_action(Action::Down);
    let changed = |a: &mut App, f: &dyn Fn(&mut App)| {
        let before = a.rows();
        f(a);
        let after = a.rows();
        assert!(!std::rc::Rc::ptr_eq(&before, &after));
        after
    };
    let rows = changed(&mut a, &|a| {
        a.handle_action(Action::Expand);
    });
    assert_eq!(rows.len(), 3, "expand shows a/b");
    let rows = changed(&mut a, &|a| {
        a.handle_action(Action::Search);
        type_str(a, "wzr");
    });
    assert!(!rows.iter().any(|r| r.id == a.tree.find("a").unwrap()));
    let rows = changed(&mut a, &|a| {
        a.handle_key(key(KeyCode::Esc));
    });
    assert!(rows.iter().any(|r| r.id == a.tree.find("a").unwrap()));
    changed(&mut a, &|a| {
        a.handle_action(Action::CycleSort);
    });
    let rows = changed(&mut a, &|a| {
        a.apply_core_event(msg("d", "new"));
    });
    assert!(rows.iter().any(|r| r.id == a.tree.find("d").unwrap()));
}

#[test]
fn rows_drop_removed_topic() {
    let mut a = browse(&["a/b", "a/x"]);
    a.selected = a.tree.find("a");
    a.handle_action(Action::Expand);
    let b = a.tree.find("a/b").unwrap();
    assert!(a.rows().iter().any(|r| r.id == b));
    a.selected = Some(b);
    del_key(&mut a);
    a.handle_key(key(KeyCode::Char('y')));
    assert!(!a.tree.is_alive(b));
    assert!(!a.rows().iter().any(|r| r.id == b));
    // A direct remove too.
    let x = a.tree.find("a/x").unwrap();
    a.rows();
    a.tree.remove(x);
    assert!(!a.rows().iter().any(|r| r.id == x));
}

#[test]
fn count_sort_selection_follows_id_across_rebuild() {
    let mut a = browse(&["b", "b", "a", "c"]);
    a.handle_action(Action::CycleSort);
    assert_eq!(a.sort, SortOrder::Count);
    assert_eq!(row_names(&a), ["b", "a", "c"]);
    a.selected = a.tree.find("b");
    for _ in 0..2 {
        a.apply_core_event(msg("a", "more"));
    }
    // Count order refreshes at most once a second: within the same second
    // the cached order stands.
    assert_eq!(row_names(&a), ["b", "a", "c"]);
    a.created = a.created.checked_sub(Duration::from_secs(2)).unwrap();
    let rows = a.rows();
    assert_eq!(row_names(&a), ["a", "b", "c"]);
    let i = rows
        .iter()
        .position(|r| Some(r.id) == a.tree.find("b"))
        .unwrap();
    a.handle_action(Action::Down);
    assert_eq!(a.selected, Some(rows[i + 1].id));
    assert_eq!(sel(&a), "c");
}

/// See `fast_enough_at_spec_scale`: CI runs this with `--release`.
#[test]
#[cfg_attr(debug_assertions, ignore = "timing: run with --release")]
fn rows_fast_at_50k() {
    let mut a = browse(&[]);
    for i in 0..50_000 {
        a.apply_core_event(plain(&format!("dev_{i}/x"), "v"));
    }
    for i in 0..50_000 {
        a.expanded.insert(a.tree.find(&format!("dev_{i}")).unwrap());
    }
    let s = Instant::now();
    let rows = a.rows();
    let first = s.elapsed();
    assert_eq!(rows.len(), 100_000);
    let s = Instant::now();
    let again = a.rows();
    let cached = s.elapsed();
    assert!(std::rc::Rc::ptr_eq(&rows, &again));
    eprintln!("rows_fast_at_50k: first {first:?}, cached {cached:?}");
    assert!(first < Duration::from_millis(60), "first {first:?}");
    assert!(cached < Duration::from_millis(1), "cached {cached:?}");
}

#[test]
fn reset_tree_drops_cached_rows() {
    let mut a = browse(&["a"]);
    assert_eq!(row_names(&a), ["a"]);
    a.reset_tree();
    assert!(a.rows().is_empty());
    // Same shape generation as the old tree had; still the new rows.
    a.apply_core_event(msg("b", "v"));
    assert_eq!(row_names(&a), ["b"]);
}

#[test]
fn freeze_stops_tick_redraws() {
    let mut a = browse(&["a/live"]);
    a.search = "live".into();
    a.frozen = true;
    a.dirty = false;
    a.apply_core_event(msg("a/live", "next"));
    a.apply_core_event(msg("b/live", "new"));
    a.last_filter_rebuild -= Duration::from_millis(500);
    a.on_tick();
    assert!(!a.dirty, "a frozen display must not redraw on its own");
    // Messages still land in the tree; the filter waits for the thaw.
    let b = a.tree.find("b/live").expect("message applied while frozen");
    assert_eq!(a.tree.node(b).msg_count, 1);
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
fn freeze_pins_shown_payload_for_copy() {
    let mut a = browse(&[]);
    a.apply_core_event(msg("x/y", "old"));
    a.selected = a.tree.find("x/y");
    a.handle_action(Action::Freeze);
    a.apply_core_event(msg("x/y", "new"));
    match a.handle_action(Action::CopyPayload) {
        Effect::Copy { text, .. } => assert_eq!(text, "old", "Y copies what is on screen"),
        e => panic!("{e:?}"),
    }
    a.handle_action(Action::Freeze);
    assert_eq!(a.history_sel, None, "unfreeze goes back to live");
    match a.handle_action(Action::CopyPayload) {
        Effect::Copy { text, .. } => assert_eq!(text, "new"),
        e => panic!("{e:?}"),
    }
    // The pinned entry falls out of the ring while frozen: still live after.
    a.handle_action(Action::Freeze);
    for i in 0..a.history_depth + 1 {
        a.apply_core_event(msg("x/y", &format!("n{i}")));
    }
    a.handle_action(Action::Freeze);
    assert_eq!(a.history_sel, None, "an evicted pin still thaws to live");
    // A pin the user made before freezing survives the thaw.
    a.history_sel = Some(0);
    a.handle_action(Action::Freeze);
    a.handle_action(Action::Freeze);
    assert_eq!(a.history_sel, Some(0));
}

#[test]
fn freeze_key_toggles_and_redraws() {
    let mut a = browse(&["a"]);
    a.dirty = false;
    a.handle_key(key(KeyCode::Char('f')));
    assert!(a.frozen);
    assert!(a.dirty, "the frozen badge must be drawn");
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("display frozen – f to resume"))
    );
    a.dirty = false;
    // A tick alone does not redraw a frozen display; a key does.
    a.on_tick();
    assert!(!a.dirty);
    a.handle_key(key(KeyCode::Char('j')));
    assert!(a.dirty);
    a.handle_key(key(KeyCode::Char('f')));
    assert!(!a.frozen);
    assert_eq!(a.note_at(Instant::now()), Some(&Note::info("display live")));
}

#[test]
fn resize_redraws_while_frozen() {
    let mut a = browse(&["a"]);
    a.frozen = true;
    a.dirty = false;
    a.on_resize();
    a.on_tick();
    assert!(a.dirty, "a resize always redraws");
}

#[test]
fn background_note_does_not_redraw_while_frozen() {
    let mut a = browse(&["a"]);
    a.frozen = true;
    a.dirty = false;
    let now = Instant::now();
    a.set_background_note(Note::info("published"), now);
    a.on_tick();
    assert!(
        !a.dirty,
        "a background result must not redraw a frozen display"
    );
    assert_eq!(a.note_at(now), Some(&Note::info("published")));
    a.frozen = false;
    a.set_background_note(Note::info("cleared 3 retained"), now);
    assert!(a.dirty);
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

fn chart_app(payload: &str, n: usize) -> App {
    let mut a = browse(&[]);
    for _ in 0..n {
        a.apply_core_event(msg("t", payload));
    }
    a.handle_action(Action::Down);
    a
}

fn note_text(a: &App) -> String {
    a.note_at(Instant::now())
        .map(|n| n.text.clone())
        .unwrap_or_default()
}

#[test]
fn chart_pin_overlays_next_series() {
    let mut a = chart_app(r#"{"a":1,"b":2}"#, 3);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(Some("b".into())));
    assert_eq!(a.chart_pinned, vec![Some("a".to_owned())]);
    // `c` skips pinned series: past `b` the chart goes off, pins stay.
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    assert_eq!(a.chart_pinned, vec![Some("a".to_owned())]);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(Some("b".into())));
}

#[test]
fn chart_pin_again_unpins() {
    let mut a = chart_app(r#"{"a":1,"b":2}"#, 3);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    assert_eq!(a.chart_pinned.len(), 1);
    a.handle_action(Action::ChartPin);
    assert!(a.chart_pinned.is_empty());
}

#[test]
fn chart_pin_caps_at_three() {
    let mut a = chart_app(r#"{"a":1,"b":2,"c":3,"d":4}"#, 3);
    for _ in 0..4 {
        a.handle_action(Action::Chart);
        a.handle_action(Action::ChartPin);
    }
    assert_eq!(a.chart, Some(Some("d".into())));
    assert_eq!(
        a.chart_pinned,
        vec![Some("b".into()), Some("c".into()), Some("d".into())]
    );
    assert_eq!(note_text(&a), "pinned: d (dropped a)");
}

#[test]
fn chart_pins_cleared_on_select() {
    let mut a = chart_app(r#"{"a":1,"b":2}"#, 3);
    a.apply_core_event(msg("u", "1"));
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    // Turning the chart off keeps the pins.
    a.handle_action(Action::Chart);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    assert_eq!(a.chart_pinned.len(), 1);
    a.handle_action(Action::Down);
    assert!(a.chart_pinned.is_empty());
    a.chart_pinned.push(None);
    a.reset_tree();
    assert!(a.chart_pinned.is_empty());
}

#[test]
fn chart_pin_without_chart_notes() {
    let mut a = chart_app(r#"{"a":1}"#, 2);
    a.handle_action(Action::ChartPin);
    assert!(a.chart_pinned.is_empty());
    assert_eq!(note_text(&a), "nothing to pin");
}
