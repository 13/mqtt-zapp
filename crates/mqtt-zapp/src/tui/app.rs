//! UI state machine: everything the TUI shows, updated from key presses and
//! core events. Side effects (connecting, publishing, quitting) are returned
//! as `Effect`s for the event loop to carry out, so `App` stays testable.

use super::keymap::{Action, Keymap};
use super::picker::{PasswordState, PickerState};
use super::profile_form::{ProfileEdit, ProfileForm, ProfileOutcome};
use super::publish::{Field, FormOutcome, PublishForm};
use super::rows::{Row, visible_rows};
use crate::cli::Launch;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mqtt_zapp_core::client::{ConnState, CoreEvent};
use mqtt_zapp_core::config::{
    DEFAULT_TIMESTAMP_FORMAT, PasswordSource, Profile, ProfileStore, Settings, SortOrder, ThemeName,
};
use mqtt_zapp_core::search::{self, Filter};
use mqtt_zapp_core::secrets::load_password;
use mqtt_zapp_core::state::AppState;
use mqtt_zapp_core::{NodeId, TopicTree};
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::time::{Duration, Instant};

mod front;
mod mouse;
pub use mouse::HitAreas;

/// How long a node stays highlighted after it (or its subtree) got a message.
pub const FLASH: Duration = Duration::from_secs(1);
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_BUCKET: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Picker,
    Password,
    Browse,
    SearchInput,
    Publish,
    Help,
    /// The broker statistics overlay; any key closes it.
    Stats,
    /// A yes/no question; see `App::confirm`.
    Confirm,
    /// The add/edit profile form over the picker.
    ProfileForm,
}

/// Which pane the j/k keys drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Payload,
    History,
}

#[allow(clippy::large_enum_variant)] // short-lived, consumed right away
pub enum Effect {
    None,
    Quit,
    Publish {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    },
    Connect(Profile, Option<String>),
    /// Open the publish payload in `$EDITOR`; the result comes back through
    /// `App::set_publish_payload`.
    EditPayload(String),
    /// Publish an empty retained message to each topic (clears it).
    ClearRetained(Vec<String>),
    /// Put `text` on the clipboard (OSC 52); `what` names it in the note.
    Copy {
        what: &'static str,
        text: String,
    },
    /// Store a profile (and its keyring password); the result comes back
    /// through `App::profile_saved` / `App::profile_save_failed`.
    SaveProfile(ProfileEdit),
    /// Delete the named profile, reported back like `SaveProfile`.
    DeleteProfile(String),
    /// Save the picker's custom order (profile names) to `profiles.toml`.
    SaveOrder(Vec<String>),
    /// Write `state.toml`; a failure comes back through `state_save_failed`.
    SaveState(AppState),
}

// Hand-written so a connect password can never reach a debug log.
impl fmt::Debug for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Effect::None => f.write_str("None"),
            Effect::Quit => f.write_str("Quit"),
            Effect::Publish {
                topic,
                payload,
                qos,
                retain,
            } => f
                .debug_struct("Publish")
                .field("topic", topic)
                .field("payload", &format_args!("<{} bytes>", payload.len()))
                .field("qos", qos)
                .field("retain", retain)
                .finish(),
            Effect::EditPayload(t) => write!(f, "EditPayload(<{} bytes>)", t.len()),
            Effect::ClearRetained(t) => write!(f, "ClearRetained({} topics)", t.len()),
            Effect::Copy { what, text } => {
                write!(f, "Copy({what}, <{} bytes>)", text.len())
            }
            Effect::Connect(profile, password) => {
                let pw = if password.is_some() {
                    "<redacted>"
                } else {
                    "None"
                };
                write!(f, "Connect({}, {pw})", profile.name)
            }
            Effect::SaveProfile(edit) => write!(f, "SaveProfile({edit:?})"),
            Effect::DeleteProfile(name) => write!(f, "DeleteProfile({name})"),
            Effect::SaveOrder(names) => write!(f, "SaveOrder({names:?})"),
            Effect::SaveState(st) => write!(f, "SaveState({st:?})"),
        }
    }
}

