//! Command line: profile management and how the TUI is launched.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use mqtt_zapp_core::config::{
    PasswordSource, Profile, ProfileStore, Protocol, TlsConfig, default_subscriptions,
    default_ws_path,
};
use mqtt_zapp_core::secrets::store_password;
use std::path::PathBuf;

#[derive(Clone, Copy, ValueEnum)]
pub enum ProtocolArg {
    Mqtt,
    Mqtts,
    Ws,
    Wss,
}

impl From<ProtocolArg> for Protocol {
    fn from(p: ProtocolArg) -> Self {
        match p {
            ProtocolArg::Mqtt => Protocol::Mqtt,
            ProtocolArg::Mqtts => Protocol::Mqtts,
            ProtocolArg::Ws => Protocol::Ws,
            ProtocolArg::Wss => Protocol::Wss,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub enum PasswordMode {
    None,
    Keyring,
    Prompt,
}

impl From<PasswordMode> for PasswordSource {
    fn from(m: PasswordMode) -> Self {
        match m {
            PasswordMode::None => PasswordSource::None,
            PasswordMode::Keyring => PasswordSource::Keyring,
            PasswordMode::Prompt => PasswordSource::Prompt,
        }
    }
}

#[derive(clap::Args, Clone)]
pub struct ConnArgs {
    /// Broker host. Takes precedence over --profile when both are given.
    #[arg(long)]
    pub host: Option<String>,
    /// Broker port [default: the protocol's default port]
    #[arg(long)]
    pub port: Option<u16>,
    /// Transport protocol
    #[arg(long, value_enum, default_value = "mqtt")]
    pub protocol: ProtocolArg,
    /// Username
    #[arg(long = "user")]
    pub user: Option<String>,
    /// Prompt for the password at connect time
    #[arg(long)]
    pub ask_password: bool,
    /// Extra CA certificate (PEM) to trust
    #[arg(long)]
    pub ca_file: Option<PathBuf>,
    /// Skip TLS certificate verification
    #[arg(long)]
    pub insecure: bool,
}

#[derive(Parser)]
#[command(version, about = "Terminal MQTT explorer")]
pub struct Args {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
    /// Connect to a saved profile (ignored if --host is given)
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,
    #[command(flatten)]
    pub conn: ConnArgs,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Start the TUI (the default)
    Tui,
    /// Manage saved profiles
    #[command(subcommand)]
    Profile(ProfileCmd),
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// Save a profile
    Add {
        #[arg(long)]
        name: String,
        /// Where the password comes from [default: prompt if --user is set, else none]
        #[arg(long, value_enum)]
        password_mode: Option<PasswordMode>,
        #[command(flatten)]
        conn: ConnArgs,
    },
    /// List saved profiles
    List,
    /// Delete a saved profile
    Remove { name: String },
}

// TODO(task 8): drop the allow once tui::run reads the profile.
#[allow(dead_code, clippy::large_enum_variant)] // short-lived, built once per run
pub enum Launch {
    Picker,
    Connect(Profile),
}

fn build_profile(name: &str, conn: &ConnArgs, host: &str, password: PasswordSource) -> Profile {
    let protocol: Protocol = conn.protocol.into();
    Profile {
        name: name.into(),
        host: host.into(),
        port: conn.port.unwrap_or_else(|| protocol.default_port()),
        protocol,
        ws_path: default_ws_path(),
        client_id: None,
        username: conn.user.clone(),
        password,
        tls: TlsConfig {
            ca_file: conn.ca_file.clone(),
            insecure: conn.insecure,
            ..TlsConfig::default()
        },
        subscriptions: default_subscriptions(),
    }
}

pub fn resolve_launch(args: &Args, store: &ProfileStore) -> Result<Launch> {
    if let Some(host) = &args.conn.host {
        let pw = if args.conn.ask_password {
            PasswordSource::Prompt
        } else {
            PasswordSource::None
        };
        return Ok(Launch::Connect(build_profile(
            "adhoc", &args.conn, host, pw,
        )));
    }
    match &args.profile {
        Some(name) => match store.get(name) {
            Some(p) => Ok(Launch::Connect(p.clone())),
            None => bail!("unknown profile '{name}'"),
        },
        None => Ok(Launch::Picker),
    }
}

/// Builds the profile for `profile add`; does not touch the keyring.
fn profile_for_add(name: &str, mode: Option<PasswordMode>, conn: &ConnArgs) -> Result<Profile> {
    let Some(host) = &conn.host else {
        bail!("--host is required");
    };
    let password = match mode {
        Some(m) => m.into(),
        None if conn.user.is_some() || conn.ask_password => PasswordSource::Prompt,
        None => PasswordSource::None,
    };
    Ok(build_profile(name, conn, host, password))
}

pub fn format_list(store: &ProfileStore) -> String {
    let mut out = String::new();
    for p in &store.profiles {
        let proto = format!("{:?}", p.protocol).to_lowercase();
        out.push_str(&format!(
            "{}  {}://{}:{}  {}\n",
            p.name,
            proto,
            p.host,
            p.port,
            p.username.as_deref().unwrap_or("")
        ));
    }
    out
}

fn remove_profile(store: &mut ProfileStore, name: &str) -> Result<()> {
    if !store.remove(name) {
        bail!("unknown profile '{name}'");
    }
    Ok(())
}

pub fn run_profile(cmd: &ProfileCmd) -> Result<()> {
    match cmd {
        ProfileCmd::Add {
            name,
            password_mode,
            conn,
        } => {
            let profile = profile_for_add(name, *password_mode, conn)?;
            if profile.password == PasswordSource::Keyring {
                let pw = rpassword::prompt_password(format!("Password for '{name}': "))?;
                store_password(name, &pw)?;
            }
            let mut store = ProfileStore::load()?;
            store.upsert(profile);
            store.save()
        }
        ProfileCmd::List => {
            print!("{}", format_list(&ProfileStore::load()?));
            Ok(())
        }
        ProfileCmd::Remove { name } => {
            let mut store = ProfileStore::load()?;
            remove_profile(&mut store, name)?;
            store.save()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ProfileStore {
        let mut s = ProfileStore::default();
        s.upsert(Profile {
            name: "home".into(),
            host: "192.168.22.5".into(),
            port: 1883,
            protocol: Protocol::Mqtt,
            ws_path: "/".into(),
            client_id: None,
            username: None,
            password: PasswordSource::None,
            tls: TlsConfig::default(),
            subscriptions: default_subscriptions(),
        });
        s
    }

    fn launch(a: &[&str]) -> Result<Launch> {
        resolve_launch(&Args::parse_from(a), &store())
    }

    #[test]
    fn no_args_is_picker() {
        assert!(matches!(launch(&["mqtt-zapp"]).unwrap(), Launch::Picker));
    }

    #[test]
    fn profile_flag_selects() {
        let Launch::Connect(p) = launch(&["mqtt-zapp", "--profile", "home"]).unwrap() else {
            panic!()
        };
        assert_eq!(p.host, "192.168.22.5");
    }

    #[test]
    fn unknown_profile_errors() {
        let e = launch(&["mqtt-zapp", "--profile", "nope"]).err().unwrap();
        assert!(e.to_string().contains("unknown profile 'nope'"));
    }

    #[test]
    fn adhoc_wss_defaults_port() {
        let Launch::Connect(p) = launch(&[
            "mqtt-zapp",
            "--host",
            "b.example",
            "--protocol",
            "wss",
            "--user",
            "u",
            "--ask-password",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(p.port, 443);
        assert_eq!(p.name, "adhoc");
        assert!(matches!(p.password, PasswordSource::Prompt));
    }

    #[test]
    fn host_wins_over_profile_and_fills_tls() {
        let Launch::Connect(p) = launch(&[
            "mqtt-zapp",
            "--profile",
            "home",
            "--host",
            "x",
            "--port",
            "1",
            "--ca-file",
            "/ca.pem",
            "--insecure",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!((p.host.as_str(), p.port), ("x", 1));
        assert!(p.tls.insecure);
        assert_eq!(p.tls.ca_file, Some(PathBuf::from("/ca.pem")));
    }

    fn add(a: &[&str]) -> Result<Profile> {
        let args = Args::parse_from(
            std::iter::once("mqtt-zapp")
                .chain(["profile", "add"])
                .chain(a.iter().copied()),
        );
        let Some(Cmd::Profile(ProfileCmd::Add {
            name,
            password_mode,
            conn,
        })) = args.cmd
        else {
            panic!()
        };
        profile_for_add(&name, password_mode, &conn)
    }

    #[test]
    fn add_defaults() {
        let p = add(&["--name", "t", "--host", "localhost"]).unwrap();
        assert_eq!(p.port, 1883);
        assert!(matches!(p.password, PasswordSource::None));
        let p = add(&["--name", "t", "--host", "h", "--user", "u"]).unwrap();
        assert!(matches!(p.password, PasswordSource::Prompt));
        let p = add(&[
            "--name",
            "t",
            "--host",
            "h",
            "--user",
            "u",
            "--password-mode",
            "keyring",
        ])
        .unwrap();
        assert!(matches!(p.password, PasswordSource::Keyring));
        assert!(add(&["--name", "t"]).is_err());
    }

    #[test]
    fn list_format() {
        let mut s = store();
        let mut p = s.get("home").unwrap().clone();
        p.name = "b".into();
        p.protocol = Protocol::Wss;
        p.port = 443;
        p.username = Some("u".into());
        s.upsert(p);
        assert_eq!(
            format_list(&s),
            "home  mqtt://192.168.22.5:1883  \nb  wss://192.168.22.5:443  u\n"
        );
    }

    #[test]
    fn remove_unknown_errors() {
        let mut s = store();
        assert_eq!(
            remove_profile(&mut s, "x").unwrap_err().to_string(),
            "unknown profile 'x'"
        );
        remove_profile(&mut s, "home").unwrap();
        assert!(s.profiles.is_empty());
    }
}
