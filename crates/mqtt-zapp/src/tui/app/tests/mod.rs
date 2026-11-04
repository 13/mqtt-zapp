use super::*;
use crate::cli::Launch;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::client::{ConnState, CoreEvent};
use mqtt_zapp_core::config::{
    PasswordSource, Profile, ProfileStore, Protocol, Settings, TlsConfig,
};
use mqtt_zapp_core::state::{AppState, PickerSort};
use mqtt_zapp_core::{Bytes, Message};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

mod chart;
mod nav;
mod picker;
mod publish;
mod retained;
mod save;
mod search;
mod star;

fn msg(topic: &str, p: &str) -> CoreEvent {
    CoreEvent::Message {
        topic: topic.into(),
        msg: Message {
            payload: Bytes::copy_from_slice(p.as_bytes()),
            qos: 0,
            retain: true,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        },
    }
}

fn app() -> App {
    App::new(Launch::Picker, ProfileStore::default(), Settings::default()).unwrap()
}

/// A connected session showing `topics`.
fn browse(topics: &[&str]) -> App {
    let mut a = app();
    a.mode = Mode::Browse;
    a.conn_state = ConnState::Connected;
    for t in topics {
        a.apply_core_event(msg(t, "v"));
    }
    a
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
    s.keys.insert("quit".into(), "Q".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    let x = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE);
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
        retained: true,
        keepalive_s: None,
        clean_session: None,
        session_expiry_s: None,
    }
}

fn picker_app(profiles: Vec<Profile>) -> App {
    let store = ProfileStore { profiles };
    App::new(Launch::Picker, store, Settings::default()).unwrap()
}