/// `fmt` if chrono accepts it, else the default (a bad format would make
/// formatting fail at draw time).
fn valid_time_format(fmt: &str) -> String {
    use chrono::format::{Item, StrftimeItems};
    if StrftimeItems::new(fmt).any(|i| matches!(i, Item::Error)) {
        DEFAULT_TIMESTAMP_FORMAT.to_owned()
    } else {
        fmt.to_owned()
    }
}

/// Payloads remembered per topic for ctrl-p/ctrl-n in the publish form.
const SENT_PER_TOPIC: usize = 20;
/// How long a footer note (publish result) stays visible.
pub const NOTE_TTL: Duration = Duration::from_secs(5);

/// Messages per second over a sliding 10 s window, counted in 1 s buckets so
/// bursts cost O(1) memory.
#[derive(Debug, Default)]
pub struct MsgRate {
    buckets: VecDeque<(Instant, u64)>,
}

impl MsgRate {
    pub fn record(&mut self, now: Instant) {
        while self
            .buckets
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) >= RATE_WINDOW)
        {
            self.buckets.pop_front();
        }
        match self.buckets.back_mut() {
            Some((t, n)) if now.saturating_duration_since(*t) < RATE_BUCKET => *n += 1,
            _ => self.buckets.push_back((now, 1)),
        }
    }

    pub fn per_sec(&self, now: Instant) -> f64 {
        let n: u64 = self
            .buckets
            .iter()
            .filter(|(t, _)| now.saturating_duration_since(*t) < RATE_WINDOW)
            .map(|(_, n)| n)
            .sum();
        n as f64 / RATE_WINDOW.as_secs_f64()
    }
}

/// What the header shows about the broker. Raw profile values; the
/// renderer sanitizes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub url: String,
    pub subscriptions: Vec<String>,
}

/// A transient footer message, e.g. a publish result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub error: bool,
}

