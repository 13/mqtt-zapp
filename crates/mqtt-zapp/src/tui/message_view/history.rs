//! The history list of the message pane.

use super::*;

/// A one-line preview of `payload` in at most `room` display columns, with
/// `…` when cut.
pub(super) fn fit_preview(payload: &[u8], room: usize) -> String {
    let p = preview(payload, room);
    if p.width() <= room {
        return p;
    }
    // Wide characters: `preview` counts chars, the row counts columns.
    format!("{}…", take_width(&p, room.saturating_sub(1), false))
}

pub(super) fn render_history(f: &mut Frame, area: Rect, app: &App, s: &Shown) {
    let title = match app.history_sel {
        None => "history · live".to_owned(),
        Some(_) => format!(
            "history · paused ({} to newest)",
            crate::tui::ui::key_for(app, Action::Up)
        ),
    };
    let focused = app.pane_focused(Focus::History);
    let block = theme::pane(&title, focused);
    let inner = block.inner(area);
    let height = inner.height as usize;
    let h = &s.node.history;
    // Newest first: row `r` is history index `len - 1 - r`.
    let sel_row = s.idx.map(|i| h.len() - 1 - i);
    let offset = sel_row.map_or(0, |r| (r + 1).saturating_sub(height));
    app.hit.set(HitAreas {
        history: area,
        history_offset: offset,
        ..app.hit.get()
    });
    let lines: Vec<Line> = h
        .iter()
        .rev()
        .enumerate()
        .skip(offset)
        .take(height)
        .map(|(row, m)| {
            let marked = s.base == Some(h.len() - 1 - row);
            let time = clock(m.received_at, "%H:%M:%S");
            let marker = if marked { " ◆ " } else { "  " };
            // The preview gets whatever the row has left.
            let room = usize::from(inner.width).saturating_sub(time.width() + marker.width());
            let line = Line::from(vec![
                Span::styled(time, theme::TITLE),
                Span::styled(marker, Style::new().fg(theme::ACCENT)),
                Span::raw(fit_preview(&m.payload, room)),
            ]);
            if Some(row) == sel_row && app.history_sel.is_some() {
                theme::select(line, focused)
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}
