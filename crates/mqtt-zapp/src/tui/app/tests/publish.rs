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
            props,
        } => {
            assert_eq!(topic, "dev/cmnd/POWER");
            assert_eq!(payload, b"q");
            assert_eq!((qos, retain), (0, false));
            assert_eq!(props, None, "3.1.1 session: no props");
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

/// A history app whose oldest entry (index 0) is the message `tweak` builds.
fn resend_app(tweak: impl FnOnce(&mut Message)) -> App {
    let mut a = history_app(10, &[]);
    let CoreEvent::Message { topic, mut msg } = msg("t", "old") else {
        unreachable!()
    };
    tweak(&mut msg);
    a.apply_core_event(CoreEvent::Message { topic, msg });
    a.apply_core_event(self::msg("t", "new"));
    a.selected = a.tree.find("t");
    a.focus = Focus::History;
    a.history_sel = Some(0);
    a
}

#[test]
fn resend_prefills_from_history() {
    let mut a = resend_app(|m| {
        m.qos = 2;
        m.retain = true;
    });
    a.handle_action(Action::Publish);
    let f = a.publish.as_ref().unwrap();
    assert_eq!(f.topic.lines().join(""), "t");
    assert_eq!(f.payload_text(), "old");
    assert_eq!((f.qos, f.retain), (2, true));
}

#[test]
fn resend_needs_history_focus() {
    let mut a = resend_app(|_| {});
    a.focus = Focus::Tree;
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "");
    // History focus but the live entry (nothing selected): as today.
    let mut a = resend_app(|_| {});
    a.history_sel = None;
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "");
}

#[test]
fn resend_refuses_truncated_and_binary() {
    let mut a = resend_app(|m| m.truncated_from = Some(100));
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "");
    assert_eq!(a.publish.as_ref().unwrap().topic.lines().join(""), "t");
    assert_eq!(note_text(&a), "can't re-send: payload was truncated");

    let mut a = resend_app(|m| m.payload = Bytes::from_static(&[0xff, 0xfe, 0x00]));
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "");
    assert_eq!(note_text(&a), "can't re-send: binary payload");
}

#[test]
fn resend_sparkplug_notes_json() {
    let mut a = resend_app(|m| {
        m.sparkplug = true;
        m.wire_len = Some(12);
        m.payload = Bytes::from_static(b"{\"metrics\":[]}");
    });
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "{\"metrics\":[]}");
    assert_eq!(note_text(&a), "re-sending Sparkplug as JSON");
}

fn v5_profile() -> Profile {
    let mut p = profile("p", PasswordSource::None);
    p.mqtt_version = mqtt_zapp_core::config::MqttVersion::V5;
    p
}

/// A v5 session with a nested topic; the form is open on the payload.
fn reply_app() -> App {
    let mut a = browse(&["dev/cmnd/POWER", "other"]);
    a.session_profile = Some(v5_profile());
    a.selected = a.tree.find("other");
    a.handle_action(Action::Publish);
    for _ in 0..3 {
        a.handle_key(key(KeyCode::Tab));
    }
    type_str(&mut a, "ack/lab");
    a
}

#[test]
fn reply_follow_selects_reply() {
    let mut a = reply_app();
    match a.handle_key(ctrl_key('s')) {
        Effect::Publish { props, .. } => {
            assert_eq!(props.unwrap().response_topic.as_deref(), Some("ack/lab"));
        }
        e => panic!("{e:?}"),
    }
    assert!(a.await_reply.is_some());
    a.handle_key(key(KeyCode::Esc));
    a.apply_core_event(msg("ack/other", "no"));
    assert_eq!(sel(&a), "other", "other topics are ignored");
    a.apply_core_event(msg("ack/lab", "yes"));
    assert_eq!(sel(&a), "ack/lab");
    assert!(a.expanded.contains(&a.tree.find("ack").unwrap()));
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("reply on ack/lab"))
    );
    assert!(a.await_reply.is_none());
}

