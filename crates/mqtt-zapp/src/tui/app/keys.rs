//! Key handling: `handle_key` routes a key press by mode.

use super::*;

impl App {
    pub fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if key.kind == KeyEventKind::Release {
            return Effect::None;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
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
        }
    }

    /// The help overlay types into its own query and never reaches the
    /// keymap, so bound keys like `f` or `q` are just letters here.
    fn on_help_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        match key.code {
            KeyCode::Enter => self.mode = Mode::Browse,
            KeyCode::Esc if self.help_query.is_empty() => self.mode = Mode::Browse,
            KeyCode::Esc => self.help_query.clear(),
            KeyCode::Backspace => {
                self.help_query.pop();
            }
            KeyCode::Char('?') if self.help_query.is_empty() => self.mode = Mode::Browse,
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.help_query.push(c);
            }
            _ => {}
        }
    }
}
