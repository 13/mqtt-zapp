//! Saved broker profiles and app settings, stored as TOML in the config dir.
//! Passwords are never stored here: `PasswordSource` only names where to get one.

use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Mqtt,
    Mqtts,
    Ws,
    Wss,
}

impl Protocol {
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Mqtt => 1883,
            Protocol::Mqtts => 8883,
            Protocol::Ws => 80,
            Protocol::Wss => 443,
        }
    }

    pub fn is_tls(self) -> bool {
        matches!(self, Protocol::Mqtts | Protocol::Wss)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    pub ca_file: Option<PathBuf>,
    pub client_cert: Option<PathBuf>,
    pub client_key: Option<PathBuf>,
    pub insecure: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PasswordSource {
    #[default]
    None,
    Keyring,
    Prompt,
}

pub fn default_ws_path() -> String {
    "/".into()
}

pub fn default_subscriptions() -> Vec<String> {
    vec!["#".into(), "$SYS/#".into()]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub protocol: Protocol,
    #[serde(default = "default_ws_path")]
    pub ws_path: String,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: PasswordSource,
    #[serde(default)]
    pub tls: TlsConfig,
    #[serde(default = "default_subscriptions")]
    pub subscriptions: Vec<String>,
}

impl Profile {
    /// The configured client id, or a fresh `mqtt-zapp-xxxxxx` on every call.
    /// Callers should keep the result for the whole session.
    pub fn effective_client_id(&self) -> String {
        if let Some(id) = &self.client_id {
            return id.clone();
        }
        let n: u32 = rand::thread_rng().gen_range(0..1 << 24);
        format!("mqtt-zapp-{n:06x}")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileStore {
    #[serde(rename = "profile", default)]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub history_depth: usize,
    pub keys: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            history_depth: 100,
            keys: BTreeMap::new(),
        }
    }
}

/// `MQTT_ZAPP_CONFIG_DIR`, else the XDG config dir for mqtt-zapp.
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("MQTT_ZAPP_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    directories::ProjectDirs::from("", "", "mqtt-zapp")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".mqtt-zapp"))
}

fn read_or_default<T: Default + serde::de::DeserializeOwned>(file: &str) -> Result<T> {
    let path = config_dir().join(file);
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

impl ProfileStore {
    pub fn load() -> Result<Self> {
        read_or_default("profiles.toml")
    }

    pub fn save(&self) -> Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("profiles.toml");
        std::fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    pub fn upsert(&mut self, profile: Profile) {
        match self.profiles.iter_mut().find(|p| p.name == profile.name) {
            Some(slot) => *slot = profile,
            None => self.profiles.push(profile),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.name != name);
        self.profiles.len() != before
    }
}

impl Settings {
    pub fn load() -> Result<Self> {
        read_or_default("settings.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn sample(name: &str) -> Profile {
        Profile {
            name: name.into(),
            host: "192.168.22.5".into(),
            port: 1883,
            protocol: Protocol::Mqtt,
            ws_path: "/".into(),
            client_id: None,
            username: Some("u".into()),
            password: PasswordSource::Keyring,
            tls: TlsConfig::default(),
            subscriptions: vec!["#".into(), "$SYS/#".into()],
        }
    }

    #[test]
    fn defaults_fill_in() {
        let p: Profile =
            toml::from_str("name=\"home\"\nhost=\"192.168.22.5\"\nport=1883\nprotocol=\"mqtt\"")
                .unwrap();
        assert_eq!(p.subscriptions, vec!["#", "$SYS/#"]);
        assert_eq!(p.ws_path, "/");
        assert!(matches!(p.password, PasswordSource::None));
    }

    #[test]
    fn store_round_trip_and_never_contains_password() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        // SAFETY: env mutation is serialized by LOCK; only tests touch this var.
        unsafe { std::env::set_var("MQTT_ZAPP_CONFIG_DIR", d.path()) };
        let mut s = ProfileStore::default();
        s.upsert(sample("home"));
        s.save().unwrap();
        let text = std::fs::read_to_string(d.path().join("profiles.toml")).unwrap();
        for l in text
            .lines()
            .filter(|l| l.trim_start().starts_with("password"))
        {
            assert!(
                ["\"none\"", "\"keyring\"", "\"prompt\""]
                    .iter()
                    .any(|v| l.trim_end().ends_with(v)),
                "{l}"
            );
        }
        assert_eq!(
            ProfileStore::load().unwrap().get("home").unwrap().host,
            "192.168.22.5"
        );
    }

    #[test]
    fn save_creates_missing_dir() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("nested");
        // SAFETY: env mutation is serialized by LOCK.
        unsafe { std::env::set_var("MQTT_ZAPP_CONFIG_DIR", &sub) };
        ProfileStore::default().save().unwrap();
        assert!(sub.join("profiles.toml").exists());
    }

    #[test]
    fn client_id_generated_format() {
        let id = sample("x").effective_client_id();
        assert!(id.starts_with("mqtt-zapp-") && id.len() == 16);
        assert!(
            id[10..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        let mut p = sample("x");
        p.client_id = Some("fixed".into());
        assert_eq!(p.effective_client_id(), "fixed");
    }

    #[test]
    fn settings_missing_file_defaults() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        // SAFETY: env mutation is serialized by LOCK.
        unsafe { std::env::set_var("MQTT_ZAPP_CONFIG_DIR", d.path()) };
        let s = Settings::load().unwrap();
        assert_eq!(s.history_depth, 100);
        assert!(s.keys.is_empty());
    }

    #[test]
    fn upsert_replaces_by_name() {
        let mut s = ProfileStore::default();
        s.upsert(sample("a"));
        let mut b = sample("a");
        b.port = 1;
        s.upsert(b);
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.get("a").unwrap().port, 1);
        assert!(s.remove("a"));
        assert!(!s.remove("a"));
    }

    #[test]
    fn protocol_ports() {
        assert_eq!(Protocol::Mqtt.default_port(), 1883);
        assert_eq!(Protocol::Mqtts.default_port(), 8883);
        assert_eq!(Protocol::Ws.default_port(), 80);
        assert_eq!(Protocol::Wss.default_port(), 443);
        assert!(Protocol::Wss.is_tls() && !Protocol::Ws.is_tls());
    }
}
