//! The chart pane: per-message chart facts, their cache, and the plot.

use super::*;
use crate::tui::app::MAX_PINNED;

/// Line colour of each overlaid series: the current one, then each pinned
/// one in pin order.
pub(in crate::tui) const SERIES_COLORS: [Color; 4] =
    [theme::ACCENT, Color::Magenta, Color::Yellow, Color::Green];

/// How an open chart splits the payload area: the chart on top, the
/// payload pane below.
pub(in crate::tui) fn chart_split(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Min(5), Constraint::Percentage(50)]).areas(area)
}

/// Everything the chart derives from the topic's history and the shown
/// message, parsed once per change rather than once per frame.
pub(super) type HistoryKey = (usize, usize, Option<Stamp>);

pub(super) type ChartKey = (HistoryKey, Stamp);

/// Chart points, shared between the cache and the frame being drawn.
pub(super) type Points = Rc<Vec<(f64, f64)>>;

/// Points of each cached series path.
type SeriesPoints = Vec<(Option<String>, Points)>;

/// Per-message chart facts, shared between the cache and its readers.
pub struct ChartMeta {
    /// What `c` cycles through: the plain value, then each numeric field.
    pub series: Vec<Option<String>>,
    /// Payload line of each numeric series, for the highlight.
    lines: Vec<(usize, Option<String>)>,
}

impl ChartMeta {
    /// The payload line of `path`, if it is a numeric series of the message.
    pub fn line_of(&self, path: &Option<String>) -> Option<usize> {
        self.lines.iter().find(|(_, p)| p == path).map(|(l, _)| *l)
    }
}

/// The last computed chart data, reused until the topic, its history or the
/// shown message change: parsing every history entry (or the shown payload)
/// each frame is costly for big JSON payloads.
#[derive(Default)]
pub struct ChartCache {
    /// Points per series path, under the history key; at most one entry
    /// per charted series, so overlays don't evict each other.
    slot: RefCell<Option<(HistoryKey, SeriesPoints)>>,
    /// Series and highlight lines of the shown message.
    meta: RefCell<Option<(ChartKey, Rc<ChartMeta>)>>,
    /// Rebuilds of the shown message's series and lines.
    #[cfg(test)]
    pub builds: std::cell::Cell<u32>,
    /// Rebuilds of the points.
    #[cfg(test)]
    pub point_builds: std::cell::Cell<u32>,
}

pub(super) fn history_key(app: &App, s: &Shown) -> HistoryKey {
    let h = &s.node.history;
    (
        app.selected.unwrap_or_default(),
        h.len(),
        h.back().map(|m| (m.received_at, m.payload.len())),
    )
}

pub(super) fn chart_key(app: &App, s: &Shown) -> ChartKey {
    (
        history_key(app, s),
        (s.msg.received_at, s.msg.payload.len()),
    )
}

impl ChartCache {
    pub fn clear(&self) {
        *self.slot.borrow_mut() = None;
        *self.meta.borrow_mut() = None;
    }

    #[cfg(test)]
    pub fn is_cached(&self) -> bool {
        self.slot.borrow().is_some() || self.meta.borrow().is_some()
    }
}

/// Series and highlight lines of the shown message, cached. A payload above
/// `PRETTY_MAX` is not parsed as JSON here either (it is shown as text, so
/// there are no JSON fields to chart or highlight): only a plain number
/// stays chartable, which such a payload practically never is.
pub fn chart_meta(app: &App) -> Option<Rc<ChartMeta>> {
    use mqtt_zapp_core::payload::{numeric_at, numeric_lines, numeric_paths};
    let s = shown(app)?;
    let key = chart_key(app, &s);
    if let Some((k, m)) = &*app.chart_cache.meta.borrow()
        && *k == key
    {
        return Some(Rc::clone(m));
    }
    #[cfg(test)]
    app.chart_cache.builds.set(app.chart_cache.builds.get() + 1);
    let p = &s.msg.payload;
    let mut series = Vec::new();
    if numeric_at(p, None).is_some() {
        series.push(None);
    }
    let lines = if p.len() > PRETTY_MAX {
        Vec::new()
    } else {
        series.extend(numeric_paths(p).into_iter().map(Some));
        numeric_lines(p)
    };
    let meta = Rc::new(ChartMeta { series, lines });
    *app.chart_cache.meta.borrow_mut() = Some((key, Rc::clone(&meta)));
    Some(meta)
}

