//! The publish form: a topic line, a multi-line payload, QoS and retain.
//! State and key handling live here; the overlay is drawn by `render`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::payload::sanitize;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tui_textarea::TextArea;

const INVALID_TOPIC: &str = "invalid topic";
pub const HINT: &str = "ctrl-s send  ctrl-f format json  tab next  esc close";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Topic,
    Payload,
    Qos,
    Retain,
}

impl Field {
    const ORDER: [Field; 4] = [Field::Topic, Field::Payload, Field::Qos, Field::Retain];

    fn step(self, delta: isize) -> Field {
        let i = Self::ORDER.iter().position(|f| *f == self).unwrap_or(0) as isize;
        Self::ORDER[(i + delta).rem_euclid(Self::ORDER.len() as isize) as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormOutcome {
    Continue,
    Cancel,
    Send {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    },
}

pub struct PublishForm {
    pub topic: TextArea<'static>,
    pub payload: TextArea<'static>,
    pub qos: u8,
    pub retain: bool,
    pub field: Field,
    /// Shown in the form's title; cleared by the next key press.
    pub error: Option<String>,
}

impl PublishForm {
    /// `prefill_topic` is used verbatim so the form publishes to exactly the
    /// selected topic. Rendering goes through `topic_line`, which sanitizes
    /// per char, so control characters never reach the terminal.
    pub fn new(prefill_topic: &str) -> PublishForm {
        let mut topic = TextArea::new(vec![prefill_topic.to_owned()]);
        topic.move_cursor(tui_textarea::CursorMove::End);
        let mut form = PublishForm {
            topic,
            payload: TextArea::default(),
            qos: 0,
            retain: false,
            field: Field::Topic,
            error: None,
        };
        form.sync_focus();
        form
    }

    pub fn on_key(&mut self, key: KeyEvent) -> FormOutcome {
        self.error = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return FormOutcome::Cancel,
            KeyCode::Char('s') if ctrl => return self.send(),
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

    fn focus(&mut self, delta: isize) {
        self.field = self.field.step(delta);
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
        FormOutcome::Send {
            topic,
            payload: self.payload.lines().join("\n").into_bytes(),
            qos: self.qos,
            retain: self.retain,
        }
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

/// Bottom panel over the main area: about 40% of its height.
pub fn overlay_area(area: Rect) -> Rect {
    let h = (area.height * 2 / 5).max(7).min(area.height);
    Rect::new(area.x, area.y + area.height - h, area.width, h)
}

pub fn render(f: &mut Frame, area: Rect, form: &PublishForm) {
    let area = overlay_area(area);
    f.render_widget(Clear, area);
    let block = match &form.error {
        Some(e) => Block::bordered()
            .title(Span::styled(
                format!("publish - {}", sanitize(e).replace('\n', " ")),
                Style::new().fg(Color::Red),
            ))
            .border_style(Style::new().fg(Color::Red)),
        None => Block::bordered()
            .title("publish")
            .border_style(Style::new().fg(Color::Cyan)),
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [topic, payload, opts, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
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
    fn prefilled_topic() {
        assert_eq!(
            PublishForm::new("tasmota/cmnd/x/POWER").topic.lines()[0],
            "tasmota/cmnd/x/POWER"
        );
    }

    #[test]
    fn ctrl_s_sends_multiline_payload() {
        let mut f = PublishForm::new("a/b");
        f.on_key(key(KeyCode::Tab));
        assert_eq!(f.field, Field::Payload);
        type_str(&mut f, "{\n\"a\":1}");
        assert_eq!(
            f.on_key(ctrl('s')),
            FormOutcome::Send {
                topic: "a/b".into(),
                payload: b"{\n\"a\":1}".to_vec(),
                qos: 0,
                retain: false
            }
        );
    }

    #[test]
    fn enter_in_topic_moves_to_payload() {
        let mut f = PublishForm::new("a/b");
        f.on_key(key(KeyCode::Enter));
        assert_eq!(f.field, Field::Payload);
        assert_eq!(f.topic.lines().len(), 1);
    }

    #[test]
    fn qos_and_retain_fields() {
        let mut f = PublishForm::new("a/b");
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
                retain: true
            }
        );
    }

    #[test]
    fn shift_tab_goes_back() {
        let mut f = PublishForm::new("a");
        f.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(f.field, Field::Retain);
    }

    #[test]
    fn wildcard_or_empty_topic_rejected_and_error_clears() {
        for t in ["a/#", "a/+/b", ""] {
            let mut f = PublishForm::new(t);
            assert_eq!(f.on_key(ctrl('s')), FormOutcome::Continue, "{t}");
            assert_eq!(f.error.as_deref(), Some("invalid topic"), "{t}");
        }
        let mut f = PublishForm::new("a/#");
        f.on_key(ctrl('s'));
        f.on_key(key(KeyCode::Backspace));
        f.on_key(key(KeyCode::Backspace));
        assert!(matches!(f.on_key(ctrl('s')), FormOutcome::Send { .. }));
        assert_eq!(f.error, None);
    }

    #[test]
    fn format_json_pretty_prints_or_errors() {
        let mut f = PublishForm::new("a");
        f.field = Field::Payload;
        type_str(&mut f, "{\"a\":1}");
        assert!(f.format_json().is_ok());
        assert_eq!(f.payload.lines(), ["{", "  \"a\": 1", "}"]);

        let mut f = PublishForm::new("a");
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
            let mut f = PublishForm::new(t);
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
        let f = PublishForm::new("a/\x1b[2Jb\tc\nd\re");
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
        let f = PublishForm::new(&long);
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
        let mut f = PublishForm::new("a");
        f.topic.insert_newline();
        f.topic.insert_str("b");
        assert_eq!(f.on_key(ctrl('s')), FormOutcome::Continue);
        assert_eq!(f.error.as_deref(), Some("invalid topic"));
    }

    #[test]
    fn esc_cancels() {
        let mut f = PublishForm::new("a");
        assert_eq!(f.on_key(key(KeyCode::Esc)), FormOutcome::Cancel);
    }
}
