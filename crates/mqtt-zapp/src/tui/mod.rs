//! The terminal UI: terminal setup/restore and the async event loop. State and
//! drawing live in `app` and `ui`; this module only wires them to the
//! terminal, the broker connection and a redraw tick.

mod app;
mod expanded;
mod field_picker;
mod keymap;
mod message_view;
mod picker;
mod profile_form;
mod profiles;
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
use mqtt_zapp_core::payload::base64;
use mqtt_zapp_core::state::AppState;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{Stdout, Write, stdout};
use std::time::{Duration, Instant};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc::{Receiver, UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Most core events applied per loop iteration, so input stays responsive
/// during a retained-message burst.
const MAX_DRAIN: usize = 2000;

pub async fn run(launch: Launch, store: ProfileStore, store_error: Option<String>) -> Result<()> {
    let (settings, settings_err) = Settings::load_or_default();
    let mut app = App::new(launch, store, settings)?;
    app.store_error = store_error;
    let (state, state_err) = AppState::load();
    app.set_app_state(state);
    let note = state_err.map(|e| app.app_state().problem_note(&e));
    app.set_startup_errors(note, settings_err, Instant::now());
    app.theme = theme::effective(app.theme, std::env::var("NO_COLOR").ok().as_deref());
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    let (mut session, mut notes, mut passwords) = Session::new();
    let first = app.startup_effect();
    session.apply(first, &mut app);
    let result = event_loop(
        &mut terminal,
        &mut app,
        &mut session,
        &mut notes,
        &mut passwords,
    )
    .await;
    session.close().await;
    result
}

/// The live broker connection, if any, and its event receiver.
struct Session {
    conn: Option<Connection>,
    rx: Option<Receiver<CoreEvent>>,
    /// Publish outcomes, sent by the spawned publish tasks.
    notes_tx: UnboundedSender<Note>,
    /// Keyring lookup results `(generation, result)` from blocking tasks.
    password_tx: UnboundedSender<PasswordLoaded>,
}

/// `App::password_loaded`'s arguments, sent back by the lookup task.
type PasswordLoaded = (u64, Result<Option<String>, String>);

impl Session {
    fn new() -> (
        Session,
        UnboundedReceiver<Note>,
        UnboundedReceiver<PasswordLoaded>,
    ) {
        let (notes_tx, notes_rx) = unbounded_channel();
        let (password_tx, password_rx) = unbounded_channel();
        let session = Session {
            conn: None,
            rx: None,
            notes_tx,
            password_tx,
        };
        (session, notes_rx, password_rx)
    }
}

impl Session {
    /// Carry out an effect. Returns true when the app should quit.
    fn apply(&mut self, effect: Effect, app: &mut App) -> bool {
        match effect {
            Effect::None => {}
            Effect::Quit => return true,
            // Handled by the event loop, which owns the terminal.
            Effect::EditPayload(_) => {}
            Effect::ClearRetained(topics) => match self.conn.clone() {
                // A clear queued while disconnected must not go out later.
                Some(_) if !app.conn_state.is_connected() => {
                    app.set_note(Note::error("clear failed: not connected"), Instant::now())
                }
                Some(conn) => {
                    let notes = self.notes_tx.clone();
                    // Off the UI loop: each publish may wait for the broker.
                    tokio::spawn(async move {
                        let _ = notes.send(match conn.clear_retained(&topics).await {
                            Ok(n) => Note::info(format!("cleared {n} retained")),
                            Err(e) => Note::error(format!("{e:#}")),
                        });
                    });
                }
                None => app.set_note(Note::error("clear failed: not connected"), Instant::now()),
            },
            Effect::Copy { what, text } => {
                // Defence in depth: producers already cap at COPY_MAX.
                if text.len() > app::COPY_MAX {
                    let note = format!(
                        "too large to copy ({} KiB), use w to save",
                        text.len().div_ceil(1024)
                    );
                    app.set_note(Note::error(note), Instant::now());
                    return false;
                }
                // OSC 52: the terminal (or tmux/ssh in between) sets the clipboard.
                let mut out = stdout();
                let ok = write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))
                    .and_then(|()| out.flush());
                app.set_note(
                    match ok {
                        Ok(()) => Note::info(format!("copied {what}")),
                        Err(e) => Note::error(format!("copy failed: {e}")),
                    },
                    Instant::now(),
                );
            }
            Effect::LoadPassword {
                profile,
                generation,
                load,
            } => {
                // Secret Service can wait on an unlock prompt: never on the
                // UI thread. The result re-enters through `password_loaded`.
                let tx = self.password_tx.clone();
                let name = profile.name;
                tokio::spawn(async move {
                    // A panic in the lookup must still answer, or the
                    // picker would wait forever.
                    let r = match tokio::task::spawn_blocking(move || load(&name)).await {
                        Ok(r) => r.map_err(|e| format!("{e:#}")),
                        Err(_) => Err("keyring lookup failed".to_owned()),
                    };
                    let _ = tx.send((generation, r));
                });
            }
            Effect::SaveProfile(edit) => {
                let name = edit.profile.name.clone();
                let original = edit.original.clone();
                match profiles::persist_edit(edit, &profiles::RealSecrets) {
                    Ok(store) => {
                        let renamed = original.as_deref().is_some_and(|o| o != name);
                        app.profile_edit_saved(store.profiles, original.as_deref(), &name);
                        if renamed {
                            save_state(app);
                        }
                    }
                    Err(e) => app.profile_save_failed(format!("{e:#}")),
                }
            }
            Effect::DeleteProfile(name) => {
                match profiles::persist_delete(&name, &profiles::RealSecrets) {
                    Ok(store) => {
                        app.profile_saved(store.profiles, None, &format!("deleted {name}"));
                        app.profile_deleted(&name);
                        save_state(app);
                    }
                    Err(e) => app.profile_save_failed(format!("{e:#}")),
                }
            }
            Effect::SaveOrder(names) => {
                let selected = app.picker.selected().map(|p| p.name.clone());
                match profiles::persist_order(&names) {
                    Ok(store) => {
                        app.profile_saved(store.profiles, selected.as_deref(), "order saved");
                        // J/K may have switched the sort back to custom.
                        save_state(app);
                    }
                    Err(e) => app.profile_save_failed(format!("{e:#}")),
                }
            }
            Effect::SaveState(state) => {
                if let Err(e) = state.save() {
                    app.state_save_failed(&format!("{e:#}"));
                }
            }
            Effect::Connect(profile, password) => {
                // connect_effect stamped the last-used time; best effort.
                save_state(app);
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
                props,
            } => {
                // Never await on the UI loop: publish can wait up to 5 s.
                match self.conn.clone() {
                    Some(_) if !app.conn_state.is_connected() => {
                        app.set_note(Note::error("publish failed: not connected"), Instant::now())
                    }
                    Some(conn) => {
                        let notes = self.notes_tx.clone();
                        tokio::spawn(async move {
                            let note = match conn
                                .publish(&topic, payload, qos, retain, props.as_ref())
                                .await
                            {
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

    /// Quit: send DISCONNECT and wait (briefly) until it has been written.
    async fn close(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.close(Duration::from_secs(2)).await;
        }
        self.rx = None;
    }

    fn disconnect(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.disconnect();
        }
        self.rx = None;
    }
}

/// Write `state.toml` from the app; a failure is only a note.
fn save_state(app: &mut App) {
    if let Err(e) = app.app_state().save() {
        app.state_save_failed(&format!("{e:#}"));
    }
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    session: &mut Session,
    notes: &mut UnboundedReceiver<Note>,
    passwords: &mut UnboundedReceiver<PasswordLoaded>,
) -> Result<()> {
    let mut input = EventStream::new();
    // The next tick is due `tick_interval` after the last one; recomputed
    // every iteration, so an event that starts an animation shortens the wait.
    let mut last_tick = Instant::now();
    // Set by every non-tick event: a redraw it asks for is due within one
    // fast tick, even when the idle tick is much slower.
    let mut boost: Option<Instant> = None;
    // These end the loop normally, so the terminal guard restores it. Raw
    // mode normally turns ^C/^\ into keys, but a `kill -INT` or a signal
    // from another process still arrives as a signal.
    let mut sigint = signal(SignalKind::interrupt()).context("installing SIGINT handler")?;
    let mut sigquit = signal(SignalKind::quit()).context("installing SIGQUIT handler")?;
    let mut sigterm = signal(SignalKind::terminate()).context("installing SIGTERM handler")?;
    let mut sighup = signal(SignalKind::hangup()).context("installing SIGHUP handler")?;
    loop {
        let wake = tokio::time::Instant::from_std(next_deadline(
            last_tick,
            app.tick_interval(Instant::now()),
            boost,
        ));
        tokio::select! {
            _ = sigint.recv() => return Ok(()),
            _ = sigquit.recv() => return Ok(()),
            _ = sigterm.recv() => return Ok(()),
            _ = sighup.recv() => return Ok(()),
            ev = input.next() => {
                match ev {
                Some(Ok(Event::Key(key))) => {
                    let effect = app.handle_key(key);
                    if let Effect::EditPayload(text) = effect {
                        // A fresh input stream afterwards: the old reader
                        // must not compete with the editor for stdin.
                        input = EventStream::new();
                        run_editor(terminal, app, &text)?;
                        continue;
                    }
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
                Some(Ok(Event::Resize(..))) => app.on_resize(),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e).context("reading terminal input"),
                None => return Ok(()),
                }
                // Input is drawn at once; only MQTT and notes coalesce.
                draw_if_dirty(terminal, app)?;
            }
            // Skipped while there is no connection, so an empty slot never blocks.
            ev = recv(&mut session.rx), if session.rx.is_some() => match ev {
                Some(ev) => {
                    boost = coalesce(boost, Instant::now());
                    app.apply_core_event(ev);
                    if let Some(rx) = session.rx.as_mut() {
                        for _ in 0..MAX_DRAIN {
                            let Ok(ev) = rx.try_recv() else { break };
                            app.apply_core_event(ev);
                        }
                    }
                }
                // The connection task ended.
                None => {
                    session.rx = None;
                    app.on_connection_ended();
                }
            },
            Some((generation, result)) = passwords.recv() => {
                let effect = app.password_loaded(generation, result);
                if session.apply(effect, app) {
                    return Ok(());
                }
                draw_if_dirty(terminal, app)?;
            }
            Some(note) = notes.recv() => {
                boost = coalesce(boost, Instant::now());
                app.set_background_note(note, Instant::now());
            }
            _ = tokio::time::sleep_until(wake) => {
                last_tick = Instant::now();
                boost = None;
                app.on_tick();
                draw_if_dirty(terminal, app)?;
            }
        }
    }
}

/// Note that a coalesced (MQTT or note) event arrived: its redraw is due
/// within one fast tick. An earlier pending deadline is kept, so a steady
/// stream of events never pushes the redraw later.
fn coalesce(boost: Option<Instant>, now: Instant) -> Option<Instant> {
    boost.or(Some(now + app::FAST_TICK))
}

/// When the loop must wake for the next tick.
fn next_deadline(last_tick: Instant, interval: Duration, boost: Option<Instant>) -> Instant {
    let due = last_tick + interval;
    boost.map_or(due, |b| due.min(b))
}

/// Draw now if something asked for a redraw.
fn draw_if_dirty<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    if app.dirty {
        terminal.draw(|f| ui::render(f, app))?;
        app.dirty = false;
    }
    Ok(())
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

/// Leave the TUI, edit `text` in the user's editor, and come back; the
/// result (or an error note) goes to the publish form.
fn run_editor(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    text: &str,
) -> Result<()> {
    let cmd = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| "vi".into());
    restore_terminal();
    let result = edit_text(text, &cmd);
    enable_raw_mode().context("enabling raw mode")?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)
        .context("entering alternate screen")?;
    terminal.clear()?;
    match result {
        Ok(new) => app.set_publish_payload(&new),
        Err(e) => app.set_note(Note::error(format!("editor failed: {e:#}")), Instant::now()),
    }
    app.dirty = true;
    Ok(())
}

/// A new file only the user can read (0600, never an existing one), in
/// `$XDG_RUNTIME_DIR` when set, holding `initial`.
fn private_temp_file(initial: &str) -> Result<std::path::PathBuf> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    for attempt in 0..16u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let path = dir.join(format!(
            "mqtt-zapp-{}-{nanos:x}{attempt}.txt",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(initial.as_bytes())
                    .context("writing temp file")?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).context("creating temp file"),
        }
    }
    anyhow::bail!("could not create a temp file in {}", dir.display())
}

/// Write `initial` to a private temp file, run `cmd "<file>"` through `sh` (so the
/// editor command may carry arguments) and read the file back. One trailing
/// newline, as editors add, is dropped.
fn edit_text(initial: &str, cmd: &str) -> Result<String> {
    let path = private_temp_file(initial)?;
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{cmd} \"$1\""))
        .arg("mqtt-zapp")
        .arg(&path)
        .status();
    let read = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let status = status.with_context(|| format!("running {cmd}"))?;
    anyhow::ensure!(status.success(), "{cmd} exited with {status}");
    let mut text = read.context("reading temp file")?;
    if text.ends_with('\n') {
        text.pop();
    }
    Ok(text)
}

