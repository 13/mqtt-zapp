//! Chart cycling and clicking: which series is charted and where.

use super::*;

impl App {
    /// off → plain number (if the shown payload is one) → each numeric JSON
    /// field → off.
    pub(super) fn cycle_chart(&mut self) {
        if super::super::message_view::shown_message(self).is_none() {
            self.set_note(Note::info("nothing to chart"), Instant::now());
            return;
        }
        let series = self.chart_series();
        // Pinned series are already charted: `c` steps over them.
        let free = |s: &&Option<String>| !self.chart_pinned.contains(s);
        let next = match &self.chart {
            None => series.iter().find(free).cloned(),
            Some(cur) => series
                .iter()
                .position(|s| s == cur)
                .and_then(|i| series[i + 1..].iter().find(free).cloned()),
        };
        let view = self.charted_body();
        self.chart = next;
        let note = match (&self.chart, series.is_empty()) {
            (_, true) => "no numeric values to chart".to_owned(),
            (None, _) => "chart off".to_owned(),
            (Some(s), _) => format!("chart: {}", s.as_deref().unwrap_or("value")),
        };
        self.set_note(Note::info(note), Instant::now());
        self.scroll_to_chart_line(usize::from(view.height));
    }

    /// `C`: pin the current series (or unpin it if pinned already).
    pub(super) fn pin_chart(&mut self) {
        let name = |s: &Option<String>| s.as_deref().unwrap_or("value").to_owned();
        let Some(cur) = self.chart.clone() else {
            self.set_note(Note::info("nothing to pin"), Instant::now());
            return;
        };
        let note = if let Some(i) = self.chart_pinned.iter().position(|p| *p == cur) {
            self.chart_pinned.remove(i);
            format!("unpinned: {}", name(&cur))
        } else if self.chart_pinned.len() >= MAX_PINNED {
            let old = self.chart_pinned.remove(0);
            self.chart_pinned.push(cur.clone());
            format!("pinned: {} (dropped {})", name(&cur), name(&old))
        } else {
            self.chart_pinned.push(cur.clone());
            format!("pinned: {}", name(&cur))
        };
        self.set_note(Note::info(note), Instant::now());
    }

    /// What `c` cycles through for the shown payload, in order: the plain
    /// value (`None`) if it is a number, then each numeric JSON field.
    pub fn chart_series(&self) -> Vec<Option<String>> {
        super::super::message_view::chart_meta(self).map_or_else(Vec::new, |m| m.series.clone())
    }

    /// The payload line of the charted value in the shown payload; `None`
    /// with the chart off, in the diff view, or when the field is not there.
    pub fn chart_line(&self) -> Option<usize> {
        let cur = self.chart.as_ref().filter(|_| !self.show_diff)?;
        super::super::message_view::chart_meta(self)?.line_of(cur)
    }

    /// The payload lines of the current series and each pinned one, for the
    /// highlight; series missing from the shown message have none.
    pub fn chart_lines(&self) -> Vec<usize> {
        let Some(cur) = self.chart.as_ref().filter(|_| !self.show_diff) else {
            return Vec::new();
        };
        let Some(meta) = super::super::message_view::chart_meta(self) else {
            return Vec::new();
        };
        let mut out: Vec<usize> = std::iter::once(cur)
            .chain(&self.chart_pinned)
            .filter_map(|p| meta.line_of(p))
            .collect();
        out.dedup();
        out
    }

    /// A click on payload line `line`: chart the value there, or turn the
    /// chart off if it is the charted one. Lines without a number do nothing.
    pub(super) fn chart_click(&mut self, line: usize) {
        if self.show_diff {
            return;
        }
        let Some(m) = super::super::message_view::shown_message(self) else {
            return;
        };
        let Some((_, path)) = mqtt_zapp_core::payload::numeric_lines(&m.payload)
            .into_iter()
            .find(|(l, _)| *l == line)
        else {
            return;
        };
        let note = if self.chart.as_ref() == Some(&path) {
            self.chart = None;
            "chart off".to_owned()
        } else {
            let hit = self.hit.get();
            let (old, new) = (inner(hit.payload), self.charted_body());
            let note = format!("chart: {}", path.as_deref().unwrap_or("value"));
            self.chart = Some(path);
            if old != new {
                // The chart just opened and moved the pane: keep the clicked
                // line under the pointer, or else in view.
                let y = old.y + u16::try_from(line.saturating_sub(hit.payload_skip)).unwrap_or(0);
                if y >= new.y && y < new.bottom() {
                    self.payload_scroll = line - usize::from(y - new.y);
                } else {
                    self.scroll_to_chart_line(usize::from(new.height));
                }
            }
            note
        };
        self.set_note(Note::info(note), Instant::now());
    }

    /// The payload pane's body as it will be with the chart open, going by
    /// the last frame's layout.
    fn charted_body(&self) -> ratatui::layout::Rect {
        let pane = self.hit.get().payload;
        if self.chart.is_some() {
            inner(pane)
        } else {
            inner(super::super::message_view::chart_split(pane)[1])
        }
    }

    /// Scroll the payload just enough that the charted line shows in a
    /// pane body `height` rows tall.
    fn scroll_to_chart_line(&mut self, height: usize) {
        let Some(l) = self.chart_line() else { return };
        let height = height.max(1);
        if l < self.payload_scroll {
            self.payload_scroll = l;
        } else if l >= self.payload_scroll + height {
            self.payload_scroll = l + 1 - height;
        }
    }
}