impl Note {
    pub fn info(text: impl Into<String>) -> Note {
        Note {
            text: text.into(),
            error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Note {
        Note {
            text: text.into(),
            error: true,
        }
    }
}

/// A pending destructive action: what to ask, and what to do on yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub prompt: String,
    pub action: ConfirmAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    /// The retained topics to clear.
    ClearRetained(Vec<String>),
    /// The profile to delete; asked from the picker.
    DeleteProfile(String),
}

const STORE_BROKEN: &str = "profiles.toml has errors – fix it by hand first";

pub struct App {
    pub tree: TopicTree,
    pub expanded: HashSet<NodeId>,
    pub selected: Option<NodeId>,
    pub filter: Option<Filter>,
    pub search: String,
    /// `ctrl-r` in the search input: the query is a regex.
    pub search_regex: bool,
    /// The current (invalid) regex's error; the last valid filter stays.
    pub search_error: Option<String>,
    pub mode: Mode,
    pub conn_state: ConnState,
    pub dirty: bool,
    /// Set when messages arrived since the last tick; gates the filter rebuild.
    tree_changed: bool,
    pub msg_rate: MsgRate,
    /// First visible row of the tree pane; maintained by the renderer.
    pub tree_offset: usize,
    pub focus: Focus,
    /// Index into the selected node's `history` (0 = oldest); `None` follows
    /// the latest message.
    pub history_sel: Option<usize>,
    /// Unix second of the last redraw for the header's live age.
    pub age_second: u64,
    pub show_diff: bool,
    /// History entry the diff compares against (`m`), by (time, size);
    /// `None` diffs against the previous message.
    pub diff_base: Option<(std::time::SystemTime, usize)>,
    pub payload_scroll: usize,
    /// Memoised payload-area content; see `message_view`.
    pub body_cache: super::message_view::BodyCache,
    /// Memoised chart points; see `message_view`.
    pub chart_cache: super::message_view::ChartCache,
    /// Rows the payload area had in the last frame; sizes half-page scrolls.
    pub payload_height: Cell<u16>,
    pub keymap: Keymap,
    history_depth: usize,
    max_history_bytes: usize,
    /// New nodes at this depth or less start expanded.
    auto_expand: usize,
    pending_connect: Option<Profile>,
    pub picker: PickerState,
    /// Set while in `Mode::Password`.
    pub password: Option<PasswordState>,
    /// The broker last connected to, shown in the header.
    pub server: Option<ServerInfo>,
    /// Why `profiles.toml` could not be loaded; shown in the empty picker.
    pub store_error: Option<String>,
    /// Open while in `Mode::Publish`.
    pub publish: Option<PublishForm>,
    /// A transient footer message (publish result) and when it was set.
    pub status_note: Option<(Note, Instant)>,
    /// The chart: `None` off, `Some(None)` the plain numeric payload,
    /// `Some(Some(path))` a JSON field. Reset when another topic is selected.
    pub chart: Option<Option<String>>,
    /// Colour theme; `tui::run` applies `NO_COLOR`.
    pub theme: ThemeName,
    /// Validated strftime format for message times.
    pub timestamp_format: String,
    /// Recently published payloads per topic, newest first (at most 20).
    pub sent: HashMap<String, VecDeque<String>>,
    /// While walking `sent` in the form: the index shown and the draft to
    /// restore past the newest.
    sent_walk: Option<(usize, String)>,
    /// Order of sibling topics; `s` cycles it.
    pub sort: SortOrder,
    /// Set while in `Mode::Confirm`.
    pub confirm: Option<Confirm>,
    /// Open while in `Mode::ProfileForm`.
    pub profile_form: Option<ProfileForm>,
    /// Pane rectangles of the last frame, for mouse hit-testing.
    pub hit: Cell<HitAreas>,
}

impl App {
    pub fn new(launch: Launch, store: ProfileStore, settings: Settings) -> Result<App> {
        let (mode, pending_connect, conn_state) = match launch {
            Launch::Picker => (
                Mode::Picker,
                None,
                ConnState::Disconnected {
                    reason: "not connected".into(),
                    retry_in_ms: 0,
                },
            ),
            Launch::Connect(p) => (Mode::Browse, Some(p), ConnState::Connecting),
        };
        Ok(App {
            tree: TopicTree::with_limits(
                settings.history_depth,
                settings.max_history_mb.saturating_mul(1024 * 1024),
            ),
            expanded: HashSet::new(),
            selected: None,
            filter: None,
            search: String::new(),
            search_regex: false,
            search_error: None,
            mode,
            conn_state,
            dirty: true,
            tree_changed: false,
            msg_rate: MsgRate::default(),
            tree_offset: 0,
            focus: Focus::Tree,
            history_sel: None,
            age_second: 0,
            show_diff: false,
            diff_base: None,
            payload_scroll: 0,
            body_cache: Default::default(),
            chart_cache: Default::default(),
            payload_height: Cell::new(0),
            keymap: Keymap::from_settings(&settings.keys)?,
            history_depth: settings.history_depth,
            max_history_bytes: settings.max_history_mb.saturating_mul(1024 * 1024),
            auto_expand: settings.auto_expand,
            pending_connect,
            picker: PickerState::new(store.profiles, &Default::default()),
            password: None,
            server: None,
            store_error: None,
            publish: None,
            status_note: None,
            hit: Cell::new(HitAreas::default()),
            confirm: None,
            profile_form: None,
            sort: settings.sort,
            chart: None,
            sent: HashMap::new(),
            theme: settings.theme,
            timestamp_format: valid_time_format(&settings.timestamp_format),
            sent_walk: None,
        })
    }

    /// The effect to run once at startup: connect when launched with a profile.
    pub fn startup_effect(&mut self) -> Effect {
        match self.pending_connect.take() {
            Some(p) => self.begin_connect(p),
            None => Effect::None,
        }
    }

