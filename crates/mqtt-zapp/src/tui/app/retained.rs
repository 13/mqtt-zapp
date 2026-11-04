//! Deleting topics and clearing retained messages, behind a confirm.

use super::*;

impl App {
    /// Only `y` confirms (enter is too easy to hit by reflex); any other key
    /// cancels.
    pub(super) fn on_confirm_key(&mut self, key: KeyEvent) -> Effect {
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
            (KeyCode::Char('y'), Some(c)) => match c.action {
                ConfirmAction::ClearRetained(topics) => Effect::ClearRetained(topics),
                ConfirmAction::DeleteProfile(name) => Effect::DeleteProfile(name),
                ConfirmAction::DeleteTopic { topic, retained } => {
                    self.delete_topic(&topic, &retained)
                }
            },
            _ => {
                self.set_note(Note::info("cancelled"), Instant::now());
                Effect::None
            }
        }
    }

    /// The retained, clearable (`$` topics are the broker's) topics at `id`,
    /// or (`subtree`) at it and everything below.
    fn retained_under(&self, id: NodeId, subtree: bool) -> Vec<String> {
        let mut topics = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = self.tree.node(n);
            let topic = self.tree.full_topic(n);
            if node.retained && !topic.starts_with('$') {
                topics.push(topic);
            }
            if subtree {
                stack.extend(node.child_ids());
            }
        }
        topics
    }

    /// Ask before deleting the selected topic and everything below it:
    /// their retained messages on the broker, then the list entries.
    pub(super) fn ask_delete_topic(&mut self) {
        let Some(id) = self.selected else {
            self.set_note(Note::info("no topic selected"), Instant::now());
            return;
        };
        let topic = self.tree.full_topic(id);
        let retained = self.retained_under(id, true);
        // Offline the clear would fail, and the topic would come back on
        // reconnect; a list-only removal is fine.
        if !retained.is_empty() && !self.conn_state.is_connected() {
            self.set_note(Note::error("not connected"), Instant::now());
            return;
        }
        self.confirm_delete(topic, retained);
    }

    fn confirm_delete(&mut self, topic: String, retained: Vec<String>) {
        let prompt = match retained.len() {
            0 => format!("remove {topic} from the list? (nothing retained on the broker)"),
            n => format!("delete {topic} and clear {n} retained on the broker?"),
        };
        self.confirm = Some(Confirm {
            prompt,
            action: ConfirmAction::DeleteTopic { topic, retained },
        });
        self.mode = Mode::Confirm;
    }

    /// Remove `topic` (and below) from the tree; the broker's echo of each
    /// cleared message (an empty payload) is dropped instead of re-adding it.
    fn delete_topic(&mut self, topic: &str, asked: &[String]) -> Effect {
        let Some(id) = self.tree.find(topic) else {
            return Effect::None;
        };
        // Again now: retained topics may have arrived while the prompt was
        // up. Clearing them unannounced, or leaving them to come back, would
        // both surprise; ask again with the new count.
        let retained = self.retained_under(id, true);
        if retained.iter().any(|t| !asked.contains(t)) {
            self.confirm_delete(topic.to_owned(), retained);
            return Effect::None;
        }
        self.tree.remove(id);
        self.cleared_echo.extend(retained.iter().cloned());
        // Select the nearest ancestor still in the tree, else nothing.
        let levels: Vec<&str> = topic.split('/').collect();
        let parent = (1..levels.len())
            .rev()
            .find_map(|n| self.tree.find(&levels[..n].join("/")));
        match parent {
            Some(p) => self.select(p),
            None => {
                self.selected = None;
                self.chart = None;
                self.chart_pinned.clear();
                self.diff_base = None;
                self.history_sel = None;
                self.payload_scroll = 0;
            }
        }
        if !self.search.is_empty() {
            self.rebuild_filter();
        }
        self.set_note(Note::info(format!("deleted {topic}")), Instant::now());
        if retained.is_empty() {
            Effect::None
        } else {
            Effect::ClearRetained(retained)
        }
    }

    /// Ask before clearing the retained messages of the selected node, or
    /// (`subtree`) of it and everything below.
    pub(super) fn ask_clear_retained(&mut self, subtree: bool) {
        let Some(id) = self.selected else {
            self.set_note(Note::info("no retained messages here"), Instant::now());
            return;
        };
        // `$` topics (e.g. `$SYS`) belong to the broker; clients cannot
        // clear them.
        let topics = self.retained_under(id, subtree);
        if !topics.is_empty() && !self.conn_state.is_connected() {
            self.set_note(Note::error("not connected"), Instant::now());
            return;
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
}
