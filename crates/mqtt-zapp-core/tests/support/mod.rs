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

/// Start a broker on fresh ports; `None` (after printing SKIP) without mosquitto.
pub fn start(auth: bool) -> Option<Broker> {
    let ports = free_ports();
    start_on(auth, ports)
}

/// Start a broker on the given `(port, ws_port, tls_port)`, retrying the bind for
/// up to 5 s so a port freed by a just-killed broker can be reused.
pub fn start_on(auth: bool, (port, ws_port, tls_port): (u16, u16, u16)) -> Option<Broker> {
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
    let conf = write_conf(dir.path(), auth, port, ws_port, tls_port);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut child = Command::new("mosquitto")
            .arg("-c")
            .arg(&conf)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
                panic!("mosquitto did not start within 5s");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if Instant::now() > deadline {
            panic!("mosquitto could not bind its ports within 5s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
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

fn write_conf(dir: &Path, auth: bool, port: u16, ws_port: u16, tls_port: u16) -> PathBuf {
    let mut conf = String::from("per_listener_settings false\n");
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
