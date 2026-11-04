//! Favourite topics: `*` stars the selection, `'` shows only starred ones.

use super::*;
use mqtt_zapp_core::payload::sanitize;

impl App {
    /// The favourites key: the connected profile's name (empty before any
    /// connect, which only tests see).
    fn star_profile(&self) -> &str {
        self.session_profile.as_ref().map_or("", |p| &p.name)
    }

    /// Starred topics of the current profile.
    fn current_favorites(&self) -> Option<&BTreeSet<String>> {
        self.favorites.get(self.star_profile())
    }

    pub fn starred_count(&self) -> usize {
        self.current_favorites().map_or(0, BTreeSet::len)
    }

    pub fn is_starred(&self, topic: &str) -> bool {
        self.current_favorites().is_some_and(|f| f.contains(topic))
    }

    /// Flip the star of `topic`; returns whether it is now starred.
    pub fn toggle_star_topic(&mut self, topic: &str) -> bool {
        let profile = self.star_profile().to_owned();
        let set = self.favorites.entry(profile.clone()).or_default();
        let starred = set.insert(topic.to_owned());
        if !starred {
            set.remove(topic);
            if set.is_empty() {
                self.favorites.remove(&profile);
            }
        }
        self.star_gen += 1;
        starred
    }

    /// `*`: star or unstar the selected topic and save at once.
    pub(super) fn toggle_star(&mut self) -> Effect {
        let Some(id) = self.selected else {
            self.set_note(Note::info("nothing to star"), Instant::now());
            return Effect::None;
        };
        let topic = self.tree.full_topic(id);
        let verb = if self.toggle_star_topic(&topic) {
            "starred"
        } else {
            "unstarred"
        };
        self.set_note(
            Note::info(format!("{verb} {}", sanitize(&topic))),
            Instant::now(),
        );
        Effect::SaveState(self.app_state())
    }

    /// `'`: only starred topics and their ancestors.
    pub(super) fn toggle_starred_only(&mut self) {
        if !self.starred_only && self.starred_count() == 0 {
            self.set_note(Note::info("no starred topics"), Instant::now());
            return;
        }
        self.starred_only = !self.starred_only;
        if self.starred_only
            && !self
                .selected
                .is_some_and(|s| self.rows().iter().any(|r| r.id == s))
            && let Some(first) = self.rows().first().copied()
        {
            self.select(first.id);
        }
    }

    /// Starred nodes (those matching the search, if any) and their
    /// ancestors. A favourite that has not arrived this session is skipped.
    pub(super) fn starred_visible(&self) -> HashSet<NodeId> {
        let mut out = HashSet::new();
        for topic in self.current_favorites().into_iter().flatten() {
            let Some(id) = self.tree.find(topic) else {
                continue;
            };
            if self
                .filter
                .as_ref()
                .is_some_and(|f| !f.matches.contains(&id))
            {
                continue;
            }
            let mut cur = Some(id);
            while let Some(n) = cur.filter(|&n| n != TopicTree::ROOT) {
                if !out.insert(n) {
                    break;
                }
                cur = self.tree.node(n).parent;
            }
        }
        out
    }
}
