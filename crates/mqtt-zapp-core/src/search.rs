use crate::{NodeId, TopicTree};
use serde::Serialize;
use std::collections::HashSet;
use std::ops::Range;

#[derive(Debug, Clone, Serialize)]
pub struct Filter {
    pub query: String,
    pub matches: HashSet<NodeId>,
    /// Matches, all their ancestors, and ROOT.
    pub visible: HashSet<NodeId>,
}

/// Plain case-insensitive substring search over topic paths and latest payloads.
/// Returns `None` when the trimmed query is empty (no filter).
pub fn filter(tree: &TopicTree, query: &str) -> Option<Filter> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    let needle = query.to_lowercase();
    let mut matches = HashSet::new();
    let mut visible = HashSet::from([TopicTree::ROOT]);
    for id in 1..tree.len() {
        let node = tree.node(id);
        let hit = tree.full_topic(id).to_lowercase().contains(&needle)
            || node.latest.as_ref().is_some_and(|m| {
                String::from_utf8_lossy(&m.payload)
                    .to_lowercase()
                    .contains(&needle)
            });
        if hit {
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
    Some(Filter {
        query: query.to_owned(),
        matches,
        visible,
    })
}

/// Case-insensitive, non-overlapping byte ranges of `query` in `haystack`.
pub fn match_ranges(haystack: &str, query: &str) -> Vec<Range<usize>> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
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
        assert_eq!(match_ranges("aWZRbwzr", "wzr"), vec![1..4, 5..8]);
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
        assert!(
            s.elapsed() < Duration::from_millis(200),
            "{:?}",
            s.elapsed()
        );
    }
}
