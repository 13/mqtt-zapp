//! UI state machine: everything the TUI shows, updated from key presses and
//! core events. Side effects (connecting, publishing, quitting) are returned
//! as `Effect`s for the event loop to carry out, so `App` stays testable.

use super::keymap::{Action, Keymap};
use super::rows::{Row, visible_rows};
use crate::cli::Launch;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mqtt_zapp_core::client::{ConnState, CoreEvent};
use mqtt_zapp_core::config::{Profile, ProfileStore, Settings};
use mqtt_zapp_core::search::{self, Filter};
use mqtt_zapp_core::{NodeId, TopicTree};
use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// How long a node stays highlighted after it (or its subtree) got a message.
pub const FLASH: Duration = Duration::from_secs(1);
const RATE_WINDOW: Duration = Duration::from_secs(10);
const RATE_BUCKET: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Picker,
    #[allow(dead_code)] // TODO(task 11): password prompt
    Password,
    Browse,
    SearchInput,
    #[allow(dead_code)] // TODO(task 12): publish form
    Publish,
    #[allow(dead_code)] // TODO(task 11): help overlay
    Help,
}

/// Which pane the j/k keys drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    History,
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // short-lived, consumed right away
pub enum Effect {
    None,
    Quit,
    #[allow(dead_code)] // TODO(task 12): built by the publish form
    Publish {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    },
    Connect(Profile, Option<String>),
}

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
    keymap: Keymap,
    history_depth: usize,
    pending_connect: Option<Profile>,
    #[allow(dead_code)] // TODO(task 11): the picker lists these profiles
    store: ProfileStore,
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
            store,
        })
    }

    /// The effect to run once at startup: connect when launched with a profile.
    pub fn startup_effect(&mut self) -> Effect {
        match self.pending_connect.take() {
            Some(p) => Effect::Connect(p, None),
            None => Effect::None,
        }
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
        // ROOT is touched by every insert, so it tells whether any row flashes.
        let flashing = self.is_flashing(TopicTree::ROOT, now);
        // The msg/s figure in the status bar decays without new messages.
        if flashing || self.msg_rate.per_sec(now) > 0.0 {
            self.dirty = true;
        }
    }

    pub fn is_flashing(&self, id: NodeId, now: Instant) -> bool {
        self.tree
            .node(id)
            .last_update
            .is_some_and(|t| now.saturating_duration_since(t) < FLASH)
    }

    pub fn rows(&self) -> Vec<Row> {
        visible_rows(&self.tree, &self.expanded, self.filter.as_ref())
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
            // Placeholder until the picker exists: only leaving is possible.
            Mode::Picker => match self.keymap.lookup(key) {
                Some(Action::Quit | Action::Cancel) => Effect::Quit,
                _ => Effect::None,
            },
            Mode::SearchInput => {
                self.on_search_key(key);
                Effect::None
            }
            _ => Effect::None,
        }
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
            KeyCode::Enter => self.mode = Mode::Browse,
            KeyCode::Esc => {
                self.clear_search();
                self.mode = Mode::Browse;
            }
            _ => {}
        }
    }

    fn clear_search(&mut self) {
        self.search.clear();
        self.filter = None;
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
            Action::Up => self.move_selection(|i, _| i.saturating_sub(1)),
            Action::Down => self.move_selection(|i, n| (i + 1).min(n - 1)),
            Action::FocusNext => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::History,
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
            Action::ToggleExpand => {
                if let Some(row) = self.selected_row().filter(|r| r.has_children)
                    && !self.expanded.remove(&row.id)
                {
                    self.expanded.insert(row.id);
                }
            }
            Action::Collapse => self.collapse(),
            Action::Search => self.mode = Mode::SearchInput,
            Action::Cancel => self.clear_search(),
            // TODO(tasks 11-12): publish, help.
            Action::Publish | Action::Select | Action::Help => {}
        }
        Effect::None
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
        let height = self.payload_height.get();
        let half = usize::from((height / 2).max(1));
        let max = super::message_view::content_len(self).saturating_sub(height as usize);
        self.payload_scroll = if down {
            self.payload_scroll.saturating_add(half).min(max)
        } else {
            self.payload_scroll.saturating_sub(half)
        };
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
    use mqtt_zapp_core::config::{ProfileStore, Settings};
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
}