#[test]
fn reply_follow_expires_and_fires_once() {
    let mut a = reply_app();
    a.handle_key(ctrl_key('s'));
    a.handle_key(key(KeyCode::Esc));
    let (t, _) = a.await_reply.take().unwrap();
    a.await_reply = Some((t, Instant::now() - REPLY_WINDOW - Duration::from_secs(1)));
    a.apply_core_event(msg("ack/lab", "late"));
    assert_eq!(sel(&a), "other", "expired: no jump");
    assert!(a.await_reply.is_none(), "cleared silently");
    assert_eq!(a.note_at(Instant::now()), None);

    // fires once
    let mut a = reply_app();
    a.handle_key(ctrl_key('s'));
    a.handle_key(key(KeyCode::Esc));
    a.apply_core_event(msg("ack/lab", "1"));
    a.selected = a.tree.find("other");
    a.apply_core_event(msg("ack/lab", "2"));
    assert_eq!(sel(&a), "other");
}

#[test]
fn reconnect_clears_await_reply() {
    let mut a = reply_app();
    a.handle_key(ctrl_key('s'));
    assert!(a.await_reply.is_some());
    a.reset_tree();
    assert!(a.await_reply.is_none());
}

#[test]
fn stale_history_sel_is_clamped() {
    let mut a = resend_app(|_| {});
    a.history_sel = Some(99);
    a.handle_action(Action::Publish);
    assert_eq!(
        a.publish.as_ref().unwrap().payload_text(),
        "new",
        "past the end means the newest entry, as the viewer shows"
    );
}

#[test]
fn reply_follow_respects_hidden_rows() {
    let mut a = reply_app();
    a.handle_key(ctrl_key('s'));
    a.handle_key(key(KeyCode::Esc));
    a.starred_only = true;
    a.apply_core_event(msg("ack/lab", "yes"));
    assert_eq!(sel(&a), "other", "hidden target: no jump");
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("reply on ack/lab"))
    );
    assert!(a.await_reply.is_none());
}

#[test]
fn esc_keeps_an_unsent_draft_for_its_topic() {
    let mut a = browse(&["t", "u"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "half");
    a.handle_key(key(KeyCode::Esc));
    a.selected = a.tree.find("u");
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "", "another topic starts empty");
    a.handle_key(key(KeyCode::Esc));
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "half");
    assert!(
        a.status_note
            .as_ref()
            .unwrap()
            .0
            .text
            .contains("draft restored")
    );
}

#[test]
fn a_sent_payload_is_not_a_draft() {
    let mut a = browse(&["t"]);
    publish_one(&mut a, "t", "one");
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "");
}

#[test]
fn walking_sent_payloads_shows_the_position() {
    let mut a = browse(&["t"]);
    publish_one(&mut a, "t", "one");
    publish_one(&mut a, "t", "two");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    let pos = |a: &App| a.publish.as_ref().unwrap().sent_pos;
    a.handle_key(ctrl_key('p'));
    assert_eq!(pos(&a), Some((1, 2)));
    a.handle_key(ctrl_key('p'));
    assert_eq!(pos(&a), Some((2, 2)));
    a.handle_key(ctrl_key('n'));
    a.handle_key(ctrl_key('n'));
    assert_eq!(pos(&a), None, "back on the draft");
    a.handle_key(ctrl_key('p'));
    type_str(&mut a, "!");
    assert_eq!(pos(&a), None, "editing leaves the walk");
}

#[test]
fn ctrl_o_leaves_the_form_open_and_p_comes_back() {
    let mut a = browse(&["cmd", "status"]);
    a.selected = a.tree.find("cmd");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "on");
    a.handle_key(ctrl_key('o'));
    assert_eq!(a.mode, Mode::Browse);
    assert_eq!(a.focus, Focus::Tree);
    assert!(a.publish.is_some(), "still open");
    // Browse keys work again: move to another topic to watch it.
    a.handle_action(Action::Down);
    assert_eq!(sel(&a), "status");
    a.handle_action(Action::Publish);
    assert_eq!(a.mode, Mode::Publish);
    let form = a.publish.as_ref().unwrap();
    assert_eq!(form.topic_text(), "cmd", "the form kept its topic");
    assert_eq!(payload_of(&a), "on");
}

