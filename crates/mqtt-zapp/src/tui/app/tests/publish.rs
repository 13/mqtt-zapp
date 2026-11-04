use super::*;

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
