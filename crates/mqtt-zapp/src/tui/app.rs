//! UI state machine: everything the TUI shows, updated from key presses and
//! core events. Side effects (connecting, publishing, quitting) are returned
//! as `Effect`s for the event loop to carry out, so `App` stays testable.

use super::expanded::Expanded;
use super::keymap::{Action, Keymap};
use super::picker::{PasswordState, PickerState};
use super::profile_form::{ProfileEdit, ProfileForm, ProfileOutcome};
use super::publish::{Field, FormOutcome, PublishForm};
use super::rows::{Row, visible_rows};
use crate::cli::Launch;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mouse::inner;
use mqtt_zapp_core::client::{ConnState, CoreEvent};
use mqtt_zapp_core::config::{
    DEFAULT_TIMESTAMP_FORMAT, PasswordSource, Profile, ProfileStore, Settings, SortOrder, ThemeName,
};
use mqtt_zapp_core::payload::Props;
use mqtt_zapp_core::search::Filter;
use mqtt_zapp_core::secrets::load_password;
use mqtt_zapp_core::state::AppState;
use mqtt_zapp_core::{NodeId, TopicTree};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

mod actions;
mod chart;
mod front;
mod keys;
mod mouse;
mod publish;
mod retained;
mod search;
mod star;
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
    /// The chart field picker; see `App::field_picker`.
    FieldPicker,
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
        /// MQTT 5 properties (ignored on 3.1.1).
        props: Option<Props>,
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
                props,
            } => f
                .debug_struct("Publish")
                .field("topic", topic)
                .field("payload", &format_args!("<{} bytes>", payload.len()))
                .field("qos", qos)
                .field("retain", retain)
                .field("props", props)
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
/// How long after sending with a response topic the first message on it
/// is followed.
const REPLY_WINDOW: Duration = Duration::from_secs(30);
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
    /// Shown in the header after the URL; see `front::security_warning`.
    pub warning: Option<&'static str>,
}

/// Most series that can stay pinned beside the current chart series.
pub const MAX_PINNED: usize = 3;

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
    /// Remove `topic` and its subtree from the list, clearing what is
    /// retained there on the broker (looked up again on confirm).
    DeleteTopic(String),
}

const STORE_BROKEN: &str = "profiles.toml has errors – fix it by hand first";

/// Minimum gap between tick-driven live-filter rebuilds.
const FILTER_REBUILD_EVERY: Duration = Duration::from_millis(500);

