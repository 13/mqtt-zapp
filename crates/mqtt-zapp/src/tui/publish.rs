//! The publish form: a topic line, a multi-line payload, QoS, retain and
//! the payload format. State and key handling live here; `render` draws it
//! docked in the message pane. Its key hints are in the footer.

mod format;

pub use format::Format;

use super::theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::payload::{Props, is_unsafe_char, sanitize};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use tui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

const INVALID_TOPIC: &str = "invalid topic";

/// Text for a text area: CRLF and a lone CR (how VTE terminals and xterm
/// paste line breaks) become LF, and every other control or
/// invisible char becomes U+FFFD, so nothing reaches the terminal raw (the
/// text areas draw their chars as they are).
pub(crate) fn input_safe(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .map(|c| if is_unsafe_char(c) { '\u{fffd}' } else { c })
        .collect()
}

/// Payload rows the form shows: it grows with the text between these.
const PAYLOAD_MIN_ROWS: u16 = 4;
const PAYLOAD_MAX_ROWS: u16 = 12;
/// Rows the error line may wrap to.
const ERROR_MAX_ROWS: u16 = 2;

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
    Format,
}

impl Field {
    const ORDER: [Field; 8] = [
        Field::Topic,
        Field::Payload,
        Field::ContentType,
        Field::ResponseTopic,
        Field::UserProps,
        Field::Qos,
        Field::Retain,
        Field::Format,
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
    /// Leave the form open and go back to the topics.
    Leave,
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
    /// Checks and pretty-prints the payload; the bytes sent are as typed.
    pub format: Format,
    /// Shown on its own line under the options; cleared by the next key.
    pub error: Option<String>,
    /// While walking the sent payloads: the entry shown (1 = newest) and
    /// how many there are.
    pub sent_pos: Option<(usize, usize)>,
    /// What the last frame drew where, for clicks.
    hits: Vec<(Rect, Click)>,
    /// The form has the keys. Off while it stays open beside the browsed
    /// panes: no cursor, no highlighted label.
    active: bool,
}

/// A click target in the form.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Click {
    Focus(Field),
    Qos(u8),
    Retain,
    Format(Format),
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
            format: Format::Raw,
            error: None,
            sent_pos: None,
            hits: Vec::new(),
            active: true,
        };
        form.sync_focus();
        form
    }

    /// A form with the payload, QoS and retain of a message to re-send.
    pub fn prefilled(topic: &str, payload: &str, qos: u8, retain: bool, v5: bool) -> PublishForm {
        let mut form = PublishForm::new(topic, v5);
        form.set_payload(payload);
        form.format = Format::detect(payload);
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
            KeyCode::Char('o') if ctrl => return FormOutcome::Leave,
            KeyCode::Char('s') if ctrl => return self.send(),
            KeyCode::Char('e') if ctrl => return FormOutcome::Edit(self.payload_text()),
            KeyCode::Char('f') if ctrl => {
                if let Err(e) = self.format_payload() {
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
                Field::Qos => match key.code {
                    KeyCode::Char(c @ '0'..='2') => self.qos = c as u8 - b'0',
                    KeyCode::Left => self.qos = self.qos.saturating_sub(1),
                    KeyCode::Right => self.qos = (self.qos + 1).min(2),
                    KeyCode::Char(' ') => self.qos = (self.qos + 1) % 3,
                    _ => {}
                },
                Field::Retain => {
                    if matches!(key.code, KeyCode::Char(' ') | KeyCode::Enter) {
                        self.retain = !self.retain;
                    }
                }
                Field::Format => match key.code {
                    KeyCode::Left => self.format = self.format.step(-1),
                    KeyCode::Right | KeyCode::Char(' ') => self.format = self.format.step(1),
                    KeyCode::Char('r') => self.format = Format::Raw,
                    KeyCode::Char('j') => self.format = Format::Json,
                    KeyCode::Char('x') => self.format = Format::Xml,
                    _ => {}
                },
            },
        }
        FormOutcome::Continue
    }

    /// The payload as one string, lines joined by `\n`.
    pub fn payload_text(&self) -> String {
        self.payload.lines().join("\n")
    }

    /// Replace the payload text (from the editor or the history), through
    /// `input_safe`.
    pub fn set_payload(&mut self, text: &str) {
        let text = input_safe(text);
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

    /// Whether the form has the keys; see `active`.
    pub fn set_active(&mut self, active: bool) {
        if self.active != active {
            self.active = active;
            self.sync_focus();
        }
    }

    /// Only the focused text area of the active form shows a cursor.
    fn sync_focus(&mut self) {
        let shown = if self.active {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
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
        for (name, area) in [
            ("content type", &self.content_type),
            ("response topic", &self.response_topic),
        ] {
            if area.lines().len() > 1 {
                return Err(format!("{name}: one line only"));
            }
        }
        let text = |a: &TextArea| a.lines().join("").trim().to_owned();
        if text(&self.response_topic).contains(['+', '#']) {
            return Err("response topic: no wildcards".into());
        }
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

    /// A paste: the payload takes all of it, a single-line field its first
    /// line, the option fields nothing. Unsafe chars become U+FFFD.
    pub fn paste(&mut self, text: &str) {
        self.error = None;
        match self.field {
            Field::Payload => {
                self.payload.insert_str(input_safe(text));
            }
            Field::Topic => {
                self.topic
                    .insert_str(crate::tui::profile_form::first_line(text));
            }
            Field::ContentType | Field::ResponseTopic | Field::UserProps => {
                let line = crate::tui::profile_form::first_line(text);
                self.prop_area(self.field).insert_str(line);
            }
            Field::Qos | Field::Retain | Field::Format => {}
        }
    }

    /// Pretty-print the payload in its format. A raw payload that is JSON
    /// or XML switches to that format first. On an error the payload is
    /// left as typed.
    pub fn format_payload(&mut self) -> Result<(), String> {
        let text = self.payload_text();
        if self.format == Format::Raw {
            match Format::detect(&text) {
                Format::Raw => return Err("not JSON or XML: nothing to format".into()),
                f => self.format = f,
            }
        }
        let pretty = self.format.pretty(&text)?;
        self.payload = TextArea::new(pretty.lines().map(str::to_owned).collect());
        self.sync_focus();
        Ok(())
    }

    /// Nothing typed beyond the topic: no payload and no properties.
    pub fn is_blank(&self) -> bool {
        let empty = |a: &TextArea| a.lines().iter().all(|l| l.is_empty());
        empty(&self.payload)
            && empty(&self.content_type)
            && empty(&self.response_topic)
            && empty(&self.user_props)
    }

    /// The topic as one line, as it would be sent.
    pub fn topic_text(&self) -> String {
        self.topic.lines().join("")
    }

    /// A left click at `at`: focus the field under it, or set the QoS,
    /// retain or format clicked. Whether anything was hit.
    pub fn click(&mut self, at: Position) -> bool {
        let Some(&(_, target)) = self.hits.iter().find(|(r, _)| r.contains(at)) else {
            return false;
        };
        self.error = None;
        match target {
            Click::Focus(f) => self.field = f,
            Click::Qos(q) => (self.field, self.qos) = (Field::Qos, q),
            Click::Retain => (self.field, self.retain) = (Field::Retain, !self.retain),
            Click::Format(f) => (self.field, self.format) = (Field::Format, f),
        }
        self.sync_focus();
        true
    }
}

/// The error line's text and the rows it wraps to in `width` columns.
fn error_text(form: &PublishForm, width: u16) -> Option<(String, u16)> {
    let e = form.error.as_deref()?;
    let text = format!("✕ {}", sanitize(e).replace('\n', " "));
    let rows = (text.width() as u16).div_ceil(width.max(1));
    Some((text, rows.clamp(1, ERROR_MAX_ROWS)))
}

/// Rows the form wants at `width` columns, borders included: the payload
/// grows with its text, and an error adds its line.
pub fn height(form: &PublishForm, width: u16) -> u16 {
    let inner_w = width.saturating_sub(2);
    let payload = (form.payload.lines().len() as u16).clamp(PAYLOAD_MIN_ROWS, PAYLOAD_MAX_ROWS);
    let props = if form.v5 { 3 } else { 0 };
    let error = error_text(form, inner_w).map_or(0, |(_, r)| r);
    // Borders, topic, the payload's rule and rows, props, options, error.
    2 + 1 + 1 + payload + props + 1 + error
}

/// Columns of the label column: the widest label shown.
fn label_width(form: &PublishForm) -> u16 {
    if form.v5 { 16 } else { 7 }
}

/// The payload's title: `payload · 42 B · json ✓ · sent 2/5`.
fn payload_title(form: &PublishForm) -> Line<'static> {
    let muted = Style::new().fg(theme::MUTED);
    let text = form.payload_text();
    let mut spans = vec![
        Span::styled("payload", label_style(form, Field::Payload)),
        Span::styled(format!(" · {} B", text.len()), muted),
    ];
    match form.format.check(&text) {
        Some(Ok(())) => spans.push(Span::styled(
            format!(" · {} ✓", form.format.label()),
            Style::new().fg(theme::OK),
        )),
        Some(Err(_)) => spans.push(Span::styled(
            format!(" · {} ✗", form.format.label()),
            Style::new().fg(theme::ERR),
        )),
        None => {}
    }
    if let Some((i, n)) = form.sent_pos {
        spans.push(Span::styled(format!(" · sent {i}/{n}"), muted));
    }
    Line::from(spans)
}

/// The publish form, filling `area`: docked in the message pane, below
/// history. Records its click targets in the form.
pub fn render(f: &mut Frame, area: Rect, form: &mut PublishForm) {
    form.hits.clear();
    f.render_widget(Clear, area);
    // The thick border marks focus, so an inactive form keeps the rounded
    // one even with an error.
    let border = if form.active {
        BorderType::Thick
    } else {
        BorderType::Rounded
    };
    let block = match &form.error {
        Some(_) => Block::bordered()
            .border_type(border)
            .title(Span::styled(
                " publish ",
                Style::new().fg(theme::ERR).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::new().fg(theme::ERR)),
        None => theme::pane("publish", form.active),
    };
    let inner = block.inner(area);
    f.render_widget(block, area);
    let props_h = if form.v5 { 3 } else { 0 };
    let error = error_text(form, inner.width);
    let [topic, payload, props, opts, err] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(props_h),
        Constraint::Length(1),
        Constraint::Length(error.as_ref().map_or(0, |(_, r)| *r)),
    ])
    .areas(inner);
    let lw = label_width(form);
    if form.v5 {
        let rows = Layout::vertical([Constraint::Length(1); 3]).split(props);
        for (row, (label, field)) in rows.iter().zip([
            ("content type: ", Field::ContentType),
            ("response topic: ", Field::ResponseTopic),
            ("user props: ", Field::UserProps),
        ]) {
            let [l, body] =
                Layout::horizontal([Constraint::Length(lw), Constraint::Min(1)]).areas(*row);
            f.render_widget(Paragraph::new(label).style(label_style(form, field)), l);
            f.render_widget(&*form.prop_area(field), body);
            form.hits.push((*row, Click::Focus(field)));
        }
    }
    form.hits.push((topic, Click::Focus(Field::Topic)));
    let [label, topic_body] =
        Layout::horizontal([Constraint::Length(lw), Constraint::Min(1)]).areas(topic);
    f.render_widget(
        Paragraph::new("topic: ").style(label_style(form, Field::Topic)),
        label,
    );
    f.render_widget(
        Paragraph::new(topic_line(form, topic_body.width as usize)),
        topic_body,
    );
    let block = Block::new()
        .borders(Borders::TOP)
        .title(payload_title(form));
    let body = block.inner(payload);
    f.render_widget(block, payload);
    f.render_widget(&form.payload, body);
    form.hits.push((payload, Click::Focus(Field::Payload)));
    let mut x = opts.x;
    let mut spans = Vec::new();
    for (span, click) in options_spans(form) {
        let w = (span.content.width() as u16).min(opts.right().saturating_sub(x));
        if let Some(c) = click {
            form.hits.push((Rect::new(x, opts.y, w, 1), c));
        }
        x += w;
        spans.push(span);
    }
    f.render_widget(Paragraph::new(Line::from(spans)), opts);
    if let Some((text, _)) = error {
        f.render_widget(
            Paragraph::new(text)
                .style(Style::new().fg(theme::ERR))
                .wrap(Wrap { trim: true }),
            err,
        );
    }
}

