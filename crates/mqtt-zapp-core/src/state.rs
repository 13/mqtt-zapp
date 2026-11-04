//! Data mqtt-zapp keeps for itself between runs (picker sort, when each
//! profile was last used). It lives in `state.toml` in the XDG state dir,
//! never in the user's config files, and is best effort: a missing or broken
//! file means defaults.

use crate::config::{config_dir, create_private_dir, write_private};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// How the connection picker orders profiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickerSort {
    /// The order of `profiles.toml`, changed with `J`/`K`.
    #[default]
    Custom,
    /// Natural sort on the profile name.
    Name,
    /// Most recently connected first; never-used profiles last.
    LastUsed,
}

impl PickerSort {
    pub fn name(self) -> &'static str {
        match self {
            PickerSort::Custom => "custom",
            PickerSort::Name => "name",
            PickerSort::LastUsed => "last used",
        }
    }

    pub fn next(self) -> PickerSort {
        match self {
            PickerSort::Custom => PickerSort::Name,
            PickerSort::Name => PickerSort::LastUsed,
            PickerSort::LastUsed => PickerSort::Custom,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppState {
    pub picker_sort: PickerSort,
    /// Profile name → unix seconds of the last connect.
    pub last_used: BTreeMap<String, i64>,
    /// Profile name → full topics starred in that profile's sessions.
    pub favorites: BTreeMap<String, BTreeSet<String>>,
}

/// `MQTT_ZAPP_STATE_DIR`, else `MQTT_ZAPP_CONFIG_DIR` (keeps a relocated
/// config self-contained), else the XDG state dir, else the config dir.
pub fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("MQTT_ZAPP_STATE_DIR") {
        return PathBuf::from(d);
    }
    if std::env::var_os("MQTT_ZAPP_CONFIG_DIR").is_some() {
        return config_dir();
    }
    directories::ProjectDirs::from("", "", "mqtt-zapp")
        .and_then(|d| d.state_dir().map(|p| p.to_path_buf()))
        .unwrap_or_else(config_dir)
}

impl AppState {
    /// Never fails: a missing or unreadable file gives the defaults.
    pub fn load() -> AppState {
        std::fs::read_to_string(state_dir().join("state.toml"))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Write atomically (temp file + rename), creating the directory.
    pub fn save(&self) -> Result<()> {
        let dir = state_dir();
        create_private_dir(&dir)?;
        write_private(&dir.join("state.toml"), &toml::to_string_pretty(self)?)
    }

    pub fn touch(&mut self, name: &str, now: i64) {
        self.last_used.insert(name.to_owned(), now);
    }

    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(t) = self.last_used.remove(old) {
            self.last_used.insert(new.to_owned(), t);
        }
    }

    pub fn forget(&mut self, name: &str) {
        self.last_used.remove(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn in_temp_dir() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        // SAFETY: env mutation is serialized by LOCK; only these tests touch it.
        unsafe { std::env::set_var("MQTT_ZAPP_STATE_DIR", d.path().join("nested")) };
        (g, d)
    }

    #[test]
    fn state_round_trip() {
        let (_g, _d) = in_temp_dir();
        let mut s = AppState::default();
        s.touch("a", 10);
        s.picker_sort = PickerSort::Name;
        s.save().unwrap();
        assert_eq!(AppState::load(), s);
    }

    #[test]
    fn state_missing_file_defaults() {
        let (_g, _d) = in_temp_dir();
        assert_eq!(AppState::load(), AppState::default());
    }

    #[test]
    fn state_corrupt_file_defaults() {
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        std::fs::write(state_dir().join("state.toml"), "not = [toml").unwrap();
        assert_eq!(AppState::load(), AppState::default());
    }

    #[test]
    fn picker_sort_cycles() {
        assert_eq!(PickerSort::default(), PickerSort::Custom);
        assert_eq!(PickerSort::Custom.next(), PickerSort::Name);
        assert_eq!(PickerSort::Name.next(), PickerSort::LastUsed);
        assert_eq!(PickerSort::LastUsed.next(), PickerSort::Custom);
        assert_eq!(PickerSort::LastUsed.name(), "last used");
    }

    #[test]
    fn rename_and_forget() {
        let mut s = AppState::default();
        s.touch("a", 1);
        s.rename("a", "b");
        assert_eq!(s.last_used, BTreeMap::from([("b".to_string(), 1)]));
        s.forget("b");
        assert!(s.last_used.is_empty());
    }

    #[test]
    fn old_state_without_favorites_loads() {
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        std::fs::write(
            state_dir().join("state.toml"),
            "picker_sort = \"name\"\n\n[last_used]\na = 5\n",
        )
        .unwrap();
        let s = AppState::load();
        assert_eq!(s.picker_sort, PickerSort::Name);
        assert_eq!(s.last_used["a"], 5);
        assert!(s.favorites.is_empty());
    }
}