/// (history index, value) for each history entry with a number. Oversize
/// entries only count as plain numbers: their JSON fields are not parsed.
pub(super) fn chart_points(app: &App, s: &Shown, path: Option<&str>) -> Points {
    use mqtt_zapp_core::payload::numeric_at;
    let h = &s.node.history;
    let key = history_key(app, s);
    if let Some((k, v)) = &*app.chart_cache.slot.borrow()
        && *k == key
        && let Some((_, pts)) = v.iter().find(|(p, _)| p.as_deref() == path)
    {
        return Rc::clone(pts);
    }
    #[cfg(test)]
    app.chart_cache
        .point_builds
        .set(app.chart_cache.point_builds.get() + 1);
    let points: Points = Rc::new(
        h.iter()
            .enumerate()
            .filter(|(_, m)| path.is_none() || m.payload.len() <= PRETTY_MAX)
            .filter_map(|(i, m)| numeric_at(&m.payload, path).map(|y| (i as f64, y)))
            .collect(),
    );
    let mut slot = app.chart_cache.slot.borrow_mut();
    match &mut *slot {
        Some((k, v)) if *k == key => {
            if v.len() > MAX_PINNED {
                v.remove(0);
            }
            v.push((path.map(str::to_owned), Rc::clone(&points)));
        }
        _ => *slot = Some((key, vec![(path.map(str::to_owned), Rc::clone(&points))])),
    }
    points
}

/// `12.50` → `12.5`, `3.00` → `3`.
pub(super) fn fmt_num(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A braille line chart of the current series and any pinned ones over the
/// topic's history (oldest left); entries without a number are skipped.
pub(super) fn render_chart(f: &mut Frame, area: Rect, app: &App, s: &Shown, path: Option<&str>) {
    use ratatui::symbols::Marker;
    use ratatui::widgets::{Axis, Chart, Dataset, GraphType, LegendPosition};
    let name = |p: Option<&str>| mqtt_zapp_core::payload::sanitize(p.unwrap_or("value"));
    // The current series first, then each pinned one that is not it.
    let paths: Vec<Option<&str>> = std::iter::once(path)
        .chain(
            app.chart_pinned
                .iter()
                .map(Option::as_deref)
                .filter(|p| *p != path),
        )
        .collect();
    let title = if paths.len() > 1 {
        let names: Vec<String> = paths.iter().map(|p| name(*p)).collect();
        format!("chart · {}", names.join(" + "))
    } else {
        let series = app.chart_series();
        match series.iter().position(|s| s.as_deref() == path) {
            Some(i) => format!("chart · {} · {}/{}", name(path), i + 1, series.len()),
            None => format!("chart · {}", name(path)),
        }
    };
    let block = theme::pane(&title, false);
    let all: Vec<Points> = paths.iter().map(|p| chart_points(app, s, *p)).collect();
    if all.iter().all(|p| p.len() < 2) {
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new("not enough numeric values").style(Style::new().fg(theme::MUTED)),
            inner,
        );
        return;
    }
    let (lo, hi) = all
        .iter()
        .flat_map(|p| p.iter())
        .fold((f64::MAX, f64::MIN), |(lo, hi), &(_, y)| {
            (lo.min(y), hi.max(y))
        });
    let pad = if hi > lo { (hi - lo) * 0.05 } else { 1.0 };
    let x_max = all
        .iter()
        .filter_map(|p| p.last())
        .fold(1.0, |m: f64, p| m.max(p.0));
    let data: Vec<Dataset> = paths
        .iter()
        .zip(&all)
        .enumerate()
        .map(|(i, (p, pts))| {
            Dataset::default()
                .name(name(*p))
                .marker(Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::new().fg(SERIES_COLORS[i % SERIES_COLORS.len()]))
                .data(pts)
        })
        .collect();
    let muted = Style::new().fg(theme::MUTED);
    let chart = Chart::new(data)
        .block(block)
        // The default hides a legend over a quarter of the chart, which is
        // always the case in the short chart pane.
        .hidden_legend_constraints((Constraint::Ratio(1, 2), Constraint::Ratio(2, 3)))
        .legend_position((paths.len() > 1).then_some(LegendPosition::TopLeft))
        .x_axis(Axis::default().bounds([0.0, x_max]))
        .y_axis(Axis::default().bounds([lo - pad, hi + pad]).labels([
            Span::styled(fmt_num(lo), muted),
            Span::styled(fmt_num(hi), muted),
        ]));
    f.render_widget(chart, area);
}
