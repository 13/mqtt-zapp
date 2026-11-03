//! The tree filter. Plain mode needs no syntax: every space-separated term
//! (or `"quoted phrase"`) must appear, case-insensitively, in the topic path
//! or the latest payload. Regex mode is opt-in (`ctrl-r` in the TUI).

use crate::{NodeId, TopicTree};
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::collections::HashSet;
use std::ops::Range;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum QueryMode {
    #[default]
    Plain,
    Regex,
}

/// A parsed query, ready to match many nodes.
#[derive(Debug, Clone)]
pub enum Query {
    /// Lowercased terms; all must match.
    Terms(Vec<String>),
    Regex(Regex),
}

#[derive(Debug, Clone, Serialize)]
pub struct Filter {
    pub query: String,
    pub mode: QueryMode,
    #[serde(skip)]
    pub compiled: Query,
    pub matches: HashSet<NodeId>,
    /// Matches, all their ancestors, and ROOT.
    pub visible: HashSet<NodeId>,
}

impl Filter {
    /// Byte ranges to highlight in `haystack`.
    pub fn ranges(&self, haystack: &str) -> Vec<Range<usize>> {
        self.compiled.ranges(haystack)
    }
}

/// Split on whitespace; `"double quoted"` is one term, an unclosed quote runs
/// to the end, and empty terms are dropped.
pub fn terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl Query {
    /// `Ok(None)` for an empty query (no filter); `Err` for a bad regex.
    pub fn parse(query: &str, mode: QueryMode) -> Result<Option<Query>, String> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(None);
        }
        Ok(Some(match mode {
            QueryMode::Plain => {
                let t: Vec<String> = terms(query).iter().map(|t| t.to_lowercase()).collect();
                if t.is_empty() {
                    return Ok(None);
                }
                Query::Terms(t)
            }
            QueryMode::Regex => Query::Regex(
                RegexBuilder::new(query)
                    .case_insensitive(true)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|e| e.to_string())?,
            ),
        }))
    }

    /// Whether a node matches. Terms may each match the path or the payload.
    fn matches(&self, path: &str, payload: &str) -> bool {
        match self {
            Query::Terms(terms) => {
                let (path, payload) = (path.to_lowercase(), payload.to_lowercase());
                terms
                    .iter()
                    .all(|t| path.contains(t.as_str()) || payload.contains(t.as_str()))
            }
            Query::Regex(re) => re.is_match(path) || re.is_match(payload),
        }
    }

    /// Sorted, merged byte ranges of every match in `haystack`.
    pub fn ranges(&self, haystack: &str) -> Vec<Range<usize>> {
        let mut all: Vec<Range<usize>> = match self {
            Query::Terms(terms) => terms
                .iter()
                .flat_map(|t| term_ranges(haystack, t))
                .collect(),
            Query::Regex(re) => re
                .find_iter(haystack)
                .filter(|m| !m.is_empty())
                .map(|m| m.range())
                .collect(),
        };
        all.sort_by_key(|r| r.start);
        let mut out: Vec<Range<usize>> = Vec::new();
        for r in all {
            match out.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => out.push(r),
            }
        }
        out
    }
}

/// Filter `tree` by `query`: `Ok(None)` for an empty query, `Err` for an
/// invalid regex.
pub fn filter(tree: &TopicTree, query: &str, mode: QueryMode) -> Result<Option<Filter>, String> {
    let Some(compiled) = Query::parse(query, mode)? else {
        return Ok(None);
    };
    let mut matches = HashSet::new();
    let mut visible = HashSet::from([TopicTree::ROOT]);
    for id in (1..tree.len()).filter(|&id| tree.is_alive(id)) {
        let node = tree.node(id);
        let payload = node
            .latest
            .as_ref()
            .map(|m| String::from_utf8_lossy(&m.payload))
            .unwrap_or_default();
        if compiled.matches(&tree.full_topic(id), &payload) {
            matches.insert(id);
            let mut cur = Some(id);
            while let Some(i) = cur {
                if !visible.insert(i) {
                    break;
                }
                cur = tree.node(i).parent;
            }
        }
    }
    Ok(Some(Filter {
        query: query.trim().to_owned(),
        mode,
        compiled,
        matches,
        visible,
    }))
}

/// Highlight ranges of `query` in `haystack`; nothing for an empty or
/// invalid query.
pub fn match_ranges(haystack: &str, query: &str, mode: QueryMode) -> Vec<Range<usize>> {
    match Query::parse(query, mode) {
        Ok(Some(q)) => q.ranges(haystack),
        _ => Vec::new(),
    }
}