    /// Forget all topics, e.g. when switching connections.
    pub fn reset_tree(&mut self) {
        self.tree = TopicTree::with_limits(self.history_depth, self.max_history_bytes);
        self.expanded.clear();
        self.selected = None;
        self.history_sel = None;
        self.diff_base = None;
        self.payload_scroll = 0;
        self.filter = None;
        self.search.clear();
        self.tree_offset = 0;
        self.msg_rate = MsgRate::default();
        self.body_cache.clear();
        self.dirty = true;
    }

    pub fn apply_core_event(&mut self, ev: CoreEvent) {
        match ev {
            CoreEvent::State(s) => self.conn_state = s,
            CoreEvent::Message { topic, msg } => {
                let before = self
                    .tree
                    .find(&topic)
                    .map(|id| self.tree.node(id).history.len());
                let first_new = self.tree.len();
                let id = self.tree.insert(&topic, msg);
                // Only nodes this message created; a hand-collapsed one stays.
                for new in first_new..self.tree.len() {
                    if self.depth(new) <= self.auto_expand {
                        self.expanded.insert(new);
                    }
                }
                // A full ring drops its oldest entry, shifting every index down
                // by one; keep a frozen selection on its entry (or the oldest).
                if before.is_some_and(|n| n > 0 && n == self.tree.node(id).history.len())
                    && self.selected == Some(id)
                {
                    self.history_sel = self.history_sel.map(|i| i.saturating_sub(1));
                }
                // The marked entry fell out of the ring.
                if self.selected == Some(id)
                    && let Some(base) = self.diff_base
                    && !self
                        .tree
                        .node(id)
                        .history
                        .iter()
                        .any(|m| (m.received_at, m.payload.len()) == base)
                {
                    self.diff_base = None;
                }
                self.msg_rate.record(Instant::now());
                self.tree_changed = true;
            }
        }
        self.dirty = true;
    }

    /// Called every UI tick (100 ms): refresh the live filter so new messages
    /// show up in it, and keep redrawing while something is flashing.
    pub fn on_tick(&mut self) {
        if std::mem::take(&mut self.tree_changed) && !self.search.is_empty() {
            self.rebuild_filter();
        }
        let now = Instant::now();
        if self.status_note.is_some() {
            // Redraw to show the note, and once more to drop it.
            self.dirty = true;
            if self.note_at(now).is_none() {
                self.status_note = None;
            }
        }
        // ROOT is touched by every insert, so it tells whether any row flashes.
        let flashing = self.is_flashing(TopicTree::ROOT, now);
        // The message header's age ("3s ago") ticks once a second.
        if self.selected.is_some() {
            let sec = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            if sec != self.age_second {
                self.age_second = sec;
                self.dirty = true;
            }
        }
        // The msg/s figure in the header decays without new messages.
        if flashing || self.msg_rate.per_sec(now) > 0.0 {
            self.dirty = true;
        }
    }

    pub fn set_note(&mut self, note: Note, now: Instant) {
        self.status_note = Some((note, now));
        self.dirty = true;
    }

    /// The note, unless it is older than `NOTE_TTL` at `now`.
    pub fn note_at(&self, now: Instant) -> Option<&Note> {
        self.status_note
            .as_ref()
            .filter(|(_, t)| now.saturating_duration_since(*t) < NOTE_TTL)
            .map(|(n, _)| n)
    }

    pub fn is_flashing(&self, id: NodeId, now: Instant) -> bool {
        self.tree
            .node(id)
            .last_update
            .is_some_and(|t| now.saturating_duration_since(t) < FLASH)
    }

    pub fn rows(&self) -> Vec<Row> {
        visible_rows(&self.tree, &self.expanded, self.filter.as_ref(), self.sort)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if key.kind == KeyEventKind::Release {
            return Effect::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Effect::Quit;
        }
        match self.mode {
            Mode::Browse => match self.keymap.lookup(key) {
                Some(action) => self.handle_action(action),
                None => Effect::None,
            },
            Mode::Picker => match self.keymap.lookup(key) {
                Some(action) => self.on_picker_action(action),
                None => Effect::None,
            },
            Mode::Password => self.on_password_key(key),
            Mode::SearchInput => {
                self.on_search_key(key);
                Effect::None
            }
            Mode::Publish => self.on_publish_key(key),
            Mode::Help | Mode::Stats => {
                self.mode = Mode::Browse;
                self.dirty = true;
                Effect::None
            }
            Mode::Confirm => self.on_confirm_key(key),
            Mode::ProfileForm => self.on_profile_form_key(key),
        }
    }

