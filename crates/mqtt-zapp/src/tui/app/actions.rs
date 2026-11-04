//! Browse-mode actions.

use super::*;

impl App {
    /// Browse-mode actions.
    pub fn handle_action(&mut self, action: Action) -> Effect {
        self.dirty = true;
        match action {
            Action::Quit => return Effect::Quit,
            // The list is newest first: down the screen is older.
            Action::Up if self.focus == Focus::History => self.history_newer(),
            Action::Down if self.focus == Focus::History => self.history_older(),
            Action::Up if self.focus == Focus::Payload => self.scroll_payload_by(-1),
            Action::Down if self.focus == Focus::Payload => self.scroll_payload_by(1),
            Action::Top if self.focus == Focus::Payload => self.payload_scroll = 0,
            Action::Bottom if self.focus == Focus::Payload => self.scroll_payload_by(isize::MAX),
            Action::Top if self.focus == Focus::History => self.history_newest(),
            Action::Bottom if self.focus == Focus::History => self.history_oldest(),
            // Tree-only keys do nothing while the payload or history has focus.
            Action::Collapse | Action::Expand | Action::ToggleExpand | Action::Select
                if self.focus != Focus::Tree => {}
            Action::Up => self.move_selection(|i, _| i.saturating_sub(1)),
            Action::Down => self.move_selection(|i, n| (i + 1).min(n - 1)),
            Action::FocusNext => {
                self.focus = match self.focus {
                    // The docked form can take the whole message pane.
                    Focus::Tree if !self.message_shown.get() => Focus::Tree,
                    Focus::Tree => Focus::Payload,
                    Focus::Payload if self.history_shown.get() => Focus::History,
                    Focus::Payload | Focus::History => Focus::Tree,
                };
                // Past the last pane, an open publish form is next.
                if self.focus == Focus::Tree && self.publish.is_some() {
                    self.enter_publish();
                }
            }
            Action::ToggleDiff => {
                self.show_diff = !self.show_diff;
                self.payload_scroll = 0;
            }
            Action::MarkDiffBase => self.mark_diff_base(),
            Action::ToggleWrap => {
                self.wrap = !self.wrap;
                self.payload_scroll = 0;
                let note = match (self.wrap, self.chart.is_some()) {
                    (true, true) => "wrap on (after the chart closes)",
                    (true, false) => "wrap on",
                    (false, _) => "wrap off",
                };
                self.set_note(Note::info(note), Instant::now());
            }
            Action::Brokers => self.open_brokers(),
            Action::DeleteTopic => self.ask_delete_topic(),
            Action::ScrollDown => self.scroll_payload(true),
            Action::ScrollUp => self.scroll_payload(false),
            Action::Top => self.move_selection(|_, _| 0),
            Action::Bottom => self.move_selection(|_, n| n - 1),
            Action::Expand => {
                if let Some(row) = self.selected_row().filter(|r| r.has_children) {
                    self.expanded.insert(row.id);
                }
            }
            Action::ToggleExpand | Action::Select => {
                if let Some(row) = self.selected_row().filter(|r| r.has_children)
                    && !self.expanded.remove(&row.id)
                {
                    self.expanded.insert(row.id);
                }
            }
            Action::Collapse => self.collapse(),
            Action::Search => self.mode = Mode::SearchInput,
            Action::Cancel => {
                if !self.close_chart() {
                    self.clear_search();
                }
            }
            Action::Publish => self.open_publish(),
            Action::Help => self.open_help(Mode::Browse),
            Action::CycleSort => {
                self.sort = self.sort.next();
                self.set_note(
                    Note::info(format!("sort: {}", self.sort.name())),
                    Instant::now(),
                );
            }
            Action::Chart => self.cycle_chart(),
            Action::ChartPin => self.pin_chart(),
            Action::ChartPrev => self.cycle_chart_back(),
            Action::ChartPick => self.open_field_picker(),
            Action::Stats => self.mode = Mode::Stats,
            Action::Freeze => {
                self.toggle_freeze();
                let note = if self.frozen {
                    let f = crate::tui::ui::key_for(self, Action::Freeze);
                    format!("display frozen · {f} to resume")
                } else {
                    "display live".into()
                };
                self.set_note(Note::info(note), Instant::now());
            }
            Action::ToggleStar => return self.toggle_star(),
            Action::StarredOnly => self.toggle_starred_only(),
            Action::SavePayload => self.save_payload(),
            Action::SaveHistory => self.save_history(),
            // Picker-only.
            Action::AddProfile
            | Action::EditProfile
            | Action::DeleteProfile
            | Action::MoveUp
            | Action::MoveDown => {}
            Action::ExpandAll => self.expand_subtree(true),
            Action::CollapseAll => self.expand_subtree(false),
            Action::DeleteRetained => self.ask_clear_retained(false),
            Action::ClearRetained => self.ask_clear_retained(true),
            Action::CopyTopic => return self.copy(true),
            Action::CopyPayload => return self.copy(false),
        }
        Effect::None
    }
}
