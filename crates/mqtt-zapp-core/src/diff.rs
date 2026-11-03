use crate::payload::{PayloadKind, detect, pretty_lines};
use serde::Serialize;
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Diff {
    Json(Vec<JsonChange>),
    Text(Vec<LineChange>),
    Binary { equal: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonChange {
    pub path: String,
    pub kind: ChangeKind,
    pub old: Option<String>,
    pub new: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ChangeKind {
    Added,
    Removed,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineChange {
    pub tag: LineTag,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LineTag {
    Same,
    Added,
    Removed,
}

pub fn diff(prev: &[u8], cur: &[u8]) -> Diff {
    let (pk, ck) = (detect(prev), detect(cur));
    match (&pk, &ck) {
        (PayloadKind::Binary, PayloadKind::Binary) => Diff::Binary { equal: prev == cur },
        (PayloadKind::Binary, _) | (_, PayloadKind::Binary) => Diff::Binary { equal: false },
        (PayloadKind::Json(p), PayloadKind::Json(c)) => {
            let mut out = Vec::new();
            walk("", p, c, &mut out);
            Diff::Json(out)
        }
        _ => {
            let a = joined(&pk, prev);
            let b = joined(&ck, cur);
            let lines = TextDiff::from_lines(&a, &b)
                .iter_all_changes()
                .map(|ch| LineChange {
                    tag: match ch.tag() {
                        ChangeTag::Equal => LineTag::Same,
                        ChangeTag::Insert => LineTag::Added,
                        ChangeTag::Delete => LineTag::Removed,
                    },
                    text: ch.value().trim_end_matches('\n').to_string(),
                })
                .collect();
            Diff::Text(lines)
        }
    }
}

/// Lines each terminated by `\n`, so a trailing line compares equal on both sides.
fn joined(kind: &PayloadKind, payload: &[u8]) -> String {
    pretty_lines(kind, payload)
        .iter()
        .map(|l| format!("{l}\n"))
        .collect()
}

fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn push(
    out: &mut Vec<JsonChange>,
    path: &str,
    kind: ChangeKind,
    old: Option<&Value>,
    new: Option<&Value>,
) {
    out.push(JsonChange {
        path: path.to_string(),
        kind,
        old: old.map(Value::to_string),
        new: new.map(Value::to_string),
    });
}

fn walk(path: &str, prev: &Value, cur: &Value, out: &mut Vec<JsonChange>) {
    match (prev, cur) {
        (Value::Object(p), Value::Object(c)) => {
            for (k, cv) in c {
                let sub = child(path, k);
                match p.get(k) {
                    Some(pv) => walk(&sub, pv, cv, out),
                    None => push(out, &sub, ChangeKind::Added, None, Some(cv)),
                }
            }
            for (k, pv) in p {
                if !c.contains_key(k) {
                    let sub = child(path, k);
                    push(out, &sub, ChangeKind::Removed, Some(pv), None);
                }
            }
        }
        (Value::Array(p), Value::Array(c)) => {
            for i in 0..p.len().max(c.len()) {
                let sub = format!("{path}[{i}]");
                match (p.get(i), c.get(i)) {
                    (Some(pv), Some(cv)) => walk(&sub, pv, cv, out),
                    (Some(pv), None) => push(out, &sub, ChangeKind::Removed, Some(pv), None),
                    (None, Some(cv)) => push(out, &sub, ChangeKind::Added, None, Some(cv)),
                    (None, None) => {}
                }
            }
        }
        _ if prev != cur => push(out, path, ChangeKind::Changed, Some(prev), Some(cur)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_key_changes() {
        let d = diff(
            br#"{"ENERGY":{"Power":10},"A":1}"#,
            br#"{"ENERGY":{"Power":12},"B":2}"#,
        );
        let Diff::Json(c) = d else { panic!() };
        assert!(c.iter().any(|x| x.path == "ENERGY.Power"
            && matches!(x.kind, ChangeKind::Changed)
            && x.old.as_deref() == Some("10")
            && x.new.as_deref() == Some("12")));
        assert!(
            c.iter()
                .any(|x| x.path == "A" && matches!(x.kind, ChangeKind::Removed))
        );
        assert!(
            c.iter()
                .any(|x| x.path == "B" && matches!(x.kind, ChangeKind::Added))
        );
    }
    #[test]
    fn array_index_paths() {
        let Diff::Json(c) = diff(br#"{"a":[1,2]}"#, br#"{"a":[1,3]}"#) else {
            panic!()
        };
        assert_eq!(c[0].path, "a[1]");
    }
    #[test]
    fn identical_json_has_no_changes() {
        let Diff::Json(c) = diff(br#"{"a":1}"#, br#"{"a":1}"#) else {
            panic!()
        };
        assert!(c.is_empty());
    }
    #[test]
    fn text_line_diff() {
        let Diff::Text(l) = diff(b"online", b"offline") else {
            panic!()
        };
        assert_eq!(
            l.iter()
                .filter(|x| matches!(x.tag, LineTag::Removed))
                .count(),
            1
        );
    }
    #[test]
    fn appended_line_has_no_false_removal() {
        let Diff::Text(l) = diff(b"a", b"a\nb") else {
            panic!()
        };
        let n = |t| l.iter().filter(|x| x.tag == t).count();
        assert_eq!(n(LineTag::Removed), 0);
        assert_eq!(n(LineTag::Added), 1);
        assert_eq!(n(LineTag::Same), 1);
    }
    #[test]
    fn mixed_json_and_text_falls_back_to_text() {
        assert!(matches!(diff(br#"{"a":1}"#, b"oops"), Diff::Text(_)));
    }
    #[test]
    fn binary() {
        assert!(matches!(
            diff(&[0xff], &[0xff]),
            Diff::Binary { equal: true }
        ));
    }
    #[test]
    fn binary_vs_text_is_unequal_binary() {
        assert!(matches!(diff(&[0xff], b"x"), Diff::Binary { equal: false }));
    }
    #[test]
    fn changes_in_document_order() {
        let Diff::Json(c) = diff(br#"{"x":1,"y":1,"z":1}"#, br#"{"x":2,"w":1,"z":2}"#) else {
            panic!()
        };
        let p: Vec<_> = c.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(p, ["x", "w", "z", "y"]);
    }
}
