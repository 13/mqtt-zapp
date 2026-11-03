//! UI state machine: everything the TUI shows, updated from key presses and
//! core events. Side effects (connecting, publishing, quitting) are returned
//! as `Effect`s for the event loop to carry out, so `App` stays testable.

use super::keymap::{Action, Keymap};
use super::picker::{PasswordState, PickerState};
use super::publish::{FormOutcome, PublishForm};
use super::rows::{Row, visible_rows};
use crate::cli::Launch;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mqtt_zapp_core::client::{ConnState, CoreEvent};
use mqtt_zapp_core::config::{PasswordSource, Profile, ProfileStore, Settings, SortOrder};
use mqtt_zapp_core::search::{self, Filter};
use mqtt_zapp_core::secrets::load_password;
use mqtt_zapp_core::{NodeId, TopicTree};
use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::time::{Duration, Instant};

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
    /// A yes/no question; see `App::confirm`.
    Confirm,
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
    /// Publish an empty retained message to each topic (clears it).
    ClearRetained(Vec<String>),
    /// Put `text` on the clipboard (OSC 52); `what` names it in the note.
    Copy {
        what: &'static str,
        text: String,
    },
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
        }
    }
}

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

/// A pending destructive action: what to ask, and the retained topics to
/// clear on yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub prompt: String,
    pub topics: Vec<String>,
}

