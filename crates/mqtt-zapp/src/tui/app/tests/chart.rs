use super::*;

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

#[test]
fn chart_cycle_when_all_pinned_shows_first() {
    let mut a = chart_app(r#"{"a":1,"b":2}"#, 3);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(Some("a".into())));
}

#[test]
fn esc_closes_chart() {
    let mut a = chart_app(&fifty_fields(), 2);
    a.handle_action(Action::Chart);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, Some(Some("f1".into())));
    a.handle_action(Action::Cancel);
    assert_eq!(a.chart, None);
    assert_eq!(note_text(&a), "chart off");
}

#[test]
fn esc_closes_chart_before_clearing_search() {
    let mut a = chart_app(r#"{"a":1}"#, 2);
    filter_on(&mut a, "t");
    a.handle_action(Action::Chart);
    a.handle_action(Action::Cancel);
    assert_eq!(a.chart, None);
    assert!(a.filter.is_some(), "first esc keeps the filter");
    a.handle_action(Action::Cancel);
    assert!(a.filter.is_none(), "second esc clears it");
}

#[test]
fn esc_clears_chart_pins() {
    let mut a = chart_app(r#"{"a":1,"b":2}"#, 3);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Chart);
    a.handle_action(Action::Cancel);
    assert_eq!(a.chart, None);
    assert!(a.chart_pinned.is_empty());
}

#[test]
fn esc_without_chart_still_clears_search() {
    let mut a = chart_app(r#"{"a":1}"#, 1);
    filter_on(&mut a, "t");
    a.handle_action(Action::Cancel);
    assert!(a.filter.is_none());
}

#[test]
fn esc_with_chart_cycled_off_clears_search_not_pins() {
    let mut a = chart_app(r#"{"a":1,"b":2}"#, 3);
    filter_on(&mut a, "t");
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Chart);
    a.handle_action(Action::Chart);
    assert_eq!(a.chart, None);
    a.handle_action(Action::Cancel);
    assert!(a.filter.is_none(), "esc goes to the search");
    assert_eq!(a.chart_pinned, vec![Some("a".to_owned())]);
}

#[test]
fn esc_note_says_pins_cleared() {
    let mut a = chart_app(&fifty_fields(), 2);
    a.handle_action(Action::Chart);
    a.handle_action(Action::Chart);
    a.handle_action(Action::ChartPin);
    a.handle_action(Action::Cancel);
    assert_eq!(note_text(&a), "chart off, pins cleared");
}
