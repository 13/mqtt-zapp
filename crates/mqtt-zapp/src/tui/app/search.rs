//! The search bar and the filter it drives.

use super::*;
use mqtt_zapp_core::search;

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

    pub(super) fn clear_search(&mut self) {
        self.reveal_selected();
        self.search.clear();
        self.search_error = None;
        self.set_filter(None);
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

    /// Recompute the filter from `search`; if the selection is not shown,
    /// move it to the first match.
    pub(super) fn rebuild_filter(&mut self) {
        self.last_filter_rebuild = Instant::now();
        #[cfg(test)]
        self.filter_builds.set(self.filter_builds.get() + 1);
        let mode = if self.search_regex {
            search::QueryMode::Regex
        } else {
            search::QueryMode::Plain
        };
        match search::filter(&self.tree, &self.search, mode) {
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
