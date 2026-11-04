//! Data mqtt-zapp keeps for itself between runs (picker sort, when each
//! profile was last used). It lives in `state.toml` in the XDG state dir,
//! never in the user's config files. A missing file means defaults. A file
//! that does not parse is backed up and its good keys are kept; if it cannot
//! be read or backed up, saving is disabled so it is never overwritten.

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
    /// Set by `load` when the file on disk could be neither read nor backed
    /// up: `save` then does nothing, so the file is not overwritten.
    #[serde(skip)]
    pub read_only: bool,
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

/// Where `load` copies a state file it could not fully parse.
pub fn backup_path() -> PathBuf {
    state_dir().join("state.toml.bak")
}

impl AppState {
    /// Never fails. A missing file gives the defaults. A file that does not
    /// parse is copied to `state.toml.bak` (replacing an older backup) and
    /// the returned text says why. Each top-level key is parsed on its own,
    /// so one bad key keeps the others.
    pub fn load() -> (AppState, Option<String>) {
        let path = state_dir().join("state.toml");
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (AppState::default(), None);
            }
            Err(e) => {
                let state = AppState {
                    read_only: true,
                    ..AppState::default()
                };
                return (state, Some(e.to_string()));
            }
        };
        let text = String::from_utf8(bytes);
        let mut state = AppState::default();
        let mut errors = Vec::new();
        let table = match &text {
            Ok(t) => t.parse::<toml::Table>().map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match table {
            Err(e) => errors.push(e),
            Ok(table) => {
                fn key<T: serde::de::DeserializeOwned>(
                    table: &toml::Table,
                    name: &str,
                    slot: &mut T,
                    errors: &mut Vec<String>,
                ) {
                    if let Some(v) = table.get(name) {
                        match v.clone().try_into() {
                            Ok(x) => *slot = x,
                            Err(e) => errors.push(format!("{name}: {e}")),
                        }
                    }
                }
                key(&table, "picker_sort", &mut state.picker_sort, &mut errors);
                key(&table, "last_used", &mut state.last_used, &mut errors);
                key(&table, "favorites", &mut state.favorites, &mut errors);
            }
        }
        if errors.is_empty() {
            return (state, None);
        }
        let mut msg = errors.join("; ");
        if let Err(e) = std::fs::copy(&path, backup_path()) {
            state.read_only = true;
            msg = format!("{msg}; backup failed: {e}");
        }
        (state, Some(msg))
    }

    /// The startup note for a load error `err` returned with `self`.
    pub fn problem_note(&self, err: &str) -> String {
        if self.read_only {
            format!("state file unreadable ({err}), not saving this session")
        } else {
            format!(
                "state file unreadable, backed up to {}",
                backup_path().display()
            )
        }
    }

    /// Write atomically (temp file + rename), creating the directory.
    pub fn save(&self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
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
        assert_eq!(AppState::load(), (s, None));
    }

    #[test]
    fn state_missing_file_defaults() {
        let (_g, _d) = in_temp_dir();
        assert_eq!(AppState::load(), (AppState::default(), None));
    }

    #[test]
    fn state_corrupt_file_defaults() {
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        std::fs::write(state_dir().join("state.toml"), "not = [toml").unwrap();
        let (s, err) = AppState::load();
        assert_eq!(s, AppState::default());
        assert!(err.is_some());
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
        let (s, err) = AppState::load();
        assert!(err.is_none());
        assert_eq!(s.picker_sort, PickerSort::Name);
        assert_eq!(s.last_used["a"], 5);
        assert!(s.favorites.is_empty());
    }

    #[test]
    fn corrupt_state_is_backed_up_not_wiped() {
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        let original = "picker_sort = \"sideways\"\n\n[favorites]\nlocal = [\"a/b\"]\n\n[last_used]\nlocal = 7\n";
        std::fs::write(state_dir().join("state.toml"), original).unwrap();
        let (s, err) = AppState::load();
        assert!(err.is_some());
        assert_eq!(s.picker_sort, PickerSort::Custom);
        assert_eq!(s.last_used["local"], 7);
        assert!(s.favorites["local"].contains("a/b"));
        s.save().unwrap();
        let (again, err) = AppState::load();
        assert!(err.is_none());
        assert_eq!(again.favorites, s.favorites);
        assert_eq!(
            std::fs::read_to_string(state_dir().join("state.toml.bak")).unwrap(),
            original
        );
    }

    #[test]
    fn stale_backup_is_replaced() {
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        std::fs::write(state_dir().join("state.toml.bak"), "old backup").unwrap();
        std::fs::write(state_dir().join("state.toml"), "not = [toml").unwrap();
        let (s, err) = AppState::load();
        assert_eq!(s, AppState::default());
        assert!(err.is_some());
        assert_eq!(
            std::fs::read_to_string(state_dir().join("state.toml.bak")).unwrap(),
            "not = [toml"
        );
    }

    #[test]
    fn non_utf8_state_is_backed_up() {
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        let bytes = b"picker_sort = \"name\"\n# \xff\xfe\n";
        std::fs::write(state_dir().join("state.toml"), bytes).unwrap();
        let (s, err) = AppState::load();
        assert!(err.is_some());
        assert!(!s.read_only);
        assert_eq!(std::fs::read(backup_path()).unwrap(), bytes);
    }

    #[test]
    fn failed_backup_blocks_saving() {
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        std::fs::create_dir(backup_path()).unwrap();
        let original = b"not = [toml";
        std::fs::write(state_dir().join("state.toml"), original).unwrap();
        let (mut s, err) = AppState::load();
        let err = err.unwrap();
        assert!(s.read_only);
        let note = s.problem_note(&err);
        assert!(
            note.starts_with("state file unreadable (")
                && note.ends_with("), not saving this session"),
            "{note}"
        );
        s.touch("a", 1);
        s.save().unwrap();
        assert_eq!(
            std::fs::read(state_dir().join("state.toml")).unwrap(),
            original
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_state_blocks_saving() {
        use std::os::unix::fs::PermissionsExt;
        let (_g, _d) = in_temp_dir();
        std::fs::create_dir_all(state_dir()).unwrap();
        let path = state_dir().join("state.toml");
        std::fs::write(&path, "picker_sort = \"name\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_ok() {
            return; // root ignores the mode
        }
        let (s, err) = AppState::load();
        assert!(err.is_some());
        assert!(s.read_only);
        s.save().unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "picker_sort = \"name\"\n"
        );
    }
}
