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
            // Tree-only keys do nothing while the payload has focus.
            Action::Collapse | Action::Expand | Action::ToggleExpand | Action::Select
                if self.focus == Focus::Payload => {}
            Action::Up => self.move_selection(|i, _| i.saturating_sub(1)),
            Action::Down => self.move_selection(|i, n| (i + 1).min(n - 1)),
            Action::FocusNext => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Payload,
                    Focus::Payload if self.history_shown.get() => Focus::History,
                    Focus::Payload | Focus::History => Focus::Tree,
                }
            }
            Action::ToggleDiff => {
                self.show_diff = !self.show_diff;
                self.payload_scroll = 0;
            }
            Action::MarkDiffBase => self.mark_diff_base(),
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
            Action::Help => {
                self.help_query.clear();
                self.mode = Mode::Help;
            }
            Action::CycleSort => {
                self.sort = self.sort.next();
                self.set_note(
                    Note::info(format!("sort: {}", self.sort.name())),
                    Instant::now(),
                );
            }
            Action::Chart => self.cycle_chart(),
            Action::ChartPin => self.pin_chart(),
            Action::Stats => self.mode = Mode::Stats,
            Action::Freeze => {
                self.toggle_freeze();
                let note = if self.frozen {
                    "display frozen – f to resume"
                } else {
                    "display live"
                };
                self.set_note(Note::info(note), Instant::now());
            }
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
