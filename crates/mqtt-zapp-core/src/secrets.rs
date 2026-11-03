//! Passwords live in the OS keyring (service "mqtt-zapp", user = profile name).

use anyhow::Result;

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
