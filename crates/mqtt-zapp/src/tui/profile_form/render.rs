//! Drawing the profile form card.

use super::*;

/// One cell per char of `text`, sanitized char by char so `cursor` (a char
/// index) still lines up; chars are skipped from the left to keep the cursor
/// within `width` columns (wide chars count two).
pub(super) fn input_spans(text: &str, cursor: Option<usize>, width: usize) -> Vec<Span<'static>> {
    let mut cells: Vec<String> = text
        .chars()
        .map(|c| sanitize(&c.to_string()).replace('\n', " "))
        .collect();
    if cursor.is_some_and(|c| c >= cells.len()) {
        cells.push(" ".into());
    }
    let skip = cursor.map_or(0, |c| {
        let mut start = c + 1;
        let mut used = 0;
        while start > 0 && used + cells[start - 1].width() <= width.max(1) {
            start -= 1;
            used += cells[start].width();
        }
        start.min(c)
    });
    cells
        .into_iter()
        .enumerate()
        .skip(skip)
        .map(|(i, t)| {
            if cursor == Some(i) {
                Span::styled(t, Style::new().add_modifier(Modifier::REVERSED))
            } else {
                Span::raw(t)
            }
        })
        .collect()
}

pub(super) fn choice(text: &str) -> Vec<Span<'static>> {
    vec![Span::styled(
        format!("‹ {text} ›"),
        Style::new().add_modifier(Modifier::BOLD),
    )]
}

pub(super) fn value_spans(form: &ProfileForm, field: PField, width: usize) -> Vec<Span<'static>> {
    let focused = form.field == field;
    match field {
        PField::Protocol => choice(&format!("{:?}", form.protocol).to_lowercase()),
        PField::Version => choice(form.version.label()),
        PField::PasswordSource => choice(&format!("{:?}", form.source).to_lowercase()),
        PField::Insecure => vec![Span::raw(if form.insecure {
            "[x] skip certificate checks"
        } else {
            "[ ] skip certificate checks"
        })],
        PField::Retained => vec![Span::raw(if form.retained {
            "[x] send on subscribe (MQTT 5 only)"
        } else {
            "[ ] send on subscribe (MQTT 5 only)"
        })],
        PField::Password => {
            let input = &form.password;
            if input.text().is_empty() && form.had_keyring {
                let mut v = input_spans("", focused.then_some(0), width);
                v.push(Span::styled("(unchanged)", Style::new().fg(theme::MUTED)));
                return v;
            }
            let masked = "•".repeat(input.text().chars().count());
            input_spans(&masked, focused.then(|| input.cursor()), width)
        }
        f => {
            let input = form.input(f).expect("text field");
            input_spans(input.text(), focused.then(|| input.cursor()), width)
        }
    }
}

/// The form as a card centred over `area`; the rows scroll to keep the
/// focused one in view.
pub fn render(f: &mut Frame, area: Rect, form: &ProfileForm) {
    let fields = form.visible_fields();
    theme::dim_backdrop(f, area);
    let card = theme::centered(area, 72, fields.len() as u16 + 4);
    f.render_widget(Clear, card);
    let title = match &form.original {
        Some(name) => format!("edit profile · {}", sanitize(name).replace('\n', " ")),
        None => "add profile".into(),
    };
    let block = match &form.error {
        Some(e) => Block::bordered()
            .border_type(BorderType::Thick)
            .title(Span::styled(
                format!(" {title} - {} ", sanitize(e).replace('\n', " ")),
                Style::new().fg(theme::ERR).add_modifier(Modifier::BOLD),
            ))
            .border_style(Style::new().fg(theme::ERR)),
        None => theme::pane(&title, true),
    };
    let inner = block.inner(card);
    f.render_widget(block, card);
    let [rows, hint] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let height = rows.height.max(1) as usize;
    let focus = fields.iter().position(|x| *x == form.field).unwrap_or(0);
    let skip = (focus + 1).saturating_sub(height);
    let width = (inner.width as usize).saturating_sub(LABEL_WIDTH + 3);
    let lines: Vec<Line> = fields
        .iter()
        .skip(skip)
        .map(|&field| {
            let sel = field == form.field;
            let mut spans = vec![
                Span::styled(
                    if sel { " ▶ " } else { "   " },
                    Style::new().fg(theme::ACCENT),
                ),
                Span::styled(
                    format!("{:<LABEL_WIDTH$}", field.label()),
                    if sel {
                        Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(theme::MUTED)
                    },
                ),
            ];
            spans.extend(value_spans(form, field, width));
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), rows);
    f.render_widget(
        Paragraph::new(format!(" {HINT}")).style(Style::new().fg(theme::MUTED)),
        hint,
    );
}
