//! Process-wide environment isolation for tests that read or write the
//! config and state dirs.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

/// A fresh config (and state) dir for one test; removed on drop. Holding it
/// holds the env lock, so tests that touch `MQTT_ZAPP_*_DIR` never overlap.
pub(crate) struct ConfigDir(
    pub(crate) PathBuf,
    #[allow(dead_code)] MutexGuard<'static, ()>,
);

impl ConfigDir {
    pub(crate) fn new(tag: &str) -> ConfigDir {
        let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = std::env::temp_dir().join(format!("mqtt-zapp-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        // SAFETY: env mutation is serialized by LOCK; only tests holding a
        // ConfigDir touch these vars in this binary. The state dir follows
        // the config dir when no state override is set.
        unsafe {
            std::env::set_var("MQTT_ZAPP_CONFIG_DIR", &d);
            std::env::remove_var("MQTT_ZAPP_STATE_DIR");
        }
        ConfigDir(d, g)
    }
}

impl Drop for ConfigDir {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
