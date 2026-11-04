use bytes::Bytes;
use serde::Serialize;
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    /// Shared buffer: clones (`latest`, history) point at the same bytes.
    pub payload: Bytes,
    pub qos: u8,
    pub retain: bool,
    pub received_at: SystemTime,
    /// The stored payload's length before `truncate_to` cut it (the decoded
    /// JSON length for Sparkplug, not the wire length).
    pub truncated_from: Option<usize>,
    /// MQTT 5 publish properties worth showing; `None` when there are none.
    /// Shared (`Arc`): `latest` and its history entries point at one copy.
    pub props: Option<Arc<Props>>,
    /// The payload is the JSON form of a decoded Sparkplug B message.
    pub sparkplug: bool,
    /// The received length when ingest replaced the payload (Sparkplug
    /// decoding); `None` when the payload is as received.
    pub wire_len: Option<usize>,
}

/// The MQTT 5 publish properties the header shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Props {
    pub content_type: Option<String>,
    pub response_topic: Option<String>,
    pub correlation_data: Option<Bytes>,
    /// Seconds.
    pub message_expiry: Option<u32>,
    /// `payload_format_indicator == 1`.
    pub payload_utf8: Option<bool>,
    pub user: Vec<(String, String)>,
    /// User properties cut by `capped`, so the header can still count them.
    pub dropped_user: usize,
}

/// Limits applied to wire-supplied properties before they are stored.
pub const PROPS_MAX_USER: usize = 32;
pub const PROPS_MAX_STR: usize = 256;
pub const PROPS_MAX_CORR: usize = 64;

