//! The publish form: a topic line, a multi-line payload, QoS and retain.
//! State and key handling live here; the overlay is drawn by `render`.

use super::theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::payload::{Props, sanitize};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use tui_textarea::TextArea;

const INVALID_TOPIC: &str = "invalid topic";
pub const HINT: &str =
    "ctrl-s send  ctrl-e editor  ctrl-p/n history  ctrl-f json  tab next  esc close";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Topic,
    Payload,
    /// MQTT 5 only, like the next two.
    ContentType,
    ResponseTopic,
    UserProps,
    Qos,
    Retain,
}

impl Field {
    const ORDER: [Field; 7] = [
        Field::Topic,
        Field::Payload,
        Field::ContentType,
        Field::ResponseTopic,
        Field::UserProps,
        Field::Qos,
        Field::Retain,
    ];

    fn is_v5_only(self) -> bool {
        matches!(
            self,
            Field::ContentType | Field::ResponseTopic | Field::UserProps
        )
    }

    /// The next field `delta` steps away among those shown.
    fn step(self, delta: isize, v5: bool) -> Field {
        let shown: Vec<Field> = Self::ORDER
            .into_iter()
            .filter(|f| v5 || !f.is_v5_only())
            .collect();
        let i = shown.iter().position(|f| *f == self).unwrap_or(0) as isize;
        shown[(i + delta).rem_euclid(shown.len() as isize) as usize]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FormOutcome {
    Continue,
    Cancel,
    /// Open the payload (given) in `$EDITOR`.
    Edit(String),
    Send {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
        /// MQTT 5 properties; `None` when none are set.
        props: Option<Props>,
    },
}

pub struct PublishForm {
    pub topic: TextArea<'static>,
    pub payload: TextArea<'static>,
    pub qos: u8,
    pub retain: bool,
    /// MQTT 5 session: the property fields are shown.
    pub v5: bool,
    pub content_type: TextArea<'static>,
    pub response_topic: TextArea<'static>,
    /// `k=v, k2=v2`.
    pub user_props: TextArea<'static>,
    pub field: Field,
    /// Shown in the form's title; cleared by the next key press.
    pub error: Option<String>,
}

impl PublishForm {
    /// `prefill_topic` is used verbatim so the form publishes to exactly the
    /// selected topic. Rendering goes through `topic_line`, which sanitizes
    /// per char, so control characters never reach the terminal.
    pub fn new(prefill_topic: &str, v5: bool) -> PublishForm {
        let mut topic = TextArea::new(vec![prefill_topic.to_owned()]);
        topic.move_cursor(tui_textarea::CursorMove::End);
        let mut form = PublishForm {
            topic,
            payload: TextArea::default(),
            qos: 0,
            retain: false,
            v5,
            content_type: TextArea::default(),
            response_topic: TextArea::default(),
            user_props: TextArea::default(),
            field: Field::Topic,
            error: None,
        };
        form.sync_focus();
        form
    }

    /// A form with the payload, QoS and retain of a message to re-send.
    pub fn prefilled(topic: &str, payload: &str, qos: u8, retain: bool, v5: bool) -> PublishForm {
        let mut form = PublishForm::new(topic, v5);
        form.set_payload(payload);
        form.qos = qos.min(2);
        form.retain = retain;
        form.field = Field::Topic;
        form.sync_focus();
        form
    }

    pub fn on_key(&mut self, key: KeyEvent) -> FormOutcome {
        self.error = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return FormOutcome::Cancel,
            KeyCode::Char('s') if ctrl => return self.send(),
            KeyCode::Char('e') if ctrl => return FormOutcome::Edit(self.payload_text()),
            KeyCode::Char('f') if ctrl => {
                if let Err(e) = self.format_json() {
                    self.error = Some(e);
                }
            }
            KeyCode::Tab => self.focus(1),
            KeyCode::BackTab => self.focus(-1),
            _ => match self.field {
                Field::Topic => match key.code {
                    KeyCode::Enter => self.focus(1),
                    // Ctrl-m would insert a newline into the single-line topic.
                    KeyCode::Char('m') if ctrl => {}
                    _ => {
                        self.topic.input(key);
                    }
                },
                Field::Payload => {
                    self.payload.input(key);
                }
                Field::ContentType | Field::ResponseTopic | Field::UserProps => {
                    if key.code == KeyCode::Enter {
                        self.focus(1);
                    } else if !(ctrl && key.code == KeyCode::Char('m')) {
                        self.prop_area(self.field).input(key);
                    }
                }
                Field::Qos => {
                    if let KeyCode::Char(c @ '0'..='2') = key.code {
                        self.qos = c as u8 - b'0';
                    }
                }
                Field::Retain => {
                    if key.code == KeyCode::Char(' ') {
                        self.retain = !self.retain;
                    }
                }
            },
        }
        FormOutcome::Continue
    }

    /// The payload as one string, lines joined by `\n`.
    pub fn payload_text(&self) -> String {
        self.payload.lines().join("\n")
    }

    /// Replace the payload text (from the editor or the history).
    pub fn set_payload(&mut self, text: &str) {
        let mut ta = TextArea::new(text.split('\n').map(str::to_owned).collect());
        ta.move_cursor(tui_textarea::CursorMove::Bottom);
        ta.move_cursor(tui_textarea::CursorMove::End);
        self.payload = ta;
        self.sync_focus();
    }

    fn prop_area(&mut self, field: Field) -> &mut TextArea<'static> {
        match field {
            Field::ContentType => &mut self.content_type,
            Field::ResponseTopic => &mut self.response_topic,
            _ => &mut self.user_props,
        }
    }

