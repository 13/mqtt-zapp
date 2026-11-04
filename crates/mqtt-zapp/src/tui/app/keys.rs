//! Key handling: `handle_key` routes a key press by mode.

use super::*;

impl App {
    pub fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if key.kind == KeyEventKind::Release {
            return Effect::None;
        }
        let ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        let guarded_esc = key.code == KeyCode::Esc && self.mode == Mode::ProfileForm;
        if !ctrl_c && !guarded_esc {
            self.armed = None;
        }
        if ctrl_c {
            if self.holds_work() && !self.take_armed(Armed::Quit) {
                return self.arm(Armed::Quit, "ctrl-c again to quit");
            }
            return Effect::Quit;
        }
        match self.mode {
            Mode::Browse => match self.keymap.lookup(key) {
                Some(action) => self.handle_action(action),
                None => Effect::None,
            },
            Mode::Picker => match self.keymap.lookup(key) {
                Some(action) => self.on_picker_action(action),
                None => Effect::None,
            },
            Mode::Password => self.on_password_key(key),
            Mode::SearchInput => {
                self.on_search_key(key);
                Effect::None
            }
            Mode::Publish => self.on_publish_key(key),
            Mode::Help => {
                self.on_help_key(key);
                Effect::None
            }
            Mode::Stats => {
                self.mode = Mode::Browse;
                self.dirty = true;
                Effect::None
            }
            Mode::Confirm => self.on_confirm_key(key),
            Mode::ProfileForm => self.on_profile_form_key(key),
            Mode::FieldPicker => self.on_field_picker_key(key),
        }
    }

    /// Open the help overlay over `from` (browse or the picker), which it
    /// returns to when closed.
    pub(super) fn open_help(&mut self, from: Mode) {
        self.help_query.clear();
        self.help_scroll = 0;
        self.help_return = from;
        self.mode = Mode::Help;
        self.dirty = true;
    }

    /// Whether quitting now would throw away typed work: a filled publish
    /// form or an edited profile form.
    fn holds_work(&self) -> bool {
        match self.mode {
            Mode::Publish => self.publish.as_ref().is_some_and(|f| !f.is_blank()),
            Mode::ProfileForm => self.profile_form.as_ref().is_some_and(|f| f.is_modified()),
            _ => false,
        }
    }

    /// Arm `what` and say how to confirm it.
    pub(super) fn arm(&mut self, what: Armed, note: &str) -> Effect {
        self.dirty = true;
        let now = Instant::now();
        self.armed = Some((what, now));
        self.set_note(Note::info(note), now);
        Effect::None
    }

    /// Whether `what` was armed within `ARM_WINDOW`; disarms either way.
    pub(super) fn take_armed(&mut self, what: Armed) -> bool {
        matches!(self.armed.take(), Some((w, t)) if w == what && t.elapsed() < ARM_WINDOW)
    }

    /// A bracketed paste: text for the focused input, never keys. Single-line
    /// inputs take the first line; modes without an input drop it.
    pub fn handle_paste(&mut self, text: &str) -> Effect {
        use crate::tui::profile_form::first_line;
        self.dirty = true;
        match self.mode {
            Mode::Publish => {
                if let Some(form) = self.publish.as_mut() {
                    form.paste(text);
                }
                self.sent_walk = None;
            }
            Mode::ProfileForm => {
                if let Some(form) = self.profile_form.as_mut() {
                    form.paste(text);
                }
            }
            Mode::SearchInput => {
                self.search.push_str(&first_line(text));
                self.schedule_filter();
            }
            Mode::Password => {
                if let Some(st) = self.password.as_mut() {
                    st.input.push_str(&first_line(text));
                }
            }
            Mode::FieldPicker => {
                if let Some(p) = self.field_picker.as_mut() {
                    p.query.push_str(&first_line(text));
                    p.sel = 0;
                }
            }
            Mode::Help => {
                self.help_query.push_str(&first_line(text));
                self.help_scroll = 0;
            }
            Mode::Browse | Mode::Picker | Mode::Stats | Mode::Confirm => {}
        }
        Effect::None
    }

    /// The help overlay types into its own query and never reaches the
    /// keymap, so bound keys like `f` or `q` are just letters here.
    fn on_help_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        match key.code {
            KeyCode::Enter => self.mode = self.help_return,
            KeyCode::Esc if self.help_query.is_empty() => self.mode = self.help_return,
            KeyCode::Esc => {
                self.help_query.clear();
                self.help_scroll = 0;
            }
            KeyCode::Backspace => {
                self.help_query.pop();
                self.help_scroll = 0;
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                let (max, page) = self.help_view.get();
                let at = self.help_scroll.min(max);
                self.help_scroll = match key.code {
                    KeyCode::Up => at.saturating_sub(1),
                    KeyCode::Down => (at + 1).min(max),
                    KeyCode::PageUp => at.saturating_sub(page),
                    _ => (at + page).min(max),
                };
            }
            KeyCode::Char('?') if self.help_query.is_empty() => self.mode = self.help_return,
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.help_query.push(c);
                self.help_scroll = 0;
            }
            _ => {}
        }
    }
}
