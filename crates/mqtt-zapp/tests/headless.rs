//! The headless `sub` and `pub` commands, run as real processes against a
//! throwaway mosquitto (skipped when it is not installed).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// A mosquitto on a free localhost port; killed on drop.
struct Broker {
    port: u16,
    child: Child,
    _dir: TempDir,
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(auth: bool) -> Option<Broker> {
    let tool = |name: &str| {
        Command::new(name)
            .arg("-h")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    };
    if !tool("mosquitto") || (auth && !tool("mosquitto_passwd")) {
        eprintln!("SKIP: mosquitto not on PATH");
        return None;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut conf = String::new();
    if auth {
        let pw = dir.path().join("passwd");
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
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let path = dir.path().join("mosquitto.conf");
        std::fs::write(&path, format!("{conf}listener {port} 127.0.0.1\n")).unwrap();
        let mut child = Command::new("mosquitto")
            .arg("-c")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        while Instant::now() < deadline {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Some(Broker {
                    port,
                    child,
                    _dir: dir,
                });
            }
            if child.try_wait().unwrap().is_some() {
                break; // most likely a lost port race: pick another
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(Instant::now() < deadline, "mosquitto did not start");
    }
}

/// Config and state dirs that are not the user's.
struct Home(TempDir);

impl Home {
    fn new() -> Home {
        Home(tempfile::tempdir().unwrap())
    }
    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mqtt-zapp"));
        c.args(args)
            .env("MQTT_ZAPP_CONFIG_DIR", self.0.path())
            .env("MQTT_ZAPP_STATE_DIR", self.0.path());
        c
    }
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run to completion (killed after 15 s so a hang fails instead of stalling).
fn run(mut cmd: Command, stdin: Option<&[u8]>) -> Out {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    if let Some(data) = stdin {
        child.stdin.take().unwrap().write_all(data).unwrap();
    }
    let (mut o, mut e) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let ho = std::thread::spawn(move || {
        let mut s = String::new();
        o.read_to_string(&mut s).unwrap();
        s
    });
    let he = std::thread::spawn(move || {
        let mut s = String::new();
        e.read_to_string(&mut s).unwrap();
        s
    });
    let code = wait_timeout(&mut child, Duration::from_secs(15));
    Out {
        code,
        stdout: ho.join().unwrap(),
        stderr: he.join().unwrap(),
    }
}

/// The exit code, or panics (after killing the child) on timeout.
fn wait_timeout(child: &mut Child, d: Duration) -> Option<i32> {
    let end = Instant::now() + d;
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s.code();
        }
        if Instant::now() > end {
            let _ = child.kill();
            let _ = child.wait();
            panic!("process did not exit within {d:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn publish(h: &Home, b: &Broker, topic: &str, msg: &str, retain: bool) {
    let p = b.port.to_string();
    let mut args = vec![
        "pub",
        "--host",
        "127.0.0.1",
        "--port",
        &p,
        "-t",
        topic,
        "-m",
        msg,
        "-q",
        "1",
    ];
    if retain {
        args.push("-r");
    }
    let o = run(h.cmd(&args), None);
    assert_eq!(o.code, Some(0), "pub failed: {}", o.stderr);
}

fn sub(h: &Home, b: &Broker, extra: &[&str]) -> Out {
    let p = b.port.to_string();
    let mut args = vec![
        "sub",
        "--host",
        "127.0.0.1",
        "--port",
        &p,
        "--sub",
        "zapp/#",
    ];
    args.extend_from_slice(extra);
    run(h.cmd(&args), None)
}

#[test]
fn pub_then_sub_retained_round_trip() {
    let Some(b) = start(false) else { return };
    let h = Home::new();
    publish(&h, &b, "zapp/rt", "hello\nworld", true);
    let o = sub(&h, &b, &["--retained-only"]);
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.stdout, "zapp/rt hello\\nworld\n");
}

#[test]
fn pub_reads_stdin_and_file() {
    let Some(b) = start(false) else { return };
    let h = Home::new();
    let p = b.port.to_string();
    let o = run(
        h.cmd(&[
            "pub",
            "--host",
            "127.0.0.1",
            "--port",
            &p,
            "-t",
            "zapp/in",
            "--stdin",
            "-r",
        ]),
        Some(b"from stdin"),
    );
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let file = h.0.path().join("payload.txt");
    std::fs::write(&file, "from file").unwrap();
    let o = run(
        h.cmd(&[
            "pub",
            "--host",
            "127.0.0.1",
            "--port",
            &p,
            "-t",
            "zapp/f",
            "-f",
            file.to_str().unwrap(),
            "-r",
            "-q",
            "0",
        ]),
        None,
    );
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let o = sub(&h, &b, &["--retained-only"]);
    let mut lines: Vec<&str> = o.stdout.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, ["zapp/f from file", "zapp/in from stdin"]);
}

#[test]
fn sub_jsonl_shape() {
    let Some(b) = start(false) else { return };
    let h = Home::new();
    publish(&h, &b, "zapp/j", r#"{ "a": 1 }"#, true);
    let o = sub(&h, &b, &["--format", "jsonl", "-n", "1"]);
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.stdout.lines().count(), 1, "{}", o.stdout);
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim_end()).unwrap();
    let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["topic", "time", "qos", "retain", "payload"]);
    assert_eq!(v["topic"], "zapp/j");
    assert_eq!(v["retain"], true);
    assert_eq!(v["payload"], serde_json::json!({"a": 1}));
}

