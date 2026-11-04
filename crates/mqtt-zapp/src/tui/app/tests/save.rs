use super::*;

fn save_app(dir: &std::path::Path) -> App {
    let mut a = browse(&[]);
    a.save_dir = dir.to_path_buf();
    a
}

fn files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

#[test]
fn w_saves_shown_payload() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = save_app(dir.path());
    a.apply_core_event(msg("x/y", "old"));
    a.apply_core_event(msg("x/y", r#"{"n":1}"#));
    a.selected = a.tree.find("x/y");
    a.history_sel = Some(0);
    assert!(matches!(
        a.handle_key(key(KeyCode::Char('w'))),
        Effect::None
    ));
    let f = files(dir.path());
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].extension().unwrap(), "txt");
    assert_eq!(std::fs::read(&f[0]).unwrap(), b"old");
    let note = a.note_at(Instant::now()).unwrap();
    assert!(note.text.starts_with("saved "), "{}", note.text);
    // The live (newest) payload is JSON.
    a.history_sel = None;
    a.handle_action(Action::SavePayload);
    let f = files(dir.path());
    assert_eq!(f.len(), 2);
    assert!(
        f.iter()
            .any(|p| p.extension().unwrap() == "json" && std::fs::read(p).unwrap() == br#"{"n":1}"#)
    );
}

#[test]
fn shift_w_saves_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = save_app(dir.path());
    a.apply_core_event(msg("x/y", "first"));
    a.apply_core_event(msg("x/y", "second"));
    a.selected = a.tree.find("x/y");
    a.handle_key(key(KeyCode::Char('W')));
    let f = files(dir.path());
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].extension().unwrap(), "jsonl");
    let text = std::fs::read_to_string(&f[0]).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains(r#""payload":"first""#), "{text}");
    assert!(lines[1].contains(r#""payload":"second""#), "{text}");
    assert!(
        a.note_at(Instant::now())
            .unwrap()
            .text
            .starts_with("saved ")
    );
}

#[test]
fn nothing_to_save_notes() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = save_app(dir.path());
    a.handle_action(Action::SavePayload);
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("nothing to save"))
    );
    a.handle_action(Action::SaveHistory);
    assert_eq!(
        a.note_at(Instant::now()),
        Some(&Note::info("nothing to save"))
    );
    assert!(files(dir.path()).is_empty());
}

#[test]
fn save_failure_is_noted() {
    let dir = tempfile::tempdir().unwrap();
    let mut a = save_app(&dir.path().join("missing"));
    a.apply_core_event(msg("x/y", "v"));
    a.selected = a.tree.find("x/y");
    a.handle_action(Action::SavePayload);
    let note = a.note_at(Instant::now()).unwrap();
    assert!(note.text.starts_with("save failed: "), "{}", note.text);
}
