//! The terminal UI: terminal setup/restore and the async event loop. State and
//! drawing live in `app` and `ui`; this module only wires them to the
//! terminal, the broker connection and a redraw tick.

mod app;
mod keymap;
mod message_view;
mod picker;
mod publish;
mod rows;
mod theme;
mod ui;

use crate::cli::Launch;
use anyhow::{Context, Result};
use app::{App, Effect, Note};
use crossterm::cursor::Show;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use mqtt_zapp_core::client::{ConnState, Connection, CoreEvent};
use mqtt_zapp_core::config::{ProfileStore, Settings};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{Stdout, stdout};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver, UnboundedSender, unbounded_channel};

const TICK: Duration = Duration::from_millis(100);
/// Most core events applied per loop iteration, so input stays responsive
/// during a retained-message burst.
const MAX_DRAIN: usize = 2000;

pub async fn run(launch: Launch, store: ProfileStore, store_error: Option<String>) -> Result<()> {
    let mut app = App::new(launch, store, Settings::load()?)?;
    app.store_error = store_error;
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    let (mut session, mut notes) = Session::new();
    let first = app.startup_effect();
    session.apply(first, &mut app);
    let result = event_loop(&mut terminal, &mut app, &mut session, &mut notes).await;
    session.disconnect();
    result
}

/// The live broker connection, if any, and its event receiver.
struct Session {
    conn: Option<Connection>,
    rx: Option<Receiver<CoreEvent>>,
    /// Publish outcomes, sent by the spawned publish tasks.
    notes_tx: UnboundedSender<Note>,
}

impl Session {
    fn new() -> (Session, UnboundedReceiver<Note>) {
        let (notes_tx, notes_rx) = unbounded_channel();
        let session = Session {
            conn: None,
            rx: None,
            notes_tx,
        };
        (session, notes_rx)
    }
}

impl Session {
    /// Carry out an effect. Returns true when the app should quit.
    fn apply(&mut self, effect: Effect, app: &mut App) -> bool {
        match effect {
            Effect::None => {}
            Effect::Quit => return true,
            Effect::Connect(profile, password) => {
                self.disconnect();
                app.reset_tree();
                match Connection::connect(&profile, password) {
                    Ok((conn, rx)) => {
                        self.conn = Some(conn);
                        self.rx = Some(rx);
                        app.conn_state = ConnState::Connecting;
                    }
                    // A bad profile (e.g. unreadable CA file) is shown, not fatal.
                    Err(e) => {
                        app.conn_state = ConnState::Disconnected {
                            reason: format!("{e:#}"),
                            retry_in_ms: 0,
                        };
                    }
                }
            }
            Effect::Publish {
                topic,
                payload,
                qos,
                retain,
            } => {
                // Never await on the UI loop: publish can wait up to 5 s.
                match self.conn.clone() {
                    Some(conn) => {
                        let notes = self.notes_tx.clone();
                        tokio::spawn(async move {
                            let note = match conn.publish(&topic, payload, qos, retain).await {
                                Ok(()) => Note::info(format!("published {topic}")),
                                Err(e) => Note::error(format!("publish failed: {e:#}")),
                            };
                            let _ = notes.send(note);
                        });
                    }
                    None => {
                        app.set_note(Note::error("publish failed: not connected"), Instant::now())
                    }
                }
            }
        }
        false
    }

    fn disconnect(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.disconnect();
        }
        self.rx = None;
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    session: &mut Session,
    notes: &mut UnboundedReceiver<Note>,
) -> Result<()> {
    let mut input = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(Event::Key(key))) => {
                    let effect = app.handle_key(key);
                    if session.apply(effect, app) {
                        return Ok(());
                    }
                }
                Some(Ok(Event::Mouse(m))) => {
                    let effect = app.handle_mouse(m);
                    if session.apply(effect, app) {
                        return Ok(());
                    }
                }
                Some(Ok(Event::Resize(..))) => app.dirty = true,
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e).context("reading terminal input"),
                None => return Ok(()),
            },
            // Skipped while there is no connection, so an empty slot never blocks.
            ev = recv(&mut session.rx), if session.rx.is_some() => match ev {
                Some(ev) => {
                    app.apply_core_event(ev);
                    if let Some(rx) = session.rx.as_mut() {
                        for _ in 0..MAX_DRAIN {
                            let Ok(ev) = rx.try_recv() else { break };
                            app.apply_core_event(ev);
                        }
                    }
                }
                // The connection task ended.
                None => session.rx = None,
            },
            Some(note) = notes.recv() => app.set_note(note, Instant::now()),
            _ = tick.tick() => {
                app.on_tick();
                if app.dirty {
                    terminal.draw(|f| ui::render(f, app))?;
                    app.dirty = false;
                }
            }
        }
    }
}

async fn recv(rx: &mut Option<Receiver<CoreEvent>>) -> Option<CoreEvent> {
    match rx {
        Some(rx) => rx.recv().await,
        None => None,
    }
}

/// Raw mode + alternate screen for as long as it lives; restores the terminal
/// on drop, including on error returns and panics.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<TerminalGuard> {
        install_panic_hook();
        enable_raw_mode().context("enabling raw mode")?;
        // From here on a failure must still restore, so build the guard first.
        let guard = TerminalGuard;
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)
            .context("entering alternate screen")?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen, Show);
}

/// Restore the terminal before the panic message is printed, so it is readable.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}