fn cut(s: &mut String, max: usize) {
    if s.len() > max {
        let mut n = max;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
}

impl Props {
    /// Bound the heap a publisher can make us keep per message.
    pub fn capped(mut self) -> Props {
        for s in [&mut self.content_type, &mut self.response_topic]
            .into_iter()
            .flatten()
        {
            cut(s, PROPS_MAX_STR);
        }
        if let Some(c) = &mut self.correlation_data
            && c.len() > PROPS_MAX_CORR
        {
            *c = c.slice(..PROPS_MAX_CORR);
        }
        if self.user.len() > PROPS_MAX_USER {
            self.dropped_user += self.user.len() - PROPS_MAX_USER;
            self.user.truncate(PROPS_MAX_USER);
        }
        for (k, v) in &mut self.user {
            cut(k, PROPS_MAX_STR);
            cut(v, PROPS_MAX_STR);
        }
        self
    }

    /// Heap bytes held, counted toward the history budget.
    pub fn heap_len(&self) -> usize {
        self.content_type.as_ref().map_or(0, String::len)
            + self.response_topic.as_ref().map_or(0, String::len)
            + self.correlation_data.as_ref().map_or(0, Bytes::len)
            + self
                .user
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
    }
}

impl Props {
    pub fn is_empty(&self) -> bool {
        *self == Props::default()
    }

    /// Display chips: type, reply, corr, expiry, utf8, then `k=v` pairs.
    /// Every wire-derived string is sanitized.
    pub fn summary(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(t) = &self.content_type {
            v.push(format!("type {}", sanitize(t)));
        }
        if let Some(t) = &self.response_topic {
            v.push(format!("reply {}", sanitize(t)));
        }
        if let Some(c) = &self.correlation_data {
            let hex: String = c.iter().take(8).map(|b| format!("{b:02x}")).collect();
            let more = if c.len() > 8 { "\u{2026}" } else { "" };
            v.push(format!("corr {hex}{more}"));
        }
        if let Some(e) = self.message_expiry {
            v.push(format!("expiry {e}s"));
        }
        if self.payload_utf8 == Some(true) {
            v.push("utf8".to_owned());
        }
        for (k, val) in &self.user {
            v.push(format!("{}={}", sanitize(k), sanitize(val)));
        }
        v
    }
}

impl Message {
    /// Bytes this message adds to the history budget: payload plus props.
    pub fn size(&self) -> usize {
        self.payload.len() + self.props.as_ref().map_or(0, |p| p.heap_len())
    }

    /// Keep at most `max` payload bytes, recording the original length.
    /// The kept bytes are copied so the large buffer is freed.
    pub fn truncate_to(&mut self, max: usize) {
        let len = self.payload.len();
        if len > max {
            self.truncated_from = Some(len);
            self.payload = Bytes::copy_from_slice(&self.payload[..max]);
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PayloadKind {
    Json(serde_json::Value),
    Text(String),
    Binary,
}

/// Classify a payload. Only JSON objects/arrays count as `Json`; scalars stay `Text`.
pub fn detect(payload: &[u8]) -> PayloadKind {
    if payload.contains(&0) {
        return PayloadKind::Binary;
    }
    let Ok(text) = std::str::from_utf8(payload) else {
        return PayloadKind::Binary;
    };
    let trimmed = text.trim();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(v) = serde_json::from_str(trimmed)
    {
        return PayloadKind::Json(v);
    }
    PayloadKind::Text(text.to_owned())
}

/// Make text safe to hand to a terminal: keep `\n`, expand tabs, replace other controls.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() || is_invisible_format(c) => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

/// Upper bound on bytes handed to the clipboard.
pub const CLIPBOARD_MAX: usize = 1 << 20;

/// Bidi controls and zero-width characters that hide or reorder text.
/// U+200D (ZWJ) is deliberately excluded: emoji sequences need it.
fn is_invisible_format(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{200B}' | '\u{200C}' | '\u{200E}' | '\u{200F}'
        | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}'
        | '\u{FEFF}')
}

/// Text for the clipboard: keeps `\n` and `\t`, drops other controls and invisible
/// format characters, and stops at a char boundary at or below `max` bytes.
pub fn clipboard_safe(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max));
    for c in s.chars() {
        if (c.is_control() && c != '\n' && c != '\t') || is_invisible_format(c) {
            continue;
        }
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

pub fn pretty_lines(kind: &PayloadKind, payload: &[u8]) -> Vec<String> {
    match kind {
        PayloadKind::Json(v) => serde_json::to_string_pretty(v)
            .unwrap_or_default()
            .split('\n')
            .map(sanitize)
            .collect(),
        PayloadKind::Text(t) => sanitize(t).split('\n').map(str::to_owned).collect(),
        PayloadKind::Binary => hex_dump(payload),
    }
}

pub fn hex_dump(payload: &[u8]) -> Vec<String> {
    payload
        .chunks(16)
        .enumerate()
        .map(|(i, chunk)| {
            let hex = chunk
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            let ascii: String = chunk
                .iter()
                .map(|&b| {
                    if (0x20..=0x7e).contains(&b) {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            format!("{:08x}  {:<48}  {}", i * 16, hex, ascii)
        })
        .collect()
}

/// Most numeric paths `numeric_paths` returns.
const MAX_NUMERIC_PATHS: usize = 50;

/// Dotted paths (`ENERGY.Power`, `a.0`) to the numeric leaves of a JSON
/// object or array, in document order. Empty for anything else.
pub fn numeric_paths(payload: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    if let PayloadKind::Json(v) = detect(payload) {
        walk_numbers(&v, String::new(), &mut out);
    }
    out
}

/// Where each chartable value sits in `pretty_lines` of `payload` (0-based
/// line, dotted path), in `numeric_paths` order with the same cap. A payload
/// that is a plain number is `[(line, None)]` on its line; anything else
/// without numbers is empty.
pub fn numeric_lines(payload: &[u8]) -> Vec<(usize, Option<String>)> {
    match detect(payload) {
        PayloadKind::Json(v) => {
            let mut out = Vec::new();
            walk_lines(&v, String::new(), &mut 0, &mut out);
            out
        }
        PayloadKind::Text(t) if numeric_at(payload, None).is_some() => {
            let line = t.split('\n').position(|l| !l.trim().is_empty());
            vec![(line.unwrap_or(0), None)]
        }
        _ => Vec::new(),
    }
}

/// Lay `v` out as `serde_json::to_string_pretty` does, starting on `*line`
/// and leaving `*line` on the next free one: scalars and empty containers
/// take one line; other containers open on their line, put each child on
/// the following ones and close on a line of their own.
fn walk_lines(
    v: &serde_json::Value,
    path: String,
    line: &mut usize,
    out: &mut Vec<(usize, Option<String>)>,
) {
    use serde_json::Value;
    let join = |k: &str| {
        if path.is_empty() {
            k.to_owned()
        } else {
            format!("{path}.{k}")
        }
    };
    match v {
        Value::Object(m) if !m.is_empty() => {
            *line += 1;
            for (k, child) in m {
                walk_lines(child, join(k), line, out);
            }
            *line += 1;
        }
        Value::Array(a) if !a.is_empty() => {
            *line += 1;
            for (i, child) in a.iter().enumerate() {
                walk_lines(child, join(&i.to_string()), line, out);
            }
            *line += 1;
        }
        Value::Number(_) if !path.is_empty() && out.len() < MAX_NUMERIC_PATHS => {
            out.push((*line, Some(path)));
            *line += 1;
        }
        _ => *line += 1,
    }
}

fn walk_numbers(v: &serde_json::Value, path: String, out: &mut Vec<String>) {
    use serde_json::Value;
    let join = |k: &str| {
        if path.is_empty() {
            k.to_owned()
        } else {
            format!("{path}.{k}")
        }
    };
    match v {
        Value::Number(_) if !path.is_empty() && out.len() < MAX_NUMERIC_PATHS => out.push(path),
        Value::Object(m) => {
            for (k, child) in m {
                walk_numbers(child, join(k), out);
            }
        }
        Value::Array(a) => {
            for (i, child) in a.iter().enumerate() {
                walk_numbers(child, join(&i.to_string()), out);
            }
        }
        _ => {}
    }
}

/// Walk `path` through `v`. At an object the longest key equal to the rest
/// of the path (or a prefix of it followed by `.`) wins, so keys containing
/// dots resolve; at an array the leading segment is the index.
fn resolve<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut v = v;
    let mut rest = path;
    loop {
        match v {
            serde_json::Value::Object(m) => {
                let (key, child) = m
                    .iter()
                    .filter(|(k, _)| {
                        rest.strip_prefix(k.as_str())
                            .is_some_and(|r| r.is_empty() || r.starts_with('.'))
                    })
                    .max_by_key(|(k, _)| k.len())?;
                v = child;
                rest = &rest[key.len()..];
            }
            serde_json::Value::Array(a) => {
                let seg = rest.split('.').next()?;
                v = a.get(seg.parse::<usize>().ok()?)?;
                rest = &rest[seg.len()..];
            }
            _ => return None,
        }
        match rest.strip_prefix('.') {
            Some(r) => rest = r,
            None if rest.is_empty() => return Some(v),
            None => return None,
        }
    }
}

/// The payload as a finite number (`path` `None`), or the number at a
/// dotted JSON path.
pub fn numeric_at(payload: &[u8], path: Option<&str>) -> Option<f64> {
    let x = match path {
        None => std::str::from_utf8(payload)
            .ok()?
            .trim()
            .parse::<f64>()
            .ok()?,
        Some(path) => {
            let root: serde_json::Value = serde_json::from_slice(payload).ok()?;
            resolve(&root, path)?.as_f64()?
        }
    };
    x.is_finite().then_some(x)
}

/// Standard base64 (RFC 4648, with padding).
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Single-line, sanitized preview of at most `max_chars` chars; ends with `…` if truncated.
/// Only the first `max_chars * 4` bytes are examined, so cost is bounded.
pub fn preview(payload: &[u8], max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let window = max_chars.saturating_mul(4);
    let slice = &payload[..payload.len().min(window)];
    let flat = sanitize(&String::from_utf8_lossy(slice)).replace('\n', "\u{23ce}");
    if payload.len() > window || flat.chars().count() > max_chars {
        let mut out: String = flat.chars().take(max_chars - 1).collect();
        out.push('\u{2026}');
        out
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn props_summary_order() {
        let p = Props {
            content_type: Some("application/json".into()),
            response_topic: Some("r/t".into()),
            correlation_data: Some(Bytes::from_static(&[1, 2, 3, 4, 5, 6, 7, 8, 9])),
            message_expiry: Some(60),
            payload_utf8: Some(true),
            user: vec![("a".into(), "1".into())],
            dropped_user: 0,
        };
        assert_eq!(
            p.summary(),
            [
                "type application/json",
                "reply r/t",
                "corr 0102030405060708\u{2026}",
                "expiry 60s",
                "utf8",
                "a=1"
            ]
        );
    }

    #[test]
    fn props_summary_sanitizes() {
        let p = Props {
            user: vec![("k".into(), "x\u{202e}y".into())],
            ..Props::default()
        };
        assert!(!p.summary()[0].contains('\u{202e}'));
    }

    #[test]
    fn capped_bounds_props() {
        let p = Props {
            content_type: Some("é".repeat(400)),
            correlation_data: Some(Bytes::from(vec![1u8; 500])),
            user: (0..300)
                .map(|i| (format!("k{i}"), "v".repeat(1024)))
                .collect(),
            ..Props::default()
        }
        .capped();
        assert_eq!(p.user.len(), PROPS_MAX_USER);
        assert_eq!(p.dropped_user, 300 - PROPS_MAX_USER);
        assert!(p.user.iter().all(|(k, v)| k.len() <= 256 && v.len() == 256));
        assert_eq!(p.content_type.as_ref().unwrap().len(), 256);
        assert_eq!(p.correlation_data.as_ref().unwrap().len(), PROPS_MAX_CORR);
        assert!(p.heap_len() <= 256 + 64 + 32 * (256 + 256));
    }

    #[test]
    fn empty_props_is_empty() {
        assert!(Props::default().is_empty());
    }

    #[test]
    fn truncate_to_marks_original_len() {
        let mut m = Message {
            payload: Bytes::copy_from_slice(b"0123456789"),
            qos: 0,
            retain: false,
            received_at: std::time::SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        };
        m.truncate_to(10);
        assert_eq!(
            (&m.payload[..], m.truncated_from),
            (&b"0123456789"[..], None)
        );
        m.truncate_to(4);
        assert_eq!(&m.payload[..], b"0123");
        assert_eq!(m.truncated_from, Some(10));
    }

    #[test]
    fn sanitize_replaces_bidi_and_zero_width() {
        assert_eq!(sanitize("x/\u{202E}evil\u{200B}"), "x/\u{fffd}evil\u{fffd}");
        assert_eq!(sanitize("👩\u{200D}💻"), "👩\u{200D}💻");
    }

    #[test]
    fn clipboard_safe_strips_escapes_keeps_newlines() {
        assert_eq!(
            clipboard_safe("a\x1b[201~\nb\tc\u{202E}", 100),
            "a[201~\nb\tc"
        );
    }

    #[test]
    fn clipboard_safe_cuts_on_char_boundary() {
        assert_eq!(clipboard_safe("aé", 2), "a");
    }

    fn pretty(p: &[u8]) -> Vec<String> {
        pretty_lines(&detect(p), p)
    }

    #[test]
    fn numeric_lines_basic() {
        let p = br#"{"a":1,"b":{"c":2,"d":"x"},"e":[3,4]}"#;
        let got = numeric_lines(p);
        let want = [(1, "a"), (3, "b.c"), (7, "e.0"), (8, "e.1")];
        assert_eq!(
            got,
            want.map(|(l, k)| (l, Some(k.to_string()))).to_vec(),
            "{:#?}",
            pretty(p)
        );
        let lines = pretty(p);
        assert!(lines[3].contains("\"c\": 2"), "{}", lines[3]);
        assert_eq!(lines[8].trim(), "4");
    }

    #[test]
    fn numeric_lines_match_pretty_output() {
        let cases: &[&[u8]] = &[
            br#"{"a":1}"#,
            br#"[1,2,3]"#,
            br#"[{"x":1},{"y":[2,{"z":3}]}]"#,
            br#"{"e":{},"f":[],"g":{"h":[[],{}],"i":7}}"#,
            br#"{"deep":{"a":{"b":{"c":{"d":-1.5e3}}}}}"#,
            br#"{"s":"5","n":null,"b":true,"v":5}"#,
            br#"{"ENERGY":{"Power":12,"Voltage":230},"t":21.5}"#,
            br#"[[1,[2,[3]]],{"k":[{"m":4}]}]"#,
            br#"{"arr":[{"a":1,"b":[]},{"a":2,"b":[9]}]}"#,
            br#"  {"pad": 1}  "#,
        ];
        for p in cases {
            let lines = pretty(p);
            let got = numeric_lines(p);
            let paths: Vec<String> = got.iter().map(|(_, k)| k.clone().unwrap()).collect();
            assert_eq!(paths, numeric_paths(p), "{}", String::from_utf8_lossy(p));
            for (l, path) in &got {
                let path = path.as_deref().unwrap();
                let text = lines[*l].trim().trim_end_matches(',');
                let value = text.rsplit(": ").next().unwrap();
                let want = numeric_at(p, Some(path)).unwrap();
                assert_eq!(
                    value.parse::<f64>().ok(),
                    Some(want),
                    "{path} on {l}: {text}"
                );
                let last = path.rsplit('.').next().unwrap();
                if text.contains(": ") {
                    assert!(text.starts_with(&format!("\"{last}\"")), "{path}: {text}");
                }
            }
        }
    }

    #[test]
    fn numeric_lines_plain_number_and_text() {
        assert_eq!(numeric_lines(b"21.5"), vec![(0, None)]);
        assert_eq!(numeric_lines(b" 7 \n"), vec![(0, None)]);
        assert!(numeric_lines(b"hello").is_empty());
        assert!(numeric_lines(b"\x00\x01").is_empty());
    }

    #[test]
    fn numeric_at_dotted_key() {
        let p = br#"{"metrics":{"Inputs.A":3}}"#;
        assert_eq!(numeric_at(p, Some("metrics.Inputs.A")), Some(3.0));
    }

    #[test]
    fn numeric_at_prefers_longest_key() {
        let p = br#"{"a.b":1,"a":{"b":2}}"#;
        assert_eq!(numeric_at(p, Some("a.b")), Some(1.0));
    }

    #[test]
    fn numeric_at_array_then_dotted() {
        let p = br#"{"x":[{"y.z":5}]}"#;
        assert_eq!(numeric_at(p, Some("x.0.y.z")), Some(5.0));
    }

    #[test]
    fn every_numeric_path_resolves() {
        let p = br#"{"a.b":{"c":1},"d":[2,{"e.f":3}]}"#;
        let paths = numeric_paths(p);
        assert_eq!(paths.len(), 3);
        for path in paths {
            assert!(numeric_at(p, Some(&path)).is_some(), "{path}");
        }
    }

    #[test]
    fn numeric_paths_and_at() {
        let p = br#"{"ENERGY":{"Power":12,"V":"x"},"a":[1,2.5],"ok":true}"#;
        assert_eq!(numeric_paths(p), vec!["ENERGY.Power", "a.0", "a.1"]);
        assert_eq!(numeric_at(p, Some("a.1")), Some(2.5));
        assert_eq!(numeric_at(p, Some("ENERGY.Power")), Some(12.0));
        assert_eq!(numeric_at(p, Some("ENERGY.V")), None);
        assert_eq!(numeric_at(p, Some("nope.x")), None);
        assert_eq!(numeric_at(b" 42 ", None), Some(42.0));
        assert_eq!(numeric_at(b"nan", None), None);
        assert_eq!(numeric_at(b"inf", None), None);
        assert_eq!(numeric_at(p, None), None);
        assert!(numeric_paths(b"42").is_empty());
        assert!(numeric_paths(b"\xff").is_empty());
        let many: String = format!("[{}]", vec!["1"; 80].join(","));
        assert_eq!(numeric_paths(many.as_bytes()).len(), 50);
    }

    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(i.as_bytes()), o, "{i}");
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }
    #[test]
    fn detects_json_object() {
        assert!(matches!(detect(br#"{"a":1}"#), PayloadKind::Json(_)));
    }
    #[test]
    fn detects_multiline_pretty_json() {
        assert!(matches!(
            detect(b"{\n  \"UPSNAME\": \"apc-ups\"\n}\n"),
            PayloadKind::Json(_)
        ));
    }
    #[test]
    fn plain_number_is_text_not_json() {
        assert!(matches!(detect(b"23.5"), PayloadKind::Text(_)));
    }
    #[test]
    fn invalid_utf8_is_binary() {
        assert!(matches!(detect(&[0xff, 0xfe, 0x00]), PayloadKind::Binary));
    }
    #[test]
    fn nul_bytes_are_binary() {
        assert!(matches!(detect(b"ab\0cd"), PayloadKind::Binary));
    }
    #[test]
    fn sanitize_strips_escape() {
        assert_eq!(sanitize("a\x1b[31mb"), "a�[31mb");
    }
    #[test]
    fn sanitize_keeps_newline_expands_tab() {
        assert_eq!(sanitize("a\n\tb"), "a\n    b");
    }
    #[test]
    fn pretty_keeps_key_order() {
        let p = br#"{"z":1,"a":2}"#;
        assert_eq!(
            pretty_lines(&detect(p), p),
            vec!["{", "  \"z\": 1,", "  \"a\": 2", "}"]
        );
    }
    #[test]
    fn preview_truncates_and_flattens() {
        assert_eq!(preview(b"line1\nline2-long", 10), "line1⏎lin…");
    }
    #[test]
    fn hex_dump_row() {
        assert_eq!(
            hex_dump(b"AB")[0],
            "00000000  41 42                                             AB"
        );
    }
    #[test]
    fn huge_payload_preview_is_bounded() {
        let p = vec![b'x'; 1_000_000];
        assert_eq!(preview(&p, 80).chars().count(), 80);
    }
    #[test]
    fn preview_and_pretty_never_emit_escape() {
        let p = b"\x1b[2Jhi\x1b]0;x\x07";
        assert!(!preview(p, 80).contains('\x1b'));
        let k = detect(p);
        assert!(pretty_lines(&k, p).iter().all(|l| !l.contains('\x1b')));
    }
    #[test]
    fn hex_dump_wraps_at_16() {
        let d = hex_dump(&[0u8; 17]);
        assert_eq!(d.len(), 2);
        assert!(d[1].starts_with("00000010  00 "));
    }
}
