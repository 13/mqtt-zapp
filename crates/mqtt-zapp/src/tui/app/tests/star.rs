use super::*;
use crate::tui::test_env::ConfigDir;

fn star_app(profile_name: &str) -> App {
    let mut a = browse(&["a/b/c", "a/b/d", "a/x", "z/y"]);
    a.session_profile = Some(profile(profile_name, PasswordSource::None));
    a
}

fn select_topic(a: &mut App, topic: &str) {
    let id = a.tree.find(topic).unwrap();
    a.select(id);
}

/// Press a key and perform a `SaveState` effect the way the event loop does.
fn star_press(a: &mut App, c: char) {
    if let Effect::SaveState(s) = a.handle_key(key(KeyCode::Char(c))) {
        s.save().unwrap();
    }
}

fn state_file() -> String {
    std::fs::read_to_string(mqtt_zapp_core::state::state_dir().join("state.toml"))
        .unwrap_or_default()
}

#[test]
fn star_toggles_and_persists() {
    let cfg = ConfigDir::new("star-persist");
    let mut a = star_app("p1");
    select_topic(&mut a, "a/b/c");
    star_press(&mut a, '*');
    assert!(state_file().contains("a/b/c"), "{}", state_file());
    assert_eq!(a.note_at(Instant::now()).unwrap().text, "starred a/b/c");
    star_press(&mut a, '*');
    assert!(!state_file().contains("a/b/c"), "{}", state_file());
    assert_eq!(a.note_at(Instant::now()).unwrap().text, "unstarred a/b/c");
    assert!(cfg.0.join("state.toml").exists(), "wrote under test dir");
}

#[test]
fn starred_only_shows_ancestors() {
    let _cfg = ConfigDir::new("star-ancestors");
    let mut a = star_app("p1");
    select_topic(&mut a, "a/b/d");
    star_press(&mut a, '*');
    star_press(&mut a, '\'');
    assert_eq!(row_names(&a), ["a", "a/b", "a/b/d"]);
    // Toggling again restores the normal view.
    star_press(&mut a, '\'');
    assert_eq!(row_names(&a), ["a", "z"]);
}

#[test]
fn starred_only_with_search_intersects() {
    let _cfg = ConfigDir::new("star-search");
    let mut a = star_app("p1");
    select_topic(&mut a, "a/b/d");
    star_press(&mut a, '*');
    select_topic(&mut a, "z/y");
    star_press(&mut a, '*');
    star_press(&mut a, '\'');
    assert_eq!(row_names(&a), ["a", "a/b", "a/b/d", "z", "z/y"]);
    a.search = "y".into();
    a.rebuild_filter();
    assert_eq!(row_names(&a), ["z", "z/y"]);
}

#[test]
fn starred_only_empty_notes() {
    let _cfg = ConfigDir::new("star-empty");
    let mut a = star_app("p1");
    star_press(&mut a, '\'');
    assert_eq!(a.note_at(Instant::now()).unwrap().text, "no starred topics");
    assert_eq!(row_names(&a), ["a", "z"]);
    assert!(!a.starred_only);
}

#[test]
fn favorites_are_per_profile() {
    let _cfg = ConfigDir::new("star-profiles");
    let mut a = star_app("p1");
    select_topic(&mut a, "a/x");
    star_press(&mut a, '*');
    a.session_profile = Some(profile("p2", PasswordSource::None));
    star_press(&mut a, '\'');
    assert_eq!(a.note_at(Instant::now()).unwrap().text, "no starred topics");
    let st = a.app_state();
    assert!(st.favorites["p1"].contains("a/x"));
    assert!(!st.favorites.contains_key("p2"));
}
