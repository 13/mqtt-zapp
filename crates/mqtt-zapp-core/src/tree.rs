use crate::Message;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

pub type NodeId = usize;

#[derive(Debug, Clone, Serialize)]
pub struct TopicNode {
    pub name: String,
    pub parent: Option<NodeId>,
    pub children: BTreeMap<String, NodeId>,
    pub latest: Option<Message>,
    /// Newest at the back, includes `latest`.
    pub history: VecDeque<Message>,
    /// Messages received in this subtree.
    pub msg_count: u64,
    /// Descendants that have received at least one message.
    pub topic_count: u32,
    /// The broker holds a retained message here. Sticky: brokers forward
    /// live updates with RETAIN=0, so only an empty payload clears it.
    pub retained: bool,
    #[serde(skip)]
    pub last_update: Option<Instant>,
}

impl TopicNode {
    fn new(name: String, parent: Option<NodeId>) -> Self {
        Self {
            name,
            parent,
            children: BTreeMap::new(),
            latest: None,
            history: VecDeque::new(),
            msg_count: 0,
            topic_count: 0,
            retained: false,
            last_update: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicTree {
    nodes: Vec<TopicNode>,
    history_depth: usize,
}

impl TopicTree {
    pub const ROOT: NodeId = 0;

    pub fn new(history_depth: usize) -> Self {
        Self {
            nodes: vec![TopicNode::new(String::new(), None)],
            history_depth,
        }
    }

    /// Store `msg` under `topic`, creating levels as needed. Empty levels are kept as nodes.
    pub fn insert(&mut self, topic: &str, msg: Message) -> NodeId {
        let mut cur = Self::ROOT;
        for level in topic.split('/') {
            cur = match self.nodes[cur].children.get(level) {
                Some(&id) => id,
                None => {
                    let id = self.nodes.len();
                    self.nodes.push(TopicNode::new(level.to_owned(), Some(cur)));
                    self.nodes[cur].children.insert(level.to_owned(), id);
                    id
                }
            };
        }
        let first = self.nodes[cur].latest.is_none();
        let now = Instant::now();
        let node = &mut self.nodes[cur];
        if msg.payload.is_empty() {
            node.retained = false;
        } else if msg.retain {
            node.retained = true;
        }
        node.latest = Some(msg.clone());
        node.history.push_back(msg);
        while node.history.len() > self.history_depth {
            node.history.pop_front();
        }
        node.last_update = Some(now);
        let mut id = Some(cur);
        while let Some(i) = id {
            let n = &mut self.nodes[i];
            n.msg_count += 1;
            if first && i != cur {
                n.topic_count += 1;
            }
            n.last_update = Some(now);
            id = n.parent;
        }
        cur
    }

    pub fn node(&self, id: NodeId) -> &TopicNode {
        &self.nodes[id]
    }

    pub fn full_topic(&self, id: NodeId) -> String {
        let mut parts = Vec::new();
        let mut cur = id;
        while let Some(p) = self.nodes[cur].parent {
            parts.push(self.nodes[cur].name.as_str());
            cur = p;
        }
        parts.reverse();
        parts.join("/")
    }

    pub fn find(&self, topic: &str) -> Option<NodeId> {
        topic.split('/').try_fold(Self::ROOT, |cur, level| {
            self.nodes[cur].children.get(level).copied()
        })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn iter_with_message(&self) -> impl Iterator<Item = NodeId> + '_ {
        (0..self.nodes.len()).filter(|&i| self.nodes[i].latest.is_some())
    }
}

/// Split into runs of ASCII digits and runs of everything else.
fn runs(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut digit = None;
    for (i, c) in s.char_indices() {
        let d = c.is_ascii_digit();
        if digit.is_some_and(|prev| prev != d) {
            out.push(&s[start..i]);
            start = i;
        }
        digit = Some(d);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Natural order: digit runs compare by value (`tasmota_2` < `tasmota_10`),
/// text runs case-insensitively. Ties are broken by shorter digit runs
/// (`x1` < `x01`), then by plain byte order, so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (ra, rb) = (runs(a), runs(b));
    let is_num = |r: &str| r.as_bytes()[0].is_ascii_digit();
    for (x, y) in ra.iter().zip(&rb) {
        let ord = match (is_num(x), is_num(y)) {
            (true, true) => {
                let (tx, ty) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                tx.len().cmp(&ty.len()).then_with(|| tx.cmp(ty))
            }
            (false, false) => x
                .chars()
                .flat_map(char::to_lowercase)
                .cmp(y.chars().flat_map(char::to_lowercase)),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    ra.len()
        .cmp(&rb.len())
        .then_with(|| {
            ra.iter()
                .zip(&rb)
                .map(|(x, y)| x.len().cmp(&y.len()))
                .find(|o| *o != Ordering::Equal)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_flag_is_sticky() {
        let m = |p: &str, retain| Message {
            payload: p.into(),
            qos: 0,
            retain,
            received_at: std::time::SystemTime::now(),
        };
        let mut t = TopicTree::new(5);
        let id = t.insert("a", m("1", true));
        assert!(t.node(id).retained);
        // Live updates arrive with RETAIN=0 even when published retained.
        t.insert("a", m("2", false));
        assert!(t.node(id).retained);
        // An empty payload is the broker's "cleared".
        t.insert("a", m("", false));
        assert!(!t.node(id).retained);
        t.insert("a", m("3", false));
        assert!(!t.node(id).retained);
        t.insert("a", m("", true));
        assert!(!t.node(id).retained);
    }

    #[test]
    fn natural_cmp_orders() {
        use std::cmp::Ordering::*;
        assert_eq!(natural_cmp("tasmota_2", "tasmota_10"), Less);
        assert_eq!(natural_cmp("a", "B"), Less);
        assert_eq!(natural_cmp("", "a"), Less);
        assert_ne!(natural_cmp("x01", "x1"), Equal);
        assert_eq!(natural_cmp("x01", "x1"), natural_cmp("x1", "x01").reverse());
        assert_eq!(natural_cmp("x1", "x1"), Equal);
        let big = "9".repeat(30);
        assert_eq!(natural_cmp(&big, "10"), Greater);
        let mut v = vec!["s10", "s9", "S1", "s1", "s", "s01"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["s", "S1", "s1", "s01", "s9", "s10"]);
    }
    use crate::Message;
    use std::time::SystemTime;

    fn m(p: &str) -> Message {
        Message {
            payload: p.into(),
            qos: 0,
            retain: false,
            received_at: SystemTime::now(),
        }
    }
    #[test]
    fn insert_builds_levels() {
        let mut t = TopicTree::new(10);
        let id = t.insert("tasmota/tele/tasmota_0C6423/SENSOR", m("{}"));
        assert_eq!(t.full_topic(id), "tasmota/tele/tasmota_0C6423/SENSOR");
        assert_eq!(t.len(), 5);
    }
    #[test]
    fn counts_propagate() {
        let mut t = TopicTree::new(10);
        t.insert("a/b", m("1"));
        t.insert("a/b", m("2"));
        t.insert("a/c", m("3"));
        let a = t.find("a").unwrap();
        assert_eq!(t.node(a).msg_count, 3);
        assert_eq!(t.node(a).topic_count, 2);
        assert_eq!(t.node(TopicTree::ROOT).msg_count, 3);
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 2);
    }
    #[test]
    fn history_is_bounded_newest_last() {
        let mut t = TopicTree::new(3);
        for i in 0..5 {
            t.insert("x", m(&i.to_string()));
        }
        let n = t.node(t.find("x").unwrap());
        assert_eq!(n.history.len(), 3);
        assert_eq!(n.history.back().unwrap().payload, b"4");
        assert_eq!(n.latest.as_ref().unwrap().payload, b"4");
    }
    #[test]
    fn odd_topics_round_trip() {
        let mut t = TopicTree::new(1);
        for s in ["a//b", "/lead", "trail/", "$SYS/broker/version"] {
            let id = t.insert(s, m("v"));
            assert_eq!(t.full_topic(id), s);
            assert_eq!(t.find(s), Some(id));
        }
    }
    #[test]
    fn intermediate_node_has_no_message() {
        let mut t = TopicTree::new(1);
        t.insert("a/b", m("1"));
        assert!(t.node(t.find("a").unwrap()).latest.is_none());
        assert_eq!(t.find("a/zzz"), None);
    }
    #[test]
    fn iter_with_message_lists_only_leaves_with_data() {
        let mut t = TopicTree::new(1);
        let b = t.insert("a/b", m("1"));
        let a = t.insert("a", m("2"));
        assert_eq!(t.iter_with_message().collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 2);
    }
}
