//! The chart field picker (`v`): a typed filter over the shown payload's
//! numeric fields, each row with the field's current value.

use super::app::App;
use super::theme;
use super::ui::{highlight_ranges, match_in};
use mqtt_zapp_core::payload::{numeric_at, sanitize};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

const WIDTH: u16 = 56;
const HEIGHT: u16 = 16;

#[derive(Debug, Clone, Default)]
pub struct FieldPicker {
    pub query: String,
    pub sel: usize,
}

fn name(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("value")
}

impl FieldPicker {
    /// The series whose display name contains the query, in payload order.
    pub fn matches(&self, series: &[Option<String>]) -> Vec<Option<String>> {
        series
            .iter()
            .filter(|s| self.query.is_empty() || !match_in(&self.query, name(s)).is_empty())
            .cloned()
            .collect()
    }

    /// Move the selection by `delta` within `len` rows, clamped.
    pub fn step(&mut self, delta: isize, len: usize) {
        let last = len.saturating_sub(1);
        self.sel = self.sel.min(last).saturating_add_signed(delta).min(last);
    }
}

pub(super) fn render(f: &mut Frame, area: Rect, app: &App, p: &FieldPicker) {
    theme::dim_backdrop(f, area);
    let card = theme::centered(area, WIDTH, HEIGHT);
    f.render_widget(Clear, card);
    let found = p.matches(&app.chart_series());
    let title = format!(
        "chart field · {}/{} · enter charts · esc closes",
        found.len(),
        app.chart_series().len()
    );
    let block = theme::pane(&title, true);
    let inner = block.inner(card);
    f.render_widget(block, card);
    if inner.height == 0 {
        return;
    }
    let payload = super::message_view::shown_message(app).map(|m| m.payload.clone());
    let list_h = usize::from(inner.height).saturating_sub(1).max(1);
    let sel = p.sel.min(found.len().saturating_sub(1));
    let top = sel.saturating_sub(list_h - 1);
    let muted = Style::new().fg(theme::MUTED);
    let mut lines = vec![Line::from(vec![
        Span::styled(" filter: ", muted),
        Span::raw(format!("{}▏", sanitize(&p.query).replace('\n', " "))),
    ])];
    let width = usize::from(inner.width);
    for (i, s) in found.iter().enumerate().skip(top).take(list_h) {
        let label = sanitize(name(s)).replace('\n', " ");
        let value = payload
            .as_ref()
            .and_then(|b| numeric_at(b, s.as_deref()))
            .map(|v| v.to_string())
            .unwrap_or_default();
        let room = width.saturating_sub(value.chars().count() + 3).max(1);
        let shown: String = label.chars().take(room).collect();
        let pad = room.saturating_sub(shown.chars().count());
        let base = if i == sel {
            Style::new().bg(theme::SEL_BG).fg(theme::SEL_FG)
        } else {
            Style::new()
        };
        let mut spans = vec![Span::styled(" ", base)];
        highlight_ranges(&mut spans, &shown, &match_in(&p.query, &shown), base);
        spans.push(Span::styled(" ".repeat(pad + 1), base));
        spans.push(Span::styled(
            format!("{value} "),
            base.add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(spans));
    }
    if found.is_empty() {
        lines.push(Line::styled(" no matching field", muted));
    }
    f.render_widget(Paragraph::new(lines), inner);
}
