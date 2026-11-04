//! The add/edit profile form: one line per profile field. State, keys and
//! validation live here; the card is drawn by `render`.

use super::theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::client::parse_sub;
use mqtt_zapp_core::config::{
    MqttVersion, PasswordSource, Profile, Protocol, TlsConfig, default_subscriptions,
    default_ws_path,
};
use mqtt_zapp_core::payload::sanitize;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::fmt;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

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
    /// MQTT 3.1.1 or 5.
    Version,
    WsPath,
    Username,
    PasswordSource,
    Password,
    ClientId,
    Subscriptions,
    /// MQTT 5: ask the broker to send retained messages on subscribe.
    Retained,
    /// Keep-alive in seconds; empty is the default.
    Keepalive,
    /// Unchecked asks the broker to keep the session.
    CleanSession,
    /// MQTT 5: session expiry in seconds; empty is the default.
    SessionExpiry,
    CaFile,
    ClientCert,
    ClientKey,
    Insecure,
}

impl PField {
    pub const ALL: [PField; 19] = [
        PField::Name,
        PField::Host,
        PField::Port,
        PField::Protocol,
        PField::Version,
        PField::WsPath,
        PField::Username,
        PField::PasswordSource,
        PField::Password,
        PField::ClientId,
        PField::Subscriptions,
        PField::Retained,
        PField::Keepalive,
        PField::CleanSession,
        PField::SessionExpiry,
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
            PField::Version => "mqtt version",
            PField::WsPath => "ws path",
            PField::Username => "username",
            PField::PasswordSource => "password",
            PField::Password => "secret",
            PField::ClientId => "client id",
            PField::Subscriptions => "subscriptions",
            PField::Retained => "retained",
            PField::Keepalive => "keepalive",
            PField::CleanSession => "clean session",
            PField::SessionExpiry => "session expiry",
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
    pub version: MqttVersion,
    pub source: PasswordSource,
    pub insecure: bool,
    pub retained: bool,
    pub clean_session: bool,
    name: LineInput,
    host: LineInput,
    port: LineInput,
    ws_path: LineInput,
    username: LineInput,
    password: LineInput,
    client_id: LineInput,
    keepalive: LineInput,
    session_expiry: LineInput,
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

const VERSIONS: [MqttVersion; 2] = [MqttVersion::V311, MqttVersion::V5];
const PROTOCOLS: [Protocol; 4] = [Protocol::Mqtt, Protocol::Mqtts, Protocol::Ws, Protocol::Wss];
#[cfg(feature = "keyring")]
const SOURCES: &[PasswordSource] = &[
    PasswordSource::None,
    PasswordSource::Keyring,
    PasswordSource::Prompt,
];
/// Without keyring support only an existing keyring profile shows it.
#[cfg(not(feature = "keyring"))]
const SOURCES: &[PasswordSource] = &[PasswordSource::None, PasswordSource::Prompt];

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
                    mqtt_version: Default::default(),
                    ws_path: default_ws_path(),
                    client_id: None,
                    username: None,
                    password: PasswordSource::None,
                    tls: TlsConfig::default(),
                    subscriptions: default_subscriptions(),
                    retained: true,
                    keepalive_s: None,
                    clean_session: None,
                    session_expiry_s: None,
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
            version: p.mqtt_version,
            source: p.password,
            insecure: p.tls.insecure,
            retained: p.retained,
            clean_session: p.clean_session != Some(false),
            keepalive: LineInput::new(&p.keepalive_s.map(|v| v.to_string()).unwrap_or_default()),
            session_expiry: LineInput::new(
                &p.session_expiry_s
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            ),
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
            PField::Keepalive => &self.keepalive,
            PField::SessionExpiry => &self.session_expiry,
            PField::CaFile => &self.ca_file,
            PField::ClientCert => &self.client_cert,
            PField::ClientKey => &self.client_key,
            PField::Protocol
            | PField::Version
            | PField::PasswordSource
            | PField::Insecure
            | PField::CleanSession
            | PField::Retained => {
                return None;
            }
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
            PField::Keepalive => &mut self.keepalive,
            PField::SessionExpiry => &mut self.session_expiry,
            PField::CaFile => &mut self.ca_file,
            PField::ClientCert => &mut self.client_cert,
            PField::ClientKey => &mut self.client_key,
            PField::Protocol
            | PField::Version
            | PField::PasswordSource
            | PField::Insecure
            | PField::CleanSession
            | PField::Retained => {
                return None;
            }
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
                    (PField::Version, Some(fwd)) => {
                        self.version = cycle(&VERSIONS, self.version, fwd)
                    }
                    (PField::PasswordSource, Some(fwd)) => {
                        self.source = cycle(SOURCES, self.source, fwd)
                    }
                    (PField::Insecure, Some(_)) => self.insecure = !self.insecure,
                    (PField::Retained, Some(_)) => self.retained = !self.retained,
                    (PField::CleanSession, Some(_)) => self.clean_session = !self.clean_session,
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
        for s in &subscriptions {
            parse_sub(s)?;
        }
        let keepalive_s = match optional(&self.keepalive) {
            None => None,
            Some(t) => Some(t.parse::<u16>().map_err(|_| "keepalive: 0..=65535 s")?),
        };
        let session_expiry_s = match optional(&self.session_expiry) {
            None => None,
            Some(t) => Some(
                t.parse::<u32>()
                    .map_err(|_| "session expiry: 0..=4294967295 s")?,
            ),
        };
        let tls = self.protocol.is_tls();
        let path = |l: &LineInput| optional(l).filter(|_| tls).map(PathBuf::from);
        let ws_path = self.ws_path.text.trim();
        let profile = Profile {
            name: name.to_owned(),
            host: host.to_owned(),
            port,
            protocol: self.protocol,
            mqtt_version: self.version,
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
            retained: self.retained,
            keepalive_s,
            clean_session: (!self.clean_session).then_some(false),
            session_expiry_s,
        };
        if let Some(e) = profile.session_error() {
            return Err(e);
        }
        Ok(ProfileEdit {
            original: self.original.clone(),
            profile,
            password: (keyring && !self.password.text.is_empty())
                .then(|| self.password.text.clone()),
        })
    }

    /// A note shown instead of the key hint: set, but without effect.
    pub fn warning(&self) -> Option<&'static str> {
        let expiry = optional(&self.session_expiry)?;
        (self.version == MqttVersion::V311 && expiry.parse::<u32>().is_ok())
            .then_some("session expiry needs MQTT 5")
    }
}

pub const HINT: &str = "ctrl-s save  tab next  space change  esc cancel";
const LABEL_WIDTH: usize = 14;

mod render;
pub use render::render;

#[cfg(test)]
mod tests;