pub struct App {
    pub tree: TopicTree,
    pub expanded: HashSet<NodeId>,
    pub selected: Option<NodeId>,
    pub filter: Option<Filter>,
    pub search: String,
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
    pub show_diff: bool,
    pub payload_scroll: usize,
    /// Memoised payload-area content; see `message_view`.
    pub body_cache: super::message_view::BodyCache,
    /// Rows the payload area had in the last frame; sizes half-page scrolls.
    pub payload_height: Cell<u16>,
    pub keymap: Keymap,
    history_depth: usize,
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
    /// Order of sibling topics; `s` cycles it.
    pub sort: SortOrder,
    /// Set while in `Mode::Confirm`.
    pub confirm: Option<Confirm>,
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
            tree: TopicTree::new(settings.history_depth),
            expanded: HashSet::new(),
            selected: None,
            filter: None,
            search: String::new(),
            mode,
            conn_state,
            dirty: true,
            tree_changed: false,
            msg_rate: MsgRate::default(),
            tree_offset: 0,
            focus: Focus::Tree,
            history_sel: None,
            show_diff: false,
            payload_scroll: 0,
            body_cache: Default::default(),
            payload_height: Cell::new(0),
            keymap: Keymap::from_settings(&settings.keys)?,
            history_depth: settings.history_depth,
            pending_connect,
            picker: PickerState::new(store.profiles),
            password: None,
            server: None,
            store_error: None,
            publish: None,
            status_note: None,
            hit: Cell::new(HitAreas::default()),
            confirm: None,
            sort: settings.sort,
            chart: None,
        })
    }

    /// The effect to run once at startup: connect when launched with a profile.
    pub fn startup_effect(&mut self) -> Effect {
        match self.pending_connect.take() {
            Some(p) => self.begin_connect(p),
            None => Effect::None,
        }
    }

    /// Start connecting to `p`, resolving its password: connect right away,
    /// or switch to `Mode::Password` (returning `Effect::None`) when the user
    /// has to type it.
    pub fn begin_connect(&mut self, p: Profile) -> Effect {
        self.begin_connect_with(p, load_password)
    }

    /// `begin_connect` with the keyring lookup injected, so tests never
    /// touch the real keyring.
    fn begin_connect_with(
        &mut self,
        p: Profile,
        keyring: impl FnOnce(&str) -> Result<Option<String>>,
    ) -> Effect {
        let password = match p.password {
            PasswordSource::None => None,
            // A failed or empty lookup falls back to asking.
            PasswordSource::Keyring => match keyring(&p.name) {
                Ok(Some(pw)) => Some(pw),
                Ok(None) | Err(_) => return self.ask_password(p),
            },
            PasswordSource::Prompt => return self.ask_password(p),
        };
        self.connect_effect(p, password)
    }

    fn ask_password(&mut self, profile: Profile) -> Effect {
        self.password = Some(PasswordState {
            profile,
            input: String::new(),
        });
        self.mode = Mode::Password;
        self.dirty = true;
        Effect::None
    }

    fn connect_effect(&mut self, p: Profile, password: Option<String>) -> Effect {
        self.server = Some(ServerInfo {
            name: p.name.clone(),
            url: super::picker::server_url(&p),
            subscriptions: p.subscriptions.clone(),
        });
        self.mode = Mode::Browse;
        self.dirty = true;
        Effect::Connect(p, password)
    }

    /// Keys in the picker: through the keymap.
    fn on_picker_action(&mut self, action: Action) -> Effect {
        self.dirty = true;
        match action {
            Action::Quit => return Effect::Quit,
            Action::Up => self.picker.up(),
            Action::Down => self.picker.down(),
            Action::Select => {
                if let Some(p) = self.picker.selected().cloned() {
                    return self.begin_connect(p);
                }
            }
            _ => {}
        }
        Effect::None
    }

    /// Keys in the password prompt. They bypass the keymap, so `q` and `j`
    /// are just characters. The password is never echoed or logged.
    fn on_password_key(&mut self, key: KeyEvent) -> Effect {
        self.dirty = true;
        let Some(st) = self.password.as_mut() else {
            self.mode = Mode::Picker;
            return Effect::None;
        };
        match key.code {
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                st.input.push(c);
            }
            KeyCode::Backspace => {
                st.input.pop();
            }
            KeyCode::Enter => {
                let st = self.password.take().expect("checked above");
                return self.connect_effect(st.profile, Some(st.input));
            }
            KeyCode::Esc => {
                self.password = None;
                if self.picker.profiles.is_empty() {
                    return Effect::Quit;
                }
                self.mode = Mode::Picker;
            }
            _ => {}
        }
        Effect::None
    }

    /// Forget all topics, e.g. when switching connections.
    pub fn reset_tree(&mut self) {
        self.tree = TopicTree::new(self.history_depth);
        self.expanded.clear();
        self.selected = None;
        self.history_sel = None;
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
                let id = self.tree.insert(&topic, msg);
                // A full ring drops its oldest entry, shifting every index down
                // by one; keep a frozen selection on its entry (or the oldest).
                if before.is_some_and(|n| n > 0 && n == self.tree.node(id).history.len())
                    && self.selected == Some(id)
                {
                    self.history_sel = self.history_sel.map(|i| i.saturating_sub(1));
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
            Mode::Help => {
                self.mode = Mode::Browse;
                self.dirty = true;
                Effect::None
            }
            Mode::Confirm => self.on_confirm_key(key),
        }
    }

    /// `y` or enter confirms; any other key cancels.
    fn on_confirm_key(&mut self, key: KeyEvent) -> Effect {
        self.dirty = true;
        self.mode = Mode::Browse;
        let confirm = self.confirm.take();
        match (key.code, confirm) {
            (KeyCode::Char('y') | KeyCode::Enter, Some(c)) => Effect::ClearRetained(c.topics),
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
        let is_retained = |n: &mqtt_zapp_core::TopicNode| {
            n.latest
                .as_ref()
                .is_some_and(|m| m.retain && !m.payload.is_empty())
        };
        let mut topics = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = self.tree.node(n);
            if is_retained(node) {
                topics.push(self.tree.full_topic(n));
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
            (true, n) => format!("clear {n} retained messages under {topic}?"),
        };
        self.confirm = Some(Confirm { prompt, topics });
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
        match form.on_key(key) {
            FormOutcome::Continue => Effect::None,
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
            } => Effect::Publish {
                topic,
                payload,
                qos,
                retain,
            },
        }
    }

    fn open_publish(&mut self) {
        let topic = self
            .selected
            .map(|id| self.tree.full_topic(id))
            .unwrap_or_default();
        self.publish = Some(PublishForm::new(&topic));
        self.mode = Mode::Publish;
    }

    /// Keys while typing a search. They bypass the keymap, so `q` and `j`
    /// are just characters.
    pub fn on_search_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        match key.code {
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
        self.filter = search::filter(&self.tree, &self.search);
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
    fn copy(&mut self, topic: bool) -> Effect {
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
    fn select(&mut self, id: NodeId) {
        if self.selected != Some(id) {
            self.chart = None;
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
mod tests {
    use super::*;
    use crate::cli::Launch;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mqtt_zapp_core::Message;
    use mqtt_zapp_core::client::{ConnState, CoreEvent};
    use mqtt_zapp_core::config::{
        PasswordSource, Profile, ProfileStore, Protocol, Settings, TlsConfig,
    };
    use std::time::{Duration, Instant, SystemTime};

    fn msg(topic: &str, p: &str) -> CoreEvent {
        CoreEvent::Message {
            topic: topic.into(),
            msg: Message {
                payload: p.into(),
                qos: 0,
                retain: true,
                received_at: SystemTime::now(),
            },
        }
    }

    fn app() -> App {
        App::new(Launch::Picker, ProfileStore::default(), Settings::default()).unwrap()
    }

    fn browse(topics: &[&str]) -> App {
        let mut a = app();
        a.mode = Mode::Browse;
        for t in topics {
            a.apply_core_event(msg(t, "v"));
        }
        a
    }

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

    fn plain(topic: &str, p: &str) -> CoreEvent {
        match msg(topic, p) {
            CoreEvent::Message { topic, mut msg } => {
                msg.retain = false;
                CoreEvent::Message { topic, msg }
            }
            e => e,
        }
    }

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
            a.handle_key(key(KeyCode::Enter)),
            Effect::ClearRetained(_)
        ));
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
        let mut t = c.topics.clone();
        t.sort();
        assert_eq!(t, vec!["a/y", "a/y/z"]);
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
        assert_eq!(a.expanded, ids(&a, &["a", "a/b", "a/d"]));
        a.expanded.insert(a.tree.find("z").unwrap());
        a.handle_action(Action::CollapseAll);
        assert_eq!(a.expanded, ids(&a, &["z"]));
    }

    fn sel(a: &App) -> String {
        a.tree.full_topic(a.selected.unwrap())
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
    fn quit_and_ctrl_c() {
        let mut a = browse(&[]);
        assert!(matches!(a.handle_action(Action::Quit), Effect::Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(a.handle_key(ctrl_c), Effect::Quit));
        a.mode = Mode::Picker;
        assert!(matches!(a.handle_key(ctrl_c), Effect::Quit));
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        a.mode = Mode::Browse;
        assert!(matches!(a.handle_key(q), Effect::Quit));
    }

    #[test]
    fn keymap_comes_from_settings() {
        let mut s = Settings::default();
        s.keys.insert("quit".into(), "x".into());
        let mut a = App::new(Launch::Picker, ProfileStore::default(), s).unwrap();
        a.mode = Mode::Browse;
        let x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(matches!(a.handle_key(x), Effect::Quit));
        s = Settings::default();
        s.keys.insert("nope".into(), "x".into());
        assert!(App::new(Launch::Picker, ProfileStore::default(), s).is_err());
    }

    #[test]
    fn launch_decides_mode_and_startup_effect() {
        let mut a = app();
        assert_eq!(a.mode, Mode::Picker);
        assert!(matches!(a.startup_effect(), Effect::None));
        let store = ProfileStore::default();
        let p = crate::cli::resolve_launch(
            &<crate::cli::Args as clap::Parser>::parse_from(["mqtt-zapp", "--host", "h"]),
            &store,
        )
        .unwrap();
        let mut a = App::new(p, store, Settings::default()).unwrap();
        assert_eq!(a.mode, Mode::Browse);
        match a.startup_effect() {
            Effect::Connect(p, None) => assert_eq!(p.host, "h"),
            _ => panic!("expected connect"),
        }
        assert!(matches!(a.startup_effect(), Effect::None), "only once");
    }

    #[test]
    fn state_event_updates_status() {
        let mut a = browse(&[]);
        a.dirty = false;
        a.apply_core_event(CoreEvent::State(ConnState::Connected));
        assert!(matches!(a.conn_state, ConnState::Connected));
        assert!(a.dirty);
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn type_str(a: &mut App, s: &str) {
        for c in s.chars() {
            a.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn row_names(a: &App) -> Vec<String> {
        a.rows().iter().map(|r| a.tree.full_topic(r.id)).collect()
    }

    fn search_app() -> App {
        browse(&[
            "muh/esp/esp32-WZR1/LWT",
            "tasmota/tele/x/STATE",
            "other/thing",
        ])
    }

    #[test]
    fn typing_filters_each_keystroke() {
        let mut a = search_app();
        a.handle_key(key(KeyCode::Char('/')));
        assert_eq!(a.mode, Mode::SearchInput);
        type_str(&mut a, "wz");
        assert!(a.filter.is_some());
        assert!(row_names(&a).contains(&"muh/esp/esp32-WZR1".to_string()));
        assert!(!row_names(&a).contains(&"other".to_string()));
        type_str(&mut a, "r");
        assert!(row_names(&a).contains(&"muh/esp/esp32-WZR1".to_string()));
        a.handle_key(key(KeyCode::Backspace));
        assert_eq!(a.search, "wz");
    }

    #[test]
    fn payload_match_shows_topic() {
        let mut a = search_app();
        a.apply_core_event(msg("tasmota/tele/x/STATE", r#"{"DeviceName":"wzr-lamp"}"#));
        a.handle_key(key(KeyCode::Char('/')));
        type_str(&mut a, "wzr");
        assert!(row_names(&a).contains(&"tasmota/tele/x/STATE".to_string()));
    }

    #[test]
    fn new_matching_topic_appears_while_filtered() {
        let mut a = search_app();
        a.handle_key(key(KeyCode::Char('/')));
        type_str(&mut a, "wzr");
        a.handle_key(key(KeyCode::Enter));
        assert_eq!(a.mode, Mode::Browse);
        assert!(a.filter.is_some(), "enter keeps the filter");
        a.apply_core_event(msg("new/wzr2", "v"));
        a.on_tick();
        assert!(row_names(&a).contains(&"new/wzr2".to_string()));
    }

    #[test]
    fn esc_clears_filter() {
        let mut a = search_app();
        a.handle_key(key(KeyCode::Char('/')));
        type_str(&mut a, "wzr");
        a.handle_key(key(KeyCode::Esc));
        assert_eq!(a.mode, Mode::Browse);
        assert!(a.filter.is_none() && a.search.is_empty());
        // In Browse, esc clears a kept filter too.
        a.handle_key(key(KeyCode::Char('/')));
        type_str(&mut a, "wzr");
        a.handle_key(key(KeyCode::Enter));
        a.handle_key(key(KeyCode::Esc));
        assert!(a.filter.is_none() && a.search.is_empty());
    }

    #[test]
    fn clearing_filter_keeps_selected_deep_node_visible() {
        for keep in [false, true] {
            let mut a = search_app();
            a.handle_key(key(KeyCode::Char('/')));
            type_str(&mut a, "wzr");
            let deep = a.tree.find("muh/esp/esp32-WZR1/LWT").unwrap();
            a.selected = Some(deep);
            if keep {
                a.handle_key(key(KeyCode::Enter));
                a.handle_key(key(KeyCode::Esc));
            } else {
                a.handle_key(key(KeyCode::Esc));
            }
            let rows = a.rows();
            let i = rows.iter().position(|r| r.id == deep).expect("visible");
            a.handle_action(Action::Down);
            assert_eq!(a.selected, Some(rows[i + 1].id), "keep={keep}");
            assert_eq!(sel(&a), "other");
        }
    }

    #[test]
    fn enter_keeping_filter_reveals_selection() {
        let mut a = search_app();
        a.handle_key(key(KeyCode::Char('/')));
        type_str(&mut a, "wzr");
        let deep = a.tree.find("muh/esp/esp32-WZR1/LWT").unwrap();
        a.selected = Some(deep);
        a.handle_key(key(KeyCode::Enter));
        a.filter = None;
        assert!(a.rows().iter().any(|r| r.id == deep));
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
    fn selection_jumps_to_first_match_when_hidden() {
        let mut a = search_app();
        let other = a.tree.find("other").unwrap();
        a.selected = Some(other);
        a.handle_key(key(KeyCode::Char('/')));
        type_str(&mut a, "wzr");
        assert_eq!(sel(&a), "muh/esp/esp32-WZR1");
    }

    #[test]
    fn typing_q_in_search_does_not_quit() {
        let mut a = search_app();
        a.handle_key(key(KeyCode::Char('/')));
        assert!(matches!(
            a.handle_key(key(KeyCode::Char('q'))),
            Effect::None
        ));
        assert_eq!(a.search, "q");
    }

    #[test]
    fn on_tick_rebuilds_active_filter() {
        let mut a = browse(&["a/wzr"]);
        a.search = "wzr".into();
        a.on_tick();
        assert_eq!(a.filter.as_ref().unwrap().matches.len(), 1);
        a.apply_core_event(msg("b/wzr2", "v"));
        a.on_tick();
        assert_eq!(a.filter.as_ref().unwrap().matches.len(), 2);
    }

    #[test]
    fn on_tick_marks_dirty_while_flashing() {
        let mut a = browse(&["a"]);
        a.dirty = false;
        a.on_tick();
        assert!(a.dirty, "fresh message is still flashing");
    }

    fn profile(name: &str, pw: PasswordSource) -> Profile {
        Profile {
            name: name.into(),
            host: "h".into(),
            port: 1883,
            protocol: Protocol::Mqtt,
            ws_path: "/".into(),
            client_id: None,
            username: Some("u".into()),
            password: pw,
            tls: TlsConfig::default(),
            subscriptions: vec!["#".into()],
        }
    }

    fn picker_app(profiles: Vec<Profile>) -> App {
        let store = ProfileStore { profiles };
        App::new(Launch::Picker, store, Settings::default()).unwrap()
    }

    #[test]
    fn picker_enter_on_none_password_connects() {
        let mut a = picker_app(vec![profile("a", PasswordSource::None)]);
        match a.handle_key(key(KeyCode::Enter)) {
            Effect::Connect(p, None) => assert_eq!(p.name, "a"),
            e => panic!("expected connect, got {e:?}"),
        }
        assert_eq!(a.mode, Mode::Browse);
        assert_eq!(a.server.as_ref().map(|s| s.name.as_str()), Some("a"));
    }

    #[test]
    fn picker_moves_selection_and_connects_to_it() {
        let mut a = picker_app(vec![
            profile("a", PasswordSource::None),
            profile("b", PasswordSource::None),
        ]);
        a.handle_key(key(KeyCode::Down));
        a.handle_key(key(KeyCode::Down));
        assert_eq!(a.picker.sel, 1, "stops at the last");
        a.handle_key(key(KeyCode::Up));
        a.handle_key(key(KeyCode::Down));
        assert!(matches!(a.handle_key(key(KeyCode::Enter)),
            Effect::Connect(p, None) if p.name == "b"));
        let mut a = picker_app(vec![]);
        assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
        assert_eq!(a.mode, Mode::Picker);
        assert!(matches!(
            a.handle_key(key(KeyCode::Char('q'))),
            Effect::Quit
        ));
    }

    #[test]
    fn picker_prompt_goes_to_password_mode_then_connects() {
        let mut a = picker_app(vec![profile("a", PasswordSource::Prompt)]);
        assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
        assert_eq!(a.mode, Mode::Password);
        // Keys bypass the keymap: q and j are characters.
        type_str(&mut a, "pqj");
        a.handle_key(key(KeyCode::Backspace));
        type_str(&mut a, "w");
        match a.handle_key(key(KeyCode::Enter)) {
            Effect::Connect(p, Some(pw)) => {
                assert_eq!((p.name.as_str(), pw.as_str()), ("a", "pqw"))
            }
            e => panic!("expected connect, got {e:?}"),
        }
        assert_eq!(a.mode, Mode::Browse);
        assert!(a.password.is_none(), "input cleared");
        assert_eq!(a.server.as_ref().map(|s| s.name.as_str()), Some("a"));
    }

    #[test]
    fn password_esc_returns_to_picker() {
        let mut a = picker_app(vec![profile("a", PasswordSource::Prompt)]);
        a.handle_key(key(KeyCode::Enter));
        type_str(&mut a, "secret");
        assert!(matches!(a.handle_key(key(KeyCode::Esc)), Effect::None));
        assert_eq!(a.mode, Mode::Picker);
        assert!(a.password.is_none(), "input dropped");
        // Without any profile (ad-hoc launch) there is no picker to go back to.
        let mut a = App::new(
            Launch::Connect(profile("adhoc", PasswordSource::Prompt)),
            ProfileStore::default(),
            Settings::default(),
        )
        .unwrap();
        assert!(matches!(a.startup_effect(), Effect::None));
        assert_eq!(a.mode, Mode::Password);
        assert!(matches!(a.handle_key(key(KeyCode::Esc)), Effect::Quit));
    }

    #[test]
    fn keyring_failure_falls_back_to_password_prompt() {
        let lookup = |r: Result<Option<String>>| {
            let mut a = picker_app(vec![]);
            let e = a.begin_connect_with(profile("k", PasswordSource::Keyring), |_| r);
            (a.mode, e)
        };
        let (mode, e) = lookup(Err(anyhow::anyhow!("no secret service")));
        assert_eq!(mode, Mode::Password);
        assert!(matches!(e, Effect::None));
        let (mode, e) = lookup(Ok(None));
        assert_eq!(mode, Mode::Password);
        assert!(matches!(e, Effect::None));
        let (mode, e) = lookup(Ok(Some("pw".into())));
        assert_ne!(mode, Mode::Password);
        assert!(matches!(e, Effect::Connect(_, Some(pw)) if pw == "pw"));
    }

    #[test]
    fn startup_resolves_password_like_the_picker() {
        let mut a = App::new(
            Launch::Connect(profile("p", PasswordSource::Prompt)),
            ProfileStore::default(),
            Settings::default(),
        )
        .unwrap();
        assert!(matches!(a.startup_effect(), Effect::None));
        assert_eq!(a.mode, Mode::Password);
        assert_eq!(a.password.as_ref().unwrap().profile.name, "p");
    }

    #[test]
    fn password_debug_does_not_leak() {
        let mut a = picker_app(vec![profile("a", PasswordSource::Prompt)]);
        a.handle_key(key(KeyCode::Enter));
        type_str(&mut a, "hunter2");
        assert!(!format!("{:?}", a.password).contains("hunter2"));
    }

    #[test]
    fn reset_tree_clears_body_cache() {
        let mut a = browse(&["a"]);
        a.handle_action(Action::Down);
        super::super::message_view::content_len(&a);
        assert!(a.body_cache.is_cached());
        a.reset_tree();
        assert!(!a.body_cache.is_cached());
    }

    #[test]
    fn msg_rate_is_a_ten_second_window() {
        let mut r = MsgRate::default();
        let t0 = Instant::now();
        for _ in 0..50 {
            r.record(t0);
        }
        assert_eq!(r.per_sec(t0), 5.0);
        assert_eq!(r.per_sec(t0 + Duration::from_secs(9)), 5.0);
        assert_eq!(r.per_sec(t0 + Duration::from_secs(11)), 0.0);
    }

    #[test]
    fn effect_debug_redacts_password() {
        let e = Effect::Connect(
            profile("home", PasswordSource::Prompt),
            Some("hunter2".into()),
        );
        let s = format!("{e:?}");
        assert!(!s.contains("hunter2"), "{s}");
        assert_eq!(s, "Connect(home, <redacted>)");
        let e = Effect::Connect(profile("home", PasswordSource::None), None);
        assert!(format!("{e:?}").contains("home"));
    }

    #[test]
    fn p_opens_prefilled_form_and_ctrl_s_publishes_keeping_it_open() {
        let mut a = browse(&["dev/cmnd/POWER"]);
        for _ in 0..2 {
            a.handle_action(Action::Down);
            a.handle_action(Action::Expand);
        }
        a.handle_action(Action::Down);
        assert_eq!(sel(&a), "dev/cmnd/POWER");
        let k = |c, m| KeyEvent::new(c, m);
        assert!(matches!(
            a.handle_key(k(KeyCode::Char('p'), KeyModifiers::NONE)),
            Effect::None
        ));
        assert_eq!(a.mode, Mode::Publish);
        assert_eq!(
            a.publish.as_ref().unwrap().topic.lines()[0],
            "dev/cmnd/POWER"
        );
        // keys go to the form, not the keymap: 'q' is not quit
        a.handle_key(k(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(
            a.handle_key(k(KeyCode::Char('q'), KeyModifiers::NONE)),
            Effect::None
        ));
        match a.handle_key(k(KeyCode::Char('s'), KeyModifiers::CONTROL)) {
            Effect::Publish {
                topic,
                payload,
                qos,
                retain,
            } => {
                assert_eq!(topic, "dev/cmnd/POWER");
                assert_eq!(payload, b"q");
                assert_eq!((qos, retain), (0, false));
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(a.mode, Mode::Publish);
        assert!(a.publish.is_some());
        assert!(matches!(
            a.handle_key(k(KeyCode::Esc, KeyModifiers::NONE)),
            Effect::None
        ));
        assert_eq!(a.mode, Mode::Browse);
        assert!(a.publish.is_none());
        // ctrl-c still quits from the form
        a.handle_action(Action::Publish);
        assert!(matches!(
            a.handle_key(k(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Effect::Quit
        ));
    }

    #[test]
    fn p_without_selection_opens_empty_form() {
        let mut a = browse(&[]);
        a.handle_action(Action::Publish);
        assert_eq!(a.mode, Mode::Publish);
        assert_eq!(a.publish.as_ref().unwrap().topic.lines(), [""]);
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
    fn help_opens_on_question_mark_and_any_key_closes() {
        let mut a = app();
        a.mode = Mode::Browse;
        let k = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        a.handle_key(k('?'));
        assert_eq!(a.mode, Mode::Help);
        // A key that is bound (q = quit) only closes the overlay.
        assert!(matches!(a.handle_key(k('q')), Effect::None));
        assert_eq!(a.mode, Mode::Browse);
        a.handle_key(k('?'));
        assert!(matches!(
            a.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Effect::Quit
        ));
    }
}