#[test]
fn sub_count_exits() {
    let Some(b) = start(false) else { return };
    let h = Home::new();
    for t in ["zapp/c/1", "zapp/c/2", "zapp/c/3"] {
        publish(&h, &b, t, "x", true);
    }
    let o = sub(&h, &b, &["-n", "2"]);
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(o.stdout.lines().count(), 2, "{}", o.stdout);
}

#[test]
fn sub_exits_quietly_on_closed_stdout() {
    let Some(b) = start(false) else { return };
    let h = Home::new();
    publish(&h, &b, "zapp/live/0", "first", true);
    let p = b.port.to_string();
    let mut cmd = h.cmd(&[
        "sub",
        "--host",
        "127.0.0.1",
        "--port",
        &p,
        "--sub",
        "zapp/live/#",
    ]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line, "zapp/live/0 first\n");
    drop(out); // the reader goes away, like `| head -1`
    publish(&h, &b, "zapp/live/1", "second", false);
    let code = wait_timeout(&mut child, Duration::from_secs(10));
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert_eq!(code, Some(0));
    assert_eq!(err, "", "a closed pipe is not an error");
}

#[test]
fn pub_auth_failure_exits_2() {
    let Some(b) = start(true) else { return };
    let h = Home::new();
    let p = b.port.to_string();
    // A saved profile with a user and no password source: the broker refuses.
    let o = run(
        h.cmd(&[
            "profile",
            "add",
            "--name",
            "locked",
            "--host",
            "127.0.0.1",
            "--port",
            &p,
            "--user",
            "zapp",
            "--password-mode",
            "none",
        ]),
        None,
    );
    assert_eq!(o.code, Some(0), "profile add: {}", o.stderr);
    let o = run(
        h.cmd(&["pub", "--profile", "locked", "-t", "zapp/x", "-m", "hi"]),
        None,
    );
    assert_eq!(o.code, Some(2), "stderr: {}", o.stderr);
    assert!(!o.stderr.trim().is_empty());
    assert_eq!(o.stdout, "");
}

#[test]
fn password_prompt_is_refused_headless() {
    let h = Home::new();
    let o = run(
        h.cmd(&[
            "pub",
            "--host",
            "127.0.0.1",
            "--port",
            "1",
            "--user",
            "u",
            "-t",
            "t",
            "-m",
            "m",
        ]),
        None,
    );
    assert_eq!(o.code, Some(2));
    assert!(
        o.stderr
            .contains("password prompt is not available in sub/pub; use keyring"),
        "{}",
        o.stderr
    );
}

#[test]
fn connect_failure_exits_2() {
    let h = Home::new();
    let p = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
        .to_string(); // closed again: nothing listens
    let o = run(
        h.cmd(&["sub", "--host", "127.0.0.1", "--port", &p, "-n", "1"]),
        None,
    );
    assert_eq!(o.code, Some(2));
    assert!(!o.stderr.trim().is_empty());
}