#[test]
fn tab_past_the_last_pane_enters_an_open_form() {
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    a.handle_key(ctrl_key('o'));
    a.handle_action(Action::FocusNext);
    assert_eq!((a.mode, a.focus), (Mode::Browse, Focus::Payload));
    a.history_shown.set(false);
    a.handle_action(Action::FocusNext);
    assert_eq!(a.mode, Mode::Publish);
    // Closed form: tab cycles the panes only.
    a.handle_key(key(KeyCode::Esc));
    a.handle_action(Action::FocusNext);
    a.handle_action(Action::FocusNext);
    assert_eq!((a.mode, a.focus), (Mode::Browse, Focus::Tree));
}

#[test]
fn resend_refuses_control_characters() {
    let mut a = resend_app(|m| m.payload = Bytes::from_static(b"on\x1b]0;t\x07"));
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "");
    assert_eq!(a.publish.as_ref().unwrap().topic_text(), "t");
    let note = &a.status_note.as_ref().unwrap().0.text;
    assert_eq!(note, "can't re-send: control characters");
}

#[test]
fn paste_in_browse_runs_no_actions() {
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    assert!(matches!(a.handle_paste("xyq"), Effect::None));
    assert_eq!(a.mode, Mode::Browse);
    assert!(a.confirm.is_none());
}

#[test]
fn paste_goes_into_the_focused_field() {
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    a.handle_paste("{\r\n\t\"a\": 1\x1b\r\n}");
    assert_eq!(payload_of(&a), "{\n\t\"a\": 1\u{fffd}\n}");
    assert_eq!(a.publish.as_ref().unwrap().field, Field::Payload);
    a.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    a.handle_paste("/x\nignored");
    assert_eq!(a.publish.as_ref().unwrap().topic_text(), "t/x");
}

#[test]
fn paste_with_cr_line_breaks_keeps_the_lines() {
    // VTE terminals and xterm send pasted line breaks as a lone CR.
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    a.handle_paste("{\r  \"a\": 1\r}");
    assert_eq!(payload_of(&a), "{\n  \"a\": 1\n}");
}

#[test]
fn resend_keeps_the_open_form_as_draft() {
    let mut a = resend_app(|_| {});
    a.focus = Focus::Tree;
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "mine");
    a.handle_key(ctrl_key('o'));
    a.focus = Focus::History;
    a.history_sel = Some(0);
    a.handle_action(Action::Publish);
    assert_eq!(payload_of(&a), "old");
    let draft = a.publish_draft.as_ref().expect("old form kept");
    assert_eq!(draft.payload_text(), "mine");
}

#[test]
fn refused_resend_leaves_the_open_form_alone() {
    let mut a = resend_app(|m| m.payload = Bytes::from_static(&[0xff, 0x00]));
    a.focus = Focus::Tree;
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "mine");
    a.handle_key(ctrl_key('o'));
    a.focus = Focus::History;
    a.history_sel = Some(0);
    a.handle_action(Action::Publish);
    assert_eq!(a.mode, Mode::Publish);
    assert_eq!(payload_of(&a), "mine");
    let note = &a.status_note.as_ref().unwrap().0.text;
    assert_eq!(note, "can't re-send: binary payload");
}

#[test]
fn offline_send_stays_in_the_form() {
    let mut a = browse(&["t"]);
    a.conn_state = ConnState::Disconnected {
        reason: "x".into(),
        retry_in_ms: 1000,
    };
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "on");
    assert!(matches!(a.handle_key(ctrl_key('s')), Effect::None));
    let form = a.publish.as_ref().unwrap();
    assert_eq!(form.error.as_deref(), Some("not connected"));
    assert!(a.sent.is_empty(), "not recorded as sent");
}

