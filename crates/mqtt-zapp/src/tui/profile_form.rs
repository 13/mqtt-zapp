//! The add/edit profile form: one line per profile field. State, keys and
//! validation live here; the card is drawn by `render`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::config::{
    PasswordSource, Profile, Protocol, TlsConfig, default_subscriptions, default_ws_path,
};
use std::fmt;
use std::path::PathBuf;

/// A single-line text input with a cursor (a char index).
#[derive(Clone, Default)]
pub struct LineInput {
    text: String,
    cursor: usize,
}

impl LineInput {
    /// `text` with the cursor at its end.
    pub fn new(text: &str) -> LineInput {
        LineInput {
            text: text.to_owned(),
            cursor: text.chars().count(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(b, _)| b)
    }

    /// Edit with `key`; false if the key is not an editing key.
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        let len = self.text.chars().count();
        match key.code {
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let at = self.byte_at(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(len),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = len,
            KeyCode::Backspace | KeyCode::Delete => {}
            _ => return false,
        }
        true
    }
}

/// The form's rows, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PField {
    Name,
    Host,
    Port,
    Protocol,
    WsPath,
    Username,
    PasswordSource,
    Password,
    ClientId,
    Subscriptions,
    CaFile,
    ClientCert,
    ClientKey,
    Insecure,
}

impl PField {
    pub const ALL: [PField; 14] = [
        PField::Name,
        PField::Host,
        PField::Port,
        PField::Protocol,
        PField::WsPath,
        PField::Username,
        PField::PasswordSource,
        PField::Password,
        PField::ClientId,
        PField::Subscriptions,
        PField::CaFile,
        PField::ClientCert,
        PField::ClientKey,
        PField::Insecure,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PField::Name => "name",
            PField::Host => "host",
            PField::Port => "port",
            PField::Protocol => "protocol",
            PField::WsPath => "ws path",
            PField::Username => "username",
            PField::PasswordSource => "password",
            PField::Password => "secret",
            PField::ClientId => "client id",
            PField::Subscriptions => "subscriptions",
            PField::CaFile => "ca file",
            PField::ClientCert => "client cert",
            PField::ClientKey => "client key",
            PField::Insecure => "insecure",
        }
    }
}

#[allow(clippy::large_enum_variant)] // short-lived, consumed right away
pub enum ProfileOutcome {
    Continue,
    Cancel,
    Save(ProfileEdit),
}

/// A validated profile to store. `password` is set only for a keyring
/// profile when the user typed one; it goes to the keyring, never the file.
pub struct ProfileEdit {
    /// The name the profile had before the edit; `None` for a new one.
    pub original: Option<String>,
    pub profile: Profile,
    pub password: Option<String>,
}

// Hand-written so the typed password can never reach a debug log.
impl fmt::Debug for ProfileEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileEdit")
            .field("original", &self.original)
            .field("profile", &self.profile)
            .field("password", &self.password.as_ref().map(|_| "<set>"))
            .finish()
    }
}

pub struct ProfileForm {
    pub field: PField,
    /// Shown in the title; cleared by the next key.
    pub error: Option<String>,
    /// The name of the profile being edited; `None` when adding.
    pub original: Option<String>,
    /// Whether the edited profile already keeps its password in the keyring.
    pub had_keyring: bool,
    /// Names of the other profiles.
    taken: Vec<String>,
    pub protocol: Protocol,
    pub source: PasswordSource,
    pub insecure: bool,
    name: LineInput,
    host: LineInput,
    port: LineInput,
    ws_path: LineInput,
    username: LineInput,
    password: LineInput,
    client_id: LineInput,
    subscriptions: LineInput,
    ca_file: LineInput,
    client_cert: LineInput,
    client_key: LineInput,
}

// Hand-written so the typed password can never reach a debug log.
impl fmt::Debug for ProfileForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileForm")
            .field("original", &self.original)
            .field("field", &self.field)
            .field("name", &self.name.text)
            .field("host", &self.host.text)
            .field(
                "password",
                &format_args!("<{} chars>", self.password.text.chars().count()),
            )
            .finish_non_exhaustive()
    }
}

fn path_text(p: &Option<PathBuf>) -> LineInput {
    LineInput::new(
        &p.as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    )
}

fn optional(l: &LineInput) -> Option<String> {
    let t = l.text.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, forward: bool) -> T {
    let i = all.iter().position(|x| *x == cur).unwrap_or(0);
    let n = all.len();
    all[if forward {
        (i + 1) % n
    } else {
        (i + n - 1) % n
    }]
}

