//! Headless end-to-end run: the real `Session` and `App` against a throwaway
//! Mosquitto, driven through the same sequence as `event_loop` but without a
//! terminal or input stream.

#[path = "../../../mqtt-zapp-core/tests/support/mod.rs"]
#[allow(dead_code)]
mod support;

use super::*;
use crate::tui::test_env::ConfigDir;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::config::{PasswordSource, Profile, Protocol, TlsConfig};
use ratatui::backend::TestBackend;
use std::time::Duration;

fn profile(port: u16, subs: &[&str]) -> Profile {
    Profile {
        name: "adhoc".into(),
        host: "127.0.0.1".into(),
        port,
        protocol: Protocol::Mqtt,
        mqtt_version: Default::default(),
        ws_path: "/".into(),
        client_id: None,
        username: None,
        password: PasswordSource::None,
        tls: TlsConfig::default(),
        subscriptions: subs.iter().map(|s| (*s).to_owned()).collect(),
    }
}

/// Feed events, notes and ticks to `app` until `pred` holds; panics after `secs`.
async fn pump_until(
    app: &mut App,
    session: &mut Session,
    notes: &mut UnboundedReceiver<Note>,
    pred: impl Fn(&App) -> bool,
    secs: u64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut tick = tokio::time::interval(TICK);
    while !pred(app) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out after {secs}s waiting for the app state"
        );
        tokio::select! {
            ev = recv(&mut session.rx), if session.rx.is_some() => match ev {
                Some(ev) => app.apply_core_event(ev),
                None => session.rx = None,
            },
            Some(note) = notes.recv() => app.set_background_note(note, Instant::now()),
            _ = tick.tick() => app.on_tick(),
        }
    }
}

fn key(app: &mut App, session: &mut Session, code: KeyCode) {
    let effect = app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    assert!(!session.apply(effect, app), "unexpected quit");
}

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|f| ui::render(f, app)).unwrap();
    let buf = terminal.backend().buffer();
    let w = buf.area.width as usize;
    buf.content()
        .chunks(w)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn wait_connected(rx: &mut Receiver<CoreEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(ev) = rx.recv().await {
            if matches!(ev, CoreEvent::State(ConnState::Connected)) {
                return;
            }
        }
        panic!("connection ended before connecting");
    })
    .await
    .expect("side connection did not connect");
}

#[tokio::test(flavor = "multi_thread")]
async fn tui_against_mosquitto() {
    let Some(broker) = support::start(false) else {
        return;
    };
    // Config and state go to a tempdir (and the env lock is held) for the
    // whole run, so the developer's real files are never touched.
    let dir = ConfigDir::new("e2e");

    // Side connection seeds two retained topics.
    let (side, mut side_rx) = Connection::connect(&profile(broker.port, &[]), None).unwrap();
    wait_connected(&mut side_rx).await;
    side.publish("zapp/e2e/temp", br#"{"t":21.5}"#.to_vec(), 1, true, None)
        .await
        .unwrap();
    side.publish("zapp/e2e/other", b"x".to_vec(), 1, true, None)
        .await
        .unwrap();

    let mut app = App::new(
        Launch::Connect(profile(broker.port, &["zapp/#"])),
        ProfileStore::default(),
        Settings::default(),
    )
    .unwrap();
    let (mut session, mut notes) = Session::new();
    let first = app.startup_effect();
    session.apply(first, &mut app);

    pump_until(
        &mut app,
        &mut session,
        &mut notes,
        |a| a.tree.find("zapp/e2e/temp").is_some() && a.tree.find("zapp/e2e/other").is_some(),
        10,
    )
    .await;

    // Expand the tree: both topics show before any filter is set.
    for t in ["zapp", "zapp/e2e"] {
        let id = app.tree.find(t).unwrap();
        app.expanded.insert(id);
    }
    app.on_tick();
    let before = screen(&mut app);
    assert!(
        before.contains("temp") && before.contains("other"),
        "{before}"
    );

    // Search for "temp", accept it, and select the matching topic.
    for c in ['/', 't', 'e', 'm', 'p'] {
        key(&mut app, &mut session, KeyCode::Char(c));
    }
    key(&mut app, &mut session, KeyCode::Enter);
    app.selected = app.tree.find("zapp/e2e/temp");
    app.on_tick();
    let shown = screen(&mut app);
    assert!(shown.contains("temp"), "{shown}");
    assert!(shown.contains("21.5"), "{shown}");
    assert!(!shown.contains("other"), "{shown}");

    // Delete the retained message: `x`, then `y` to confirm.
    key(&mut app, &mut session, KeyCode::Char('x'));
    assert_eq!(app.mode, app::Mode::Confirm);
    key(&mut app, &mut session, KeyCode::Char('y'));
    pump_until(
        &mut app,
        &mut session,
        &mut notes,
        |a| {
            a.note_at(Instant::now())
                .is_some_and(|n| n.text.contains("cleared 1 retained"))
        },
        10,
    )
    .await;

    // A fresh subscriber must see no retained message on the topic.
    // `zapp/e2e/other` is the positive control: it must still arrive.
    let (fresh, mut fresh_rx) =
        Connection::connect(&profile(broker.port, &["zapp/e2e/#"]), None).unwrap();
    wait_connected(&mut fresh_rx).await;
    let mut seen = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(ev) = fresh_rx.recv().await {
            if let CoreEvent::Message { topic, .. } = ev {
                seen.push(topic);
            }
        }
    })
    .await;
    assert!(
        seen.iter().any(|t| t == "zapp/e2e/other"),
        "positive control missing: {seen:?}"
    );
    assert!(
        !seen.iter().any(|t| t == "zapp/e2e/temp"),
        "retained message survived: {seen:?}"
    );

    fresh.close(Duration::from_secs(1)).await;
    side.close(Duration::from_secs(1)).await;
    session.close().await;

    // The redirect worked: the state file went to the tempdir.
    assert!(
        dir.0.join("state.toml").exists(),
        "state.toml not redirected"
    );
}