#[test]
fn ctrl_c_in_a_filled_form_asks_first() {
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    // A blank form quits at once, as everywhere.
    let mut blank = browse(&["t"]);
    blank.handle_action(Action::Publish);
    assert!(matches!(blank.handle_key(ctrl_key('c')), Effect::Quit));
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "x");
    assert!(matches!(a.handle_key(ctrl_key('c')), Effect::None));
    let note = &a.status_note.as_ref().unwrap().0.text;
    assert_eq!(note, "ctrl-c again to quit");
    assert!(matches!(a.handle_key(ctrl_key('c')), Effect::Quit));
    // Any other key in between disarms it.
    let mut b = browse(&["t"]);
    b.handle_action(Action::Publish);
    b.handle_key(key(KeyCode::Tab));
    type_str(&mut b, "x");
    b.handle_key(ctrl_key('c'));
    type_str(&mut b, "y");
    assert!(matches!(b.handle_key(ctrl_key('c')), Effect::None));
}

#[test]
fn qos1_publish_note_waits_for_the_ack() {
    let mut a = browse(&["t"]);
    a.publish_started("t", 1);
    assert_eq!(note_text(&a), "sent t · waiting for ack");
    a.apply_core_event(CoreEvent::PublishSent { pkid: 7 });
    a.apply_core_event(CoreEvent::PublishAcked {
        pkid: 7,
        failure: None,
    });
    // The ack names no topic: packet ids cannot be tied to a send reliably
    // (clear-retained shares the client, sends race in their own tasks).
    assert_eq!(note_text(&a), "published · acked by the broker");
    // Refused (MQTT 5): an error that names the topic.
    a.publish_started("t", 2);
    a.apply_core_event(CoreEvent::PublishSent { pkid: 8 });
    a.apply_core_event(CoreEvent::PublishAcked {
        pkid: 8,
        failure: Some("not authorized".into()),
    });
    let n = a.note_at(Instant::now()).unwrap();
    assert!(
        n.error && n.text == "publish refused: not authorized",
        "{n:?}"
    );
}

#[test]
fn qos0_publish_note_is_left_to_the_send() {
    let mut a = browse(&["t"]);
    a.publish_started("t", 0);
    assert_eq!(a.note_at(Instant::now()), None);
}

#[test]
fn reply_timeout_is_announced() {
    let mut a = reply_app();
    a.handle_key(ctrl_key('s'));
    a.handle_key(key(KeyCode::Esc));
    let (t, _) = a.await_reply.take().unwrap();
    a.await_reply = Some((t, Instant::now() - REPLY_WINDOW - Duration::from_secs(1)));
    a.on_tick();
    assert!(a.await_reply.is_none());
    assert_eq!(note_text(&a), "no reply on ack/lab within 30s");
}

#[test]
fn ctrl_p_with_nothing_sent_says_so() {
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    a.handle_key(ctrl_key('p'));
    assert_eq!(note_text(&a), "nothing sent to t yet");
}

#[test]
fn quitting_beside_a_filled_docked_form_asks_first() {
    let mut a = browse(&["t"]);
    a.selected = a.tree.find("t");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    type_str(&mut a, "x");
    a.handle_key(ctrl_key('o'));
    assert_eq!(a.mode, Mode::Browse, "left the form, it stays docked");
    assert!(matches!(a.handle_key(ctrl_key('c')), Effect::None));
    assert_eq!(note_text(&a), "ctrl-c again to quit");
    assert!(matches!(a.handle_key(ctrl_key('c')), Effect::Quit));
    // The quit key asks the same way.
    a.armed = None;
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('q'))),
        Effect::None
    ));
    assert_eq!(note_text(&a), "q again to quit");
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('q'))),
        Effect::Quit
    ));
}

#[test]
fn paste_after_walking_sent_payloads_drops_the_position() {
    let mut a = browse(&["t"]);
    publish_one(&mut a, "t", "one");
    a.handle_action(Action::Publish);
    a.handle_key(key(KeyCode::Tab));
    a.handle_key(ctrl_key('p'));
    assert!(a.publish.as_ref().unwrap().sent_pos.is_some());
    a.handle_paste("x");
    assert_eq!(
        a.publish.as_ref().unwrap().sent_pos,
        None,
        "no longer a sent payload"
    );
}
