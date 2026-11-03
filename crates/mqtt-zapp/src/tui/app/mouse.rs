//! Mouse input: clicks focus and select, the wheel scrolls the pane under
//! the pointer. Positions are matched against the pane rectangles the
//! renderer recorded in `App::hit` during the last frame.

use super::{App, Effect, Focus, Mode};
use crate::tui::keymap::Action;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Margin, Position, Rect};

/// Lines the payload moves per wheel notch.
const WHEEL_LINES: isize = 3;

/// Where the last frame drew each clickable pane (outer rects, borders
/// included). Panes not drawn in that frame stay `Rect::default()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct HitAreas {
    pub tree: Rect,
    pub payload: Rect,
    pub history: Rect,
    /// First history row drawn (rows are newest first).
    pub history_offset: usize,
    /// The picker card; its first profile row is two rows below its top.
    pub picker: Rect,
    pub picker_skip: usize,
    /// The message header's path row; a click copies the topic.
    pub header_topic: Rect,
    /// The message header's `⧉ copy`; a click copies the topic.
    pub header_copy: Rect,
    /// The payload pane's `⧉ copy` title; a click copies the payload.
    pub payload_copy: Rect,
    /// First payload line the last frame drew (the clamped scroll).
    pub payload_skip: usize,
    /// The payload pane's title on its top border; a click copies the payload.
    pub payload_title: Rect,
    /// The broker name in the top bar; a click opens the picker.
    pub header_broker: Rect,
}

pub(super) fn inner(r: Rect) -> Rect {
    r.inner(Margin::new(1, 1))
}

impl App {
    pub fn handle_mouse(&mut self, ev: MouseEvent) -> Effect {
        let at = Position::new(ev.column, ev.row);
        let hit = self.hit.get();
        let wheel = match ev.kind {
            MouseEventKind::ScrollDown => Some(true),
            MouseEventKind::ScrollUp => Some(false),
            _ => None,
        };
        let left_click = ev.kind == MouseEventKind::Down(MouseButton::Left);
        let right_click = ev.kind == MouseEventKind::Down(MouseButton::Right);
        match self.mode {
            Mode::Help | Mode::Stats if matches!(ev.kind, MouseEventKind::Down(_)) => {
                self.mode = Mode::Browse;
                self.dirty = true;
            }
            Mode::Picker if left_click => return self.picker_click(at, hit),
            Mode::Picker if wheel.is_some() => {
                self.dirty = true;
                if wheel == Some(true) {
                    self.picker.down();
                } else {
                    self.picker.up();
                }
            }
            Mode::Browse if left_click => return self.browse_click(at, hit),
            Mode::Browse if right_click => return self.tree_copy_click(at, inner(hit.tree)),
            Mode::Browse => {
                if let Some(down) = wheel {
                    self.browse_wheel(at, hit, down);
                }
            }
            _ => {}
        }
        Effect::None
    }

    /// A click on a profile selects it; a click on the selected one connects.
    fn picker_click(&mut self, at: Position, hit: HitAreas) -> Effect {
        let card = inner(hit.picker);
        if !card.contains(at) || at.y < card.y + 1 {
            return Effect::None;
        }
        let idx = usize::from(at.y - card.y - 1) + hit.picker_skip;
        if idx >= self.picker.profiles.len() {
            return Effect::None;
        }
        self.dirty = true;
        if idx == self.picker.sel {
            return self.on_picker_action(Action::Select);
        }
        self.picker.sel = idx;
        Effect::None
    }

    fn browse_click(&mut self, at: Position, hit: HitAreas) -> Effect {
        if hit.header_broker.contains(at) {
            return self.handle_action(Action::Brokers);
        }
        // The copy targets sit on top of the panes, so they go first.
        if hit.header_copy.contains(at) || hit.header_topic.contains(at) {
            return self.copy(true);
        }
        if hit.payload_copy.contains(at) || hit.payload_title.contains(at) {
            return self.copy(false);
        }
        if hit.tree.contains(at) {
            self.dirty = true;
            self.focus = Focus::Tree;
            self.tree_click(at, inner(hit.tree));
        } else if hit.payload.contains(at) {
            self.dirty = true;
            self.focus = Focus::Payload;
            // A click on a number charts it.
            let body = inner(hit.payload);
            if body.contains(at) {
                self.chart_click(hit.payload_skip + usize::from(at.y - body.y));
            }
        } else if hit.history.contains(at) {
            self.dirty = true;
            self.focus = Focus::History;
            self.history_click(at, inner(hit.history), hit.history_offset);
        }
        Effect::None
    }

    /// Select the clicked row. A click on its `▸`/`▾` marker, or on the row
    /// that was already selected, also expands or collapses it.
    fn tree_click(&mut self, at: Position, area: Rect) {
        if !area.contains(at) {
            return;
        }
        let idx = usize::from(at.y - area.y) + self.tree_offset;
        let Some(row) = self.rows().get(idx).copied() else {
            return;
        };
        let marker = area.x + row.depth.saturating_mul(2);
        let on_marker = at.x == marker || at.x == marker + 1;
        let was_selected = self.selected == Some(row.id);
        self.select(row.id);
        if row.has_children && (on_marker || was_selected) {
            self.handle_action(Action::ToggleExpand);
        }
    }

    /// A right click on a tree row copies that row's topic; the selection
    /// stays where it was.
    fn tree_copy_click(&mut self, at: Position, area: Rect) -> Effect {
        if !area.contains(at) {
            return Effect::None;
        }
        let idx = usize::from(at.y - area.y) + self.tree_offset;
        match self.rows().get(idx) {
            Some(row) => Effect::Copy {
                what: "topic",
                text: mqtt_zapp_core::payload::clipboard_safe(
                    &self.tree.full_topic(row.id),
                    mqtt_zapp_core::payload::CLIPBOARD_MAX,
                ),
            },
            None => Effect::None,
        }
    }

    /// Pin the clicked history entry; the newest one means live.
    fn history_click(&mut self, at: Position, area: Rect, offset: usize) {
        if !area.contains(at) {
            return;
        }
        let len = self.history_len();
        let row = usize::from(at.y - area.y) + offset;
        if row >= len {
            return;
        }
        let idx = len - 1 - row;
        self.history_sel = (idx + 1 < len).then_some(idx);
        self.payload_scroll = 0;
    }

    /// The wheel scrolls the pane under the pointer without moving focus.
    fn browse_wheel(&mut self, at: Position, hit: HitAreas, down: bool) {
        self.dirty = true;
        if hit.tree.contains(at) {
            if down {
                self.move_selection(|i, n| (i + 1).min(n - 1));
            } else {
                self.move_selection(|i, _| i.saturating_sub(1));
            }
        } else if hit.payload.contains(at) {
            self.scroll_payload_by(if down { WHEEL_LINES } else { -WHEEL_LINES });
        } else if hit.history.contains(at) {
            // The list is newest first: wheel down goes to older entries.
            if down {
                self.history_older();
            } else {
                self.history_newer();
            }
        }
    }
}
