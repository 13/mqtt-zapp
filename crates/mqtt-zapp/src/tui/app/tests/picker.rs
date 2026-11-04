use super::*;

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

fn keyring_app(loader: PasswordLoader) -> (App, u64) {
    let mut a = picker_app(vec![profile("k", PasswordSource::Keyring)]);
    a.password_loader = loader;
    let e = a.begin_connect(profile("k", PasswordSource::Keyring));
    let Effect::LoadPassword { generation, .. } = e else {
        panic!("expected LoadPassword, got {e:?}");
    };
    (a, generation)
}

#[test]
fn keyring_failure_falls_back_to_password_prompt() {
    let lookup = |r: std::result::Result<Option<String>, String>| {
        let (mut a, g) = keyring_app(Arc::new(|_| Ok(None)));
        let e = a.password_loaded(g, r);
        (a.mode, e)
    };
    let (mode, e) = lookup(Err("no secret service".into()));
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
fn keyring_lookup_does_not_block() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let called = Arc::new(AtomicBool::new(false));
    let c = called.clone();
    let slow: PasswordLoader = Arc::new(move |_| {
        c.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(5));
        Ok(Some("pw".into()))
    });
    let mut a = picker_app(vec![profile("k", PasswordSource::Keyring)]);
    a.password_loader = slow;
    let start = Instant::now();
    let e = a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!called.load(Ordering::SeqCst), "lookup ran inline");
    assert!(matches!(e, Effect::LoadPassword { .. }), "got {e:?}");
    assert!(a.picker.waiting_keyring);
}

fn pending_two() -> (App, u64) {
    let mut a = picker_app(vec![
        profile("k", PasswordSource::Keyring),
        profile("o", PasswordSource::None),
    ]);
    let e = a.begin_connect(profile("k", PasswordSource::Keyring));
    let Effect::LoadPassword { generation, .. } = e else {
        panic!("{e:?}")
    };
    (a, generation)
}

#[test]
fn late_password_after_cancel_is_ignored() {
    let (mut a, g) = pending_two();
    a.server = Some(ServerInfo {
        name: "live".into(),
        url: String::new(),
        subscriptions: vec![],
        warning: None,
    });
    a.connected = Some("o".into());
    a.handle_key(key(KeyCode::Esc));
    assert_eq!(a.mode, Mode::Browse);
    assert!(!a.picker.waiting_keyring);
    let e = a.password_loaded(g, Ok(Some("pw".into())));
    assert!(matches!(e, Effect::None), "{e:?}");
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(a.connected.as_deref(), Some("o"));
}

#[test]
fn late_password_during_profile_edit_is_ignored() {
    let (mut a, g) = pending_two();
    press(&mut a, 'e');
    assert_eq!(a.mode, Mode::ProfileForm);
    let e = a.password_loaded(g, Ok(None));
    assert!(matches!(e, Effect::None), "{e:?}");
    assert_eq!(a.mode, Mode::ProfileForm);
    // Even if the mode were back at the picker, the lookup was cancelled.
    a.mode = Mode::Picker;
    let e = a.password_loaded(g, Ok(Some("pw".into())));
    assert!(matches!(e, Effect::None), "{e:?}");
}

#[test]
fn enter_while_pending_does_not_relookup() {
    let (mut a, g) = pending_two();
    let e = a.handle_key(key(KeyCode::Enter));
    assert!(matches!(e, Effect::None), "{e:?}");
    assert!(a.picker.waiting_keyring);
    // The first lookup is still the live one.
    let e = a.password_loaded(g, Ok(Some("pw".into())));
    assert!(matches!(e, Effect::Connect(..)), "{e:?}");
    assert!(!a.picker.waiting_keyring);
}

#[test]
fn stale_password_result_is_ignored() {
    let (mut a, g1) = keyring_app(Arc::new(|_| Ok(None)));
    // The user picks another profile before the lookup returns.
    let e = a.begin_connect(profile("p", PasswordSource::Prompt));
    assert!(matches!(e, Effect::None));
    assert_eq!(a.mode, Mode::Password);
    let e = a.password_loaded(g1, Ok(Some("late".into())));
    assert!(matches!(e, Effect::None));
    assert_eq!(a.mode, Mode::Password);
    assert_eq!(a.password.as_ref().unwrap().profile.name, "p");
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
fn env_and_command_sources_load_off_thread_and_report_errors() {
    for pw in [
        PasswordSource::Env("PW".into()),
        PasswordSource::Command("pass show x".into()),
    ] {
        let mut a = picker_app(vec![profile("e", pw.clone())]);
        let e = a.begin_connect(profile("e", pw));
        let Effect::LoadPassword { generation, .. } = e else {
            panic!("expected LoadPassword, got {e:?}");
        };
        assert!(a.picker.waiting_keyring);
        // A failure is shown, not turned into a password prompt.
        let e = a.password_loaded(generation, Err("password env var PW not set".into()));
        assert!(matches!(e, Effect::None));
        assert_eq!(a.mode, Mode::Picker);
        assert_eq!(note_text(&a), "password env var PW not set");
    }
}

fn startup_app(pw: PasswordSource) -> (App, u64) {
    let mut a = App::new(
        Launch::Connect(profile("adhoc", pw)),
        ProfileStore::default(),
        Settings::default(),
    )
    .unwrap();
    let e = a.startup_effect();
    let Effect::LoadPassword { generation, .. } = e else {
        panic!("expected LoadPassword, got {e:?}");
    };
    (a, generation)
}

#[test]
fn startup_connect_with_loaded_password_connects() {
    for src in [
        PasswordSource::Keyring,
        PasswordSource::Env("PW".into()),
        PasswordSource::Command("echo pw".into()),
    ] {
        let (mut a, g) = startup_app(src);
        let e = a.password_loaded(g, Ok(Some("pw".into())));
        assert!(
            matches!(e, Effect::Connect(_, Some(ref pw)) if pw == "pw"),
            "{e:?}"
        );
        assert_eq!(a.mode, Mode::Browse);
    }
}

#[test]
fn startup_connect_failed_lookup_is_visible() {
    let (mut a, g) = startup_app(PasswordSource::Env("PW".into()));
    let e = a.password_loaded(g, Err("PW is not set".into()));
    assert!(matches!(e, Effect::None), "{e:?}");
    assert!(
        matches!(&a.conn_state, ConnState::Failed { reason } if reason.contains("PW is not set")),
        "{:?}",
        a.conn_state
    );
    assert!(a.status_note.is_some());
}

#[test]
fn startup_lookup_dropped_after_mode_change() {
    let (mut a, g) = startup_app(PasswordSource::Keyring);
    a.mode = Mode::Picker;
    let e = a.password_loaded(g, Ok(Some("pw".into())));
    assert!(matches!(e, Effect::None), "{e:?}");
}

#[test]
fn app_state_keeps_unknown_keys() {
    let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
    let mut st = AppState::default();
    st.extra.insert("future_key".into(), 1.into());
    a.set_app_state(st);
    assert!(a.app_state().extra.contains_key("future_key"));
}
