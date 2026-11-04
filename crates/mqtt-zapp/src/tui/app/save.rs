//! `w` and `W`: write the shown payload or a topic's history to a file.

use super::*;
use chrono::Local;
use mqtt_zapp_core::payload::sanitize;
use mqtt_zapp_core::{Message, export};

impl App {
    /// `w`: the shown payload's raw bytes.
    pub(super) fn save_payload(&mut self) {
        let Some(id) = self.selected else {
            return self.nothing_to_save();
        };
        let Some(m) = crate::tui::message_view::shown_message(self) else {
            return self.nothing_to_save();
        };
        let topic = self.tree.full_topic(id);
        let ext = export::extension(&m.payload);
        let bytes = m.payload.clone();
        self.save(&topic, ext, &bytes);
    }

    /// `W`: the topic's whole history, oldest first, as JSON lines.
    pub(super) fn save_history(&mut self) {
        let Some(id) = self.selected else {
            return self.nothing_to_save();
        };
        let node = self.tree.node(id);
        let msgs: Vec<&Message> = if node.history.is_empty() {
            node.latest().into_iter().collect()
        } else {
            node.history.iter().collect()
        };
        if msgs.is_empty() {
            return self.nothing_to_save();
        }
        let bytes = export::history_jsonl(&msgs);
        let topic = self.tree.full_topic(id);
        self.save(&topic, "jsonl", &bytes);
    }

    fn nothing_to_save(&mut self) {
        self.set_note(Note::info("nothing to save"), Instant::now());
    }

    fn save(&mut self, topic: &str, ext: &str, bytes: &[u8]) {
        let stem = export::file_stem(topic, Local::now());
        let note = match export::write_new(&self.save_dir, &stem, ext, bytes) {
            Ok(p) => Note::info(format!("saved {}", sanitize(&p.display().to_string()))),
            Err(e) => Note::error(format!("save failed: {}", sanitize(&e.to_string()))),
        };
        self.set_note(note, Instant::now());
    }
}
