use super::*;

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
    assert_eq!(
        parent
            .children
            .iter()
            .map(|(n, _)| &**n)
            .collect::<Vec<_>>(),
        ["x"]
    );
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