/// Case-insensitive, non-overlapping byte ranges of one lowercased `term`.
fn term_ranges(haystack: &str, term: &str) -> Vec<Range<usize>> {
    let q: Vec<char> = term.chars().collect();
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    let mut from = 0;
    while from < haystack.len() {
        let mut qi = 0;
        let mut end = from;
        let mut chars = haystack[from..].char_indices();
        while qi < q.len() {
            let Some((off, c)) = chars.next() else { break };
            let mut ok = true;
            for lc in c.to_lowercase() {
                if qi < q.len() && q[qi] == lc {
                    qi += 1;
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok {
                break;
            }
            end = from + off + c.len_utf8();
        }
        if qi == q.len() {
            out.push(from..end);
            from = end;
        } else {
            from += haystack[from..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use std::time::{Duration, Instant, SystemTime};

    fn m(s: &str) -> Message {
        Message {
            payload: s.as_bytes().to_vec(),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
        }
    }

    /// Plain-mode filter, as the single-term tests used it.
    fn filter(t: &TopicTree, q: &str) -> Option<Filter> {
        super::filter(t, q, QueryMode::Plain).unwrap()
    }

    fn hits(t: &TopicTree, q: &str, mode: QueryMode) -> Vec<String> {
        let f = super::filter(t, q, mode).unwrap().unwrap();
        let mut v: Vec<String> = f.matches.iter().map(|&id| t.full_topic(id)).collect();
        v.sort();
        v
    }

    #[test]
    fn terms_split_and_quotes() {
        assert_eq!(terms("  wzr  online "), ["wzr", "online"]);
        assert_eq!(terms(r#"a "b c" d"#), ["a", "b c", "d"]);
        assert_eq!(terms(r#"x "open quote"#), ["x", "open quote"]);
        assert!(terms(r#" "" "#).is_empty());
    }

    #[test]
    fn and_terms_across_path_and_payload() {
        let t = tree_with(&[
            ("muh/esp/wzr1/LWT", "online"),
            ("muh/esp/wzr2/LWT", "offline"),
            ("muh/esp/abc/LWT", "online"),
        ]);
        assert_eq!(
            hits(&t, "wzr online", QueryMode::Plain),
            ["muh/esp/wzr1/LWT"]
        );
    }

    #[test]
    fn and_requires_all() {
        let t = tree_with(&[("a/wzr", "x")]);
        assert!(filter(&t, "wzr nope").unwrap().matches.is_empty());
    }

    #[test]
    fn quoted_phrase_matches_with_space() {
        let t = tree_with(&[("ups", "ON LINE"), ("ups2", "ON BATT LINE")]);
        assert_eq!(hits(&t, r#""on line""#, QueryMode::Plain), ["ups"]);
    }

    #[test]
    fn regex_mode_matches() {
        let t = tree_with(&[("dev/esp32-01", "a"), ("dev/esp8266", "b")]);
        assert_eq!(hits(&t, r"esp32-\d+$", QueryMode::Regex), ["dev/esp32-01"]);
        assert_eq!(
            hits(&t, "ESP", QueryMode::Regex).len(),
            2,
            "case-insensitive"
        );
    }

    #[test]
    fn invalid_regex_is_err() {
        let t = tree_with(&[("a", "b")]);
        assert!(super::filter(&t, "(", QueryMode::Regex).is_err());
        assert!(super::filter(&t, "(", QueryMode::Plain).unwrap().is_some());
    }

    #[test]
    fn match_ranges_multi_term_merged() {
        assert_eq!(
            match_ranges("wzr-online", "wzr line onl", QueryMode::Plain),
            vec![0..3, 4..10]
        );
        assert_eq!(
            match_ranges("a1b22", r"\d+", QueryMode::Regex),
            vec![1..2, 3..5]
        );
        assert!(match_ranges("abc", "(", QueryMode::Regex).is_empty());
    }

    fn tree_with(items: &[(&str, &str)]) -> TopicTree {
        let mut t = TopicTree::new(1);
        for (topic, payload) in items {
            t.insert(topic, m(payload));
        }
        t
    }

    #[test]
    fn plain_substring_matches_topic_anywhere() {
        let t = tree_with(&[("muh/esp/esp32-WZR1/LWT", "online")]);
        let f = filter(&t, "wzr").unwrap();
        assert!(f.matches.contains(&t.find("muh/esp/esp32-WZR1").unwrap()));
    }

    #[test]
    fn matches_latest_payload() {
        let t = tree_with(&[(
            "tasmota/tele/tasmota_A/STATE",
            r#"{"DeviceName":"wzr-lamp"}"#,
        )]);
        let f = filter(&t, "WZR").unwrap();
        assert!(
            f.matches
                .contains(&t.find("tasmota/tele/tasmota_A/STATE").unwrap())
        );
    }

    #[test]
    fn ancestors_are_visible_non_matches_hidden() {
        let t = tree_with(&[("a/wzr", "1"), ("b/c", "2")]);
        let f = filter(&t, "wzr").unwrap();
        assert!(f.visible.contains(&t.find("a").unwrap()));
        assert!(f.visible.contains(&TopicTree::ROOT));
        assert!(!f.visible.contains(&t.find("b").unwrap()));
    }

    #[test]
    fn empty_query_is_no_filter() {
        assert!(filter(&tree_with(&[("a", "1")]), "  ").is_none());
    }

    #[test]
    fn no_prefix_syntax_needed() {
        let t = tree_with(&[("x/device:wzr", "1")]);
        assert!(!filter(&t, "wzr").unwrap().matches.is_empty());
    }

    #[test]
    fn binary_payload_does_not_panic() {
        let mut t = TopicTree::new(1);
        t.insert(
            "bin",
            Message {
                payload: vec![0xff, 0x00],
                qos: 0,
                retain: false,
                received_at: SystemTime::now(),
            },
        );
        assert!(filter(&t, "zz").unwrap().matches.is_empty());
    }

    #[test]
    fn match_ranges_case_insensitive() {
        assert_eq!(
            match_ranges("aWZRbwzr", "wzr", QueryMode::Plain),
            vec![1..4, 5..8]
        );
    }

    #[test]
    fn fast_enough_at_spec_scale() {
        // 5000 topics x 1 KB payloads
        let mut t = TopicTree::new(1);
        let p = "x".repeat(1024);
        for i in 0..5000 {
            t.insert(&format!("tasmota/tele/dev_{i}/SENSOR"), m(&p));
        }
        let s = Instant::now();
        filter(&t, "dev_4999");
        filter(&t, "tele dev_4999 xxx");
        super::filter(&t, r"dev_49\d9", QueryMode::Regex).unwrap();
        assert!(
            s.elapsed() < Duration::from_millis(200),
            "{:?}",
            s.elapsed()
        );
    }
}
