//! The payload format picked in the publish form: raw text, JSON or XML.
//! The format only checks and pretty-prints; the bytes sent are always the
//! text as typed.

use quick_xml::events::Event;
use quick_xml::{Reader, Writer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Raw,
    Json,
    Xml,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Raw, Format::Json, Format::Xml];

    pub fn label(self) -> &'static str {
        match self {
            Format::Raw => "raw",
            Format::Json => "json",
            Format::Xml => "xml",
        }
    }

    /// The format `delta` steps away, wrapping.
    pub fn step(self, delta: isize) -> Format {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0) as isize;
        Self::ALL[(i + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    /// JSON for a valid object or array, XML for a well-formed document,
    /// raw otherwise. A bare `42` or `"x"` stays raw.
    pub fn detect(text: &str) -> Format {
        let t = text.trim_start();
        if t.starts_with(['{', '[']) && serde_json::from_str::<serde_json::Value>(text).is_ok() {
            Format::Json
        } else if t.starts_with('<') && check_xml(text).is_ok() {
            Format::Xml
        } else {
            Format::Raw
        }
    }

    /// Whether `text` is valid in this format; `None` for raw or an empty
    /// payload, which have nothing to check.
    pub fn check(self, text: &str) -> Option<Result<(), String>> {
        if text.trim().is_empty() {
            return None;
        }
        match self {
            Format::Raw => None,
            Format::Json => Some(
                serde_json::from_str::<serde_json::Value>(text)
                    .map(drop)
                    .map_err(|e| format!("invalid JSON: {e}")),
            ),
            Format::Xml => Some(check_xml(text)),
        }
    }

    /// `text` pretty-printed in this format.
    pub fn pretty(self, text: &str) -> Result<String, String> {
        match self {
            Format::Raw => Err("raw: pick json or xml to format".into()),
            Format::Json => {
                let value: serde_json::Value =
                    serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
                serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
            }
            Format::Xml => pretty_xml(text),
        }
    }
}

/// Walk `text` as XML: tags must nest and close, with one root element and
/// no text outside it. Calls `each` on every event.
fn walk_xml(text: &str, mut each: impl FnMut(Event) -> Result<(), String>) -> Result<(), String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let (mut depth, mut roots) = (0usize, 0usize);
    loop {
        let ev = reader
            .read_event()
            .map_err(|e| format!("invalid XML: {e}"))?;
        match &ev {
            Event::Eof => break,
            Event::Start(_) => {
                roots += usize::from(depth == 0);
                depth += 1;
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Empty(_) => roots += usize::from(depth == 0),
            Event::Text(_) | Event::CData(_) if depth == 0 => {
                return Err("invalid XML: text outside the root element".into());
            }
            _ => {}
        }
        if roots > 1 {
            return Err("invalid XML: more than one root element".into());
        }
        each(ev)?;
    }
    match (depth, roots) {
        (0, 1) => Ok(()),
        (0, _) => Err("invalid XML: no root element".into()),
        _ => Err("invalid XML: unclosed element".into()),
    }
}

fn check_xml(text: &str) -> Result<(), String> {
    walk_xml(text, |_| Ok(()))
}

fn pretty_xml(text: &str) -> Result<String, String> {
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    walk_xml(text, |ev| writer.write_event(ev).map_err(|e| e.to_string()))?;
    String::from_utf8(writer.into_inner()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_picks_json_xml_or_raw() {
        assert_eq!(Format::detect(r#"{"a": 1}"#), Format::Json);
        assert_eq!(Format::detect(" [1, 2]"), Format::Json);
        assert_eq!(Format::detect("<a><b/></a>"), Format::Xml);
        assert_eq!(Format::detect("42"), Format::Raw);
        assert_eq!(Format::detect("{broken"), Format::Raw);
        assert_eq!(Format::detect("<a>"), Format::Raw);
        assert_eq!(Format::detect(""), Format::Raw);
    }

    #[test]
    fn check_reports_validity() {
        assert_eq!(Format::Raw.check("anything"), None);
        assert_eq!(Format::Json.check("  "), None);
        assert_eq!(Format::Json.check("[1]"), Some(Ok(())));
        assert!(matches!(Format::Json.check("[1"), Some(Err(_))));
        assert_eq!(
            Format::Xml.check("<?xml version=\"1.0\"?><a x=\"1\">t</a>"),
            Some(Ok(()))
        );
        for bad in ["<a></b>", "<a>", "<a/><b/>", "text <a/>", "</a>"] {
            assert!(matches!(Format::Xml.check(bad), Some(Err(_))), "{bad}");
        }
    }

    #[test]
    fn pretty_indents_json_and_xml() {
        assert_eq!(
            Format::Json.pretty(r#"{"a":1}"#).unwrap(),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(
            Format::Xml.pretty("<a><b>t</b><c/></a>").unwrap(),
            "<a>\n  <b>t</b>\n  <c/>\n</a>"
        );
        assert!(Format::Xml.pretty("<a>").is_err());
        assert!(Format::Raw.pretty("x").is_err());
    }

    #[test]
    fn step_wraps() {
        assert_eq!(Format::Raw.step(-1), Format::Xml);
        assert_eq!(Format::Xml.step(1), Format::Raw);
    }
}
