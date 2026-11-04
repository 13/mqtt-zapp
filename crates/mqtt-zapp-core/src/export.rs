//! Saving payloads and histories to files.

use crate::payload::{self, Message, PayloadKind};
use chrono::{DateTime, Local, SecondsFormat};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};

const TOPIC_MAX: usize = 80;

/// A safe file name stem: the topic reduced to `[A-Za-z0-9._-]` (cut to 80
/// bytes) followed by `-YYYYmmdd-HHMMSS`.
pub fn file_stem(topic: &str, now: DateTime<Local>) -> String {
    let mut s = String::new();
    for c in topic.chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            c
        } else {
            '_'
        };
        if c == '_' && s.ends_with('_') {
            continue;
        }
        s.push(c);
    }
    let s = s.trim_start_matches(['.', '_']);
    // ASCII only, so any byte index is a char boundary.
    let s = &s[..s.len().min(TOPIC_MAX)];
    let s = if s.is_empty() { "topic" } else { s };
    format!("{s}-{}", now.format("%Y%m%d-%H%M%S"))
}

/// Create `dir/stem.ext` without overwriting: on a clash try `stem-1.ext`
/// up to `stem-99.ext`. Files are mode 0600, like the config files.
pub fn write_new(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    for n in 0..=99 {
        let name = if n == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{n}.{ext}")
        };
        let path = dir.join(name);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&path) {
            Ok(mut f) => {
                f.write_all(bytes)?;
                f.sync_all()?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "too many files with this name",
    ))
}

/// File extension for a payload: `json`, `txt` or `bin`.
pub fn extension(payload: &[u8]) -> &'static str {
    match payload::detect(payload) {
        PayloadKind::Json(_) => "json",
        PayloadKind::Text(_) => "txt",
        PayloadKind::Binary => "bin",
    }
}

/// One JSON object per line, in the order given.
pub fn history_jsonl(msgs: &[&Message]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in msgs {
        let time =
            DateTime::<Local>::from(m.received_at).to_rfc3339_opts(SecondsFormat::Millis, false);
        let value = match payload::detect(&m.payload) {
            PayloadKind::Json(v) => v,
            PayloadKind::Text(t) => Value::String(t),
            PayloadKind::Binary => json!({ "base64": payload::base64(&m.payload) }),
        };
        let mut o = serde_json::Map::new();
        o.insert("time".into(), json!(time));
        o.insert("qos".into(), json!(m.qos));
        o.insert("retain".into(), json!(m.retain));
        o.insert("payload".into(), value);
        if let Some(n) = m.truncated_from {
            o.insert("truncated_from".into(), json!(n));
        }
        if m.sparkplug {
            o.insert("sparkplug".into(), json!(true));
        }
        out.extend_from_slice(Value::Object(o).to_string().as_bytes());
        out.push(b'\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use chrono::TimeZone;
    use std::time::SystemTime;

    fn msg(p: &[u8]) -> Message {
        Message {
            payload: Bytes::copy_from_slice(p),
            qos: 1,
            retain: false,
            received_at: SystemTime::UNIX_EPOCH,
            truncated_from: None,
            props: None,
            sparkplug: false,
            wire_len: None,
        }
    }

    #[test]
    fn save_name_is_safe_and_unique() {
        let now = Local.with_ymd_and_hms(2026, 11, 4, 12, 30, 5).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let long = "x".repeat(300);
        for t in ["../../etc/passwd", "$SYS/broker", &long, "a\0b", "ü/ä"] {
            let stem = file_stem(t, now);
            assert!(!stem.contains('/') && !stem.contains('\0'), "{stem}");
            assert!(!stem.starts_with(".."), "{stem}");
            assert!(stem.len() <= 80 + 16, "{stem}");
            assert!(stem.ends_with("-20261104-123005"), "{stem}");
            let a = write_new(dir.path(), &stem, "txt", b"1").unwrap();
            let b = write_new(dir.path(), &stem, "txt", b"2").unwrap();
            assert_ne!(a, b);
            assert_eq!(a.parent(), Some(dir.path()));
        }
        assert_eq!(file_stem("ü/ä", now), "topic-20261104-123005");
        assert_eq!(file_stem("a//b", now), "a_b-20261104-123005");
    }

    #[test]
    fn write_new_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_new(dir.path(), "s", "bin", b"one").unwrap();
        let second = write_new(dir.path(), "s", "bin", b"two").unwrap();
        assert_eq!(first.file_name().unwrap(), "s.bin");
        assert_eq!(second.file_name().unwrap(), "s-1.bin");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        for _ in 0..98 {
            write_new(dir.path(), "s", "bin", b"x").unwrap();
        }
        assert!(write_new(dir.path(), "s", "bin", b"x").is_err());
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&first).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn history_jsonl_shapes() {
        let json = msg(br#"{"a":1}"#);
        let text = msg(b"hello");
        let bin = msg(&[0xff, 0xfe, 0x00]);
        let mut cut = msg(b"abc");
        cut.truncated_from = Some(1000);
        let mut sp = msg(br#"{"metrics":[]}"#);
        sp.sparkplug = true;
        let out = history_jsonl(&[&json, &text, &bin, &cut, &sp]);
        let lines: Vec<Value> = std::str::from_utf8(&out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0]["payload"], json!({"a": 1}));
        assert_eq!(lines[0]["qos"], 1);
        assert_eq!(lines[0]["retain"], false);
        assert!(lines[0]["time"].as_str().unwrap().contains('T'));
        assert_eq!(lines[1]["payload"], "hello");
        assert_eq!(lines[2]["payload"], json!({"base64": "//4A"}));
        assert_eq!(lines[3]["truncated_from"], 1000);
        assert!(lines[0].get("truncated_from").is_none());
        assert_eq!(lines[4]["sparkplug"], true);
        assert!(lines[0].get("sparkplug").is_none());
    }

    #[test]
    fn extension_follows_kind() {
        assert_eq!(extension(b"{}"), "json");
        assert_eq!(extension(b"hi"), "txt");
        assert_eq!(extension(&[0, 1]), "bin");
    }
}
