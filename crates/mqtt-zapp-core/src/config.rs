//! Saved broker profiles and app settings, stored as TOML in the config dir.
//! Passwords are never stored here: `PasswordSource` only names where to get one.

use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

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

/// The MQTT protocol version spoken to the broker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MqttVersion {
    #[default]
    #[serde(rename = "3.1.1")]
    V311,
    #[serde(rename = "5")]
    V5,
}

impl MqttVersion {
    pub fn is_default(&self) -> bool {
        *self == MqttVersion::V311
    }

    pub fn label(self) -> &'static str {
        match self {
            MqttVersion::V311 => "3.1.1",
            MqttVersion::V5 => "5",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub protocol: Protocol,
    /// Left out of the file while it is the default, so existing
    /// `profiles.toml` files round-trip unchanged.
    #[serde(default, skip_serializing_if = "MqttVersion::is_default")]
    pub mqtt_version: MqttVersion,
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

/// Order of sibling topics in the tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    /// Natural name order (`tasmota_2` before `tasmota_10`).
    #[default]
    Name,
    /// Most messages first.
    Count,
    /// Most recently updated first.
    Recent,
}

impl SortOrder {
    pub fn name(self) -> &'static str {
        match self {
            SortOrder::Name => "name",
            SortOrder::Count => "count",
            SortOrder::Recent => "recent",
        }
    }

    /// name → count → recent → name.
    pub fn next(self) -> SortOrder {
        match self {
            SortOrder::Name => SortOrder::Count,
            SortOrder::Count => SortOrder::Recent,
            SortOrder::Recent => SortOrder::Name,
        }
    }
}

/// Colour theme. `NO_COLOR` in the environment forces `Mono`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    #[default]
    Dark,
    Light,
    Mono,
}

/// The default `timestamp_format` (chrono strftime).
pub const DEFAULT_TIMESTAMP_FORMAT: &str = "%H:%M:%S%.3f";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub history_depth: usize,
    pub keys: BTreeMap<String, String>,
    pub sort: SortOrder,
    pub theme: ThemeName,
    /// strftime format of the message time in the message view.
    pub timestamp_format: String,
    /// Cap on payload bytes kept in all topics' history together, in MiB.
    pub max_history_mb: usize,
    /// Payloads longer than this many KiB are cut and marked truncated.
    pub max_payload_kb: usize,
    /// Cap on topic levels in the tree; messages that need more are refused.
    pub max_topics: usize,
    /// New topics this many levels deep or less open expanded (0: none).
    pub auto_expand: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            history_depth: 100,
            keys: BTreeMap::new(),
            sort: SortOrder::Name,
            theme: ThemeName::Dark,
            timestamp_format: DEFAULT_TIMESTAMP_FORMAT.into(),
            max_history_mb: 256,
            max_payload_kb: 16384,
            max_topics: 200_000,
            auto_expand: 0,
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

/// Create `dir` if missing; a directory created here is made 0700 (an
/// existing one keeps whatever mode the user gave it).
pub(crate) fn create_private_dir(dir: &Path) -> Result<()> {
    let ctx = || format!("creating {}", dir.display());
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).with_context(ctx)?;
    }
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(ctx),
    }
}

/// Follow symlinks at `path` (including dangling ones, up to 40 hops) to the
/// file that should be replaced. A missing non-link path is returned as is.
fn resolve_write_target(path: &Path) -> Result<PathBuf> {
    let mut current = path.to_path_buf();
    let mut followed = false;
    for _ in 0..40 {
        match std::fs::canonicalize(&current) {
            Ok(p) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("resolving {}", current.display())),
        }
        match std::fs::symlink_metadata(&current) {
            Ok(md) if md.file_type().is_symlink() => {
                let target = std::fs::read_link(&current)
                    .with_context(|| format!("reading link {}", current.display()))?;
                current = match current.parent() {
                    Some(p) => p.join(target),
                    None => target,
                };
                followed = true;
            }
            Ok(_) => return Ok(current),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if followed && let Some(parent) = current.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                return Ok(current);
            }
            Err(e) => return Err(e).with_context(|| format!("resolving {}", current.display())),
        }
    }
    anyhow::bail!("too many levels of symbolic links: {}", path.display())
}

