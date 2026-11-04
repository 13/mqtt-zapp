use super::*;
use crate::cli::Launch;
use crate::tui::app::{App, Effect, Mode, Note, ServerInfo};
use crate::tui::keymap::Action;
use mqtt_zapp_core::client::CoreEvent;
use mqtt_zapp_core::config::ThemeName;
use mqtt_zapp_core::config::{ProfileStore, Settings};
use mqtt_zapp_core::{Bytes, Message};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::time::SystemTime;

mod bars;
mod mouse;
mod overlays;
mod tree;

fn app_with(items: &[(&str, &[u8])]) -> App {
    let mut a = App::new(Launch::Picker, ProfileStore::default(), Settings::default()).unwrap();
    a.mode = Mode::Browse;
    for (topic, payload) in items {
        a.apply_core_event(CoreEvent::Message {
            topic: (*topic).into(),
            msg: Message {
                payload: Bytes::copy_from_slice(payload),
                qos: 0,
                retain: false,
                received_at: SystemTime::now(),
                truncated_from: None,
                sparkplug: false,
                wire_len: None,
                props: None,
            },
        });
    }
    a
}

fn draw(a: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render(f, a)).unwrap();
    t.backend().buffer().clone()
}

fn text(b: &Buffer) -> String {
    let w = b.area.width as usize;
    b.content
        .chunks(w)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn mouse(a: &mut App, kind: crossterm::event::MouseEventKind, x: u16, y: u16) -> Effect {
    a.handle_mouse(crossterm::event::MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    })
}

fn click(a: &mut App, x: u16, y: u16) -> Effect {
    use crossterm::event::{MouseButton, MouseEventKind};
    mouse(a, MouseEventKind::Down(MouseButton::Left), x, y)
}

/// Screen row of the first line containing `needle` in the tree pane.
fn tree_row(b: &Buffer, needle: &str) -> u16 {
    text(b)
        .lines()
        .position(|l| l.chars().take(40).collect::<String>().contains(needle))
        .unwrap_or_else(|| panic!("{needle}: {}", text(b))) as u16
}

/// Screen position of the first occurrence of `needle`.
fn find_at(b: &Buffer, needle: &str) -> (u16, u16) {
    let t = text(b);
    for (y, l) in t.lines().enumerate() {
        if let Some(i) = l.find(needle) {
            return (l[..i].chars().count() as u16, y as u16);
        }
    }
    panic!("{needle} not on screen: {t}");
}

/// Screen positions of every occurrence of `needle`, top to bottom.
fn find_all(b: &Buffer, needle: &str) -> Vec<(u16, u16)> {
    let t = text(b);
    let mut v = vec![];
    for (y, l) in t.lines().enumerate() {
        for (i, _) in l.match_indices(needle) {
            v.push((l[..i].chars().count() as u16, y as u16));
        }
    }
    v
}

/// The payload pane's `⧉ copy` (the header's comes first).
fn payload_copy_at(b: &Buffer) -> (u16, u16) {
    *find_all(b, "⧉ copy").last().expect("payload copy button")
}

fn copied(e: Effect) -> (&'static str, String) {
    match e {
        Effect::Copy { what, text } => (what, text),
        e => panic!("expected a copy, got {e:?}"),
    }
}