pub struct App {
    pub tree: TopicTree,
    pub expanded: Expanded,
    pub selected: Option<NodeId>,
    /// Set it through `set_filter`, which invalidates the cached rows.
    pub filter: Option<Filter>,
    /// Bumped by `set_filter`; part of the rows cache key.
    filter_gen: u64,
    /// When the filter was last rebuilt; `on_tick` rebuilds at most every
    /// `FILTER_REBUILD_EVERY`.
    last_filter_rebuild: Instant,
    #[cfg(test)]
    filter_builds: Cell<u32>,
    pub search: String,
    /// `ctrl-r` in the search input: the query is a regex.
    pub search_regex: bool,
    /// The current (invalid) regex's error; the last valid filter stays.
    pub search_error: Option<String>,
    pub mode: Mode,
    pub conn_state: ConnState,
    pub dirty: bool,
    /// `f`: only the user's own input redraws, so a terminal selection on
    /// lines that live rows share survives (kitty clears it on any redraw of
    /// one of its lines).
    pub frozen: bool,
    /// The history entry (by stamp) that freezing pinned so `Y` copies the
    /// payload on screen; unfreezing goes back to live if it is still pinned.
    freeze_pin: Option<(std::time::SystemTime, usize)>,
    /// What the help overlay is searching for; empty when none.
    pub help_query: String,
    /// First visible help row; the renderer clamps it, `help_view` tells keys how far.
    pub help_scroll: usize,
    /// (furthest scroll, page height) from the last help draw.
    pub help_view: Cell<(usize, usize)>,
    /// Set when messages arrived since the last tick; gates the filter rebuild.
    tree_changed: bool,
    pub msg_rate: MsgRate,
    /// Messages the core dropped because this UI fell behind.
    pub dropped: u64,
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
    /// Payloads are cut to this many bytes on arrival (`max_payload_kb`
    /// 0: `usize::MAX`, no cap).
    max_payload_bytes: usize,
    max_topics: usize,
    /// New nodes at this depth or less start expanded.
    auto_expand: usize,
    /// Dim topics silent this many seconds (0: off).
    pub stale_after_s: u64,
    pending_connect: Option<Profile>,
    pub picker: PickerState,
    /// Set while in `Mode::Password`.
    pub password: Option<PasswordState>,
    /// The broker last connected to, shown in the header.
    pub server: Option<ServerInfo>,
    /// The saved profile the session is on, marked in the picker; `None`
    /// for an ad-hoc `--host` session. Follows renames, cleared on delete.
    pub connected: Option<String>,
    /// The profile exactly as the session connected with it: enter on the
    /// connected row only skips the reconnect while it is unchanged.
    session_profile: Option<Profile>,
    /// Topics whose retained message was just cleared by a delete: the
    /// broker's empty echo is dropped once, so the topic stays gone.
    cleared_echo: HashSet<String>,
    /// Why `profiles.toml` could not be loaded; shown in the empty picker.
    pub store_error: Option<String>,
    /// Open while in `Mode::Publish`.
    pub publish: Option<PublishForm>,
    /// A transient footer message (publish result) and when it was set.
    pub status_note: Option<(Note, Instant)>,
    /// The chart: `None` off, `Some(None)` the plain numeric payload,
    /// `Some(Some(path))` a JSON field. Reset when another topic is selected.
    pub chart: Option<Option<String>>,
    /// Series pinned with `C` to stay charted beside the current one, oldest
    /// first. Cleared with `chart` on selecting another topic, not by
    /// turning the chart off.
    pub chart_pinned: Vec<Option<String>>,
    /// Colour theme; `tui::run` applies `NO_COLOR`.
    pub theme: ThemeName,
    /// Validated strftime format for message times.
    pub timestamp_format: String,
    /// Recently published payloads per topic, newest first (at most 20).
    pub sent: HashMap<String, VecDeque<String>>,
    /// While walking `sent` in the form: the index shown and the draft to
    /// restore past the newest.
    sent_walk: Option<(usize, String)>,
    /// The response topic of the last send and when it was sent: the first
    /// message on it within `REPLY_WINDOW` is selected.
    await_reply: Option<(String, Instant)>,
    /// Order of sibling topics; `s` cycles it.
    pub sort: SortOrder,
    /// Set while in `Mode::Confirm`.
    pub confirm: Option<Confirm>,
    /// Open while in `Mode::ProfileForm`.
    pub profile_form: Option<ProfileForm>,
    /// Open while in `Mode::FieldPicker`.
    pub field_picker: Option<super::field_picker::FieldPicker>,
    /// Pane rectangles of the last frame, for mouse hit-testing.
    pub hit: Cell<HitAreas>,
    /// Whether the last frame had room for the history box without forcing
    /// it open; Tab only visits it then.
    pub history_shown: Cell<bool>,
    /// The rows the last frame drew, so mouse clicks hit what was on screen.
    drawn_rows: RefCell<Option<Rc<[Row]>>>,
    /// When this `App` was made: Count and Recent rows refresh once a
    /// second counted from here.
    created: Instant,
    /// The last `rows()` result and what it was built from.
    rows_cache: RefCell<Option<(RowsKey, Rc<[Row]>)>>,
    /// Starred topics per profile name; persisted in `state.toml`.
    favorites: BTreeMap<String, BTreeSet<String>>,
    /// Show only starred topics (and their ancestors).
    pub starred_only: bool,
    /// Bumped when `favorites` changes, for the rows cache.
    star_gen: u64,
}

