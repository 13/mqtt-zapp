//! Saving profile edits from the TUI: `profiles.toml` plus the keyring.
//!
//! Each change is read-modify-write against the file on disk, so profiles
//! added meanwhile (e.g. with `mqtt-zapp profile add`) survive. Keyring
//! writes that add a secret happen before the file is saved, so a keyring
//! failure leaves the file untouched, and are undone if the save fails. Old
//! secrets are deleted only after the save, and failing to delete them does
//! not fail the change.

use super::profile_form::ProfileEdit;
use anyhow::{Result, bail};
use mqtt_zapp_core::config::{PasswordSource, ProfileStore};
use mqtt_zapp_core::secrets;

/// Keyring calls, injected so tests never touch the real keyring.
pub trait Secrets {
    fn store(&self, name: &str, pw: &str) -> Result<()>;
    fn load(&self, name: &str) -> Result<Option<String>>;
    fn delete(&self, name: &str) -> Result<()>;
}

/// The system keyring.
pub struct RealSecrets;

impl Secrets for RealSecrets {
    fn store(&self, name: &str, pw: &str) -> Result<()> {
        secrets::store_password(name, pw)
    }
    fn load(&self, name: &str) -> Result<Option<String>> {
        secrets::load_password(name)
    }
    fn delete(&self, name: &str) -> Result<()> {
        secrets::delete_password(name)
    }
}

/// Add or replace a profile in `profiles.toml`; returns the saved store.
pub fn persist_edit(edit: ProfileEdit, s: &dyn Secrets) -> Result<ProfileStore> {
    let mut store = ProfileStore::load()?;
    let old = edit
        .original
        .as_deref()
        .and_then(|n| store.get(n))
        .filter(|o| o.password == PasswordSource::Keyring)
        .map(|o| o.name.clone());
    let new = edit.profile.clone();
    store.replace(edit.original.as_deref(), edit.profile)?;
    let keyring = new.password == PasswordSource::Keyring;
    // The secret written below and the one it replaced, to undo on failure.
    let mut written = None;
    if keyring {
        let pw = match (&edit.password, &old) {
            (Some(pw), _) => Some(pw.clone()),
            (None, Some(o)) if *o != new.name => s.load(o)?,
            (None, Some(_)) => None,
            // The form believed a secret was stored, but on disk the
            // profile does not use the keyring (changed meanwhile).
            (None, None) => {
                if s.load(&new.name)?.is_none() {
                    bail!("type a password, or use prompt");
                }
                None
            }
        };
        if let Some(pw) = pw {
            let previous = s.load(&new.name)?;
            s.store(&new.name, &pw)?;
            written = Some(previous);
        }
    }
    if let Err(e) = store.save() {
        match written {
            Some(Some(previous)) => _ = s.store(&new.name, &previous),
            Some(None) => _ = s.delete(&new.name),
            None => {}
        }
        return Err(e);
    }
    if let Some(o) = old.filter(|o| !keyring || *o != new.name) {
        let _ = s.delete(&o);
    }
    Ok(store)
}

/// Delete a profile from `profiles.toml` (and its keyring secret).
pub fn persist_delete(name: &str, s: &dyn Secrets) -> Result<ProfileStore> {
    let mut store = ProfileStore::load()?;
    let keyring = store
        .get(name)
        .is_some_and(|p| p.password == PasswordSource::Keyring);
    if !store.remove(name) {
        bail!("unknown profile '{name}'");
    }
    store.save()?;
    if keyring {
        let _ = s.delete(name);
    }
    Ok(store)
}

