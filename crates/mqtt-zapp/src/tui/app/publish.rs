//! The publish form and the per-topic sent history.

use super::*;

impl App {
    /// Keys in the publish form; they bypass the keymap. A send leaves the
    /// form open so the user can test repeatedly.
    pub(super) fn on_publish_key(&mut self, key: KeyEvent) -> Effect {
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

    pub(super) fn open_publish(&mut self) {
        let topic = self
            .selected
            .map(|id| self.tree.full_topic(id))
            .unwrap_or_default();
        self.publish = Some(PublishForm::new(&topic));
        self.sent_walk = None;
        self.mode = Mode::Publish;
    }
}