    /// `y` or enter confirms; any other key cancels.
    fn on_confirm_key(&mut self, key: KeyEvent) -> Effect {
        self.dirty = true;
        let confirm = self.confirm.take();
        self.mode = match &confirm {
            Some(Confirm {
                action: ConfirmAction::DeleteProfile(_),
                ..
            }) => Mode::Picker,
            _ => Mode::Browse,
        };
        match (key.code, confirm) {
            (KeyCode::Char('y') | KeyCode::Enter, Some(c)) => match c.action {
                ConfirmAction::ClearRetained(topics) => Effect::ClearRetained(topics),
                ConfirmAction::DeleteProfile(name) => Effect::DeleteProfile(name),
            },
            _ => {
                self.set_note(Note::info("cancelled"), Instant::now());
                Effect::None
            }
        }
    }

    /// Ask before clearing the retained messages of the selected node, or
    /// (`subtree`) of it and everything below.
    fn ask_clear_retained(&mut self, subtree: bool) {
        let Some(id) = self.selected else {
            self.set_note(Note::info("no retained messages here"), Instant::now());
            return;
        };
        let mut topics = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = self.tree.node(n);
            // `$` topics (e.g. `$SYS`) belong to the broker; clients cannot
            // clear them.
            let topic = self.tree.full_topic(n);
            if node.retained && !topic.starts_with('$') {
                topics.push(topic);
            }
            if subtree {
                stack.extend(node.children.values());
            }
        }
        let topic = self.tree.full_topic(id);
        let prompt = match (subtree, topics.len()) {
            (_, 0) => {
                self.set_note(Note::info("no retained messages here"), Instant::now());
                return;
            }
            (false, _) => format!("delete retained message on {topic}?"),
            (true, 1) => format!("clear 1 retained message under {topic}?"),
            (true, n) => format!("clear {n} retained messages under {topic}?"),
        };
        self.confirm = Some(Confirm {
            prompt,
            action: ConfirmAction::ClearRetained(topics),
        });
        self.mode = Mode::Confirm;
    }