/// The topic line, drawn by hand rather than by the text area: the text area
/// hands control characters (a broker topic may contain ESC) to the buffer
/// unfiltered, so each char is sanitized here, one for one, keeping the
/// cursor position. The text itself stays raw, so a send is exact. Chars are
/// skipped from the left so the cursor stays inside `width` columns.
fn topic_line(form: &PublishForm, width: usize) -> Line<'static> {
    let lines = form.topic.lines();
    let single = lines.len() == 1;
    let cursor =
        (single && form.active && form.field == Field::Topic).then(|| form.topic.cursor().1);
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
    let widths: Vec<usize> = cells.iter().map(|(t, _)| t.width()).collect();
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
    if form.active && form.field == field {
        Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    }
}

/// `qos [0] 1 2  retain [ ]  format [raw] json xml`, each value with its
/// click target; the focused field's label is highlighted. 46 columns: it
/// fits the message pane of an 80-column terminal.
fn options_spans(form: &PublishForm) -> Vec<(Span<'static>, Option<Click>)> {
    let pick = |on: bool, text: &str| {
        if on {
            Span::styled(
                format!(" [{text}]"),
                Style::new().add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw(format!(" {text}"))
        }
    };
    let mut spans = vec![(
        Span::styled("qos", label_style(form, Field::Qos)),
        Some(Click::Focus(Field::Qos)),
    )];
    for q in 0..=2u8 {
        spans.push((pick(q == form.qos, &q.to_string()), Some(Click::Qos(q))));
    }
    spans.push((Span::raw("  "), None));
    spans.push((
        Span::styled("retain", label_style(form, Field::Retain)),
        Some(Click::Retain),
    ));
    let mark = if form.retain { " [x]" } else { " [ ]" };
    spans.push((Span::raw(mark), Some(Click::Retain)));
    spans.push((Span::raw("  "), None));
    spans.push((
        Span::styled("format", label_style(form, Field::Format)),
        Some(Click::Focus(Field::Format)),
    ));
    for fmt in Format::ALL {
        spans.push((
            pick(fmt == form.format, fmt.label()),
            Some(Click::Format(fmt)),
        ));
    }
    spans
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
        assert_eq!(f.field, Field::Format);
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
        assert!(f.format_payload().is_ok());
        assert_eq!(f.payload.lines(), ["{", "  \"a\": 1", "}"]);
        assert_eq!(
            f.format,
            Format::Json,
            "raw switches to the detected format"
        );

        let mut f = PublishForm::new("a", false);
        f.field = Field::Payload;
        type_str(&mut f, "{oops");
        assert!(f.format_payload().is_err());
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
        let mut f = PublishForm::new("a/\x1b[2Jb\tc\nd\re", false);
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|fr| render(fr, fr.area(), &mut f)).unwrap();
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
    fn cjk_topic_keeps_cursor_visible() {
        let f = PublishForm::new(&"\u{6e29}".repeat(40), false);
        let line = topic_line(&f, 20);
        let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
        assert!(used <= 20, "{used}");
        let cur = line
            .spans
            .iter()
            .position(|s| s.style.add_modifier.contains(Modifier::REVERSED));
        assert_eq!(cur, Some(line.spans.len() - 1), "cursor at end, visible");
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
    fn form_response_topic_rejects_wildcards() {
        for t in ["a/+", "a/#", "+"] {
            let mut f = PublishForm::new("a/b", true);
            f.field = Field::ResponseTopic;
            type_str(&mut f, t);
            assert_eq!(f.on_key(ctrl('s')), FormOutcome::Continue, "{t}");
            assert_eq!(f.error.as_deref(), Some("response topic: no wildcards"));
        }
        for which in [Field::ContentType, Field::ResponseTopic] {
            let mut f = PublishForm::new("a/b", true);
            f.prop_area(which).insert_newline();
            f.prop_area(which).insert_str("x");
            assert_eq!(f.on_key(ctrl('s')), FormOutcome::Continue, "{which:?}");
            assert!(f.error.as_deref().unwrap().ends_with("one line only"));
        }
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

    fn draw(f: &mut PublishForm, w: u16, h: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|fr| render(fr, fr.area(), f)).unwrap();
        let b = t.backend().buffer().clone();
        b.content
            .chunks(w as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn format_field_cycles_and_pretty_prints_xml() {
        let mut f = PublishForm::new("a", false);
        f.field = Field::Format;
        f.on_key(key(KeyCode::Right));
        assert_eq!(f.format, Format::Json);
        f.on_key(key(KeyCode::Char('x')));
        assert_eq!(f.format, Format::Xml);
        f.on_key(key(KeyCode::Left));
        assert_eq!(f.format, Format::Json);
        f.format = Format::Xml;
        f.set_payload("<a><b>1</b></a>");
        f.on_key(ctrl('f'));
        assert_eq!(f.error, None);
        assert_eq!(f.payload.lines(), ["<a>", "  <b>1</b>", "</a>"]);
        // The bytes sent are the text as typed (here, pretty-printed).
        match f.on_key(ctrl('s')) {
            FormOutcome::Send { payload, .. } => {
                assert_eq!(payload, b"<a>\n  <b>1</b>\n</a>")
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn prefilled_detects_format() {
        let f = PublishForm::prefilled("a", "{\"x\":1}", 0, false, false);
        assert_eq!(f.format, Format::Json);
        let f = PublishForm::prefilled("a", "<r/>", 0, false, false);
        assert_eq!(f.format, Format::Xml);
        let f = PublishForm::prefilled("a", "on", 0, false, false);
        assert_eq!(f.format, Format::Raw);
    }

    #[test]
    fn qos_keys_step_and_cycle() {
        let mut f = PublishForm::new("a", false);
        f.field = Field::Qos;
        f.on_key(key(KeyCode::Right));
        f.on_key(key(KeyCode::Right));
        f.on_key(key(KeyCode::Right));
        assert_eq!(f.qos, 2, "right stops at 2");
        f.on_key(key(KeyCode::Char(' ')));
        assert_eq!(f.qos, 0, "space wraps");
        f.on_key(key(KeyCode::Left));
        assert_eq!(f.qos, 0);
    }

    #[test]
    fn title_shows_size_validity_and_sent_position() {
        let mut f = PublishForm::new("a", false);
        f.format = Format::Json;
        f.set_payload("{\"a\":1}");
        let s = draw(&mut f, 60, 12);
        assert!(s.contains("payload · 7 B · json ✓"), "{s}");
        f.set_payload("{");
        f.sent_pos = Some((2, 5));
        let s = draw(&mut f, 60, 12);
        assert!(s.contains("payload · 1 B · json ✗ · sent 2/5"), "{s}");
        assert!(!s.contains("ctrl-s"), "no hint line in the form: {s}");
    }

    #[test]
    fn error_has_its_own_line_and_keeps_the_title() {
        let mut f = PublishForm::new("", false);
        f.on_key(ctrl('s'));
        let before = height(&f, 60);
        assert_eq!(before, height(&PublishForm::new("", false), 60) + 1);
        let s = draw(&mut f, 60, before);
        assert!(s.contains(" publish "), "{s}");
        assert!(s.contains("✕ invalid topic"), "{s}");
    }

    #[test]
    fn height_grows_with_payload_within_bounds() {
        let mut f = PublishForm::new("a", false);
        let min = height(&f, 60);
        f.set_payload(&"x\n".repeat(8));
        assert_eq!(height(&f, 60), min + 9 - PAYLOAD_MIN_ROWS);
        f.set_payload(&"x\n".repeat(100));
        assert_eq!(height(&f, 60), min + PAYLOAD_MAX_ROWS - PAYLOAD_MIN_ROWS);
    }

    #[test]
    fn v5_labels_line_up() {
        let mut f = PublishForm::new("tt", true);
        f.content_type.insert_str("ct");
        let s = draw(&mut f, 60, 20);
        let col = |label: &str, value: &str| {
            let row = s.lines().find(|l| l.contains(label)).unwrap();
            row[..row.find(value).unwrap()].chars().count()
        };
        assert_eq!(col("topic:", "tt"), col("content type:", "ct"), "{s}");
    }

    #[test]
    fn clicks_focus_fields_and_set_options() {
        let mut f = PublishForm::new("a", false);
        let s = draw(&mut f, 70, 10);
        let rows: Vec<&str> = s.lines().collect();
        let opts_y = rows.iter().position(|r| r.contains("qos [")).unwrap() as u16;
        let x_of = |needle: &str| {
            let r = rows[opts_y as usize];
            r[..r.find(needle).unwrap()].chars().count() as u16
        };
        let (q2, retain, xml) = (x_of(" 2 "), x_of("[ ]"), x_of("xml"));
        assert!(f.click(Position::new(q2 + 1, opts_y)));
        assert_eq!((f.field, f.qos), (Field::Qos, 2));
        assert!(f.click(Position::new(retain, opts_y)));
        assert!(f.retain);
        assert!(f.click(Position::new(xml, opts_y)));
        assert_eq!((f.field, f.format), (Field::Format, Format::Xml));
        assert!(f.click(Position::new(10, 3)), "payload");
        assert_eq!(f.field, Field::Payload);
        assert!(f.click(Position::new(10, 1)), "topic");
        assert_eq!(f.field, Field::Topic);
        assert!(!f.click(Position::new(0, 0)), "border");
    }

    #[test]
    fn set_payload_replaces_control_characters() {
        let mut f = PublishForm::new("a", false);
        f.set_payload("hi\x1b[2J\r\nx\u{202e}y\tz");
        assert_eq!(f.payload_text(), "hi\u{fffd}[2J\nx\u{fffd}y\tz");
    }
}