/// Everything `visible_rows` depends on, cheaply comparable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowsKey {
    shape: u64,
    expanded: u64,
    filter: u64,
    /// `Some(star_gen)` while the starred-only view is on.
    star: Option<u64>,
    sort: SortOrder,
    /// `None` for Name sort; else whole seconds since `App::created`, so
    /// count and recency orders refresh at most once a second.
    order_second: Option<u64>,
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
                settings.max_topics,
            ),
            expanded: Expanded::default(),
            selected: None,
            filter: None,
            filter_gen: 0,
            last_filter_rebuild: Instant::now(),
            #[cfg(test)]
            filter_builds: Cell::new(0),
            search: String::new(),
            search_regex: false,
            search_error: None,
            mode,
            conn_state,
            dirty: true,
            frozen: false,
            freeze_pin: None,
            help_query: String::new(),
            help_scroll: 0,
            help_view: Cell::new((0, 1)),
            tree_changed: false,
            msg_rate: MsgRate::default(),
            dropped: 0,
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
            max_payload_bytes: match settings.max_payload_kb {
                0 => usize::MAX,
                kb => kb.saturating_mul(1024),
            },
            max_topics: settings.max_topics,
            auto_expand: settings.auto_expand,
            stale_after_s: settings.stale_after_s,
            pending_connect,
            picker: PickerState::new(store.profiles, &Default::default()),
            password: None,
            server: None,
            connected: None,
            session_profile: None,
            cleared_echo: HashSet::new(),
            store_error: None,
            publish: None,
            status_note: None,
            hit: Cell::new(HitAreas::default()),
            history_shown: Cell::new(false),
            drawn_rows: RefCell::new(None),
            confirm: None,
            field_picker: None,
            profile_form: None,
            sort: settings.sort,
            chart: None,
            chart_pinned: Vec::new(),
            sent: HashMap::new(),
            theme: settings.theme,
            timestamp_format: valid_time_format(&settings.timestamp_format),
            sent_walk: None,
            await_reply: None,
            created: Instant::now(),
            rows_cache: RefCell::new(None),
            favorites: BTreeMap::new(),
            starred_only: false,
            star_gen: 0,
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
        self.tree =
            TopicTree::with_limits(self.history_depth, self.max_history_bytes, self.max_topics);
        self.expanded.clear();
        self.chart_cache.clear();
        self.chart_pinned.clear();
        self.selected = None;
        self.history_sel = None;
        self.diff_base = None;
        self.payload_scroll = 0;
        self.set_filter(None);
        self.starred_only = false;
        // The new tree starts its `shape_gen` over: never reuse old rows.
        self.rows_cache.take();
        // Picker frames don't draw the tree: a click before the next tree
        // frame must not hit the old tree's rows.
        self.drawn_rows.take();
        self.search.clear();
        self.tree_offset = 0;
        self.msg_rate = MsgRate::default();
        self.dropped = 0;
        self.body_cache.clear();
        self.cleared_echo.clear();
        self.dirty = true;
    }

    /// Apply a broker event. While frozen the tree still changes, but only
    /// the user's own input asks for a redraw.
    pub fn apply_core_event(&mut self, ev: CoreEvent) {
        let dirty = self.dirty;
        self.apply_core_event_inner(ev);
        if self.frozen {
            self.dirty = dirty;
        }
    }

    fn apply_core_event_inner(&mut self, ev: CoreEvent) {
        match ev {
            CoreEvent::State(s) => self.conn_state = s,
            CoreEvent::Dropped(n) => self.dropped += n,
            CoreEvent::Message { topic, mut msg } => {
                msg.truncate_to(self.max_payload_bytes);
                if self.cleared_echo.remove(&topic) && msg.payload.is_empty() {
                    return;
                }
                // The paused entry, by stamp: an insert anywhere can evict
                // the selected topic's oldest entries (depth or memory cap).
                let stamp = |m: &mqtt_zapp_core::Message| (m.received_at, m.payload.len());
                let paused = self
                    .selected
                    .zip(self.history_sel)
                    .and_then(|(s, i)| self.tree.node(s).history.get(i).map(stamp));
                let first_new = self.tree.len();
                if self.tree.insert(&topic, msg).is_none() {
                    // Refused at `max_topics`: only the header count changed.
                    self.dirty = true;
                    return;
                }
                // Only nodes this message created; a hand-collapsed one stays.
                for new in first_new..self.tree.len() {
                    if self.depth(new) <= self.auto_expand {
                        self.expanded.insert(new);
                    }
                }
                if let Some(sel) = self.selected {
                    let h = &self.tree.node(sel).history;
                    // Keep a frozen selection on its entry (or the oldest).
                    if let Some(p) = paused {
                        self.history_sel = Some(h.iter().position(|m| stamp(m) == p).unwrap_or(0));
                    }
                    // The marked entry fell out of the ring.
                    if self
                        .diff_base
                        .is_some_and(|b| !h.iter().any(|m| stamp(m) == b))
                    {
                        self.diff_base = None;
                    }
                }
                self.follow_reply(&topic);
                self.msg_rate.record(Instant::now());
                self.tree_changed = true;
            }
        }
        self.dirty = true;
    }

    /// Called every UI tick (100 ms): refresh the live filter so new messages
    /// show up in it, and keep redrawing while something is flashing. While
    /// frozen nothing here asks for a redraw; input and resizes still do.
    pub fn on_tick(&mut self) {
        let dirty = self.dirty;
        self.tick();
        if self.frozen {
            self.dirty = dirty;
        }
    }

    fn tick(&mut self) {
        // Throttled: a busy broker changes the tree every tick, and a rebuild
        // scans every node. A skipped change stays pending in `tree_changed`.
        // Frozen: a rebuild could move the selection off the shown topic,
        // so the change waits for the thaw.
        if self.tree_changed
            && !self.frozen
            && (self.search.is_empty()
                || self.last_filter_rebuild.elapsed() >= FILTER_REBUILD_EVERY)
        {
            self.tree_changed = false;
            if !self.search.is_empty() {
                self.rebuild_filter();
            }
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

    /// The terminal changed size: always redraw, frozen or not.
    pub fn on_resize(&mut self) {
        self.dirty = true;
    }

    /// A note from a background task (publish or clear result): stored like
    /// `set_note`, but a frozen display shows it at the next input-driven draw.
    pub fn set_background_note(&mut self, note: Note, now: Instant) {
        let dirty = self.dirty;
        self.set_note(note, now);
        if self.frozen {
            self.dirty = dirty;
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

    /// The rows the last frame drew (or `rows()` before any frame).
    pub(super) fn drawn_rows(&self) -> Rc<[Row]> {
        self.drawn_rows
            .borrow()
            .clone()
            .unwrap_or_else(|| self.rows())
    }

    /// Record the rows this frame drew, for the next mouse click.
    pub(super) fn set_drawn_rows(&self, rows: &Rc<[Row]>) {
        *self.drawn_rows.borrow_mut() = Some(Rc::clone(rows));
    }

    /// The tree pane's rows, cached until the tree's shape, the expanded
    /// set, the filter or the sort changes (Count and Recent: or a second
    /// passes). Cheap to call per frame and per key.
    pub fn rows(&self) -> Rc<[Row]> {
        let key = RowsKey {
            shape: self.tree.shape_gen(),
            expanded: self.expanded.generation(),
            filter: self.filter_gen,
            star: self.starred_only.then_some(self.star_gen),
            sort: self.sort,
            order_second: match self.sort {
                SortOrder::Name => None,
                SortOrder::Count | SortOrder::Recent => Some(self.created.elapsed().as_secs()),
            },
        };
        if let Some((k, rows)) = &*self.rows_cache.borrow()
            && *k == key
        {
            return Rc::clone(rows);
        }
        let only = self.starred_only.then(|| self.starred_visible());
        let rows: Rc<[Row]> = visible_rows(
            &self.tree,
            self.expanded.as_set(),
            self.filter.as_ref(),
            only.as_ref(),
            self.sort,
        )
        .into();
        *self.rows_cache.borrow_mut() = Some((key, Rc::clone(&rows)));
        rows
    }

    /// Expand (or collapse) the selected node and every node below it.
    fn expand_subtree(&mut self, expand: bool) {
        // Like a filter, the starred view forces its own expansion.
        if self.starred_only {
            return;
        }
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
                text: mqtt_zapp_core::payload::clipboard_safe(
                    &text,
                    mqtt_zapp_core::payload::CLIPBOARD_MAX,
                ),
            },
            None => {
                self.set_note(Note::info("nothing to copy"), Instant::now());
                Effect::None
            }
        }
    }

    /// `f`. Freezing a live view pins the shown entry, so actions on the
    /// selection (`Y`) see what is on screen while messages keep landing.
    fn toggle_freeze(&mut self) {
        self.frozen = !self.frozen;
        let stamp = |m: &mqtt_zapp_core::Message| (m.received_at, m.payload.len());
        if self.frozen {
            self.freeze_pin = None;
            if self.history_sel.is_none() && self.history_len() > 0 {
                self.history_sel = Some(self.history_len() - 1);
                self.freeze_pin = super::message_view::shown_message(self).map(stamp);
            }
        } else if let Some(pin) = self.freeze_pin.take() {
            // Undo our own pin, also when its entry fell out of the ring;
            // keep one the user moved to meanwhile.
            let still_ours = super::message_view::shown_message(self).map(stamp) == Some(pin)
                || self
                    .selected
                    .is_some_and(|s| !self.tree.node(s).history.iter().any(|m| stamp(m) == pin));
            if still_ours && self.history_sel.is_some() {
                self.history_sel = None;
                self.payload_scroll = 0;
            }
        }
    }

    fn history_len(&self) -> usize {
        self.selected
            .map_or(0, |id| self.tree.node(id).history.len())
    }

    /// `j` in the history list (down the screen): towards older entries.
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

    /// `k` in the history list (up the screen): towards newer entries; the
    /// newest entry is live.
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
        self.rows().iter().copied().find(|r| r.id == id)
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
            self.freeze_pin = None;
            self.chart = None;
            self.chart_pinned.clear();
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
        if row.expanded && row.has_children && self.filter.is_none() && !self.starred_only {
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