const PROTOCOLS: [Protocol; 4] = [Protocol::Mqtt, Protocol::Mqtts, Protocol::Ws, Protocol::Wss];
const SOURCES: [PasswordSource; 3] = [
    PasswordSource::None,
    PasswordSource::Keyring,
    PasswordSource::Prompt,
];

impl ProfileForm {
    /// A form for `existing`, or for a new profile. `taken` lists the names
    /// of the other profiles.
    pub fn new(existing: Option<&Profile>, taken: Vec<String>) -> ProfileForm {
        let fresh;
        let p = match existing {
            Some(p) => p,
            None => {
                fresh = Profile {
                    name: String::new(),
                    host: String::new(),
                    port: Protocol::Mqtt.default_port(),
                    protocol: Protocol::Mqtt,
                    ws_path: default_ws_path(),
                    client_id: None,
                    username: None,
                    password: PasswordSource::None,
                    tls: TlsConfig::default(),
                    subscriptions: default_subscriptions(),
                };
                &fresh
            }
        };
        let opt = |s: &Option<String>| LineInput::new(s.as_deref().unwrap_or(""));
        ProfileForm {
            field: PField::Name,
            error: None,
            original: existing.map(|p| p.name.clone()),
            had_keyring: existing.is_some_and(|p| p.password == PasswordSource::Keyring),
            taken,
            protocol: p.protocol,
            source: p.password,
            insecure: p.tls.insecure,
            name: LineInput::new(&p.name),
            host: LineInput::new(&p.host),
            port: LineInput::new(&p.port.to_string()),
            ws_path: LineInput::new(&p.ws_path),
            username: opt(&p.username),
            password: LineInput::default(),
            client_id: opt(&p.client_id),
            subscriptions: LineInput::new(&p.subscriptions.join(" ")),
            ca_file: path_text(&p.tls.ca_file),
            client_cert: path_text(&p.tls.client_cert),
            client_key: path_text(&p.tls.client_key),
        }
    }

    /// The rows shown for the current protocol and password source.
    pub fn visible_fields(&self) -> Vec<PField> {
        PField::ALL
            .into_iter()
            .filter(|f| match f {
                PField::WsPath => matches!(self.protocol, Protocol::Ws | Protocol::Wss),
                PField::Password => self.source == PasswordSource::Keyring,
                PField::CaFile | PField::ClientCert | PField::ClientKey | PField::Insecure => {
                    self.protocol.is_tls()
                }
                _ => true,
            })
            .collect()
    }

    /// The text input behind a row; `None` for the choice rows.
    pub fn input(&self, f: PField) -> Option<&LineInput> {
        Some(match f {
            PField::Name => &self.name,
            PField::Host => &self.host,
            PField::Port => &self.port,
            PField::WsPath => &self.ws_path,
            PField::Username => &self.username,
            PField::Password => &self.password,
            PField::ClientId => &self.client_id,
            PField::Subscriptions => &self.subscriptions,
            PField::CaFile => &self.ca_file,
            PField::ClientCert => &self.client_cert,
            PField::ClientKey => &self.client_key,
            PField::Protocol | PField::PasswordSource | PField::Insecure => return None,
        })
    }

    fn input_mut(&mut self, f: PField) -> Option<&mut LineInput> {
        Some(match f {
            PField::Name => &mut self.name,
            PField::Host => &mut self.host,
            PField::Port => &mut self.port,
            PField::WsPath => &mut self.ws_path,
            PField::Username => &mut self.username,
            PField::Password => &mut self.password,
            PField::ClientId => &mut self.client_id,
            PField::Subscriptions => &mut self.subscriptions,
            PField::CaFile => &mut self.ca_file,
            PField::ClientCert => &mut self.client_cert,
            PField::ClientKey => &mut self.client_key,
            PField::Protocol | PField::PasswordSource | PField::Insecure => return None,
        })
    }

    fn step(&mut self, forward: bool) {
        let fields = self.visible_fields();
        self.field = match fields.iter().position(|f| *f == self.field) {
            Some(i) => cycle(&fields, fields[i], forward),
            None => fields[0],
        };
    }

    fn set_protocol(&mut self, p: Protocol) {
        if self.port.text == self.protocol.default_port().to_string() {
            self.port = LineInput::new(&p.default_port().to_string());
        }
        self.protocol = p;
    }