/// Write `contents` to `path` atomically (temp file in the same directory,
/// fsync, rename), with mode 0600 on unix. A symlinked target, dangling or
/// not, is written through, so the link survives.
pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    let resolved = resolve_write_target(path)?;
    let path = resolved.as_path();
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().context("path has no file name")?;
    let n: u32 = rand::thread_rng().r#gen();
    let tmp = dir.join(format!(
        ".{}.{}.{n}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
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
        create_private_dir(&dir)?;
        write_private(&dir.join("profiles.toml"), &toml::to_string_pretty(self)?)
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

    /// Add `profile` (`old_name` = `None`) or replace the profile named
    /// `old_name` in place, possibly under a new name. Refuses a name that
    /// another profile already has.
    pub fn replace(&mut self, old_name: Option<&str>, profile: Profile) -> Result<()> {
        if self
            .profiles
            .iter()
            .any(|p| p.name == profile.name && Some(p.name.as_str()) != old_name)
        {
            anyhow::bail!("profile '{}' already exists", profile.name);
        }
        match old_name {
            None => self.profiles.push(profile),
            Some(old) => match self.profiles.iter_mut().find(|p| p.name == old) {
                Some(slot) => *slot = profile,
                None => anyhow::bail!("unknown profile '{old}'"),
            },
        }
        Ok(())
    }

    /// Put the profiles named in `names` first, in that order; the rest keep
    /// their relative order after them. Unknown names are ignored.
    pub fn reorder(&mut self, names: &[String]) {
        let rank = |p: &Profile| {
            names
                .iter()
                .position(|n| *n == p.name)
                .unwrap_or(names.len())
        };
        // Stable, so unnamed profiles keep their order.
        self.profiles.sort_by_key(rank);
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

    #[test]
    fn settings_theme_and_timestamp_parse() {
        let s: Settings = toml::from_str("theme = \"mono\"\ntimestamp_format = \"%H:%M\"").unwrap();
        assert_eq!(s.theme, ThemeName::Mono);
        assert_eq!(s.timestamp_format, "%H:%M");
        let d = Settings::default();
        assert_eq!(d.theme, ThemeName::Dark);
        assert_eq!(d.timestamp_format, "%H:%M:%S%.3f");
        assert!(toml::from_str::<Settings>("theme = \"pink\"").is_err());
    }

    #[test]
    fn settings_limits_parse_and_default() {
        let d = Settings::default();
        assert_eq!((d.max_history_mb, d.auto_expand), (256, 0));
        let s: Settings = toml::from_str("max_history_mb = 8\nauto_expand = 2").unwrap();
        assert_eq!((s.max_history_mb, s.auto_expand), (8, 2));
        assert_eq!((d.max_payload_kb, d.max_topics), (16384, 200_000));
        let s: Settings = toml::from_str("max_payload_kb = 4\nmax_topics = 10").unwrap();
        assert_eq!((s.max_payload_kb, s.max_topics), (4, 10));
    }

    #[test]
    fn settings_sort_parses() {
        let s: Settings = toml::from_str("sort = \"recent\"").unwrap();
        assert_eq!(s.sort, SortOrder::Recent);
        assert_eq!(Settings::default().sort, SortOrder::Name);
    }
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn sample(name: &str) -> Profile {
        Profile {
            name: name.into(),
            host: "192.168.22.5".into(),
            port: 1883,
            protocol: Protocol::Mqtt,
            mqtt_version: Default::default(),
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
    fn mqtt_version_defaults_and_round_trips() {
        let base = "name=\"h\"\nhost=\"x\"\nport=1883\nprotocol=\"mqtt\"";
        let p: Profile = toml::from_str(base).unwrap();
        assert_eq!(p.mqtt_version, MqttVersion::V311);
        let text = toml::to_string(&p).unwrap();
        assert!(!text.contains("mqtt_version"), "{text}");
        let p: Profile = toml::from_str(&format!("{base}\nmqtt_version=\"5\"")).unwrap();
        assert_eq!(p.mqtt_version, MqttVersion::V5);
        assert!(
            toml::to_string(&p)
                .unwrap()
                .contains("mqtt_version = \"5\"")
        );
        assert!(toml::from_str::<Profile>(&format!("{base}\nmqtt_version=\"4\"")).is_err());
    }

    #[test]
    fn reorder_moves_named_first() {
        let mut s = ProfileStore::default();
        for n in ["a", "b", "c"] {
            s.upsert(sample(n));
        }
        s.reorder(&["c".into(), "a".into()]);
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["c", "a", "b"]);
        s.reorder(&["zz".into()]);
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["c", "a", "b"]);
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
    fn replace_renames_in_place() {
        let mut s = ProfileStore::default();
        s.upsert(sample("a"));
        s.upsert(sample("b"));
        s.replace(Some("a"), sample("z")).unwrap();
        let names: Vec<_> = s.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["z", "b"]);
        s.replace(None, sample("c")).unwrap();
        assert_eq!(s.profiles.len(), 3);
    }

    #[test]
    fn replace_refuses_collision() {
        let mut s = ProfileStore::default();
        s.upsert(sample("a"));
        s.upsert(sample("b"));
        let e = s.replace(Some("a"), sample("b")).unwrap_err().to_string();
        assert!(e.contains("profile 'b' already exists"), "{e}");
        assert!(s.replace(None, sample("a")).is_err());
        let e = s
            .replace(Some("nope"), sample("x"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown profile 'nope'"), "{e}");
        s.replace(Some("a"), sample("a")).unwrap(); // same name: plain edit
        assert_eq!(s.profiles.len(), 2);
    }

    #[test]
    fn protocol_ports() {
        assert_eq!(Protocol::Mqtt.default_port(), 1883);
        assert_eq!(Protocol::Mqtts.default_port(), 8883);
        assert_eq!(Protocol::Ws.default_port(), 80);
        assert_eq!(Protocol::Wss.default_port(), 443);
        assert!(Protocol::Wss.is_tls() && !Protocol::Ws.is_tls());
    }

    #[cfg(unix)]
    #[test]
    fn write_private_is_0600_and_replaces() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("f.toml");
        write_private(&path, "one").unwrap();
        write_private(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn write_private_leaves_no_temp_files() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("f.toml");
        write_private(&path, "x").unwrap();
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("f.toml")]);
    }

    #[cfg(unix)]
    #[test]
    fn profiles_saved_private() {
        use std::os::unix::fs::PermissionsExt;
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let sub = d.path().join("cfg");
        // SAFETY: env mutation is serialized by LOCK.
        unsafe { std::env::set_var("MQTT_ZAPP_CONFIG_DIR", &sub) };
        let mut s = ProfileStore::default();
        s.upsert(sample("home"));
        s.save().unwrap();
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&sub.join("profiles.toml")), 0o600);
        assert_eq!(mode(&sub), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn write_private_writes_through_symlink() {
        let d = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let real = other.path().join("real.toml");
        std::fs::write(&real, "old").unwrap();
        let link = d.path().join("profiles.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_private(&link, "new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }

    #[test]
    fn write_private_cleans_temp_on_error() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("f.toml");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("x"), "x").unwrap();
        assert!(write_private(&path, "data").is_err());
        let tmps = std::fs::read_dir(d.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(tmps, 0);
    }

    #[cfg(unix)]
    #[test]
    fn write_private_writes_through_dangling_symlink() {
        let d = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let real = other.path().join("real.toml");
        let link = d.path().join("profiles.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_private(&link, "new").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    }
}
