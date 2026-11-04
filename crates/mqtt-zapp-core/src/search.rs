//! The tree filter. Plain mode needs no syntax: every space-separated term
//! (or `"quoted phrase"`) must appear, case-insensitively, in the topic path
//! or the latest payload. Regex mode is opt-in (`ctrl-r` in the TUI).

use crate::{NodeId, TopicTree};
use regex::RegexBuilder;
use regex::bytes::{Regex as BytesRegex, RegexBuilder as BytesRegexBuilder};
use serde::Serialize;
use std::collections::HashSet;
use std::ops::Range;

/// Only this many leading payload bytes are searched.
pub const SEARCH_PAYLOAD_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum QueryMode {
    #[default]
    Plain,
    Regex,
}

/// A parsed query, ready to match many nodes.
#[derive(Debug, Clone)]
pub enum Query {
    /// All terms must match. `regexes` are the escaped, case-insensitive byte
    /// matchers; `terms` are the lowercased originals, used for highlighting.
    Terms {
        terms: Vec<String>,
        regexes: Vec<BytesRegex>,
    },
    /// `bytes` matches raw payloads; `text` highlights `&str`.
    Regex {
        text: regex::Regex,
        bytes: BytesRegex,
    },
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
                let regexes = t
                    .iter()
                    .map(|t| {
                        BytesRegexBuilder::new(&regex::escape(t))
                            .case_insensitive(true)
                            .size_limit(1 << 20)
                            .build()
                            .map_err(|e| e.to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Query::Terms { terms: t, regexes }
            }
            QueryMode::Regex => Query::Regex {
                text: RegexBuilder::new(query)
                    .case_insensitive(true)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|e| e.to_string())?,
                bytes: BytesRegexBuilder::new(query)
                    .case_insensitive(true)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|e| e.to_string())?,
            },
        }))
    }

    /// Whether a node matches. Terms may each match the path or the payload.
    fn matches(&self, path: &str, payload: &[u8]) -> bool {
        match self {
            Query::Terms { regexes, .. } => regexes
                .iter()
                .all(|re| re.is_match(path.as_bytes()) || re.is_match(payload)),
            Query::Regex { bytes, .. } => {
                bytes.is_match(path.as_bytes()) || bytes.is_match(payload)
            }
        }
    }

    /// Sorted, merged byte ranges of every match in `haystack`.
    pub fn ranges(&self, haystack: &str) -> Vec<Range<usize>> {
        let mut all: Vec<Range<usize>> = match self {
            Query::Terms { terms, .. } => terms
                .iter()
                .flat_map(|t| term_ranges(haystack, t))
                .collect(),
            Query::Regex { text, .. } => text
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
    // Depth-first with one reused path buffer; `full_topic` joins names with
    // '/', so only the first level has no leading slash. Each stack entry
    // carries its parent's path length, to truncate back to.
    let mut path = String::new();
    let mut stack: Vec<(NodeId, usize)> = tree
        .node(TopicTree::ROOT)
        .children
        .values()
        .rev()
        .map(|&id| (id, 0))
        .collect();
    while let Some((id, parent_len)) = stack.pop() {
        if !tree.is_alive(id) {
            continue;
        }
        path.truncate(parent_len);
        if parent_len > 0 || tree.node(id).parent != Some(TopicTree::ROOT) {
            path.push('/');
        }
        path.push_str(&tree.node(id).name);
        let node = tree.node(id);
        let payload: &[u8] = node.latest.as_ref().map_or(&[], |m| &m.payload);
        let payload = &payload[..payload.len().min(SEARCH_PAYLOAD_MAX)];
        if compiled.matches(&path, payload) {
            matches.insert(id);
            let mut cur = Some(id);
            while let Some(i) = cur {
                if !visible.insert(i) {
                    break;
                }
                cur = tree.node(i).parent;
            }
        }
        let here = path.len();
        for &child in node.children.values().rev() {
            stack.push((child, here));
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

/// Whether `compiled` matches node `id`.
fn node_matches(tree: &TopicTree, compiled: &Query, id: NodeId) -> bool {
    let node = tree.node(id);
    let payload: &[u8] = node.latest.as_ref().map_or(&[], |m| &m.payload);
    let payload = &payload[..payload.len().min(SEARCH_PAYLOAD_MAX)];
    compiled.matches(&tree.full_topic(id), payload)
}

/// Add `id` and its ancestors to `visible`.
fn show(tree: &TopicTree, visible: &mut HashSet<NodeId>, id: NodeId) {
    let mut cur = Some(id);
    while let Some(i) = cur {
        if !visible.insert(i) {
            break;
        }
        cur = tree.node(i).parent;
    }
}

/// Like `filter`, but re-tests only the matches of `prev`. `None` when that
/// is not sound: both queries must be plain, and every old term must sit
/// inside some new term (a match for the new term is then a match for the
/// old one), so the new matches are a subset of the old.
pub fn narrow(
    tree: &TopicTree,
    prev: &Filter,
    query: &str,
    mode: QueryMode,
) -> Option<Result<Option<Filter>, String>> {
    if mode != QueryMode::Plain || prev.mode != QueryMode::Plain {
        return None;
    }
    let compiled = match Query::parse(query, mode) {
        Ok(Some(q)) => q,
        _ => return None,
    };
    let (Query::Terms { terms: new, .. }, Query::Terms { terms: old, .. }) =
        (&compiled, &prev.compiled)
    else {
        return None;
    };
    if !old
        .iter()
        .all(|o| new.iter().any(|n| n.contains(o.as_str())))
    {
        return None;
    }
    let mut matches = HashSet::new();
    let mut visible = HashSet::from([TopicTree::ROOT]);
    for &id in &prev.matches {
        if tree.is_alive(id) && node_matches(tree, &compiled, id) {
            matches.insert(id);
            show(tree, &mut visible, id);
        }
    }
    Some(Ok(Some(Filter {
        query: query.trim().to_owned(),
        mode,
        compiled,
        matches,
        visible,
    })))
}

/// Bring `f` up to date after the `touched` nodes changed, re-testing only
/// those. `false` when a shown node stopped matching or was removed: `f` is
/// then unreliable and the caller must rebuild with `filter`.
pub fn update(tree: &TopicTree, f: &mut Filter, touched: &[NodeId]) -> bool {
    for &id in touched {
        if !tree.is_alive(id) {
            if f.visible.contains(&id) {
                return false;
            }
        } else if node_matches(tree, &f.compiled, id) {
            if f.matches.insert(id) {
                show(tree, &mut f.visible, id);
            }
        } else if f.matches.contains(&id) {
            return false;
        }
    }
    true
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
    use crate::{Bytes, Message};
    use std::time::{Duration, Instant, SystemTime};

    fn m(s: &str) -> Message {
        Message {
            payload: Bytes::copy_from_slice(s.as_bytes()),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
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
                payload: Bytes::from_static(&[0xff, 0x00]),
                qos: 0,
                retain: false,
                received_at: SystemTime::now(),
                truncated_from: None,
                sparkplug: false,
                wire_len: None,
                props: None,
            },
        );
        assert!(filter(&t, "zz").unwrap().matches.is_empty());
    }

    #[test]
    fn match_beyond_payload_cap_is_ignored() {
        let mut p = "x".repeat(SEARCH_PAYLOAD_MAX);
        p.push_str("needle");
        let t = tree_with(&[("a", &p)]);
        assert!(filter(&t, "needle").unwrap().matches.is_empty());
        let t = tree_with(&[("a", "needle")]);
        assert_eq!(filter(&t, "needle").unwrap().matches.len(), 1);
    }

    #[test]
    fn non_utf8_payload_matches_ascii_term() {
        let mut t = TopicTree::new(1);
        t.insert(
            "bin",
            Message {
                payload: Bytes::from_static(b"\xff\xfeTEMP=21"),
                qos: 0,
                retain: false,
                received_at: SystemTime::now(),
                truncated_from: None,
                sparkplug: false,
                wire_len: None,
                props: None,
            },
        );
        assert_eq!(filter(&t, "temp").unwrap().matches.len(), 1);
    }

    #[test]
    fn path_matches_use_full_topic_joining() {
        let t = tree_with(&[("a//b", "v"), ("c/d", "v")]);
        assert_eq!(hits(&t, "a//b", QueryMode::Plain), ["a//b"]);
        assert_eq!(hits(&t, "^c/d$", QueryMode::Regex), ["c/d"]);
        assert_eq!(hits(&t, "^a//b$", QueryMode::Regex), ["a//b"]);
    }

    #[test]
    fn unicode_case_folding_in_payload() {
        let t = tree_with(&[("a", "GRÜSSE")]);
        assert_eq!(filter(&t, "grüsse").unwrap().matches.len(), 1);
    }

    #[test]
    fn match_ranges_case_insensitive() {
        assert_eq!(
            match_ranges("aWZRbwzr", "wzr", QueryMode::Plain),
            vec![1..4, 5..8]
        );
    }

    /// A wall-clock bound only means something for the optimised build users
    /// run; a debug build on a shared CI runner lands either side of it. CI
    /// runs this with `cargo test --release`.
    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing: run with --release")]
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

        // 50k topics x 1 KB payloads
        let mut t = TopicTree::new(1);
        for i in 0..50_000 {
            t.insert(&format!("tasmota/tele/dev_{i}/SENSOR"), m(&p));
        }
        let s = Instant::now();
        filter(&t, "dev_49999");
        filter(&t, "tele dev_49999 xxx");
        super::filter(&t, r"dev_499\d9", QueryMode::Regex).unwrap();
        assert!(
            s.elapsed() < Duration::from_millis(150),
            "{:?}",
            s.elapsed()
        );
    }

    #[test]
    fn narrow_multi_term_requires_all_old_terms() {
        let mut t = TopicTree::new(1);
        t.insert("a/foo/bar", m("x"));
        t.insert("a/foo", m("x"));
        let prev = filter(&t, "foo bar").unwrap();
        assert!(narrow(&t, &prev, "foo", QueryMode::Plain).is_none());
        assert!(narrow(&t, &prev, "bar", QueryMode::Plain).is_none());
        assert!(narrow(&t, &prev, "foox barx", QueryMode::Plain).is_some());
        assert!(narrow(&t, &prev, "foo bar baz", QueryMode::Plain).is_some());
    }

    #[test]
    fn update_skips_removed_nodes() {
        let mut t = TopicTree::new(1);
        t.insert("keep/wzr", m("x"));
        let gone = t.insert("tmp/y", m("x")).unwrap();
        t.take_touched();
        let mut f = filter(&t, "wzr").unwrap();
        t.remove(gone);
        let touched = t.take_touched().unwrap();
        assert!(update(&t, &mut f, &touched));
        assert_eq!(f.matches.len(), 1);
        assert!(!f.matches.contains(&gone) && !f.visible.contains(&gone));
    }

    #[test]
    fn update_keeps_match_on_payload_change() {
        let mut t = TopicTree::new(1);
        let id = t.insert("a", m("wzr one")).unwrap();
        t.take_touched();
        let mut f = filter(&t, "wzr").unwrap();
        t.insert("a", m("wzr two"));
        let touched = t.take_touched().unwrap();
        assert!(update(&t, &mut f, &touched));
        assert!(f.matches.contains(&id));
    }

    #[test]
    fn incremental_matches_equal_full_filter() {
        let mut seed = 12345u64;
        let mut rnd = |n: u64| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        let names = ["new", "a", "b", "wzr", "x1", "tele"];
        let payloads = ["v", "wzr", "new", "hello wzr", ""];
        for query in ["new", "wzr", "new wzr", "tele x1", "hello"] {
            let mut t = TopicTree::new(1);
            t.insert("seed/new/a", m("v"));
            t.take_touched();
            let mut f = filter(&t, query).unwrap();
            for _ in 0..30 {
                for _ in 0..1 + rnd(5) {
                    let depth = 1 + rnd(4);
                    let topic: Vec<&str> = (0..depth)
                        .map(|_| names[rnd(names.len() as u64) as usize])
                        .collect();
                    t.insert(
                        &topic.join("/"),
                        m(payloads[rnd(payloads.len() as u64) as usize]),
                    );
                }
                let touched = t.take_touched().unwrap();
                if !update(&t, &mut f, &touched) {
                    f = filter(&t, query).unwrap();
                }
                let full = filter(&t, query).unwrap();
                assert_eq!(f.matches, full.matches, "matches for {query:?}");
                assert_eq!(f.visible, full.visible, "visible for {query:?}");
            }
        }
    }
}
