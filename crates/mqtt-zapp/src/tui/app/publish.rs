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
        form.sent_pos = None;
        match form.on_key(key) {
            FormOutcome::Continue => Effect::None,
            FormOutcome::Edit(text) => Effect::EditPayload(text),
            FormOutcome::Leave => {
                self.leave_publish();
                Effect::None
            }
            FormOutcome::Cancel => {
                // Kept for the next form on this topic, unless blank or
                // just sent as is.
                let sent = &self.sent;
                if let Some(form) = self.publish.take().filter(|f| {
                    let last = sent.get(&f.topic_text()).and_then(|l| l.front());
                    !f.is_blank() && last != Some(&f.payload_text())
                }) {
                    self.publish_draft = Some(form);
                }
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
                // Refused here, not after: an offline send is not "sent".
                if !self.conn_state.is_connected() {
                    if let Some(form) = self.publish.as_mut() {
                        form.error = Some("not connected".into());
                    }
                    return Effect::None;
                }
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

    /// A left click in the form: focus a field or set an option.
    pub(super) fn publish_click(&mut self, at: ratatui::layout::Position) {
        if let Some(form) = self.publish.as_mut()
            && form.click(at)
        {
            self.sent_walk = None;
            form.sent_pos = None;
            self.dirty = true;
        }
    }

    /// ctrl-o: back to the topics; the form stays open, keys go to browse.
    pub(super) fn leave_publish(&mut self) {
        self.sent_walk = None;
        if let Some(form) = self.publish.as_mut() {
            form.sent_pos = None;
        }
        self.mode = Mode::Browse;
        self.focus = Focus::Tree;
        self.dirty = true;
    }

    /// Give the keys back to the form left open with ctrl-o.
    pub(super) fn enter_publish(&mut self) {
        if self.publish.is_some() {
            self.mode = Mode::Publish;
            self.dirty = true;
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
        let topic = form.topic_text();
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
        form.sent_pos = next.as_ref().map(|(i, _)| (i + 1, list.len()));
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
                // The starred view or the filter may hide it: note, no jump.
                // (Both force their own expansion, so absence means hidden;
                // otherwise `reveal_selected` unfolds the ancestors.)
                let hidden = (self.starred_only || self.filter.is_some())
                    && !self.rows().iter().any(|r| r.id == id);
                if !hidden {
                    self.select(id);
                    self.reveal_selected();
                }
                let note = format!("reply on {}", mqtt_zapp_core::payload::sanitize(topic));
                self.set_note(Note::info(note), Instant::now());
            }
        }
    }

    pub(super) fn open_publish(&mut self) {
        let resend = self.focus == Focus::History && self.history_sel.is_some();
        // A form left open comes back as it was; a re-send replaces it.
        if self.publish.is_some() && !resend {
            return self.enter_publish();
        }
        let topic = self
            .selected
            .map(|id| self.tree.full_topic(id))
            .unwrap_or_default();
        let v5 = self
            .session_profile
            .as_ref()
            .is_some_and(|p| p.mqtt_version == mqtt_zapp_core::config::MqttVersion::V5);
        let mut note = None;
        let mut form = None;
        if resend && let Some(m) = crate::tui::message_view::shown_message(self) {
            match resend_text(m) {
                Ok(text) => {
                    form = Some(PublishForm::prefilled(&topic, text, m.qos, m.retain, v5));
                    if m.sparkplug {
                        note = Some("re-sending Sparkplug as JSON");
                    }
                }
                Err(why) => note = Some(why),
            }
        }
        match form {
            Some(new) => {
                // The form it replaces is kept as the draft for its topic.
                if let Some(old) = self.publish.take().filter(|f| !f.is_blank()) {
                    self.publish_draft = Some(old);
                }
                self.publish = Some(new);
            }
            // A refused re-send leaves an open form as it was.
            None if self.publish.is_some() => {}
            None => {
                let mut new = PublishForm::new(&topic, v5);
                if !resend
                    && let Some(draft) = self
                        .publish_draft
                        .take_if(|d| d.topic_text() == topic && d.v5 == v5)
                {
                    new = draft;
                    new.error = None;
                    note = Some("draft restored");
                }
                self.publish = Some(new);
            }
        }
        self.sent_walk = None;
        self.mode = Mode::Publish;
        self.dirty = true;
        if let Some(n) = note {
            self.set_note(Note::info(n), Instant::now());
        }
    }
}

/// The text of `m` for the form, or why it can't be re-sent exactly.
fn resend_text(m: &mqtt_zapp_core::Message) -> Result<&str, &'static str> {
    if m.truncated_from.is_some() {
        return Err("can't re-send: payload was truncated");
    }
    let text = std::str::from_utf8(&m.payload).map_err(|_| "can't re-send: binary payload")?;
    // The form could not show it exactly, so it would not send it exactly.
    if text.chars().any(mqtt_zapp_core::payload::is_unsafe_char) {
        return Err("can't re-send: control characters");
    }
    Ok(text)
}
