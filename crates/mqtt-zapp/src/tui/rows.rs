//! Flattening the topic tree into the rows the tree pane shows.

use mqtt_zapp_core::search::Filter;
use mqtt_zapp_core::{NodeId, TopicTree};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub id: NodeId,
    /// 0 for the top level (children of ROOT).
    pub depth: u16,
    pub expanded: bool,
    pub has_children: bool,
}

/// Depth-first walk in name order, skipping ROOT. Children of collapsed nodes
/// are hidden. With a filter only `filter.visible` nodes are shown, all expanded.
pub fn visible_rows(
    tree: &TopicTree,
    expanded: &HashSet<NodeId>,
    filter: Option<&Filter>,
) -> Vec<Row> {
    let shown = |id: &NodeId| filter.is_none_or(|f| f.visible.contains(id));
    let mut rows = Vec::new();
    // Explicit stack: topics can be arbitrarily deep.
    let mut stack: Vec<(NodeId, u16)> = Vec::new();
    let push_children = |stack: &mut Vec<(NodeId, u16)>, id: NodeId, depth: u16| {
        stack.extend(
            tree.node(id)
                .children
                .values()
                .rev()
                .filter(|c| shown(c))
                .map(|&c| (c, depth)),
        );
    };
    push_children(&mut stack, TopicTree::ROOT, 0);
    while let Some((id, depth)) = stack.pop() {
        let has_children = tree.node(id).children.values().any(shown);
        let expanded = filter.is_some() || expanded.contains(&id);
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
    use mqtt_zapp_core::search;
    use mqtt_zapp_core::{Message, TopicTree};
    use std::collections::HashSet;
    use std::time::SystemTime;

    fn m(p: &str) -> Message {
        Message {
            payload: p.into(),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
        }
    }

    fn names(t: &TopicTree, rows: &[Row]) -> Vec<String> {
        rows.iter().map(|r| t.node(r.id).name.clone()).collect()
    }

    #[test]
    fn collapsed_hides_children() {
        let mut t = TopicTree::new(1);
        t.insert("a/b", m("1"));
        t.insert("a/c", m("2"));
        let a = t.find("a").unwrap();
        let rows = visible_rows(&t, &HashSet::new(), None);
        assert_eq!(names(&t, &rows), ["a"]);
        assert!(!rows[0].expanded);
        assert!(rows[0].has_children);
        let rows = visible_rows(&t, &HashSet::from([a]), None);
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
        let f = search::filter(&t, "wzr").unwrap();
        let rows = visible_rows(&t, &HashSet::new(), Some(&f));
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
        let rows = visible_rows(&t, &all, None);
        assert_eq!(names(&t, &rows), ["a", "x", "z", "1", "b", "y"]);
    }
}
