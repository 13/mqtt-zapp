//! Flattening the topic tree into the rows the tree pane shows.

use mqtt_zapp_core::config::SortOrder;
use mqtt_zapp_core::search::Filter;
use mqtt_zapp_core::tree::{NaturalKey, natural_key};
use mqtt_zapp_core::{NodeId, TopicTree};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: NodeId,
    /// 0 for the top level (children of ROOT).
    pub depth: u16,
    pub expanded: bool,
    pub has_children: bool,
}

/// Depth-first walk in name order, skipping ROOT. Children of collapsed nodes
/// are hidden. With a filter only `filter.visible` nodes are shown, all
/// expanded; `only` (the starred view) narrows that further.
pub fn visible_rows(
    tree: &TopicTree,
    expanded: &HashSet<NodeId>,
    filter: Option<&Filter>,
    only: Option<&HashSet<NodeId>>,
    sort: SortOrder,
) -> Vec<Row> {
    let shown = |id: &NodeId| {
        filter.is_none_or(|f| f.visible.contains(id)) && only.is_none_or(|o| o.contains(id))
    };
    let order = |a: &NodeId, b: &NodeId, keys: &HashMap<NodeId, NaturalKey>| {
        let (x, y) = (tree.node(*a), tree.node(*b));
        let by_name = || keys[a].cmp(&keys[b]).then_with(|| x.name.cmp(&y.name));
        match sort {
            SortOrder::Name => by_name(),
            SortOrder::Count => y.msg_count.cmp(&x.msg_count).then_with(by_name),
            // `None` (never updated) sorts last.
            SortOrder::Recent => y.last_update.cmp(&x.last_update).then_with(by_name),
        }
    };
    let mut rows = Vec::new();
    // Explicit stack: topics can be arbitrarily deep.
    let mut stack: Vec<(NodeId, u16)> = Vec::new();
    let push_children = |stack: &mut Vec<(NodeId, u16)>, id: NodeId, depth: u16| {
        let mut kids: Vec<NodeId> = tree.node(id).child_ids().filter(shown).collect();
        // Natural keys are built here, once per rebuild of the (cached) rows.
        let keys: HashMap<NodeId, NaturalKey> = kids
            .iter()
            .map(|&k| (k, natural_key(&tree.node(k).name)))
            .collect();
        kids.sort_by(|a, b| order(a, b, &keys));
        // Reversed: the stack pops the first child first.
        stack.extend(kids.into_iter().rev().map(|c| (c, depth)));
    };
    push_children(&mut stack, TopicTree::ROOT, 0);
    while let Some((id, depth)) = stack.pop() {
        let has_children = tree.node(id).child_ids().any(|c| shown(&c));
        let expanded = filter.is_some() || only.is_some() || expanded.contains(&id);
        rows.push(Row {
            id,
            depth,
            expanded,
            has_children,
        });
        if expanded && has_children {
            push_children(&mut stack, id, depth.saturating_add(1));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqtt_zapp_core::config::SortOrder;
    use mqtt_zapp_core::search;
    use mqtt_zapp_core::{Bytes, Message, TopicTree};
    use std::collections::HashSet;
    use std::time::SystemTime;

    fn m(p: &str) -> Message {
        Message {
            payload: Bytes::copy_from_slice(p.as_bytes()),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        }
    }

    #[test]
    fn sort_by_name_count_and_recent() {
        let mut t = TopicTree::new(1);
        for topic in ["n10", "n9", "n9", "n9", "n2"] {
            t.insert(topic, m("x"));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let rows = |t: &TopicTree, o| names(t, &visible_rows(t, &HashSet::new(), None, None, o));
        assert_eq!(rows(&t, SortOrder::Name), vec!["n2", "n9", "n10"]);
        assert_eq!(rows(&t, SortOrder::Count), vec!["n9", "n2", "n10"]);
        assert_eq!(rows(&t, SortOrder::Recent), vec!["n2", "n9", "n10"]);
        t.insert("n10", m("y"));
        assert_eq!(rows(&t, SortOrder::Recent), vec!["n10", "n2", "n9"]);
    }

    fn names(t: &TopicTree, rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| t.node(r.id).name.to_string()).collect()
    }

    #[test]
    fn collapsed_hides_children() {
        let mut t = TopicTree::new(1);
        t.insert("a/b", m("1"));
        t.insert("a/c", m("2"));
        let a = t.find("a").unwrap();
        let rows = visible_rows(&t, &HashSet::new(), None, None, SortOrder::Name);
        assert_eq!(names(&t, &rows), ["a"]);
        assert!(!rows[0].expanded);
        assert!(rows[0].has_children);
        let rows = visible_rows(&t, &HashSet::from([a]), None, None, SortOrder::Name);
        assert_eq!(names(&t, &rows), ["a", "b", "c"]);
        assert_eq!(rows.iter().map(|r| r.depth).collect::<Vec<_>>(), [0, 1, 1]);
        assert!(rows[0].expanded);
        assert!(!rows[1].has_children);
    }

    #[test]
    fn filter_forces_expand_and_hides_others() {
        let mut t = TopicTree::new(1);
        t.insert("a/wzr", m("1"));
        t.insert("b/c", m("2"));
        let f = search::filter(&t, "wzr", search::QueryMode::Plain)
            .unwrap()
            .unwrap();
        let rows = visible_rows(&t, &HashSet::new(), Some(&f), None, SortOrder::Name);
        assert_eq!(names(&t, &rows), ["a", "wzr"]);
        assert!(rows[0].expanded);
    }

    #[test]
    fn depth_first_in_name_order() {
        let mut t = TopicTree::new(1);
        for s in ["b/y", "a/z/1", "a/x"] {
            t.insert(s, m("v"));
        }
        let all: HashSet<_> = (1..t.len()).collect();
        let rows = visible_rows(&t, &all, None, None, SortOrder::Name);
        assert_eq!(names(&t, &rows), ["a", "x", "z", "1", "b", "y"]);
    }
}