fn help_app(query: &str) -> App {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut a = app_with(&[("a", b"1")]);
    a.handle_action(Action::Help);
    for c in query.chars() {
        a.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    a
}

/// Style of the first cell of `needle` on the first line that has `row`.
fn style_on_row(b: &Buffer, row: &str, needle: &str) -> ratatui::style::Style {
    let w = b.area.width as usize;
    for y in 0..b.area.height as usize {
        let line: String = (0..w).map(|x| b[(x as u16, y as u16)].symbol()).collect();
        if line.contains(row) {
            let x = line.find(needle).expect(needle);
            return b[(line[..x].chars().count() as u16, y as u16)].style();
        }
    }
    panic!("no row {row}");
}

fn two_msgs() -> App {
    let mut a = app_with(&[("dev/x", b"p1"), ("dev/x", b"p2")]);
    a.selected = a.tree.find("dev/x");
    a
}

fn picker_app(profiles: Vec<mqtt_zapp_core::config::Profile>) -> App {
    let store = ProfileStore { profiles };
    App::new(Launch::Picker, store, Settings::default()).unwrap()
}

fn profile(
    name: &str,
    pw: mqtt_zapp_core::config::PasswordSource,
) -> mqtt_zapp_core::config::Profile {
    use mqtt_zapp_core::config::{Protocol, TlsConfig};
    mqtt_zapp_core::config::Profile {
        name: name.into(),
        host: "broker.lan".into(),
        port: 8883,
        protocol: Protocol::Mqtts,
        mqtt_version: Default::default(),
        ws_path: "/".into(),
        client_id: None,
        username: Some("alice".into()),
        password: pw,
        tls: TlsConfig::default(),
        subscriptions: vec!["#".into()],
        retained: true,
        keepalive_s: None,
        clean_session: None,
        session_expiry_s: None,
    }
}

fn first_line(b: &Buffer) -> String {
    text(b).lines().next().unwrap().trim_end().to_owned()
}

fn home() -> ServerInfo {
    ServerInfo {
        name: "home".into(),
        url: "mqtts://alice@broker.lan:8883".into(),
        subscriptions: vec!["#".into(), "$SYS/#".into()],
        warning: None,
    }
}

fn retained(a: &mut App, topic: &str, payload: &[u8]) {
    a.apply_core_event(CoreEvent::Message {
        topic: topic.into(),
        msg: Message {
            payload: Bytes::copy_from_slice(payload),
            qos: 0,
            retain: true,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        },
    });
}

fn themed(theme: ThemeName) -> (App, Buffer) {
    let mut a = app_with(&[("a", b"1"), ("b", b"2")]);
    a.theme = theme;
    a.handle_action(Action::Down);
    a.history_sel = None;
    let b = draw(&mut a, 100, 24);
    (a, b)
}

#[test]
fn payload_focus_ignores_tree_keys() {
    let mut a = app_with(&[("a/b", b"1"), ("c", b"2")]);
    a.handle_action(Action::Down);
    let sel = a.selected;
    a.focus = Focus::Payload;
    for x in [
        Action::Expand,
        Action::ToggleExpand,
        Action::Select,
        Action::Collapse,
    ] {
        a.handle_action(x);
        assert!(a.expanded.is_empty(), "{x:?}");
        assert_eq!(a.selected, sel, "{x:?}");
    }
}

fn last_line(b: &Buffer) -> String {
    text(b).lines().last().unwrap().trim_end().to_owned()
}

/// The screen row of the highlighted history entry (focused selection bg).
fn history_highlight_y(b: &Buffer) -> Option<u16> {
    let area = b.area;
    (0..area.height).rev().find(|&y| {
        let l: String = (0..area.width).map(|x| b[(x, y)].symbol()).collect();
        l.contains("  p") && (0..area.width).any(|x| b[(x, y)].bg == theme::SEL_BG)
    })
}

/// Click the payload row containing `needle` (the last one on screen).
fn click_payload_row(a: &mut App, b: &Buffer, needle: &str) -> Effect {
    let l: Vec<String> = text(b).lines().map(str::to_owned).collect();
    let y = l
        .iter()
        .rposition(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("{needle}"));
    click(a, 70, y as u16)
}

/// The confirm card alone on a `w`×`h` screen; its inner rows, trimmed.
fn confirm_rows(prompt: &str, w: u16, h: u16) -> Vec<String> {
    use crate::tui::app::{Confirm, ConfirmAction};
    let c = Confirm {
        prompt: prompt.to_owned(),
        action: ConfirmAction::DeleteTopic("t".into()),
    };
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| render_confirm(f, f.area(), &c)).unwrap();
    text(t.backend().buffer())
        .lines()
        .filter_map(|r| {
            let (a, b) = (r.find('┃')?, r.rfind('┃')?);
            (b > a).then(|| r[a + '┃'.len_utf8()..b].trim().to_owned())
        })
        .collect()
}
