use super::*;

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
fn on_tick_marks_dirty_while_flashing() {
    let mut a = browse(&["a"]);
    a.dirty = false;
    a.on_tick();
    assert!(a.dirty, "fresh message is still flashing");
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
        Some(&Note::info("display frozen · f to resume"))
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
fn copy_refuses_huge_text() {
    let mut a = browse(&[]);
    a.apply_core_event(msg("x/y", &"a".repeat(COPY_MAX + 1)));
    a.selected = a.tree.find("x/y");
    assert!(matches!(a.handle_action(Action::CopyPayload), Effect::None));
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::error("too large to copy (101 KiB), use w to save"))
    );
    // Exactly at the cap still copies.
    a.apply_core_event(msg("x/z", &"a".repeat(COPY_MAX)));
    a.selected = a.tree.find("x/z");
    assert!(matches!(
        a.handle_action(Action::CopyPayload),
        Effect::Copy { .. }
    ));
}

#[test]
fn history_g_and_shift_g_jump_to_newest_and_oldest() {
    let mut a = history_app(10, &["1", "2", "3"]);
    a.focus = Focus::History;
    a.handle_action(Action::Bottom);
    assert_eq!(a.history_sel, Some(0), "oldest");
    assert_eq!(sel(&a), "t");
    a.handle_action(Action::Top);
    assert_eq!(a.history_sel, None, "live");
    assert_eq!(sel(&a), "t");
}

#[test]
fn tree_keys_do_nothing_in_history() {
    let mut a = browse(&["a/b", "c"]);
    a.selected = a.tree.find("a/b");
    a.expanded.insert(a.tree.find("a").unwrap());
    a.focus = Focus::History;
    let expanded = a.expanded.as_set().clone();
    for x in [
        Action::Collapse,
        Action::Expand,
        Action::ToggleExpand,
        Action::Select,
    ] {
        a.handle_action(x);
        assert_eq!(sel(&a), "a/b", "{x:?}");
        assert_eq!(a.expanded.as_set(), &expanded, "{x:?}");
    }
}

#[test]
fn freeze_note_uses_the_remapped_key() {
    let mut s = Settings::default();
    s.keys.insert("freeze".into(), "F".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    a.handle_key(key(KeyCode::Char('F')));
    let note = &a.status_note.as_ref().unwrap().0.text;
    assert!(note.contains("F to resume"), "{note}");
}

#[test]
fn remapped_help_key_closes_help() {
    let mut s = Settings::default();
    s.keys.insert("help".into(), "!".into());
    let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
    a.mode = Mode::Browse;
    a.handle_key(key(KeyCode::Char('!')));
    assert_eq!(a.mode, Mode::Help);
    a.handle_key(key(KeyCode::Char('!')));
    assert_eq!(a.mode, Mode::Browse);
}
