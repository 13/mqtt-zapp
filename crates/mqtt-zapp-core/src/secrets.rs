//! Passwords live in the OS keyring (service "mqtt-zapp", user = profile name).

use crate::config::{PasswordSource, Profile};
use anyhow::{Result, bail};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long a password command may run.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
/// Most output read from a password command.
const OUTPUT_CAP: u64 = 64 * 1024;

/// The password a profile's source yields without asking the user: `None`
/// for `none` and `prompt` (the caller asks), the keyring entry, the
/// environment variable or the command's first output line. May block:
/// call it off the UI thread.
pub fn resolve_password(p: &Profile) -> Result<Option<String>> {
    match &p.password {
        PasswordSource::None | PasswordSource::Prompt => Ok(None),
        PasswordSource::Keyring => load_password(&p.name),
        PasswordSource::Env(name) => password_from_env(name).map(Some),
        PasswordSource::Command(cmd) => password_from_command(cmd, COMMAND_TIMEOUT).map(Some),
    }
}

pub fn password_from_env(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(v) => Ok(v),
        Err(_) => bail!("password env var {name} not set"),
    }
}

/// Printable, one line, clipped: safe to put in an error message.
fn clip(s: &str) -> String {
    let line: String = s
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = line.trim();
    match line.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_owned(),
    }
}

/// Run `sh -c cmd` (stdin closed) and return the first line of its output.
/// The child is killed after `timeout`. The password is never put into an
/// error; the stderr first line may be.
pub fn password_from_command(cmd: &str, timeout: Duration) -> Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("password command failed: {e}"))?;
    fn drain<R: Read + Send + 'static>(r: Option<R>) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(r) = r {
                let _ = r.take(OUTPUT_CAP).read_to_end(&mut buf);
            }
            let _ = tx.send(buf);
        });
        rx
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "password command failed: timed out after {}s",
                    timeout.as_secs_f32().ceil()
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => bail!("password command failed: {e}"),
        }
    };
    // A background grandchild may hold the pipes open: do not wait forever.
    let left = deadline.saturating_duration_since(Instant::now());
    let stdout = out.recv_timeout(left).unwrap_or_default();
    if !status.success() {
        let stderr = err
            .recv_timeout(Duration::from_millis(200))
            .unwrap_or_default();
        let detail = clip(&String::from_utf8_lossy(&stderr));
        if detail.is_empty() {
            bail!("password command failed: {status}");
        }
        bail!("password command failed: {status}: {detail}");
    }
    let text = String::from_utf8_lossy(&stdout);
    let line = text.split('\n').next().unwrap_or("");
    Ok(line.trim_end_matches('\r').to_owned())
}

#[cfg(feature = "keyring")]
const SERVICE: &str = "mqtt-zapp";

#[cfg(feature = "keyring")]
pub fn store_password(profile: &str, pw: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, profile)?.set_password(pw)?;
    Ok(())
}

#[cfg(feature = "keyring")]
pub fn load_password(profile: &str) -> Result<Option<String>> {
    match keyring::Entry::new(SERVICE, profile)?.get_password() {
        Ok(pw) => Ok(Some(pw)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(feature = "keyring")]
pub fn delete_password(profile: &str) -> Result<()> {
    match keyring::Entry::new(SERVICE, profile)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(not(feature = "keyring"))]
pub fn store_password(_profile: &str, _pw: &str) -> Result<()> {
    anyhow::bail!("keyring support not compiled in; use password = \"prompt\"")
}

#[cfg(not(feature = "keyring"))]
pub fn load_password(_profile: &str) -> Result<Option<String>> {
    anyhow::bail!("keyring support not compiled in; use password = \"prompt\"")
}

/// Without keyring support there is nothing to delete.
#[cfg(not(feature = "keyring"))]
pub fn delete_password(_profile: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod source_tests {
    use super::*;

    #[test]
    fn password_from_env() {
        let name = "MQTT_ZAPP_TEST_PW_FROM_ENV_13";
        // SAFETY: a variable only this test touches.
        unsafe { std::env::remove_var(name) };
        let e = super::password_from_env(name).unwrap_err().to_string();
        assert_eq!(e, format!("password env var {name} not set"));
        unsafe { std::env::set_var(name, "s3cret") };
        assert_eq!(super::password_from_env(name).unwrap(), "s3cret");
        unsafe { std::env::remove_var(name) };
    }

    #[test]
    fn password_from_command() {
        let t = Duration::from_secs(10);
        assert_eq!(
            super::password_from_command("printf secret", t).unwrap(),
            "secret"
        );
        assert_eq!(
            super::password_from_command("printf 'one\\r\\ntwo\\n'", t).unwrap(),
            "one"
        );
        // stdin is closed, not the terminal.
        assert_eq!(
            super::password_from_command("cat; printf x", t).unwrap(),
            "x"
        );
        let e = super::password_from_command("echo 'bad thing' >&2; echo leaked; exit 3", t)
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("password command failed: "), "{e}");
        assert!(e.contains("bad thing") && !e.contains("leaked"), "{e}");
    }

    #[test]
    fn password_command_timeout() {
        let start = Instant::now();
        let e = super::password_from_command("sleep 20", Duration::from_millis(200))
            .unwrap_err()
            .to_string();
        assert!(e.contains("timed out"), "{e}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}

#[cfg(all(test, not(feature = "keyring")))]
mod tests {
    #[test]
    fn without_feature_errors_with_hint() {
        let e = super::store_password("p", "x").unwrap_err().to_string();
        assert!(e.contains("keyring support not compiled in"));
        assert!(super::load_password("p").is_err());
        assert!(
            super::delete_password("p").is_ok(),
            "deleting needs no keyring"
        );
    }
}