#[cfg(test)]
mod e2e_tests;
#[cfg(test)]
mod test_env;

#[cfg(all(test, unix))]
mod tests {
    use super::edit_text;

    #[test]
    fn editor_temp_file_is_private() {
        // The editor sees the file as "$1": print its mode into the file.
        let out = edit_text("secret", "stat -c %a \"$1\" >").unwrap();
        assert_eq!(out, "600");
    }

    #[test]
    fn editor_failure_keeps_payload() {
        assert!(edit_text("x", "false").is_err());
        assert_eq!(edit_text("x", "printf hi >").unwrap(), "hi");
        // One trailing newline (as editors add) is dropped.
        assert_eq!(edit_text("x", "printf 'a\\nb\\n' >").unwrap(), "a\nb");
        // The initial text reaches the editor.
        assert_eq!(edit_text("keep", "true").unwrap(), "keep");
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use crate::cli::Launch;
    use mqtt_zapp_core::config::{ProfileStore, Settings};
    use ratatui::backend::{Backend, TestBackend};

    #[test]
    fn boost_never_moves_later() {
        let t0 = Instant::now();
        let first = coalesce(None, t0);
        assert_eq!(first, Some(t0 + app::FAST_TICK));
        // A later event inside the window keeps the earlier deadline.
        let t1 = t0 + Duration::from_millis(60);
        assert_eq!(coalesce(first, t1), first);
        // The wake is the earlier of the tick and the boost.
        assert_eq!(
            next_deadline(t0, app::IDLE_TICK, first),
            t0 + app::FAST_TICK
        );
        assert_eq!(next_deadline(t0, app::IDLE_TICK, None), t0 + app::IDLE_TICK);
        assert_eq!(
            next_deadline(t0, app::FAST_TICK, Some(t0 + app::IDLE_TICK)),
            t0 + app::FAST_TICK
        );
    }

    #[test]
    fn input_redraws_immediately() {
        let mut app =
            App::new(Launch::Picker, ProfileStore::default(), Settings::default()).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        app.dirty = true;
        draw_if_dirty(&mut terminal, &mut app).unwrap();
        assert!(!app.dirty, "drawn and cleared");
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|c| c.symbol() != " "),
            "something was drawn"
        );
        // Not dirty: no draw (the buffer is untouched).
        terminal.backend_mut().clear().unwrap();
        draw_if_dirty(&mut terminal, &mut app).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .all(|c| c.symbol() == " ")
        );
    }
}
