//! The picker side of `App`: connecting (with the password prompt), the
//! profile form, picker sorting and reordering, and the remembered state.

use super::*;
use mqtt_zapp_core::config::Protocol;

impl App {
    /// Start connecting to `p`, resolving its password: connect right away,
    /// or switch to `Mode::Password` (returning `Effect::None`) when the user
    /// has to type it.
    pub fn begin_connect(&mut self, p: Profile) -> Effect {
        self.begin_connect_with(p, load_password)
    }

    /// `begin_connect` with the keyring lookup injected, so tests never
    /// touch the real keyring.
    pub(super) fn begin_connect_with(
        &mut self,
        p: Profile,
        keyring: impl FnOnce(&str) -> Result<Option<String>>,
    ) -> Effect {
        let password = match p.password {
            PasswordSource::None => None,
            // A failed or empty lookup falls back to asking.
            PasswordSource::Keyring => match keyring(&p.name) {
                Ok(Some(pw)) => Some(pw),
                Ok(None) | Err(_) => return self.ask_password(p),
            },
            PasswordSource::Prompt => return self.ask_password(p),
        };
        self.connect_effect(p, password)
    }

    fn ask_password(&mut self, profile: Profile) -> Effect {
        self.password = Some(PasswordState {
            profile,
            input: String::new(),
        });
        self.mode = Mode::Password;
        self.dirty = true;
        Effect::None
    }

    /// Connected (or reconnecting) to a broker. The picker can then be
    /// left with esc, back to the live session.
    pub fn in_session(&self) -> bool {
        self.server.is_some()
    }

    /// From browse: the picker, on the connected profile. The session keeps
    /// running underneath until another profile is picked.
    pub(super) fn open_brokers(&mut self) {
        let profiles = std::mem::take(&mut self.picker.profiles);
        let connected = self.connected.clone();
        self.picker.set_profiles(profiles, connected.as_deref());
        self.mode = Mode::Picker;
    }

    /// The selected profile is the one the session runs on, unedited, and
    /// the connection has not given up (a hard failure never retries).
    fn is_live_session_selected(&self) -> bool {
        let gave_up = matches!(
            self.conn_state,
            ConnState::Disconnected { retry_in_ms: 0, .. }
        );
        self.connected.is_some()
            && !gave_up
            && self.picker.selected() == self.session_profile.as_ref()
    }