/// Save the picker's custom order: names first, in order; profiles added on
/// disk meanwhile keep their place after them.
pub fn persist_order(names: &[String]) -> Result<ProfileStore> {
    let mut store = ProfileStore::load()?;
    store.reorder(names);
    store.save()?;
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::profile_form::ProfileEdit;
    use crate::tui::test_env::ConfigDir;
    use mqtt_zapp_core::config::{PasswordSource, Profile, ProfileStore, Protocol, TlsConfig};
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeSecrets {
        map: RefCell<HashMap<String, String>>,
        fail_store: bool,
    }

    impl FakeSecrets {
        fn with(entries: &[(&str, &str)]) -> FakeSecrets {
            let s = FakeSecrets::default();
            for (k, v) in entries {
                s.map.borrow_mut().insert((*k).into(), (*v).into());
            }
            s
        }

        fn snapshot(&self) -> Vec<(String, String)> {
            let mut v: Vec<_> = self.map.borrow().clone().into_iter().collect();
            v.sort();
            v
        }
    }

    impl Secrets for FakeSecrets {
        fn store(&self, name: &str, pw: &str) -> anyhow::Result<()> {
            if self.fail_store {
                anyhow::bail!("keyring locked");
            }
            self.map.borrow_mut().insert(name.into(), pw.into());
            Ok(())
        }
        fn load(&self, name: &str) -> anyhow::Result<Option<String>> {
            Ok(self.map.borrow().get(name).cloned())
        }
        fn delete(&self, name: &str) -> anyhow::Result<()> {
            self.map.borrow_mut().remove(name);
            Ok(())
        }
    }

    fn profile(name: &str, pw: PasswordSource) -> Profile {
        Profile {
            name: name.into(),
            host: "h".into(),
            port: 1883,
            protocol: Protocol::Mqtt,
            mqtt_version: Default::default(),
            ws_path: "/".into(),
            client_id: None,
            username: None,
            password: pw,
            tls: TlsConfig::default(),
            subscriptions: vec!["#".into()],
            retained: true,
        }
    }

    fn on_disk(profiles: Vec<Profile>) {
        ProfileStore { profiles }.save().unwrap();
    }

    fn names(s: &ProfileStore) -> Vec<&str> {
        s.profiles.iter().map(|p| p.name.as_str()).collect()
    }

    fn edit(original: Option<&str>, p: Profile, pw: Option<&str>) -> ProfileEdit {
        ProfileEdit {
            original: original.map(Into::into),
            profile: p,
            password: pw.map(Into::into),
        }
    }

    use PasswordSource::{Keyring, None as NoPw, Prompt};

    #[test]
    fn save_preserves_profiles_added_on_disk() {
        let _d = ConfigDir::new("preserve");
        on_disk(vec![profile("x", NoPw)]);
        let s = persist_edit(
            edit(None, profile("y", NoPw), None),
            &FakeSecrets::default(),
        )
        .unwrap();
        assert_eq!(names(&s), ["x", "y"]);
        assert_eq!(names(&ProfileStore::load().unwrap()), ["x", "y"]);
    }

    #[test]
    fn new_password_stored_under_name() {
        let _d = ConfigDir::new("newpw");
        let k = FakeSecrets::default();
        persist_edit(edit(None, profile("k", Keyring), Some("x")), &k).unwrap();
        assert_eq!(k.snapshot(), [("k".into(), "x".into())]);
    }

    #[test]
    fn rename_moves_keyring_secret() {
        let _d = ConfigDir::new("rename");
        on_disk(vec![profile("a", Keyring)]);
        let k = FakeSecrets::with(&[("a", "pw")]);
        let s = persist_edit(edit(Some("a"), profile("b", Keyring), None), &k).unwrap();
        assert_eq!(names(&s), ["b"]);
        assert_eq!(k.snapshot(), [("b".into(), "pw".into())]);
    }

    #[test]
    fn rename_with_new_password_drops_old_secret() {
        let _d = ConfigDir::new("renamepw");
        on_disk(vec![profile("a", Keyring)]);
        let k = FakeSecrets::with(&[("a", "old")]);
        persist_edit(edit(Some("a"), profile("b", Keyring), Some("new")), &k).unwrap();
        assert_eq!(k.snapshot(), [("b".into(), "new".into())]);
    }

    #[test]
    fn switching_off_keyring_deletes_secret() {
        let _d = ConfigDir::new("off");
        on_disk(vec![profile("a", Keyring)]);
        let k = FakeSecrets::with(&[("a", "pw")]);
        persist_edit(edit(Some("a"), profile("a", Prompt), None), &k).unwrap();
        assert!(k.snapshot().is_empty());
    }

    #[test]
    fn keyring_failure_leaves_file_alone() {
        let _d = ConfigDir::new("kfail");
        on_disk(vec![profile("a", Keyring)]);
        let k = FakeSecrets {
            fail_store: true,
            ..FakeSecrets::with(&[("a", "pw")])
        };
        let e = persist_edit(edit(Some("a"), profile("b", Keyring), Some("n")), &k)
            .unwrap_err()
            .to_string();
        assert!(e.contains("keyring locked"), "{e}");
        assert_eq!(names(&ProfileStore::load().unwrap()), ["a"]);
        assert_eq!(k.snapshot(), [("a".into(), "pw".into())], "old secret kept");
    }

    /// Make the config dir unwritable (saves are atomic renames, so the file's
    /// own mode no longer matters), so `save` fails after `load`.
    fn read_only(d: &ConfigDir) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&d.0, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    #[test]
    fn failed_save_restores_replaced_secret() {
        let d = ConfigDir::new("restore");
        on_disk(vec![profile("a", Keyring)]);
        read_only(&d);
        let k = FakeSecrets::with(&[("a", "old")]);
        assert!(persist_edit(edit(Some("a"), profile("a", Keyring), Some("new")), &k).is_err());
        assert_eq!(k.snapshot(), [("a".into(), "old".into())]);
    }

    #[test]
    fn failed_save_leaves_no_orphan_secret() {
        let d = ConfigDir::new("orphan");
        on_disk(vec![profile("a", Keyring)]);
        read_only(&d);
        let k = FakeSecrets::with(&[("a", "pw")]);
        assert!(persist_edit(edit(Some("a"), profile("b", Keyring), None), &k).is_err());
        assert_eq!(k.snapshot(), [("a".into(), "pw".into())]);
        let k = FakeSecrets::default();
        assert!(persist_edit(edit(None, profile("n", Keyring), Some("x")), &k).is_err());
        assert!(k.snapshot().is_empty());
    }

    #[test]
    fn keyring_without_any_secret_is_refused() {
        // The picker thought `a` used the keyring, but on disk it no longer
        // does (changed with the CLI meanwhile), so there is no secret.
        let _d = ConfigDir::new("nosecret");
        on_disk(vec![profile("a", Prompt)]);
        let e = persist_edit(
            edit(Some("a"), profile("a", Keyring), None),
            &FakeSecrets::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("type a password, or use prompt"), "{e}");
        assert_eq!(ProfileStore::load().unwrap().profiles[0].password, Prompt);
        // A secret already in the keyring under that name is fine.
        let k = FakeSecrets::with(&[("a", "pw")]);
        persist_edit(edit(Some("a"), profile("a", Keyring), None), &k).unwrap();
    }

    #[test]
    fn collision_on_disk_is_refused() {
        let _d = ConfigDir::new("collide");
        on_disk(vec![profile("a", NoPw), profile("b", NoPw)]);
        let e = persist_edit(
            edit(Some("a"), profile("b", NoPw), None),
            &FakeSecrets::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("already exists"), "{e}");
    }

    #[test]
    fn broken_file_is_not_overwritten() {
        let d = ConfigDir::new("broken");
        std::fs::create_dir_all(&d.0).unwrap();
        std::fs::write(d.0.join("profiles.toml"), "not = [valid").unwrap();
        assert!(
            persist_edit(
                edit(None, profile("y", NoPw), None),
                &FakeSecrets::default()
            )
            .is_err()
        );
        assert!(persist_delete("y", &FakeSecrets::default()).is_err());
        assert_eq!(
            std::fs::read_to_string(d.0.join("profiles.toml")).unwrap(),
            "not = [valid"
        );
    }

    #[test]
    fn persist_order_keeps_profiles_added_on_disk() {
        let _d = ConfigDir::new("order");
        on_disk(vec![
            profile("a", NoPw),
            profile("b", NoPw),
            profile("x", NoPw),
        ]);
        let s = persist_order(&["b".into(), "a".into()]).unwrap();
        assert_eq!(names(&s), ["b", "a", "x"]);
        assert_eq!(names(&ProfileStore::load().unwrap()), ["b", "a", "x"]);
    }

    #[test]
    fn persist_order_broken_file_is_not_overwritten() {
        let d = ConfigDir::new("order-broken");
        std::fs::create_dir_all(&d.0).unwrap();
        std::fs::write(d.0.join("profiles.toml"), "not = [valid").unwrap();
        assert!(persist_order(&["a".into()]).is_err());
        assert_eq!(
            std::fs::read_to_string(d.0.join("profiles.toml")).unwrap(),
            "not = [valid"
        );
    }

    #[test]
    fn delete_removes_profile_and_secret() {
        let _d = ConfigDir::new("delete");
        on_disk(vec![profile("a", Keyring), profile("b", NoPw)]);
        let k = FakeSecrets::with(&[("a", "pw")]);
        let s = persist_delete("a", &k).unwrap();
        assert_eq!(names(&s), ["b"]);
        assert!(k.snapshot().is_empty());
    }

    #[test]
    fn delete_unknown_errors() {
        let _d = ConfigDir::new("unknown");
        let e = persist_delete("zz", &FakeSecrets::default())
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown profile 'zz'"), "{e}");
    }
}
