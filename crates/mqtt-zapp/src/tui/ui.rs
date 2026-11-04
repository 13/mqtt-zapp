//! Drawing. Every string that came from the broker (topic levels, payloads,
//! disconnect reasons) goes through `sanitize`/`preview` before it reaches
//! the terminal.

use super::app::{App, Focus, HitAreas, Mode};
use super::keymap::Action;
use super::rows::Row;
use super::theme;
use mqtt_zapp_core::TopicTree;
use mqtt_zapp_core::client::ConnState;
use mqtt_zapp_core::payload::{preview, sanitize, says_offline};
use mqtt_zapp_core::search::{Filter, QueryMode};
use mqtt_zapp_core::tree::{NodeId, TopicNode};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::time::{Duration, Instant, SystemTime};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const MIN_WIDTH: u16 = 40;
pub const MIN_HEIGHT: u16 = 10;
/// Narrower terminals show the tree above the message pane.
const STACK_BELOW: u16 = 80;
/// Topic names that got a message within `app::FLASH` are drawn bold yellow.
pub const FLASH_MODIFIER: Modifier = Modifier::BOLD;
const FLASH_FG: Color = Color::Yellow;
const PREVIEW_CHARS: usize = 60;
/// Longest footer note; broker error text can be long.
const NOTE_MAX_CHARS: usize = 120;
/// Search matches in tree rows.
pub(super) const MATCH_MODIFIER: Modifier = Modifier::BOLD.union(Modifier::UNDERLINED);

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    app.hit.set(Default::default());
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        f.render_widget(Paragraph::new("terminal too small"), area);
        return;
    }
    match app.mode {
        Mode::Picker | Mode::Password | Mode::ProfileForm => render_front(f, app, area),
        Mode::Confirm if on_front(app) => render_front(f, app, area),
        _ => render_browse(f, app, area),
    }
    theme::apply(f.buffer_mut(), area, app.theme);
}

/// Whether a confirm question belongs to the picker (deleting a profile).
fn on_front(app: &App) -> bool {
    matches!(
        app.confirm,
        Some(super::app::Confirm {
            action: super::app::ConfirmAction::DeleteProfile(_),
            ..
        })
    )
}

/// The picker and the password prompt: a centred card above the footer.
fn render_front(f: &mut Frame, app: &App, area: Rect) {
    let [main, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    match (app.mode, &app.password) {
        (Mode::ProfileForm, _) => {
            let _ =
                super::picker::render_picker(f, main, &app.picker, None, app.connected.as_deref());
            if let Some(form) = &app.profile_form {
                super::profile_form::render(f, main, form);
            }
        }
        (Mode::Confirm, _) => {
            let _ =
                super::picker::render_picker(f, main, &app.picker, None, app.connected.as_deref());
            if let Some(c) = &app.confirm {
                render_confirm(f, main, c);
            }
        }
        (Mode::Password, Some(st)) => {
            if !app.picker.profiles.is_empty() {
                let _ = super::picker::render_picker(
                    f,
                    main,
                    &app.picker,
                    None,
                    app.connected.as_deref(),
                );
                theme::dim_backdrop(f, main);
            }
            super::picker::render_password(f, main, st)
        }
        _ => {
            let (card, skip) = super::picker::render_picker(
                f,
                main,
                &app.picker,
                app.store_error.as_deref(),
                app.connected.as_deref(),
            );
            app.hit.set(HitAreas {
                picker: card,
                picker_skip: skip,
                ..app.hit.get()
            });
        }
    }
    f.render_widget(Paragraph::new(footer_line(app, area.width)), status);
}

fn render_browse(f: &mut Frame, app: &mut App, area: Rect) {
    let [header, main, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);
    let (top, broker) = header_line(app, area.width);
    f.render_widget(Paragraph::new(top), header);
    app.hit.set(HitAreas {
        header_broker: broker.map_or(Rect::default(), |(x, w)| Rect {
            x: header.x + x,
            width: w,
            ..header
        }),
        ..app.hit.get()
    });
    // Payloads need the width; below `STACK_BELOW` columns the panes stack.
    let stacked = main.width < STACK_BELOW;
    let [tree, message] = if stacked {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main)
    } else {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(main)
    };
    render_tree(f, app, tree);
    super::message_view::settle_history_focus(app, message, stacked);
    super::message_view::render_message(f, message, app, stacked);
    if let (Mode::Publish, Some(form)) = (app.mode, &app.publish) {
        super::publish::render(f, main, form);
    }
    if app.mode == Mode::Help {
        render_help(f, main, app);
    }
    if app.mode == Mode::Stats {
        render_stats(f, main, app);
    }
    if let (Mode::FieldPicker, Some(p)) = (app.mode, &app.field_picker) {
        super::field_picker::render(f, main, app, p);
    }
    if let (Mode::Confirm, Some(c)) = (app.mode, &app.confirm) {
        render_confirm(f, main, c);
    }
    f.render_widget(Paragraph::new(footer_line(app, area.width)), status);
}

