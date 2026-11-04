//! The search bar and the filter it drives.

use super::*;
use mqtt_zapp_core::search;

#[derive(Clone, Copy)]
enum FilterKind {
    Full,
    Narrow,
    Incr,
}

impl App {
    /// Replace the filter; the only way to change it, so the rows cache sees it.
    pub fn set_filter(&mut self, f: Option<Filter>) {
        self.filter = f;
        self.filter_gen += 1;
    }

    /// Keys while typing a search. They bypass the keymap, so `q` and `j`
    /// are just characters.
    pub fn on_search_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        match key.code {
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_regex = !self.search_regex;
                self.schedule_filter();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.search.push(c);
                self.schedule_filter();
            }
            KeyCode::Backspace => {
                self.search.pop();
                self.schedule_filter();
            }
            KeyCode::Enter => {
                if self.filter_due.is_some() {
                    self.rebuild_filter();
                }
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

    pub(super) fn clear_search(&mut self) {
        self.reveal_selected();
        self.search.clear();
        self.search_error = None;
        self.filter_due = None;
        self.set_filter(None);
        // Nothing follows the changes any more; a new filter starts afresh.
        self.tree.take_touched();
    }

    /// Expand every ancestor of the selection, so it stays visible once a
    /// filter (which shows everything expanded) goes away.
    pub(super) fn reveal_selected(&mut self) {
        let mut cur = self.selected.and_then(|s| self.tree.node(s).parent);
        while let Some(id) = cur.filter(|&id| id != TopicTree::ROOT) {
            self.expanded.insert(id);
            cur = self.tree.node(id).parent;
        }
    }

    /// A search key was typed: rebuild now on a small tree, else once typing
    /// pauses for `SEARCH_DEBOUNCE` (the tick runs it).
    pub(super) fn schedule_filter(&mut self) {
        if self.tree.len() < DEBOUNCE_MIN_NODES {
            self.rebuild_filter();
        } else {
            self.filter_due = Some(Instant::now() + SEARCH_DEBOUNCE);
        }
    }

    /// Test counters of the filter updates, in total and by kind.
    #[cfg_attr(not(test), allow(unused_variables))]
    fn count(&self, kind: FilterKind) {
        #[cfg(test)]
        {
            self.filter_builds.set(self.filter_builds.get() + 1);
            let c = match kind {
                FilterKind::Full => &self.filter_full,
                FilterKind::Narrow => &self.filter_narrow,
                FilterKind::Incr => &self.filter_incr,
            };
            c.set(c.get() + 1);
        }
    }

    /// Recompute the filter from `search`: re-test only the old matches when
    /// the query just got longer, else scan everything. Then, if the
    /// selection is not shown, move it to the first match.
    pub(super) fn rebuild_filter(&mut self) {
        self.filter_due = None;
        self.last_filter_rebuild = Instant::now();
        let mode = if self.search_regex {
            search::QueryMode::Regex
        } else {
            search::QueryMode::Plain
        };
        let narrowed = self
            .filter
            .as_ref()
            .and_then(|prev| search::narrow(&self.tree, prev, &self.search, mode));
        match narrowed {
            Some(r) => {
                self.count(FilterKind::Narrow);
                self.apply_filter(r);
            }
            None => self.rebuild_full(),
        }
    }

    /// The tick's update of a live filter: re-test only the nodes that got
    /// messages since, and scan everything when that cannot be trusted.
    pub(super) fn refresh_filter(&mut self) {
        let mode = if self.search_regex {
            search::QueryMode::Regex
        } else {
            search::QueryMode::Plain
        };
        let current = self
            .filter
            .as_ref()
            .is_some_and(|f| f.mode == mode && f.query == self.search.trim());
        if current
            && self.filter_due.is_none()
            && let Some(touched) = self.tree.take_touched()
        {
            let mut f = self.filter.take().expect("checked above");
            if search::update(&self.tree, &mut f, &touched) {
                self.last_filter_rebuild = Instant::now();
                self.count(FilterKind::Incr);
                self.apply_filter(Ok(Some(f)));
                return;
            }
            self.filter = Some(f);
        }
        self.rebuild_full();
    }

    fn rebuild_full(&mut self) {
        self.filter_due = None;
        self.last_filter_rebuild = Instant::now();
        self.count(FilterKind::Full);
        // A full scan sees every change so far.
        self.tree.take_touched();
        self.tree_changed = false;
        let mode = if self.search_regex {
            search::QueryMode::Regex
        } else {
            search::QueryMode::Plain
        };
        let result = search::filter(&self.tree, &self.search, mode);
        self.apply_filter(result);
    }

    fn apply_filter(&mut self, result: Result<Option<Filter>, String>) {
        match result {
            Ok(f) => {
                self.set_filter(f);
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
        let first = self
            .rows()
            .iter()
            .copied()
            .find(|r| f.matches.contains(&r.id));
        if let Some(row) = first {
            self.select(row.id);
        }
    }
}