    /// Keys in the publish form; they bypass the keymap. A send leaves the
    /// form open so the user can test repeatedly.
    fn on_publish_key(&mut self, key: KeyEvent) -> Effect {
        self.dirty = true;
        let Some(form) = self.publish.as_mut() else {
            self.mode = Mode::Browse;
            return Effect::None;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && form.field == Field::Payload {
            match key.code {
                KeyCode::Char('p') => return self.walk_sent(true),
                KeyCode::Char('n') => return self.walk_sent(false),
                _ => {}
            }
        }
        self.sent_walk = None;
        let Some(form) = self.publish.as_mut() else {
            return Effect::None;
        };
        match form.on_key(key) {
            FormOutcome::Continue => Effect::None,
            FormOutcome::Edit(text) => Effect::EditPayload(text),
            FormOutcome::Cancel => {
                self.publish = None;
                self.mode = Mode::Browse;
                Effect::None
            }
            FormOutcome::Send {
                topic,
                payload,
                qos,
                retain,
            } => {
                self.remember_sent(&topic, &payload);
                Effect::Publish {
                    topic,
                    payload,
                    qos,
                    retain,
                }
            }
        }
    }

    /// Replace the publish form's payload (the editor's result).
    pub fn set_publish_payload(&mut self, text: &str) {
        if let Some(form) = self.publish.as_mut() {
            form.set_payload(text);
            self.dirty = true;
        }
    }

    fn remember_sent(&mut self, topic: &str, payload: &[u8]) {
        let text = String::from_utf8_lossy(payload).into_owned();
        let list = self.sent.entry(topic.to_owned()).or_default();
        list.retain(|p| *p != text);
        list.push_front(text);
        list.truncate(SENT_PER_TOPIC);
    }

    /// ctrl-p (`older`) / ctrl-n in the payload: walk the payloads sent to
    /// the topic in the form; past the newest the draft comes back.
    fn walk_sent(&mut self, older: bool) -> Effect {
        let Some(form) = self.publish.as_mut() else {
            return Effect::None;
        };
        let topic = form.topic.lines().join("");
        let Some(list) = self.sent.get(&topic).filter(|l| !l.is_empty()) else {
            return Effect::None;
        };
        let next = match (self.sent_walk.take(), older) {
            (None, true) => Some((0, form.payload_text())),
            (None, false) => None,
            (Some((i, d)), true) => Some(((i + 1).min(list.len() - 1), d)),
            (Some((0, d)), false) => {
                form.set_payload(&d);
                None
            }
            (Some((i, d)), false) => Some((i - 1, d)),
        };
        if let Some((i, _)) = &next {
            form.set_payload(&list[*i]);
        }
        self.sent_walk = next;
        Effect::None
    }

    fn open_publish(&mut self) {
        let topic = self
            .selected
            .map(|id| self.tree.full_topic(id))
            .unwrap_or_default();
        self.publish = Some(PublishForm::new(&topic));
        self.sent_walk = None;
        self.mode = Mode::Publish;
    }

    /// Keys while typing a search. They bypass the keymap, so `q` and `j`
    /// are just characters.
    pub fn on_search_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        match key.code {
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_regex = !self.search_regex;
                self.rebuild_filter();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.search.push(c);
                self.rebuild_filter();
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.rebuild_filter();
            }
            KeyCode::Enter => {
                self.reveal_selected();
                self.mode = Mode::Browse;
            }
            KeyCode::Esc => {
                self.clear_search();
                self.mode = Mode::Browse;
            }
            _ => {}
        }
    }

    fn clear_search(&mut self) {
        self.reveal_selected();
        self.search.clear();
        self.search_error = None;
        self.filter = None;
    }

    /// Expand every ancestor of the selection, so it stays visible once a
    /// filter (which shows everything expanded) goes away.
    fn reveal_selected(&mut self) {
        let mut cur = self.selected.and_then(|s| self.tree.node(s).parent);
        while let Some(id) = cur.filter(|&id| id != TopicTree::ROOT) {
            self.expanded.insert(id);
            cur = self.tree.node(id).parent;
        }
    }

    /// Recompute the filter from `search`; if the selection is not shown,
    /// move it to the first match.
    fn rebuild_filter(&mut self) {
        let mode = if self.search_regex {
            search::QueryMode::Regex
        } else {
            search::QueryMode::Plain
        };
        match search::filter(&self.tree, &self.search, mode) {
            Ok(f) => {
                self.filter = f;
                self.search_error = None;
            }
            // Keep the last valid filter while the regex is being typed.
            Err(e) => self.search_error = Some(e),
        }
        let Some(f) = &self.filter else {
            return;
        };
        if self.selected.is_some_and(|s| f.visible.contains(&s)) {
            return;
        }
        let first = self.rows().into_iter().find(|r| f.matches.contains(&r.id));
        if let Some(row) = first {
            self.select(row.id);
        }
    }

    /// Browse-mode actions.
    pub fn handle_action(&mut self, action: Action) -> Effect {
        self.dirty = true;
        match action {
            Action::Quit => return Effect::Quit,
            Action::Up if self.focus == Focus::History => self.history_older(),
            Action::Down if self.focus == Focus::History => self.history_newer(),
            Action::Up if self.focus == Focus::Payload => self.scroll_payload_by(-1),
            Action::Down if self.focus == Focus::Payload => self.scroll_payload_by(1),
            Action::Top if self.focus == Focus::Payload => self.payload_scroll = 0,
            Action::Bottom if self.focus == Focus::Payload => self.scroll_payload_by(isize::MAX),
            // Tree-only keys do nothing while the payload has focus.
            Action::Collapse | Action::Expand | Action::ToggleExpand | Action::Select
                if self.focus == Focus::Payload => {}
            Action::Up => self.move_selection(|i, _| i.saturating_sub(1)),
            Action::Down => self.move_selection(|i, n| (i + 1).min(n - 1)),
            Action::FocusNext => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Payload,
                    Focus::Payload => Focus::History,
                    Focus::History => Focus::Tree,
                }
            }
            Action::ToggleDiff => {
                self.show_diff = !self.show_diff;
                self.payload_scroll = 0;
            }
            Action::MarkDiffBase => self.mark_diff_base(),
            Action::ScrollDown => self.scroll_payload(true),
            Action::ScrollUp => self.scroll_payload(false),
            Action::Top => self.move_selection(|_, _| 0),
            Action::Bottom => self.move_selection(|_, n| n - 1),
            Action::Expand => {
                if let Some(row) = self.selected_row().filter(|r| r.has_children) {
                    self.expanded.insert(row.id);
                }
            }
            Action::ToggleExpand | Action::Select => {
                if let Some(row) = self.selected_row().filter(|r| r.has_children)
                    && !self.expanded.remove(&row.id)
                {
                    self.expanded.insert(row.id);
                }
            }
            Action::Collapse => self.collapse(),
            Action::Search => self.mode = Mode::SearchInput,
            Action::Cancel => self.clear_search(),
            Action::Publish => self.open_publish(),
            Action::Help => self.mode = Mode::Help,
            Action::CycleSort => {
                self.sort = self.sort.next();
                self.set_note(
                    Note::info(format!("sort: {}", self.sort.name())),
                    Instant::now(),
                );
            }
            Action::Chart => self.cycle_chart(),
            Action::Stats => self.mode = Mode::Stats,
            // Picker-only.
            Action::AddProfile
            | Action::EditProfile
            | Action::DeleteProfile
            | Action::MoveUp
            | Action::MoveDown => {}
            Action::ExpandAll => self.expand_subtree(true),
            Action::CollapseAll => self.expand_subtree(false),
            Action::DeleteRetained => self.ask_clear_retained(false),
            Action::ClearRetained => self.ask_clear_retained(true),
            Action::CopyTopic => return self.copy(true),
            Action::CopyPayload => return self.copy(false),
        }
        Effect::None
    }

    /// off → plain number (if the shown payload is one) → each numeric JSON
    /// field → off.
    fn cycle_chart(&mut self) {
        use mqtt_zapp_core::payload::{numeric_at, numeric_paths};
        let Some(m) = super::message_view::shown_message(self) else {
            self.set_note(Note::info("nothing to chart"), Instant::now());
            return;
        };
        let mut series: Vec<Option<String>> = Vec::new();
        if numeric_at(&m.payload, None).is_some() {
            series.push(None);
        }
        series.extend(numeric_paths(&m.payload).into_iter().map(Some));
        let next = match &self.chart {
            None => series.first().cloned(),
            Some(cur) => series
                .iter()
                .position(|s| s == cur)
                .and_then(|i| series.get(i + 1).cloned()),
        };
        self.chart = next;
        let note = match (&self.chart, series.is_empty()) {
            (_, true) => "no numeric values to chart".to_owned(),
            (None, _) => "chart off".to_owned(),
            (Some(s), _) => format!("chart: {}", s.as_deref().unwrap_or("value")),
        };
        self.set_note(Note::info(note), Instant::now());
    }

    /// Expand (or collapse) the selected node and every node below it.
    fn expand_subtree(&mut self, expand: bool) {
        let Some(id) = self.selected else { return };
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let kids = &self.tree.node(n).children;
            if expand && !kids.is_empty() {
                self.expanded.insert(n);
            } else if !expand {
                self.expanded.remove(&n);
            }
            stack.extend(kids.values());
        }
    }

    /// Copy the selected topic, or the payload on screen (history-aware).
    pub(super) fn copy(&mut self, topic: bool) -> Effect {
        let text = if topic {
            self.selected.map(|id| self.tree.full_topic(id))
        } else {
            super::message_view::shown_message(self)
                .map(|m| String::from_utf8_lossy(&m.payload).into_owned())
        };
        match text {
            Some(text) => Effect::Copy {
                what: if topic { "topic" } else { "payload" },
                text,
            },
            None => {
                self.set_note(Note::info("nothing to copy"), Instant::now());
                Effect::None
            }
        }
    }

    fn history_len(&self) -> usize {
        self.selected
            .map_or(0, |id| self.tree.node(id).history.len())
    }

    /// `k` in the history list: towards older entries.
    fn history_older(&mut self) {
        let len = self.history_len();
        let to = match self.history_sel {
            None => len.checked_sub(2),
            Some(i) => Some(i.saturating_sub(1)),
        };
        if let Some(i) = to {
            self.history_sel = Some(i.min(len.saturating_sub(1)));
            self.payload_scroll = 0;
        }
    }

    /// `j` in the history list: towards newer entries; the newest entry is live.
    fn history_newer(&mut self) {
        if let Some(i) = self.history_sel {
            self.history_sel = (i + 2 < self.history_len()).then_some(i + 1);
            self.payload_scroll = 0;
        }
    }

    /// Half a page up or down, never past the end of the shown content.
    fn scroll_payload(&mut self, down: bool) {
        let half = (self.payload_height.get() / 2).max(1) as isize;
        self.scroll_payload_by(if down { half } else { -half });
    }

    /// Scroll the payload by `delta` lines, clamped to the shown content.
    fn scroll_payload_by(&mut self, delta: isize) {
        let height = usize::from(self.payload_height.get());
        let max = super::message_view::content_len(self).saturating_sub(height);
        self.payload_scroll = self.payload_scroll.saturating_add_signed(delta).min(max);
    }

    fn selected_row(&self) -> Option<Row> {
        let id = self.selected?;
        self.rows().into_iter().find(|r| r.id == id)
    }

    /// `to(index, row_count)` picks the new row; with nothing (visible)
    /// selected, the first row is selected.
    fn move_selection(&mut self, to: impl Fn(usize, usize) -> usize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let idx = match self
            .selected
            .and_then(|s| rows.iter().position(|r| r.id == s))
        {
            Some(i) => to(i, rows.len()),
            None => 0,
        };
        self.select(rows[idx].id);
    }

    /// Select a node; a different node starts with a live view at the top.
    /// Levels below ROOT: a top-level node is 1.
    fn depth(&self, mut id: NodeId) -> usize {
        let mut d = 0;
        while let Some(p) = self.tree.node(id).parent {
            d += 1;
            id = p;
        }
        d
    }

    /// `m`: compare against the shown entry, or stop when it already is the base.
    fn mark_diff_base(&mut self) {
        let Some(m) = super::message_view::shown_message(self) else {
            self.set_note(Note::info("nothing to mark"), Instant::now());
            return;
        };
        let stamp = (m.received_at, m.payload.len());
        if self.diff_base == Some(stamp) {
            self.diff_base = None;
            self.set_note(Note::info("diff base cleared"), Instant::now());
        } else {
            self.diff_base = Some(stamp);
            self.show_diff = true;
            self.payload_scroll = 0;
            self.set_note(Note::info("diff base marked"), Instant::now());
        }
    }

    pub(super) fn select(&mut self, id: NodeId) {
        if self.selected != Some(id) {
            self.chart = None;
            self.diff_base = None;
            self.history_sel = None;
            self.payload_scroll = 0;
        }
        self.selected = Some(id);
    }

    /// Collapse an expanded node, otherwise move to its parent.
    fn collapse(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.expanded && row.has_children && self.filter.is_none() {
            self.expanded.remove(&row.id);
        } else if let Some(parent) = self.tree.node(row.id).parent
            && parent != TopicTree::ROOT
        {
            self.select(parent);
        }
    }
}

#[cfg(test)]
mod tests;