    fn focus(&mut self, delta: isize) {
        self.field = self.field.step(delta, self.v5);
        self.sync_focus();
    }

    /// Only the focused text area shows a cursor.
    fn sync_focus(&mut self) {
        let shown = Style::default().add_modifier(Modifier::REVERSED);
        let hidden = Style::default();
        self.topic.set_cursor_style(if self.field == Field::Topic {
            shown
        } else {
            hidden
        });
        self.payload
            .set_cursor_style(if self.field == Field::Payload {
                shown
            } else {
                hidden
            });
        for (field, area) in [
            (Field::ContentType, &mut self.content_type),
            (Field::ResponseTopic, &mut self.response_topic),
            (Field::UserProps, &mut self.user_props),
        ] {
            area.set_cursor_style(if self.field == field { shown } else { hidden });
            area.set_cursor_line_style(Style::default());
        }
        self.topic.set_cursor_line_style(Style::default());
        self.payload.set_cursor_line_style(Style::default());
    }

    fn send(&mut self) -> FormOutcome {
        // More than one line (e.g. a paste) is refused, not concatenated.
        let topic = self.topic.lines().join("");
        if self.topic.lines().len() > 1 || topic.is_empty() || topic.contains(['+', '#']) {
            self.error = Some(INVALID_TOPIC.into());
            return FormOutcome::Continue;
        }
        let props = match self.props() {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return FormOutcome::Continue;
            }
        };
        FormOutcome::Send {
            topic,
            payload: self.payload.lines().join("\n").into_bytes(),
            qos: self.qos,
            retain: self.retain,
            props,
        }
    }

    /// The MQTT 5 properties typed in the form; `None` on 3.1.1 or when
    /// every field is empty.
    fn props(&self) -> Result<Option<Props>, String> {
        if !self.v5 {
            return Ok(None);
        }
        let text = |a: &TextArea| a.lines().join("").trim().to_owned();
        let some = |s: String| (!s.is_empty()).then_some(s);
        let mut user = Vec::new();
        for item in text(&self.user_props).split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            match item.split_once('=') {
                Some((k, v)) if !k.trim().is_empty() => {
                    user.push((k.trim().to_owned(), v.trim().to_owned()))
                }
                _ => return Err("user props: expected k=v".into()),
            }
        }
        let props = Props {
            content_type: some(text(&self.content_type)),
            response_topic: some(text(&self.response_topic)),
            user,
            ..Props::default()
        };
        Ok((props != Props::default()).then_some(props))
    }

    /// Pretty-print the payload as JSON. On a parse error the payload is left
    /// as typed.
    pub fn format_json(&mut self) -> Result<(), String> {
        let text = self.payload.lines().join("\n");
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}"))?;
        let pretty = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
        self.payload = TextArea::new(pretty.lines().map(str::to_owned).collect());
        self.sync_focus();
        Ok(())
    }
}

