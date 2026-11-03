use serde::Serialize;
use std::time::SystemTime;

#[derive(Clone, Debug, Serialize)]
pub struct Message {
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
    pub received_at: SystemTime,
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
            c if c.is_control() => out.push('\u{fffd}'),
            c => out.push(c),
        }
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