fn press(a: &mut App, c: char) -> Effect {
    a.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn names(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
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

fn ctrl_r() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
}

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

fn del_key(a: &mut App) -> Effect {
    a.handle_key(key(KeyCode::Delete))
}

fn warn_profile(protocol: Protocol, insecure: bool, user: bool, pw: PasswordSource) -> Profile {
    let mut p = profile("w", pw);
    p.protocol = protocol;
    p.tls.insecure = insecure;
    p.username = user.then(|| "alice".to_string());
    p
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
    let m = n.latest().unwrap();
    assert_eq!((m.payload.len(), m.truncated_from), (3000, None));
    assert!(n.retained, "a non-empty retained payload stays retained");
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

fn fifty_fields() -> String {
    let fields: Vec<String> = (0..50).map(|i| format!("\"f{i}\":{i}")).collect();
    format!("{{{}}}", fields.join(","))
}

fn filter_on(a: &mut App, q: &str) {
    let f = mqtt_zapp_core::search::filter(&a.tree, q, mqtt_zapp_core::search::QueryMode::Plain)
        .unwrap();
    a.set_filter(f);
}

#[test]
fn startup_notes_bad_settings() {
    let mut a = app();
    a.set_startup_errors(
        Some("state file unreadable, backed up to /s/state.toml.bak".into()),
        Some("expected a string\nline 2".into()),
        Instant::now(),
    );
    let n = a.note_at(Instant::now()).unwrap();
    assert!(n.error);
    assert!(
        n.text
            .contains("state file unreadable, backed up to /s/state.toml.bak"),
        "{}",
        n.text
    );
    assert!(
        n.text
            .contains("settings.toml: expected a string line 2 (using defaults)"),
        "{}",
        n.text
    );
    let mut b = app();
    b.set_startup_errors(None, None, Instant::now());
    assert!(b.note_at(Instant::now()).is_none());
}

#[test]
fn idle_tick_is_slow() {
    let mut a = app();
    let now = Instant::now();
    assert_eq!(a.tick_interval(now), Duration::from_secs(1));
    // Each animating state asks for the fast tick.
    a.set_note(Note::info("hi"), now);
    assert_eq!(a.tick_interval(now), Duration::from_millis(100));
    // An expired note no longer animates.
    assert_eq!(
        a.tick_interval(now + NOTE_TTL + Duration::from_secs(1)),
        Duration::from_secs(1)
    );
    a.status_note = None;
    a.filter_due = Some(now);
    assert_eq!(a.tick_interval(now), Duration::from_millis(100));
    a.filter_due = None;
    a.await_reply = Some(("r".into(), now));
    assert_eq!(a.tick_interval(now), Duration::from_millis(100));
    a.await_reply = None;
    a.conn_state = ConnState::Connecting;
    assert_eq!(a.tick_interval(now), Duration::from_millis(100));
    a.conn_state = ConnState::Disconnected {
        reason: "x".into(),
        retry_in_ms: 2000,
    };
    assert_eq!(a.tick_interval(now), Duration::from_millis(100));
    a.conn_state = ConnState::Connected;
    assert_eq!(a.tick_interval(now), Duration::from_secs(1));
    // A flash and a live message rate.
    a.apply_core_event(msg("a/b", "1"));
    assert_eq!(a.tick_interval(Instant::now()), Duration::from_millis(100));
    let later = Instant::now() + Duration::from_secs(60);
    assert_eq!(a.tick_interval(later), Duration::from_secs(1));
}

#[test]
fn closed_channel_marks_failed() {
    let mut a = app();
    a.apply_core_event(CoreEvent::State(ConnState::Connected));
    a.on_connection_ended();
    assert!(
        matches!(&a.conn_state, ConnState::Failed { reason } if reason == "connection task ended")
    );
    // A failure never keeps the fast tick.
    assert_eq!(a.tick_interval(Instant::now()), Duration::from_secs(1));
}

#[test]
fn closed_channel_keeps_an_earlier_failure_reason() {
    let mut a = app();
    a.apply_core_event(CoreEvent::State(ConnState::Failed {
        reason: "refused".into(),
    }));
    a.on_connection_ended();
    assert!(matches!(&a.conn_state, ConnState::Failed { reason } if reason == "refused"));
}

#[test]
fn sub_refused_note() {
    let mut a = app();
    a.apply_core_event(CoreEvent::SubRefused {
        filter: "secret/#".into(),
        reason: "NotAuthorized".into(),
    });
    let n = a.note_at(Instant::now()).unwrap().clone();
    assert!(n.error);
    assert_eq!(n.text, "subscription refused: secret/# (NotAuthorized)");
    // Several refusals from one SUBACK are all named, not overwritten.
    a.apply_core_event(CoreEvent::SubRefused {
        filter: "b".into(),
        reason: "refused by the broker".into(),
    });
    let n = a.note_at(Instant::now()).unwrap();
    assert!(n.text.contains("secret/#") && n.text.contains("; b (refused by the broker)"));
}

#[test]
fn connack_info_notes_are_joined() {
    let mut a = app();
    a.apply_core_event(CoreEvent::ConnAckInfo {
        session_present: true,
        assigned_client_id: Some("auto-1\u{7}".into()),
        reason: None,
    });
    let n = a.note_at(Instant::now()).unwrap();
    assert!(!n.error);
    assert!(n.text.contains("session resumed"), "{}", n.text);
    assert!(n.text.contains("client id assigned: auto-1"), "{}", n.text);
    assert!(!n.text.contains('\u{7}'));
    let mut b = app();
    b.apply_core_event(CoreEvent::ConnAckInfo {
        session_present: false,
        assigned_client_id: None,
        reason: None,
    });
    assert!(b.note_at(Instant::now()).is_none());
}

#[test]
fn connack_info_keeps_a_live_error_note() {
    let mut a = app();
    a.apply_core_event(CoreEvent::SubRefused {
        filter: "x".into(),
        reason: "NotAuthorized".into(),
    });
    a.apply_core_event(CoreEvent::ConnAckInfo {
        session_present: true,
        assigned_client_id: None,
        reason: None,
    });
    assert!(a.note_at(Instant::now()).unwrap().error);
}

#[test]
fn broker_strings_in_notes_are_capped_and_flat() {
    let mut a = app();
    a.apply_core_event(CoreEvent::SubRefused {
        filter: "f".into(),
        reason: format!("a\nb{}", "z".repeat(500)),
    });
    let t = a.note_at(Instant::now()).unwrap().text.clone();
    assert!(
        !t.contains('\n') && t.contains('…') && t.chars().count() < 300,
        "{t}"
    );
}

#[test]
fn broker_switch_starts_live_and_clean() {
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Freeze);
    assert!(a.frozen && a.freeze_pin.is_some());
    a.chart = Some(None);
    a.chart_pinned.push(None);
    a.show_diff = true;
    a.search_regex = true;
    a.focus = Focus::Payload;
    a.reset_tree();
    assert!(!a.frozen);
    assert_eq!(a.freeze_pin, None);
    assert_eq!(a.chart, None);
    assert!(a.chart_pinned.is_empty());
    assert!(!a.show_diff && !a.search_regex);
    assert_eq!(a.focus, Focus::Tree);
}

#[test]
fn deleting_a_profile_drops_its_stars() {
    let mut a = browse(&["t"]);
    a.favorites
        .entry("P".into())
        .or_default()
        .insert("t".into());
    let gen_before = a.star_gen;
    a.profile_deleted("P");
    assert!(!a.favorites.contains_key("P"));
    assert_ne!(a.star_gen, gen_before);
}

#[test]
fn sent_payloads_do_not_cross_brokers() {
    let mut a = picker_app(vec![
        profile("a", PasswordSource::None),
        profile("b", PasswordSource::None),
    ]);
    assert!(matches!(
        a.handle_key(key(KeyCode::Enter)),
        Effect::Connect(..)
    ));
    a.sent.entry("t".into()).or_default().push_front("x".into());
    // Back to the same profile keeps them.
    a.handle_action(Action::Brokers);
    a.handle_key(key(KeyCode::Enter));
    assert!(!a.sent.is_empty(), "same broker");
    a.handle_action(Action::Brokers);
    a.handle_key(key(KeyCode::Char('j')));
    assert!(matches!(
        a.handle_key(key(KeyCode::Enter)),
        Effect::Connect(..)
    ));
    assert!(a.sent.is_empty(), "another broker");
}

#[test]
fn info_note_does_not_hide_a_fresh_error() {
    let mut a = browse(&["t"]);
    let now = Instant::now();
    a.set_note(Note::error("publish failed: boom"), now);
    a.set_background_note(Note::info("reply on x"), now + Duration::from_millis(500));
    assert_eq!(note_text(&a), "publish failed: boom");
    // Another error replaces it; so does anything once it has expired.
    a.set_background_note(Note::error("second"), now + Duration::from_secs(1));
    assert_eq!(note_text(&a), "second");
    let later = now + Duration::from_secs(1) + NOTE_TTL;
    a.set_background_note(Note::info("later"), later);
    assert_eq!(a.note_at(later).map(|n| n.text.as_str()), Some("later"));
}

#[test]
fn expand_all_in_the_starred_view_says_why_nothing_moves() {
    let mut a = browse(&["a/b"]);
    a.selected = a.tree.find("a");
    a.handle_action(Action::ToggleStar);
    a.handle_action(Action::StarredOnly);
    a.handle_action(Action::ExpandAll);
    assert_eq!(note_text(&a), "the starred view is always expanded");
}

#[test]
fn a_key_press_answer_replaces_a_fresh_error() {
    let mut a = browse(&["t"]);
    a.set_note(Note::error("publish failed: boom"), Instant::now());
    a.handle_action(Action::ToggleWrap);
    assert_eq!(note_text(&a), "wrap on");
}