    pub fn on_key(&mut self, key: KeyEvent) -> ProfileOutcome {
        self.error = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return ProfileOutcome::Cancel,
            KeyCode::Char('s') if ctrl => return self.save(),
            KeyCode::Tab | KeyCode::Down | KeyCode::Enter => self.step(true),
            KeyCode::BackTab | KeyCode::Up => self.step(false),
            _ => {
                let change = match key.code {
                    KeyCode::Char(' ') | KeyCode::Right => Some(true),
                    KeyCode::Left => Some(false),
                    _ => None,
                };
                match (self.field, change) {
                    (PField::Protocol, Some(fwd)) => {
                        self.set_protocol(cycle(&PROTOCOLS, self.protocol, fwd))
                    }
                    (PField::PasswordSource, Some(fwd)) => {
                        self.source = cycle(&SOURCES, self.source, fwd)
                    }
                    (PField::Insecure, Some(_)) => self.insecure = !self.insecure,
                    (f, _) => {
                        if let Some(input) = self.input_mut(f) {
                            input.on_key(key);
                        }
                    }
                }
            }
        }
        ProfileOutcome::Continue
    }

    fn save(&mut self) -> ProfileOutcome {
        match self.build() {
            Ok(edit) => ProfileOutcome::Save(edit),
            Err(e) => {
                self.error = Some(e);
                ProfileOutcome::Continue
            }
        }
    }

    fn build(&self) -> Result<ProfileEdit, String> {
        let name = self.name.text.trim();
        if name.is_empty() {
            return Err("name is required".into());
        }
        if self.taken.iter().any(|t| t == name) {
            return Err(format!("profile '{name}' already exists"));
        }
        let host = self.host.text.trim();
        if host.is_empty() {
            return Err("host is required".into());
        }
        let port = match self.port.text.trim().parse::<u16>() {
            Ok(p) if p > 0 => p,
            _ => return Err("port must be 1–65535".into()),
        };
        let keyring = self.source == PasswordSource::Keyring;
        if keyring && self.password.text.is_empty() && !self.had_keyring {
            return Err("type a password, or use prompt".into());
        }
        let subscriptions: Vec<String> = self
            .subscriptions
            .text
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if subscriptions.is_empty() {
            return Err("at least one subscription".into());
        }
        let tls = self.protocol.is_tls();
        let path = |l: &LineInput| optional(l).filter(|_| tls).map(PathBuf::from);
        let ws_path = self.ws_path.text.trim();
        Ok(ProfileEdit {
            original: self.original.clone(),
            profile: Profile {
                name: name.to_owned(),
                host: host.to_owned(),
                port,
                protocol: self.protocol,
                ws_path: if ws_path.is_empty() {
                    default_ws_path()
                } else {
                    ws_path.to_owned()
                },
                client_id: optional(&self.client_id),
                username: optional(&self.username),
                password: self.source,
                tls: TlsConfig {
                    ca_file: path(&self.ca_file),
                    client_cert: path(&self.client_cert),
                    client_key: path(&self.client_key),
                    insecure: tls && self.insecure,
                },
                subscriptions,
            },
            password: (keyring && !self.password.text.is_empty())
                .then(|| self.password.text.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mqtt_zapp_core::config::{PasswordSource, Profile, Protocol, TlsConfig};

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_str(f: &mut ProfileForm, s: &str) {
        for c in s.chars() {
            f.on_key(key(KeyCode::Char(c)));
        }
    }

    fn goto(f: &mut ProfileForm, field: PField) {
        for _ in 0..20 {
            if f.field == field {
                return;
            }
            f.on_key(key(KeyCode::Tab));
        }
        panic!("{field:?} not reachable");
    }

    fn sample(name: &str, pw: PasswordSource) -> Profile {
        Profile {
            name: name.into(),
            host: "h".into(),
            port: 8883,
            protocol: Protocol::Mqtts,
            ws_path: "/".into(),
            client_id: Some("cid".into()),
            username: Some("u".into()),
            password: pw,
            tls: TlsConfig {
                ca_file: Some("/ca.pem".into()),
                client_cert: None,
                client_key: None,
                insecure: true,
            },
            subscriptions: vec!["#".into()],
        }
    }

    /// Fill name and host so the form saves.
    fn filled() -> ProfileForm {
        let mut f = ProfileForm::new(None, vec![]);
        type_str(&mut f, "home");
        goto(&mut f, PField::Host);
        type_str(&mut f, "h");
        f
    }

    fn save(f: &mut ProfileForm) -> Option<ProfileEdit> {
        match f.on_key(ctrl('s')) {
            ProfileOutcome::Save(e) => Some(e),
            _ => None,
        }
    }

    #[test]
    fn line_input_edits_mid_line() {
        let mut l = LineInput::new("");
        for c in "hst".chars() {
            l.on_key(key(KeyCode::Char(c)));
        }
        l.on_key(key(KeyCode::Left));
        l.on_key(key(KeyCode::Left));
        l.on_key(key(KeyCode::Char('o')));
        assert_eq!(l.text(), "host");
        l.on_key(key(KeyCode::Home));
        l.on_key(key(KeyCode::Delete));
        assert_eq!(l.text(), "ost");
        l.on_key(key(KeyCode::End));
        l.on_key(key(KeyCode::Backspace));
        assert_eq!(l.text(), "os");
        assert!(!l.on_key(ctrl('a')), "ctrl chars are not text");
        assert_eq!(l.text(), "os");
    }

    #[test]
    fn line_input_counts_chars_not_bytes() {
        let mut l = LineInput::new("pä");
        assert_eq!(l.cursor(), 2);
        l.on_key(key(KeyCode::Backspace));
        assert_eq!(l.text(), "p");
    }

    #[test]
    fn new_form_defaults() {
        use PField::*;
        let f = ProfileForm::new(None, vec![]);
        assert_eq!(f.field, Name);
        assert_eq!(
            f.visible_fields(),
            [
                Name,
                Host,
                Port,
                Protocol,
                Username,
                PasswordSource,
                ClientId,
                Subscriptions
            ]
        );
        assert_eq!(f.input(Port).unwrap().text(), "1883");
        assert_eq!(f.input(Subscriptions).unwrap().text(), "# $SYS/#");
    }

    #[test]
    fn protocol_cycle_moves_default_port_only() {
        let mut f = ProfileForm::new(None, vec![]);
        goto(&mut f, PField::Protocol);
        f.on_key(key(KeyCode::Char(' ')));
        assert_eq!(f.protocol, Protocol::Mqtts);
        assert_eq!(f.input(PField::Port).unwrap().text(), "8883");
        assert!(f.visible_fields().contains(&PField::CaFile));
        assert!(f.visible_fields().contains(&PField::Insecure));
        goto(&mut f, PField::Port);
        f.on_key(key(KeyCode::Backspace));
        f.on_key(key(KeyCode::Backspace));
        type_str(&mut f, "00"); // 8800
        goto(&mut f, PField::Protocol);
        f.on_key(key(KeyCode::Right));
        assert_eq!(f.protocol, Protocol::Ws);
        assert_eq!(
            f.input(PField::Port).unwrap().text(),
            "8800",
            "custom port kept"
        );
        assert!(f.visible_fields().contains(&PField::WsPath));
        assert!(!f.visible_fields().contains(&PField::CaFile));
        f.on_key(key(KeyCode::Left));
        assert_eq!(f.protocol, Protocol::Mqtts);
    }

    #[test]
    fn save_builds_profile() {
        let mut f = filled();
        goto(&mut f, PField::Subscriptions);
        f.on_key(key(KeyCode::End));
        for _ in 0.."# $SYS/#".len() {
            f.on_key(key(KeyCode::Backspace));
        }
        type_str(&mut f, "a/#  b");
        let e = save(&mut f).expect("saves");
        assert_eq!(e.original, None);
        assert_eq!(e.password, None);
        let p = e.profile;
        assert_eq!(p.name, "home");
        assert_eq!(p.host, "h");
        assert_eq!(p.port, 1883);
        assert_eq!(p.protocol, Protocol::Mqtt);
        assert_eq!(p.subscriptions, ["a/#", "b"]);
        assert_eq!(p.username, None);
        assert_eq!(p.client_id, None);
        assert_eq!(p.ws_path, "/");
        assert_eq!(p.password, PasswordSource::None);
        assert_eq!(p.tls, TlsConfig::default());
    }

    #[test]
    fn save_rejects_taken_name() {
        let mut f = ProfileForm::new(None, vec!["home".into()]);
        type_str(&mut f, "home");
        goto(&mut f, PField::Host);
        type_str(&mut f, "h");
        assert!(matches!(f.on_key(ctrl('s')), ProfileOutcome::Continue));
        assert_eq!(f.error.as_deref(), Some("profile 'home' already exists"));
    }

    #[test]
    fn save_validates_in_order() {
        let mut f = ProfileForm::new(None, vec![]);
        assert!(save(&mut f).is_none());
        assert_eq!(f.error.as_deref(), Some("name is required"));
        type_str(&mut f, "n");
        assert!(save(&mut f).is_none());
        assert_eq!(f.error.as_deref(), Some("host is required"));
        goto(&mut f, PField::Host);
        type_str(&mut f, "h");
        goto(&mut f, PField::Port);
        f.on_key(key(KeyCode::End));
        for _ in 0..4 {
            f.on_key(key(KeyCode::Backspace));
        }
        type_str(&mut f, "0");
        assert!(save(&mut f).is_none());
        assert_eq!(f.error.as_deref(), Some("port must be 1–65535"));
        f.on_key(key(KeyCode::Backspace));
        type_str(&mut f, "70000");
        assert!(save(&mut f).is_none());
        f.on_key(key(KeyCode::End));
        for _ in 0..5 {
            f.on_key(key(KeyCode::Backspace));
        }
        type_str(&mut f, "1883");
        goto(&mut f, PField::Subscriptions);
        f.on_key(key(KeyCode::End));
        for _ in 0..10 {
            f.on_key(key(KeyCode::Backspace));
        }
        assert!(save(&mut f).is_none());
        assert_eq!(f.error.as_deref(), Some("at least one subscription"));
        f.on_key(key(KeyCode::Char('x')));
        assert_eq!(f.error, None, "a key clears the error");
    }

    #[test]
    fn keyring_password_required_unless_already_stored() {
        // New profile: required.
        let mut f = filled();
        goto(&mut f, PField::PasswordSource);
        f.on_key(key(KeyCode::Char(' ')));
        assert_eq!(f.source, PasswordSource::Keyring);
        assert!(f.visible_fields().contains(&PField::Password));
        assert!(save(&mut f).is_none());
        assert_eq!(f.error.as_deref(), Some("type a password, or use prompt"));
        goto(&mut f, PField::Password);
        type_str(&mut f, "pw");
        assert_eq!(save(&mut f).unwrap().password.as_deref(), Some("pw"));

        // Editing a keyring profile: blank keeps the stored one.
        let p = sample("a", PasswordSource::Keyring);
        let mut f = ProfileForm::new(Some(&p), vec![]);
        assert_eq!(save(&mut f).unwrap().password, None);

        // Editing a profile that had no stored password: required.
        let p = sample("a", PasswordSource::Prompt);
        let mut f = ProfileForm::new(Some(&p), vec![]);
        goto(&mut f, PField::PasswordSource);
        f.on_key(key(KeyCode::Left));
        assert_eq!(f.source, PasswordSource::Keyring);
        assert!(save(&mut f).is_none());
    }

    #[test]
    fn password_dropped_unless_keyring() {
        let mut f = filled();
        goto(&mut f, PField::PasswordSource);
        f.on_key(key(KeyCode::Char(' ')));
        goto(&mut f, PField::Password);
        type_str(&mut f, "pw");
        goto(&mut f, PField::PasswordSource);
        f.on_key(key(KeyCode::Char(' '))); // -> prompt
        let e = save(&mut f).unwrap();
        assert_eq!(e.profile.password, PasswordSource::Prompt);
        assert_eq!(e.password, None);
    }

    #[test]
    fn edit_prefills_and_keeps_original() {
        let p = sample("a", PasswordSource::None);
        let mut f = ProfileForm::new(Some(&p), vec!["other".into()]);
        assert_eq!(f.input(PField::Name).unwrap().text(), "a");
        assert_eq!(f.input(PField::Port).unwrap().text(), "8883");
        assert_eq!(f.input(PField::CaFile).unwrap().text(), "/ca.pem");
        type_str(&mut f, "b");
        let e = save(&mut f).unwrap();
        assert_eq!(e.original.as_deref(), Some("a"));
        let mut want = p.clone();
        want.name = "ab".into();
        assert_eq!(e.profile, want);
    }

    #[test]
    fn esc_cancels_and_enter_moves_on() {
        let mut f = ProfileForm::new(None, vec![]);
        f.on_key(key(KeyCode::Enter));
        assert_eq!(f.field, PField::Host);
        f.on_key(key(KeyCode::BackTab));
        assert_eq!(f.field, PField::Name);
        f.on_key(key(KeyCode::BackTab));
        assert_eq!(f.field, PField::Subscriptions, "wraps");
        assert!(matches!(
            f.on_key(key(KeyCode::Esc)),
            ProfileOutcome::Cancel
        ));
    }

    #[test]
    fn insecure_toggles() {
        let p = sample("a", PasswordSource::None);
        let mut f = ProfileForm::new(Some(&p), vec![]);
        goto(&mut f, PField::Insecure);
        f.on_key(key(KeyCode::Char(' ')));
        assert!(!save(&mut f).unwrap().profile.tls.insecure);
    }

    #[test]
    fn debug_hides_password() {
        let mut f = filled();
        goto(&mut f, PField::PasswordSource);
        f.on_key(key(KeyCode::Char(' ')));
        goto(&mut f, PField::Password);
        type_str(&mut f, "s3cret");
        assert!(!format!("{f:?}").contains("s3cret"));
        let e = save(&mut f).unwrap();
        assert!(!format!("{e:?}").contains("s3cret"));
    }
}
