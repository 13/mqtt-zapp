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
                props,
            } => {
                self.remember_sent(&topic, &payload);
                self.await_reply = props
                    .as_ref()
                    .and_then(|p| p.response_topic.clone())
                    .map(|t| (t, Instant::now()));
                Effect::Publish {
                    topic,
                    payload,
                    qos,
                    retain,
                    props,
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

    /// The first message on the response topic of the last send, within
    /// `REPLY_WINDOW`, is selected; later the wait is dropped silently.
    pub(super) fn follow_reply(&mut self, topic: &str) {
        let Some((want, sent)) = &self.await_reply else {
            return;
        };
        if sent.elapsed() > REPLY_WINDOW {
            self.await_reply = None;
        } else if want == topic {
            self.await_reply = None;
            if let Some(id) = self.tree.find(topic) {
                self.select(id);
                self.reveal_selected();
                let note = format!("reply on {}", mqtt_zapp_core::payload::sanitize(topic));
                self.set_note(Note::info(note), Instant::now());
            }
        }
    }

    pub(super) fn open_publish(&mut self) {
        let topic = self
            .selected
            .map(|id| self.tree.full_topic(id))
            .unwrap_or_default();
        let mut note = None;
        let v5 = self
            .session_profile
            .as_ref()
            .is_some_and(|p| p.mqtt_version == mqtt_zapp_core::config::MqttVersion::V5);
        let mut form = PublishForm::new(&topic, v5);
        if self.focus == Focus::History
            && let (Some(id), Some(i)) = (self.selected, self.history_sel)
            && let Some(m) = self.tree.node(id).history.get(i)
        {
            if m.truncated_from.is_some() {
                note = Some("can't re-send: payload was truncated");
            } else if let Ok(text) = std::str::from_utf8(&m.payload) {
                form = PublishForm::prefilled(&topic, text, m.qos, m.retain, v5);
                if m.sparkplug {
                    note = Some("re-sending Sparkplug as JSON");
                }
            } else {
                note = Some("can't re-send: binary payload");
            }
        }
        self.publish = Some(form);
        self.sent_walk = None;
        self.mode = Mode::Publish;
        if let Some(n) = note {
            self.set_note(Note::info(n), Instant::now());
        }
    }
}
