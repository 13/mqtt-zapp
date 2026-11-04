//! Throwaway mosquitto brokers for integration tests: plain, websocket and TLS
//! listeners on free localhost ports, with a freshly generated CA.

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub struct Broker {
    pub port: u16,
    pub ws_port: u16,
    pub tls_port: u16,
    pub ca_pem_path: PathBuf,
    child: Child,
    #[allow(dead_code)] // kept alive so the config and certs outlive the broker
    dir: TempDir,
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Broker {
    /// Everything the broker has logged so far.
    #[allow(dead_code)] // only some test binaries read the log
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("mosquitto.log")).unwrap_or_default()
    }

    /// Freeze the broker (SIGSTOP): connections stay open but nothing is
    /// answered, until it is killed on drop.
    #[allow(dead_code)] // only some test binaries need it
    pub fn pause(&self) {
        let ok = Command::new("kill")
            .args(["-STOP", &self.child.id().to_string()])
            .status()
            .unwrap()
            .success();
        assert!(ok, "kill -STOP failed");
    }
}

/// Start a broker on fresh ports; `None` (after printing SKIP) without mosquitto.
pub fn start(auth: bool) -> Option<Broker> {
    let ports = free_ports();
    start_on(auth, ports)
}

/// Start a broker on the given `(port, ws_port, tls_port)`, retrying the bind for
/// up to 5 s so a port freed by a just-killed broker can be reused.
pub fn start_on(auth: bool, ports: (u16, u16, u16)) -> Option<Broker> {
    start_with(auth, ports, None)
}

/// Like `start_on`, but the broker saves its retained messages under `data`
/// (about once a second), so a later broker given the same `data` serves them.
#[allow(dead_code)] // only some test binaries restart a broker with its data
pub fn start_persistent_on(ports: (u16, u16, u16), data: &Path) -> Option<Broker> {
    start_with(false, ports, Some(data))
}

fn start_with(
    auth: bool,
    (port, ws_port, tls_port): (u16, u16, u16),
    data: Option<&Path>,
) -> Option<Broker> {
    if Command::new("mosquitto")
        .arg("-h")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("SKIP: mosquitto not on PATH");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let ca_pem_path = write_certs(dir.path());
    let conf = write_conf(dir.path(), auth, port, ws_port, tls_port, data);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let stderr = std::fs::File::create(dir.path().join("stderr.log")).unwrap();
        let mut child = Command::new("mosquitto")
            .arg("-c")
            .arg(&conf)
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap();
        loop {
            if [port, ws_port, tls_port]
                .iter()
                .all(|p| TcpStream::connect(("127.0.0.1", *p)).is_ok())
            {
                return Some(Broker {
                    port,
                    ws_port,
                    tls_port,
                    ca_pem_path,
                    child,
                    dir,
                });
            }
            if child.try_wait().unwrap().is_some() {
                break; // exited, most likely a port still in use: respawn
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("mosquitto did not start within 5s\n{}", said(dir.path()));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if Instant::now() > deadline {
            panic!("mosquitto could not bind its ports within 5s\n{}", said(dir.path()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the broker wrote (its stderr and log file), for a failed start.
fn said(dir: &Path) -> String {
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    format!(
        "stderr:\n{}\nlog:\n{}",
        read("stderr.log"),
        read("mosquitto.log")
    )
}

/// Three ports that were free a moment ago (nothing listens on them yet).
pub fn free_ports() -> (u16, u16, u16) {
    let ls: Vec<TcpListener> = (0..3)
        .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let p = |i: usize| ls[i].local_addr().unwrap().port();
    (p(0), p(1), p(2))
}

/// Writes ca.pem, server.pem, server.key; returns the CA path.
fn write_certs(dir: &Path) -> PathBuf {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca.distinguished_name
        .push(DnType::CommonName, "mqtt-zapp test CA");
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let issuer = Issuer::new(ca, ca_key);

    let srv_key = KeyPair::generate().unwrap();
    let mut srv = CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    srv.distinguished_name.push(DnType::CommonName, "localhost");
    let srv_cert = srv.signed_by(&srv_key, &issuer).unwrap();

    let ca_path = dir.join("ca.pem");
    std::fs::write(&ca_path, ca_cert.pem()).unwrap();
    std::fs::write(dir.join("server.pem"), srv_cert.pem()).unwrap();
    std::fs::write(dir.join("server.key"), srv_key.serialize_pem()).unwrap();
    ca_path
}

fn write_conf(
    dir: &Path,
    auth: bool,
    port: u16,
    ws_port: u16,
    tls_port: u16,
    data: Option<&Path>,
) -> PathBuf {
    let mut conf = format!(
        "per_listener_settings false\nlog_dest file {}/mosquitto.log\nlog_type all\n",
        dir.display()
    );
    if let Some(data) = data {
        conf += &format!(
            "persistence true\npersistence_location {}/\nautosave_interval 1\nautosave_on_changes true\n",
            data.display()
        );
    }
    if auth {
        let pw = dir.join("passwd");
        let ok = Command::new("mosquitto_passwd")
            .args(["-b", "-c"])
            .arg(&pw)
            .args(["zapp", "zapp-pw"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "mosquitto_passwd failed");
        conf += &format!("allow_anonymous false\npassword_file {}\n", pw.display());
    } else {
        conf += "allow_anonymous true\n";
    }
    let d = dir.display();
    conf += &format!(
        "listener {port} 127.0.0.1\n\
         listener {ws_port} 127.0.0.1\nprotocol websockets\n\
         listener {tls_port} 127.0.0.1\n\
         cafile {d}/ca.pem\ncertfile {d}/server.pem\nkeyfile {d}/server.key\n"
    );
    let path = dir.join("mosquitto.conf");
    std::fs::write(&path, conf).unwrap();
    path
}