    fn connect_effect(&mut self, p: Profile, password: Option<String>) -> Effect {
        // Only saved profiles; an ad-hoc `--host` connect has no picker entry.
        if self.picker.profiles.iter().any(|o| o.name == p.name) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            self.picker.last_used.insert(p.name.clone(), now);
        }
        self.connected = self.picker.profiles.contains(&p).then(|| p.name.clone());
        self.session_profile = Some(p.clone());
        self.conn_state = ConnState::Connecting;
        self.server = Some(ServerInfo {
            name: p.name.clone(),
            url: crate::tui::picker::server_url(&p),
            subscriptions: p.subscriptions.clone(),
            warning: security_warning(&p),
        });
        self.mode = Mode::Browse;
        self.dirty = true;
        Effect::Connect(p, password)
    }

    /// Keys in the picker: through the keymap.
    pub(super) fn on_picker_action(&mut self, action: Action) -> Effect {
        self.dirty = true;
        match action {
            Action::Quit => return Effect::Quit,
            Action::Cancel | Action::Brokers if self.in_session() => self.mode = Mode::Browse,
            Action::Select if self.is_live_session_selected() => {
                self.mode = Mode::Browse;
                let note = format!(
                    "already connected to {}",
                    self.connected.as_deref().unwrap_or("")
                );
                self.set_note(Note::info(note), Instant::now());
            }
            Action::Up => self.picker.up(),
            Action::Down => self.picker.down(),
            Action::CycleSort => {
                self.picker.cycle_sort();
                return Effect::SaveState(self.app_state());
            }
            Action::MoveUp | Action::MoveDown if self.store_error.is_some() => {
                self.set_note(Note::error(STORE_BROKEN), Instant::now());
            }
            Action::MoveUp | Action::MoveDown => {
                if self.picker.move_selected(action == Action::MoveDown) {
                    return Effect::SaveOrder(self.picker.order());
                }
            }
            Action::Select => {
                if let Some(p) = self.picker.selected().cloned() {
                    return self.begin_connect(p);
                }
            }
            Action::AddProfile | Action::EditProfile | Action::DeleteProfile
                if self.store_error.is_some() =>
            {
                self.set_note(Note::error(STORE_BROKEN), Instant::now());
            }
            Action::AddProfile => {
                let taken = self
                    .picker
                    .profiles
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();
                self.profile_form = Some(ProfileForm::new(None, taken));
                self.mode = Mode::ProfileForm;
            }
            Action::EditProfile => {
                if let Some(p) = self.picker.selected() {
                    let taken = self
                        .picker
                        .profiles
                        .iter()
                        .filter(|o| o.name != p.name)
                        .map(|o| o.name.clone())
                        .collect();
                    self.profile_form = Some(ProfileForm::new(Some(p), taken));
                    self.mode = Mode::ProfileForm;
                }
            }
            Action::DeleteProfile => {
                if let Some(p) = self.picker.selected() {
                    self.confirm = Some(Confirm {
                        prompt: format!("delete profile {}?", p.name),
                        action: ConfirmAction::DeleteProfile(p.name.clone()),
                    });
                    self.mode = Mode::Confirm;
                }
            }
            _ => {}
        }
        Effect::None
    }

    /// Keys in the profile form; they bypass the keymap. A save keeps the
    /// form open until `profile_saved` or `profile_save_failed`.
    pub(super) fn on_profile_form_key(&mut self, key: KeyEvent) -> Effect {
        self.dirty = true;
        let Some(form) = self.profile_form.as_mut() else {
            self.mode = Mode::Picker;
            return Effect::None;
        };
        match form.on_key(key) {
            ProfileOutcome::Continue => Effect::None,
            ProfileOutcome::Cancel => {
                self.profile_form = None;
                self.mode = Mode::Picker;
                Effect::None
            }
            ProfileOutcome::Save(edit) => Effect::SaveProfile(edit),
        }
    }

    /// A profile save or delete went through: show the new list, select
    /// `select` (or keep the position), close the form.
    pub fn profile_saved(&mut self, profiles: Vec<Profile>, select: Option<&str>, note: &str) {
        self.picker.set_profiles(profiles, select);
        self.profile_form = None;
        self.mode = Mode::Picker;
        self.set_note(Note::info(note), Instant::now());
    }

    /// A profile save or delete failed: show why in the form, if open.
    /// Apply what `state.toml` remembered (picker sort, last-used times).
    pub fn set_app_state(&mut self, state: AppState) {
        self.picker.sort = state.picker_sort;
        self.picker.last_used = state.last_used;
        self.picker.sel = 0;
        self.favorites = state.favorites;
        self.star_gen += 1;
    }

    /// What `state.toml` should hold now.
    pub fn app_state(&self) -> AppState {
        AppState {
            picker_sort: self.picker.sort,
            last_used: self.picker.last_used.clone(),
            favorites: self.favorites.clone(),
        }
    }

    pub fn state_save_failed(&mut self, err: &str) {
        self.set_note(
            Note::error(format!("could not save state: {err}")),
            Instant::now(),
        );
        self.dirty = true;
    }

    /// A saved profile edit renamed `old` to `new`: its last-used time follows.
    pub fn profile_renamed(&mut self, old: &str, new: &str) {
        if let Some(t) = self.picker.last_used.remove(old) {
            self.picker.last_used.insert(new.to_owned(), t);
        }
        if let Some(f) = self.favorites.remove(old) {
            self.favorites.insert(new.to_owned(), f);
            self.star_gen += 1;
        }
        if self.connected.as_deref() == Some(old) {
            self.connected = Some(new.to_owned());
        }
        // Stars are keyed by the session's profile name: follow the rename.
        if let Some(p) = self.session_profile.as_mut().filter(|p| p.name == old) {
            p.name = new.to_owned();
            self.star_gen += 1;
        }
    }

    /// A profile edit was saved (`original` = its old name, if any).
    pub fn profile_edit_saved(
        &mut self,
        profiles: Vec<Profile>,
        original: Option<&str>,
        name: &str,
    ) {
        // Rename the last-used time first: in last-used sort the view
        // position of `name` depends on it.
        if let Some(old) = original.filter(|o| *o != name) {
            self.profile_renamed(old, name);
        }
        self.profile_saved(profiles, Some(name), &format!("saved {name}"));
    }

    pub fn profile_deleted(&mut self, name: &str) {
        self.picker.last_used.remove(name);
        if self.connected.as_deref() == Some(name) {
            self.connected = None;
        }
    }

    pub fn profile_save_failed(&mut self, err: String) {
        match self.profile_form.as_mut() {
            Some(form) => form.error = Some(err),
            None => self.set_note(Note::error(err), Instant::now()),
        }
        self.dirty = true;
    }

    /// Keys in the password prompt. They bypass the keymap, so `q` and `j`
    /// are just characters. The password is never echoed or logged.
    pub(super) fn on_password_key(&mut self, key: KeyEvent) -> Effect {
        self.dirty = true;
        let Some(st) = self.password.as_mut() else {
            self.mode = Mode::Picker;
            return Effect::None;
        };
        match key.code {
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                st.input.push(c);
            }
            KeyCode::Backspace => {
                st.input.pop();
            }
            KeyCode::Enter => {
                let st = self.password.take().expect("checked above");
                return self.connect_effect(st.profile, Some(st.input));
            }
            KeyCode::Esc => {
                self.password = None;
                if self.picker.profiles.is_empty() {
                    return Effect::Quit;
                }
                self.mode = Mode::Picker;
            }
            _ => {}
        }
        Effect::None
    }
}

/// What is unsafe about this connection, for the header.
pub fn security_warning(p: &Profile) -> Option<&'static str> {
    match p.protocol {
        Protocol::Mqtts | Protocol::Wss if p.tls.insecure => Some("unverified tls"),
        Protocol::Mqtt | Protocol::Ws
            if p.username.is_some() && p.password != PasswordSource::None =>
        {
            Some("password in cleartext")
        }
        _ => None,
    }
}