fn render_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Tree;
    app.hit.set(HitAreas {
        tree: area,
        ..app.hit.get()
    });
    let block = theme::pane("topics", focused);
    let height = block.inner(area).height as usize;
    let rows = app.rows();
    app.set_drawn_rows(&rows);
    let selected = app
        .selected
        .and_then(|s| rows.iter().position(|r| r.id == s));
    app.tree_offset = scroll_offset(app.tree_offset, selected, rows.len(), height);

    if rows.is_empty() {
        let inner = block.inner(area);
        f.render_widget(block, area);
        render_tree_empty(f, app, inner);
        return;
    }
    let now = Instant::now();
    let stale_after = (app.stale_after_s > 0).then(|| Duration::from_secs(app.stale_after_s));
    let width = block.inner(area).width as usize;
    let query = app.filter.as_ref();
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(app.tree_offset)
        .take(height)
        .map(|(i, row)| {
            let flash = app.is_flashing(row.id, now);
            let line = Line::from(row_spans(&app.tree, row, query, width, flash, stale_after));
            if Some(i) == selected {
                theme::select(line, focused)
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// The tree pane with no rows: waiting for the first message, or a filter
/// that matches nothing.
fn render_tree_empty(f: &mut Frame, app: &App, area: Rect) {
    let muted = Style::new().fg(theme::MUTED);
    let mut lines = match &app.filter {
        Some(flt) => vec![Line::styled(
            format!("no topics match \"{}\"", single_line(&flt.query)),
            muted,
        )],
        None => vec![Line::styled("waiting for messages…", muted)],
    };
    if let (None, Some(server)) = (&app.filter, &app.server) {
        lines.push(Line::styled(
            format!(
                "subscribed to {}",
                single_line(&server.subscriptions.join(", "))
            ),
            muted,
        ));
    }
    let top = area.height.saturating_sub(lines.len() as u16) / 2;
    let [_, body] = Layout::vertical([Constraint::Length(top), Constraint::Min(0)]).areas(area);
    f.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        body,
    );
}

/// Keep the selected row inside a window of `height` rows.
fn scroll_offset(offset: usize, selected: Option<usize>, len: usize, height: usize) -> usize {
    let mut offset = offset;
    if let Some(s) = selected {
        if s < offset {
            offset = s;
        } else if height > 0 && s >= offset + height {
            offset = s + 1 - height;
        }
    }
    offset.min(len.saturating_sub(height))
}

/// `▸ name  preview ……… count`, indented by depth, the count right-aligned
/// to `width` (dropped when it does not fit). `query` matches in the name and
/// the preview are highlighted; `flash` colours the name.
fn row_spans<'a>(
    tree: &TopicTree,
    row: &Row,
    query: Option<&Filter>,
    width: usize,
    flash: bool,
    stale_after: Option<Duration>,
) -> Vec<Span<'a>> {
    let node = tree.node(row.id);
    let marker = match (row.has_children, row.expanded) {
        (false, _) => ' ',
        (true, false) => '▸',
        (true, true) => '▾',
    };
    let name = if node.name.is_empty() {
        "∅".to_owned()
    } else {
        single_line(&node.name)
    };
    let mut spans = vec![
        Span::raw("  ".repeat(row.depth as usize)),
        Span::styled(format!("{marker} "), Style::new().fg(theme::ACCENT)),
    ];
    let mut name_style = Style::new();
    if node.latest.is_some() {
        name_style = name_style.add_modifier(Modifier::BOLD);
    }
    if flash {
        name_style = name_style.fg(FLASH_FG).add_modifier(FLASH_MODIFIER);
    }
    highlight(&mut spans, name, query, name_style);
    if node.retained {
        spans.push(Span::styled(" ®", Style::new().fg(theme::MUTED)));
    }
    let offline = node_offline(tree, row.id);
    let stale = !offline
        && stale_after.is_some_and(|limit| {
            node.latest.as_ref().is_some_and(|m| {
                SystemTime::now()
                    .duration_since(m.received_at)
                    .is_ok_and(|age| age > limit)
            })
        });
    if offline {
        spans.push(Span::styled(" offline", Style::new().fg(theme::ERR)));
    } else if stale {
        spans.push(Span::styled(" stale", Style::new().fg(theme::MUTED)));
    }
    if let Some(m) = &node.latest {
        spans.push(Span::raw("  "));
        highlight(
            &mut spans,
            preview(&m.payload, PREVIEW_CHARS),
            query,
            Style::new().fg(theme::MUTED),
        );
    }
    let count = if node.topic_count > 0 {
        format!("{} · {}t", node.msg_count, node.topic_count)
    } else {
        node.msg_count.to_string()
    };
    let used = spans_width(&spans);
    let count_w = count.chars().count();
    if used + 1 + count_w <= width {
        spans.push(Span::raw(" ".repeat(width - used - count_w)));
        spans.push(Span::styled(count, Style::new().fg(theme::MUTED)));
    }
    if stale {
        for span in &mut spans {
            span.style = span.style.add_modifier(Modifier::DIM);
        }
    }
    spans
}

/// The node's own latest payload, or a direct child's, says offline.
fn node_offline(tree: &TopicTree, id: NodeId) -> bool {
    let node = tree.node(id);
    let says = |n: &TopicNode| {
        n.latest
            .as_ref()
            .is_some_and(|m| says_offline(&n.name, &m.payload))
    };
    says(node) || node.children.values().any(|&c| says(tree.node(c)))
}

/// Push `text` as spans, styling the byte ranges where `query` matches.
fn highlight<'a>(out: &mut Vec<Span<'a>>, text: String, query: Option<&Filter>, base: Style) {
    let ranges = query.map(|f| f.ranges(&text)).unwrap_or_default();
    let mut at = 0;
    for r in ranges {
        if r.start > at {
            out.push(Span::styled(text[at..r.start].to_owned(), base));
        }
        out.push(Span::styled(
            text[r.clone()].to_owned(),
            base.add_modifier(MATCH_MODIFIER),
        ));
        at = r.end;
    }
    if at < text.len() {
        out.push(Span::styled(text[at..].to_owned(), base));
    }
}

fn single_line(s: &str) -> String {
    sanitize(s).replace('\n', "\u{23ce}")
}

mod bars;
mod overlays;
pub(super) use bars::*;
use overlays::*;
pub(super) use overlays::{highlight_ranges, match_in};

#[cfg(test)]
mod tests;
