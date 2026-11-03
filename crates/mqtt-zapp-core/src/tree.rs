use crate::Message;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

pub type NodeId = usize;

/// Stale queue slots tolerated before `order` is compacted.
const COMPACT_SLACK: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct TopicNode {
    pub name: String,
    pub parent: Option<NodeId>,
    pub children: BTreeMap<String, NodeId>,
    pub latest: Option<Message>,
    /// Newest at the back, includes `latest`.
    pub history: VecDeque<Message>,
    /// Insertion sequence number of each `history` entry (same order).
    #[serde(skip)]
    history_seq: VecDeque<u64>,
    /// The cap reached this node's only entry, whose queue slot was dropped;
    /// the next insert puts it back at the front of the queue.
    #[serde(skip)]
    parked: bool,
    /// Messages received in this subtree.
    pub msg_count: u64,
    /// Descendants that have received at least one message.
    pub topic_count: u32,
    /// The broker holds a retained message here. Sticky: brokers forward
    /// live updates with RETAIN=0, so only an empty payload clears it.
    pub retained: bool,
    #[serde(skip)]
    pub last_update: Option<Instant>,
    /// Taken out by `TopicTree::remove`; the slot stays so ids are stable.
    #[serde(skip)]
    removed: bool,
}

impl TopicNode {
    fn new(name: String, parent: Option<NodeId>) -> Self {
        Self {
            name,
            parent,
            children: BTreeMap::new(),
            latest: None,
            history: VecDeque::new(),
            history_seq: VecDeque::new(),
            parked: false,
            msg_count: 0,
            topic_count: 0,
            retained: false,
            last_update: None,
            removed: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicTree {
    nodes: Vec<TopicNode>,
    history_depth: usize,
    /// Cap on the payload bytes held in all histories together.
    max_bytes: usize,
    /// Payload bytes currently held in all histories.
    bytes: usize,
    /// Every history entry in arrival order, as (node, seq). Entries already
    /// dropped by the per-topic depth are stale and skipped.
    #[serde(skip)]
    order: VecDeque<(NodeId, u64)>,
    next_seq: u64,
    /// History entries currently held (live slots in `order`).
    entries: usize,
}

impl TopicTree {
    pub const ROOT: NodeId = 0;

    pub fn new(history_depth: usize) -> Self {
        Self::with_limits(history_depth, usize::MAX)
    }

    /// `history_depth` messages per topic, and at most `max_bytes` of
    /// payload across all histories (each topic always keeps its latest).
    pub fn with_limits(history_depth: usize, max_bytes: usize) -> Self {
        Self {
            nodes: vec![TopicNode::new(String::new(), None)],
            history_depth,
            max_bytes,
            bytes: 0,
            order: VecDeque::new(),
            next_seq: 0,
            entries: 0,
        }
    }

    /// Payload bytes held in all histories.
    pub fn history_bytes(&self) -> usize {
        self.bytes
    }

    /// Drop the globally oldest history entries until under `max_bytes`,
    /// never a node's last one. Every iteration consumes a queue slot, so
    /// this is amortized O(1) per insert.
    fn enforce_cap(&mut self) {
        while self.bytes > self.max_bytes {
            let Some((id, seq)) = self.order.pop_front() else {
                break;
            };
            let node = &mut self.nodes[id];
            if node.history_seq.front() != Some(&seq) {
                continue; // already gone (depth eviction)
            }
            if node.history.len() == 1 {
                // The latest message stays. Park it: the next insert here
                // re-queues it at the front, where it was (all slots after
                // it are newer), instead of rescanning it on every insert.
                node.parked = true;
                continue;
            }
            node.history_seq.pop_front();
            if let Some(m) = node.history.pop_front() {
                self.bytes -= m.payload.len();
                self.entries -= 1;
            }
        }
    }

    /// Drop the stale slots that depth eviction leaves in `order`, once they
    /// outnumber the live ones; amortized O(1) per insert.
    fn compact_order(&mut self) {
        if self.order.len() <= 2 * self.entries + COMPACT_SLACK {
            return;
        }
        let nodes = &self.nodes;
        // A node's seqs ascend, so a slot is live iff it is not older than
        // the node's oldest remaining entry.
        self.order
            .retain(|(id, seq)| nodes[*id].history_seq.front().is_some_and(|f| seq >= f));
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
        if std::mem::take(&mut node.parked)
            && let Some(&old) = node.history_seq.front()
        {
            self.order.push_front((cur, old));
        }
        self.bytes += msg.payload.len();
        node.history.push_back(msg);
        node.history_seq.push_back(self.next_seq);
        self.order.push_back((cur, self.next_seq));
        self.next_seq += 1;
        self.entries += 1;
        while node.history.len() > self.history_depth {
            node.history_seq.pop_front();
            if let Some(m) = node.history.pop_front() {
                self.bytes -= m.payload.len();
                self.entries -= 1;
            }
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
        self.enforce_cap();
        self.compact_order();
        cur
    }

    /// Forget `id` and everything below it, then the ancestors this leaves
    /// empty (no message, no children). Returns how many topics with a
    /// message were removed. The root stays. Removed ids are never reused;
    /// their queue slots go stale and are skipped like evicted ones.
    pub fn remove(&mut self, id: NodeId) -> u32 {
        let Some(parent) = self
            .nodes
            .get(id)
            .filter(|n| !n.removed)
            .and_then(|n| n.parent)
        else {
            return 0;
        };
        let removed_msgs = self.nodes[id].msg_count;
        let removed_topics =
            self.nodes[id].topic_count + u32::from(self.nodes[id].latest.is_some());
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = &mut self.nodes[n];
            stack.extend(std::mem::take(&mut node.children).into_values());
            self.bytes -= node.history.iter().map(|m| m.payload.len()).sum::<usize>();
            self.entries -= node.history.len();
            node.history.clear();
            node.history_seq.clear();
            node.latest = None;
            node.retained = false;
            node.parked = false;
            node.msg_count = 0;
            node.topic_count = 0;
            node.removed = true;
        }
        let mut cur = Some(parent);
        while let Some(a) = cur {
            let n = &mut self.nodes[a];
            n.msg_count -= removed_msgs;
            n.topic_count -= removed_topics;
            cur = n.parent;
        }
        // Detach `id`, then prune ancestors left without content.
        let (mut child, mut up) = (id, parent);
        loop {
            let name = self.nodes[child].name.clone();
            self.nodes[up].children.remove(&name);
            let p = &self.nodes[up];
            match p.parent {
                Some(next) if p.latest.is_none() && p.children.is_empty() => {
                    self.nodes[up].removed = true;
                    (child, up) = (up, next);
                }
                _ => break,
            }
        }
        removed_topics
    }

    /// Still in the tree: not taken out by `remove`.
    pub fn is_alive(&self, id: NodeId) -> bool {
        self.nodes.get(id).is_some_and(|n| !n.removed)
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
        (0..self.nodes.len()).filter(|&i| self.nodes[i].latest.is_some() && !self.nodes[i].removed)
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

    fn msg(p: &str) -> Message {
        Message {
            payload: p.into(),
            qos: 0,
            retain: false,
            received_at: std::time::SystemTime::now(),
        }
    }

    #[test]
    fn remove_subtree_fixes_counts_bytes_and_prunes_empty_parents() {
        let mut t = TopicTree::new(5);
        t.insert("a/b/c", msg("123"));
        t.insert("a/b/c/d", msg("45"));
        t.insert("a/x", msg("6"));
        let bytes = t.history_bytes();
        let b = t.find("a/b").unwrap();
        assert_eq!(t.remove(b), 2, "c and d had messages");
        assert_eq!(t.find("a/b"), None);
        assert_eq!(t.find("a/b/c/d"), None);
        assert_eq!(t.history_bytes(), bytes - 5);
        let a = t.find("a").unwrap();
        assert_eq!(t.node(a).topic_count, 1);
        assert_eq!(t.node(a).msg_count, 1);
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 1);
        assert_eq!(t.node(TopicTree::ROOT).msg_count, 1);
        assert_eq!(t.iter_with_message().count(), 1);
        // Removing the last topic prunes the now-empty `a` too.
        let x = t.find("a/x").unwrap();
        assert_eq!(t.remove(x), 1);
        assert_eq!(t.find("a"), None);
        assert!(t.node(TopicTree::ROOT).children.is_empty());
        assert_eq!(t.history_bytes(), 0);
        assert_eq!(t.remove(TopicTree::ROOT), 0, "the root stays");
    }

    #[test]
    fn removed_nodes_are_gone_for_lookups_and_a_stale_remove_is_a_noop() {
        let mut t = TopicTree::new(5);
        t.insert("a/b", msg("1"));
        t.insert("a/x", msg("2"));
        let b = t.find("a/b").unwrap();
        t.remove(b);
        assert!(!t.is_alive(b));
        assert!(t.is_alive(t.find("a/x").unwrap()));
        assert!(t.is_alive(TopicTree::ROOT));
        // The search filter must not see removed nodes.
        let f = crate::search::filter(&t, "b", crate::search::QueryMode::Plain)
            .unwrap()
            .unwrap();
        assert!(f.matches.is_empty(), "{:?}", f.matches);
        // A second remove of the old id must not cut off the new `a/b`.
        let b2 = t.insert("a/b", msg("3"));
        assert_eq!(t.remove(b), 0);
        assert_eq!(t.find("a/b"), Some(b2));
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 2);
    }

    #[test]
    fn removed_topic_can_come_back_and_cap_still_works() {
        let mut t = TopicTree::with_limits(10, 10);
        for p in ["aaaa", "bbbb"] {
            t.insert("gone", msg(p));
        }
        let g = t.find("gone").unwrap();
        t.remove(g);
        // Stale queue slots of the removed node must not break the cap.
        for p in ["1111", "2222", "3333", "4444"] {
            t.insert("kept", msg(p));
        }
        assert!(t.history_bytes() <= 10, "{}", t.history_bytes());
        let id = t.insert("gone", msg("new"));
        assert_eq!(t.find("gone"), Some(id));
        assert_eq!(t.node(id).history.len(), 1);
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 2);
    }

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

    fn payloads(t: &TopicTree, topic: &str) -> Vec<String> {
        t.node(t.find(topic).unwrap())
            .history
            .iter()
            .map(|m| String::from_utf8_lossy(&m.payload).into_owned())
            .collect()
    }

    #[test]
    fn order_queue_stays_bounded_below_the_cap() {
        let mut t = TopicTree::with_limits(1, usize::MAX);
        for i in 0..10_000 {
            t.insert("x", m(&i.to_string()));
        }
        assert_eq!(t.node(t.find("x").unwrap()).history.len(), 1);
        assert!(t.order.len() <= 2 + COMPACT_SLACK, "{}", t.order.len());
    }

    #[test]
    fn latest_only_entries_leave_the_queue() {
        // Over the cap with nothing evictable: no slot may be scanned again
        // on later inserts (that made every insert O(topics)).
        let mut t = TopicTree::with_limits(100, 0);
        for i in 0..2000 {
            t.insert(&format!("t{i}"), m("p"));
        }
        assert_eq!(t.order.len(), 0);
        t.insert("t7", m("q"));
        assert_eq!(
            payloads(&t, "t7"),
            ["q"],
            "the parked entry is evicted first"
        );
        assert_eq!(t.history_bytes(), 2000);
    }

    #[test]
    fn memory_cap_evicts_oldest_globally() {
        let mut t = TopicTree::with_limits(100, 10);
        t.insert("a", m("aaaa"));
        t.insert("b", m("bbbb"));
        t.insert("a", m("cccc"));
        assert_eq!(payloads(&t, "a"), ["cccc"]);
        assert_eq!(payloads(&t, "b"), ["bbbb"]);
        assert_eq!(t.history_bytes(), 8);
    }

    #[test]
    fn memory_cap_keeps_latest_of_each_node() {
        let mut t = TopicTree::with_limits(100, 5);
        t.insert("a", m("aaaa"));
        t.insert("b", m("bbbb"));
        assert_eq!(t.history_bytes(), 8, "each node keeps its latest");
        t.insert("a", m("cc"));
        assert_eq!(payloads(&t, "a"), ["cc"]);
        assert_eq!(payloads(&t, "b"), ["bbbb"]);
        assert_eq!(t.history_bytes(), 6);
        // Later entries still evict once older ones are gone.
        t.insert("b", m("d"));
        assert_eq!(payloads(&t, "b"), ["d"]);
        assert_eq!(t.history_bytes(), 3);
    }

    #[test]
    fn history_bytes_tracks_depth_eviction() {
        let mut t = TopicTree::with_limits(2, usize::MAX);
        for p in ["1", "22", "333"] {
            t.insert("x", m(p));
        }
        assert_eq!(payloads(&t, "x"), ["22", "333"]);
        assert_eq!(t.history_bytes(), 5);
        // A depth-evicted entry's queue slot is stale and skipped later.
        let mut t = TopicTree::with_limits(1, 4);
        t.insert("x", m("111"));
        t.insert("x", m("22"));
        t.insert("y", m("33"));
        assert_eq!(payloads(&t, "x"), ["22"]);
        assert_eq!(t.history_bytes(), 4);
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