/// The publish form: a card centred over the main area.
pub fn render(f: &mut Frame, area: Rect, form: &PublishForm) {
    theme::dim_backdrop(f, area);
    let area = theme::centered(area, 80, if form.v5 { 21 } else { 18 });
    f.render_widget(Clear, area);
    let block = match &form.error {
        Some(e) => Block::bordered()
            .border_type(BorderType::Thick)
            .title(Span::styled(
                format!(" publish - {} ", sanitize(e).replace('\n', " ")),
                Style::new().fg(theme::ERR).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::new().fg(theme::ERR)),
        None => theme::pane("publish", true),
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    let props_h = if form.v5 { 3 } else { 0 };
    let [topic, payload, props, opts, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(props_h),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    if form.v5 {
        let rows = Layout::vertical([Constraint::Length(1); 3]).split(props);
        for (row, (label, field, area)) in rows.iter().zip([
            ("content type: ", Field::ContentType, &form.content_type),
            (
                "response topic: ",
                Field::ResponseTopic,
                &form.response_topic,
            ),
            ("user props: ", Field::UserProps, &form.user_props),
        ]) {
            let [l, body] =
                Layout::horizontal([Constraint::Length(16), Constraint::Min(1)]).areas(*row);
            f.render_widget(Paragraph::new(label).style(label_style(form, field)), l);
            f.render_widget(area, body);
        }
    }
    let [label, topic] =
        Layout::horizontal([Constraint::Length(7), Constraint::Min(1)]).areas(topic);
    f.render_widget(
        Paragraph::new("topic: ").style(label_style(form, Field::Topic)),
        label,
    );
    f.render_widget(
        Paragraph::new(topic_line(form, topic.width as usize)),
        topic,
    );
    let block = Block::new()
        .borders(Borders::TOP)
        .title("payload")
        .title_style(label_style(form, Field::Payload));
    let body = block.inner(payload);
    f.render_widget(block, payload);
    f.render_widget(&form.payload, body);
    f.render_widget(Paragraph::new(options_line(form)), opts);
    f.render_widget(
        Paragraph::new(HINT).style(Style::new().fg(Color::DarkGray)),
        hint,
    );
}

/// The topic line, drawn by hand rather than by the text area: the text area
/// hands control characters (a broker topic may contain ESC) to the buffer
/// unfiltered, so each char is sanitized here, one for one, keeping the
/// cursor position. The text itself stays raw, so a send is exact. Chars are
/// skipped from the left so the cursor stays inside `width` columns.
fn topic_line(form: &PublishForm, width: usize) -> Line<'static> {
    let lines = form.topic.lines();
    let single = lines.len() == 1;
    let cursor = (single && form.field == Field::Topic).then(|| form.topic.cursor().1);
    let mut cells: Vec<(String, bool)> = lines
        .join("\u{23ce}")
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let text = match c {
                '\n' | '\r' => "\u{23ce}".to_owned(),
                c => sanitize(&c.to_string()),
            };
            (text, cursor == Some(i))
        })
        .collect();
    if cursor.is_some_and(|c| c >= cells.len()) {
        cells.push((" ".into(), true));
    }
    let widths: Vec<usize> = cells.iter().map(|(t, _)| t.chars().count()).collect();
    let end = cells.iter().position(|(_, cur)| *cur).unwrap_or(0);
    let mut skip = 0;
    let mut used: usize = widths[..(end + 1).min(widths.len())].iter().sum();
    while used > width && skip < end {
        used -= widths[skip];
        skip += 1;
    }
    let spans = cells
        .into_iter()
        .skip(skip)
        .map(|(t, cur)| {
            let style = if cur {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new()
            };
            Span::styled(t, style)
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn label_style(form: &PublishForm, field: Field) -> Style {
    if form.field == field {
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    }
}

/// `QoS: [0] 1 2   Retain: [ ]`; the focused field's label is highlighted.
fn options_line(form: &PublishForm) -> Line<'static> {
    let mut spans = vec![Span::styled("QoS:", label_style(form, Field::Qos))];
    for q in 0..=2u8 {
        let (text, style) = if q == form.qos {
            (format!(" [{q}]"), Style::new().add_modifier(Modifier::BOLD))
        } else {
            (format!(" {q}"), Style::new())
        };
        spans.push(Span::styled(text, style));
    }
    spans.push(Span::raw("  "));
    spans.push(Span::styled(" Retain:", label_style(form, Field::Retain)));
    spans.push(Span::raw(if form.retain { " [x]" } else { " [ ]" }));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }
    fn type_str(f: &mut PublishForm, s: &str) {
        for c in s.chars() {
            let k = if c == '\n' {
                key(KeyCode::Enter)
            } else {
                key(KeyCode::Char(c))
            };
            f.on_key(k);
        }
    }

    #[test]
    fn ctrl_e_returns_edit_outcome() {
        let mut f = PublishForm::new("t", false);
        f.on_key(key(KeyCode::Tab));
        type_str(&mut f, "a");
        f.on_key(key(KeyCode::Enter));
        type_str(&mut f, "b");
        match f.on_key(ctrl('e')) {
            FormOutcome::Edit(text) => assert_eq!(text, "a\nb"),
            _ => panic!("expected edit"),
        }
        f.set_payload("x\ny");
        assert_eq!(f.payload_text(), "x\ny");
    }

    #[test]
    fn prefilled_topic() {
        assert_eq!(
            PublishForm::new("tasmota/cmnd/x/POWER", false)
                .topic
                .lines()[0],
            "tasmota/cmnd/x/POWER"
        );
    }

    #[test]
    fn ctrl_s_sends_multiline_payload() {
        let mut f = PublishForm::new("a/b", false);
        f.on_key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Payload);
        type_str(&mut f, "{\n\"a\":1}");
        assert_eq!(
            f.on_key(ctrl('s')),
            FormOutcome::Send {
                topic: "a/b".into(),
                payload: b"{\n\"a\":1}".to_vec(),
                qos: 0,
                retain: false,
                props: None
            }
        );
    }

    #[test]
    fn enter_in_topic_moves_to_payload() {
        let mut f = PublishForm::new("a/b", false);
        f.on_key(key(KeyCode::Enter));
        assert_eq!(f.field, Field::Payload);
        assert_eq!(f.topic.lines().len(), 1);
    }

    #[test]
    fn qos_and_retain_fields() {
        let mut f = PublishForm::new("a/b", false);
        f.on_key(key(KeyCode::Tab));
        f.on_key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Qos);
        f.on_key(key(KeyCode::Char('1')));
        f.on_key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Retain);
        f.on_key(key(KeyCode::Char(' ')));
        assert_eq!(
            f.on_key(ctrl('s')),
            FormOutcome::Send {
                topic: "a/b".into(),
                payload: vec![],
                qos: 1,
                retain: true,
                props: None
            }
        );
    }

    #[test]
    fn shift_tab_goes_back() {
        let mut f = PublishForm::new("a", false);
        f.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(f.field, Field::Retain);
    }

    #[test]
    fn wildcard_or_empty_topic_rejected_and_error_clears() {
        for t in ["a/#", "a/+/b", ""] {
            let mut f = PublishForm::new(t, false);
            assert_eq!(f.on_key(ctrl('s')), FormOutcome::Continue, "{t}");
            assert_eq!(f.error.as_deref(), Some("invalid topic"), "{t}");
        }
        let mut f = PublishForm::new("a/#", false);
        f.on_key(ctrl('s'));
        f.on_key(key(KeyCode::Backspace));
        f.on_key(key(KeyCode::Backspace));
        assert!(matches!(f.on_key(ctrl('s')), FormOutcome::Send { .. }));
        assert_eq!(f.error, None);
    }

    #[test]
    fn format_json_pretty_prints_or_errors() {
        let mut f = PublishForm::new("a", false);
        f.field = Field::Payload;
        type_str(&mut f, "{\"a\":1}");
        assert!(f.format_json().is_ok());
        assert_eq!(f.payload.lines(), ["{", "  \"a\": 1", "}"]);

        let mut f = PublishForm::new("a", false);
        f.field = Field::Payload;
        type_str(&mut f, "{oops");
        assert!(f.format_json().is_err());
        assert_eq!(f.payload.lines(), ["{oops"]);
        // via the key: error shown, then cleared by a successful action
        f.on_key(ctrl('f'));
        assert!(f.error.is_some());
        f.on_key(ctrl('s'));
        assert_eq!(f.error, None);
    }

    #[test]
    fn control_char_topic_round_trips_exactly() {
        for t in ["a/\tb", "a/\x1b[31mb"] {
            let mut f = PublishForm::new(t, false);
            match f.on_key(ctrl('s')) {
                FormOutcome::Send { topic, .. } => assert_eq!(topic, t),
                o => panic!("{o:?}"),
            }
        }
    }

    #[test]
    fn control_char_topic_renders_without_escape() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let f = PublishForm::new("a/\x1b[2Jb\tc\nd\re", false);
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|fr| render(fr, fr.area(), &f)).unwrap();
        let b = t.backend().buffer().clone();
        assert!(
            b.content
                .iter()
                .all(|c| !c.symbol().contains(['\x1b', '\n', '\r']))
        );
    }

    #[test]
    fn long_topic_scrolls_to_keep_cursor_visible() {
        let long = format!("{}END", "x".repeat(100));
        let f = PublishForm::new(&long, false);
        let line = topic_line(&f, 20);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("END") && text.chars().count() <= 21, "{text}");
        let cur = line
            .spans
            .iter()
            .position(|s| s.style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(cur, Some(line.spans.len() - 1), "cursor at end, visible");
        let used: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
        assert!(used <= 20, "{used}");
    }

    #[test]
    fn multi_line_topic_is_invalid() {
        let mut f = PublishForm::new("a", false);
        f.topic.insert_newline();
        f.topic.insert_str("b");
        assert_eq!(f.on_key(ctrl('s')), FormOutcome::Continue);
        assert_eq!(f.error.as_deref(), Some("invalid topic"));
    }

    #[test]
    fn form_v5_fields_build_props() {
        let mut f = PublishForm::new("a/b", true);
        f.on_key(key(KeyCode::Tab));
        f.on_key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::ContentType);
        type_str(&mut f, "text/plain");
        f.on_key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::ResponseTopic);
        type_str(&mut f, "r/t");
        f.on_key(key(KeyCode::Enter));
        assert_eq!(f.field, Field::UserProps);
        type_str(&mut f, " k = v , k2=a=b");
        match f.on_key(ctrl('s')) {
            FormOutcome::Send { props, .. } => {
                let p = props.expect("props");
                assert_eq!(p.content_type.as_deref(), Some("text/plain"));
                assert_eq!(p.response_topic.as_deref(), Some("r/t"));
                assert_eq!(
                    p.user,
                    [("k".into(), "v".into()), ("k2".into(), "a=b".into())]
                );
            }
            o => panic!("{o:?}"),
        }
        // every field empty: no props
        let mut f = PublishForm::new("a/b", true);
        assert!(matches!(
            f.on_key(ctrl('s')),
            FormOutcome::Send { props: None, .. }
        ));
    }

    #[test]
    fn form_user_props_parse_error() {
        let mut f = PublishForm::new("a/b", true);
        f.field = Field::UserProps;
        type_str(&mut f, "k=v, oops");
        assert_eq!(f.on_key(ctrl('s')), FormOutcome::Continue);
        assert_eq!(f.error.as_deref(), Some("user props: expected k=v"));
    }

    #[test]
    fn form_v311_has_no_prop_fields() {
        let mut f = PublishForm::new("a/b", false);
        f.on_key(key(KeyCode::Tab));
        f.on_key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Qos, "payload is followed by qos");
        assert!(matches!(
            f.on_key(ctrl('s')),
            FormOutcome::Send { props: None, .. }
        ));
    }

    #[test]
    fn esc_cancels() {
        let mut f = PublishForm::new("a", false);
        assert_eq!(f.on_key(key(KeyCode::Esc)), FormOutcome::Cancel);
    }
}
