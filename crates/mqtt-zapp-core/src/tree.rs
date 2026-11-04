#[cfg(test)]
use crate::payload::ENTRY_OVERHEAD;
use crate::payload::STUB_KEEP;
use crate::{Bytes, Message};
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
    /// `next_seq` at this node's latest insert; its `recent` slot is live iff
    /// it carries this stamp.
    #[serde(skip)]
    stamp: u64,
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
    /// Precomputed natural order of `name`.
    #[serde(skip)]
    pub sort_key: NaturalKey,
}

impl TopicNode {
    fn new(name: String, parent: Option<NodeId>) -> Self {
        Self {
            sort_key: natural_key(&name),
            name,
            parent,
            children: BTreeMap::new(),
            latest: None,
            history: VecDeque::new(),
            history_seq: VecDeque::new(),
            parked: false,
            stamp: 0,
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
    /// One slot per insert, as (node, stamp), oldest update first. Stubbing
    /// walks it to find the least recently updated topics.
    #[serde(skip)]
    recent: VecDeque<(NodeId, u64)>,
    next_seq: u64,
    /// History entries currently held (live slots in `order`).
    entries: usize,
    /// Cap on topic levels, see `with_limits`.
    max_topics: usize,
    /// Topic levels in the tree now (nodes below the root, not removed).
    live: usize,
    /// Messages refused because their topic would exceed `max_topics`.
    dropped_topics: u64,
    /// Bumped whenever a node is created or removed; see `shape_gen`.
    #[serde(skip)]
    shape_gen: u64,
}

impl TopicTree {
    pub const ROOT: NodeId = 0;

    pub fn new(history_depth: usize) -> Self {
        Self::with_limits(history_depth, usize::MAX, usize::MAX)
    }

    /// `history_depth` messages per topic, and at most `max_bytes` of
    /// payload across all histories (each topic always keeps its latest).
    ///
    /// `max_topics` caps the nodes below the root: every topic level counts
    /// (`a/b` is two), whether or not it holds a message. A removed one
    /// frees its slot for the cap (ids are never reused, the tombstone stays).
    /// A message whose topic needs more levels than are left is refused.
    pub fn with_limits(history_depth: usize, max_bytes: usize, max_topics: usize) -> Self {
        Self {
            nodes: vec![TopicNode::new(String::new(), None)],
            // At least 1, or the latest message would sit outside the byte cap.
            history_depth: history_depth.max(1),
            max_bytes,
            bytes: 0,
            order: VecDeque::new(),
            recent: VecDeque::new(),
            next_seq: 0,
            entries: 0,
            max_topics,
            live: 0,
            dropped_topics: 0,
            shape_gen: 0,
        }
    }

    /// Changes whenever a node is created or `remove` runs, so a view of the
    /// tree's shape (its flattened rows) can be cached until it moves.
    /// Message updates to existing nodes leave it alone.
    pub fn shape_gen(&self) -> u64 {
        self.shape_gen
    }

    /// Messages refused because the tree was at `max_topics`.
    pub fn dropped_topics(&self) -> u64 {
        self.dropped_topics
    }

    /// Accounted bytes of all stored messages (payload, props and
    /// `ENTRY_OVERHEAD` each).
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
                self.bytes -= m.size();
                self.entries -= 1;
            }
        }
        self.stub_least_recent();
    }

    /// Still over the cap with only latest entries left: cut the latest
    /// payload of the least recently updated topics to `STUB_KEEP` bytes
    /// until under it. Each iteration consumes a `recent` slot, so this is
    /// amortized O(1) per insert.
    fn stub_least_recent(&mut self) {
        while self.bytes > self.max_bytes {
            let Some((id, stamp)) = self.recent.pop_front() else {
                break;
            };
            let node = &mut self.nodes[id];
            if node.removed || node.stamp != stamp {
                continue; // removed, or updated since
            }
            let (Some(latest), Some(last)) = (node.latest.as_mut(), node.history.back_mut()) else {
                continue;
            };
            let keep = utf8_prefix_len(&latest.payload, STUB_KEEP);
            if keep == latest.payload.len() {
                continue;
            }
            // Replace both copies, so the big buffer is freed.
            let mut stub = latest.clone();
            stub.truncated_from = stub.truncated_from.or(Some(stub.payload.len()));
            stub.payload = Bytes::copy_from_slice(&stub.payload[..keep]);
            self.bytes = self.bytes - last.size() + stub.size();
            *last = stub.clone();
            *latest = stub;
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

    /// Same for `recent`, whose live slots are at most one per node.
    fn compact_recent(&mut self) {
        if self.recent.len() <= 2 * self.nodes.len() + COMPACT_SLACK {
            return;
        }
        let nodes = &self.nodes;
        self.recent
            .retain(|(id, stamp)| !nodes[*id].removed && nodes[*id].stamp == *stamp);
    }

    /// Store `msg` under `topic`, creating levels as needed. Empty levels are kept as nodes.
    /// `None` (and nothing changed) when the new levels would pass `max_topics`.
    pub fn insert(&mut self, topic: &str, msg: Message) -> Option<NodeId> {
        // Walk the levels that exist; check the cap before creating any.
        let mut cur = Self::ROOT;
        let mut found = 0;
        for level in topic.split('/') {
            match self.nodes[cur].children.get(level) {
                Some(&id) => (cur, found) = (id, found + 1),
                None => break,
            }
        }
        let missing = topic.split('/').count() - found;
        if missing > 0 {
            if self.live.saturating_add(missing) > self.max_topics {
                self.dropped_topics += 1;
                return None;
            }
            self.shape_gen += 1;
            self.live += missing;
            for level in topic.split('/').skip(found) {
                let id = self.nodes.len();
                self.nodes.push(TopicNode::new(level.to_owned(), Some(cur)));
                self.nodes[cur].children.insert(level.to_owned(), id);
                cur = id;
            }
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
        self.bytes += msg.size();
        node.history.push_back(msg);
        node.history_seq.push_back(self.next_seq);
        self.order.push_back((cur, self.next_seq));
        self.next_seq += 1;
        self.entries += 1;
        while node.history.len() > self.history_depth {
            node.history_seq.pop_front();
            if let Some(m) = node.history.pop_front() {
                self.bytes -= m.size();
                self.entries -= 1;
            }
        }
        node.last_update = Some(now);
        node.stamp = self.next_seq - 1;
        self.recent.push_back((cur, node.stamp));
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
        self.compact_recent();
        Some(cur)
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
        self.shape_gen += 1;
        let removed_msgs = self.nodes[id].msg_count;
        let removed_topics =
            self.nodes[id].topic_count + u32::from(self.nodes[id].latest.is_some());
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = &mut self.nodes[n];
            stack.extend(std::mem::take(&mut node.children).into_values());
            self.bytes -= node.history.iter().map(Message::size).sum::<usize>();
            self.entries -= node.history.len();
            node.history.clear();
            node.history_seq.clear();
            node.latest = None;
            node.retained = false;
            node.parked = false;
            node.msg_count = 0;
            node.topic_count = 0;
            node.removed = true;
            if n != id {
                node.name = String::new();
                node.sort_key = natural_key("");
            }
            self.live -= 1;
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
            self.nodes[child].name = String::new();
            self.nodes[child].sort_key = natural_key("");
            let p = &self.nodes[up];
            match p.parent {
                Some(next) if p.latest.is_none() && p.children.is_empty() => {
                    self.nodes[up].removed = true;
                    self.live -= 1;
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

/// One run of a [`NaturalKey`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeyPart {
    /// Digits without leading zeros, and the run's original width.
    Num { value: Box<str>, width: usize },
    /// Lowercased text.
    Text(Box<str>),
}

/// How many leading bytes of `b` to keep, at most `max`: when `b` is UTF-8
/// the cut does not split a character.
fn utf8_prefix_len(b: &[u8], max: usize) -> usize {
    if b.len() <= max {
        return b.len();
    }
    match std::str::from_utf8(&b[..max]) {
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        _ => max,
    }
}

/// Precomputed sort key: ordering two keys equals `natural_cmp` before its
/// final byte-order tie-break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NaturalKey(Box<[KeyPart]>);

/// Build the key for `s`; see [`natural_cmp`] for the order.
pub fn natural_key(s: &str) -> NaturalKey {
    NaturalKey(
        runs(s)
            .into_iter()
            .map(|r| {
                if r.as_bytes()[0].is_ascii_digit() {
                    KeyPart::Num {
                        value: r.trim_start_matches('0').into(),
                        width: r.len(),
                    }
                } else {
                    KeyPart::Text(r.chars().flat_map(char::to_lowercase).collect())
                }
            })
            .collect(),
    )
}

impl Ord for NaturalKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let part = |x: &KeyPart, y: &KeyPart| match (x, y) {
            (KeyPart::Num { value: a, .. }, KeyPart::Num { value: b, .. }) => {
                a.len().cmp(&b.len()).then_with(|| a.cmp(b))
            }
            (KeyPart::Text(a), KeyPart::Text(b)) => a.cmp(b),
            (KeyPart::Num { .. }, KeyPart::Text(_)) => Ordering::Less,
            (KeyPart::Text(_), KeyPart::Num { .. }) => Ordering::Greater,
        };
        let (a, b) = (&self.0, &other.0);
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| part(x, y))
            .find(|o| *o != Ordering::Equal)
            .unwrap_or_else(|| a.len().cmp(&b.len()))
            .then_with(|| {
                // Shorter digit runs first (`x1` < `x01`).
                a.iter()
                    .zip(b.iter())
                    .find_map(|(x, y)| match (x, y) {
                        (KeyPart::Num { width: p, .. }, KeyPart::Num { width: q, .. })
                            if p != q =>
                        {
                            Some(p.cmp(q))
                        }
                        _ => None,
                    })
                    .unwrap_or(Ordering::Equal)
            })
    }
}

impl PartialOrd for NaturalKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Natural order: digit runs compare by value (`tasmota_2` < `tasmota_10`),
/// text runs case-insensitively. Ties are broken by shorter digit runs
/// (`x1` < `x01`), then by plain byte order, so the order is total.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    natural_key(a).cmp(&natural_key(b)).then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Natural order: digit runs compare by value (`tasmota_2` < `tasmota_10`),
    /// text runs case-insensitively. Ties are broken by shorter digit runs
    /// (`x1` < `x01`), then by plain byte order, so the order is total.
    fn old_natural_cmp(a: &str, b: &str) -> Ordering {
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

    #[test]
    fn natural_key_agrees_with_natural_cmp() {
        let names = [
            "n10",
            "n9",
            "n2",
            "x01",
            "x1",
            "Tasmota_2",
            "tasmota_10",
            "a",
            "A",
            "",
            "0",
            "00",
            "b-1",
            "b_1",
            "é9",
            "é10",
        ];
        for a in names {
            for b in names {
                assert_eq!(
                    natural_key(a).cmp(&natural_key(b)).then(a.cmp(b)),
                    old_natural_cmp(a, b),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    fn msg(p: &str) -> Message {
        Message {
            payload: Bytes::copy_from_slice(p.as_bytes()),
            qos: 0,
            retain: false,
            received_at: std::time::SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        }
    }

    #[test]
    fn props_bytes_count_toward_history_bytes() {
        let mut t = TopicTree::new(5);
        let mut m = msg("abc");
        m.props = Some(Arc::new(crate::payload::Props {
            content_type: Some("text/plain".into()),
            user: vec![("k".into(), "vv".into())],
            ..Default::default()
        }));
        t.insert("a", m);
        assert_eq!(t.history_bytes(), 3 + 10 + 3 + ENTRY_OVERHEAD);
        let a = t.find("a").unwrap();
        t.remove(a);
        assert_eq!(t.history_bytes(), 0);
    }

    #[test]
    fn latest_and_history_share_props() {
        let mut t = TopicTree::new(5);
        let mut m = msg("abc");
        m.props = Some(Arc::new(crate::payload::Props {
            content_type: Some("text/plain".into()),
            ..Default::default()
        }));
        t.insert("a", m);
        let node = t.node(t.find("a").unwrap());
        let latest = node.latest.as_ref().unwrap();
        let history = node.history.back().unwrap();
        assert!(Arc::ptr_eq(
            latest.props.as_ref().unwrap(),
            history.props.as_ref().unwrap()
        ));
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
        assert_eq!(t.history_bytes(), bytes - (5 + 2 * ENTRY_OVERHEAD));
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
        let b2 = t.insert("a/b", msg("3")).unwrap();
        assert_eq!(t.remove(b), 0);
        assert_eq!(t.find("a/b"), Some(b2));
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 2);
    }

    #[test]
    fn removed_topic_can_come_back_and_cap_still_works() {
        let cap = 10 + 2 * ENTRY_OVERHEAD;
        let mut t = TopicTree::with_limits(10, cap, usize::MAX);
        for p in ["aaaa", "bbbb"] {
            t.insert("gone", msg(p));
        }
        let g = t.find("gone").unwrap();
        t.remove(g);
        // Stale queue slots of the removed node must not break the cap.
        for p in ["1111", "2222", "3333", "4444"] {
            t.insert("kept", msg(p));
        }
        assert!(t.history_bytes() <= cap, "{}", t.history_bytes());
        let id = t.insert("gone", msg("new")).unwrap();
        assert_eq!(t.find("gone"), Some(id));
        assert_eq!(t.node(id).history.len(), 1);
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 2);
    }

    #[test]
    fn shape_gen_bumps_on_create_and_remove_only() {
        let mut t = TopicTree::with_limits(1, usize::MAX, 3);
        let g0 = t.shape_gen();
        t.insert("a/b", msg("1"));
        let g1 = t.shape_gen();
        assert_ne!(g0, g1, "new nodes");
        t.insert("a/b", msg("2"));
        assert_eq!(t.shape_gen(), g1, "an update creates nothing");
        assert!(t.insert("x/y", msg("3")).is_none());
        assert_eq!(t.shape_gen(), g1, "refused at the cap");
        t.remove(t.find("a/b").unwrap());
        assert_ne!(t.shape_gen(), g1, "remove");
    }

    #[test]
    fn retained_flag_is_sticky() {
        let m = |p: &str, retain| Message {
            payload: Bytes::copy_from_slice(p.as_bytes()),
            qos: 0,
            retain,
            received_at: std::time::SystemTime::now(),
            truncated_from: None,
            sparkplug: false,
            wire_len: None,
            props: None,
        };
        let mut t = TopicTree::new(5);
        let id = t.insert("a", m("1", true)).unwrap();
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
    use crate::{Bytes, Message};
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

    fn payloads(t: &TopicTree, topic: &str) -> Vec<String> {
        t.node(t.find(topic).unwrap())
            .history
            .iter()
            .map(|m| String::from_utf8_lossy(&m.payload).into_owned())
            .collect()
    }

    #[test]
    fn cap_counts_entry_overhead() {
        let mut t = TopicTree::new(20_000);
        for _ in 0..10_000 {
            t.insert("x", m("a"));
        }
        assert!(t.history_bytes() >= 10_000 * 160, "{}", t.history_bytes());
    }

    #[test]
    fn churn_does_not_exhaust_max_topics() {
        let mut t = TopicTree::with_limits(10, usize::MAX, 50);
        for i in 0..200 {
            let id = t.insert(&format!("churn{i}"), m("x"));
            assert!(id.is_some(), "cycle {i} refused");
            t.remove(id.unwrap());
        }
        assert_eq!(t.dropped_topics(), 0);
    }

    #[test]
    fn removed_node_frees_history() {
        let mut t = TopicTree::new(10);
        let id = t.insert("a/b", m("payload")).unwrap();
        t.insert("a/b", m("again"));
        t.remove(id);
        assert_eq!(t.history_bytes(), 0);
        let n = &t.nodes[id];
        assert!(n.history.is_empty() && n.history_seq.is_empty() && n.latest.is_none());
        assert!(n.children.is_empty());
        assert_eq!(n.name.capacity(), 0);
    }

    #[test]
    fn history_depth_zero_still_bounded() {
        let cap = 1 << 20;
        let mut t = TopicTree::with_limits(0, cap, usize::MAX);
        let big = "x".repeat(100 * 1024);
        for i in 0..100 {
            t.insert(&format!("t{i}"), m(&big));
        }
        assert!(
            t.history_bytes() <= cap + 100 * (STUB_KEEP + ENTRY_OVERHEAD),
            "{}",
            t.history_bytes()
        );
        // The latest messages are accounted, not hidden by the empty history.
        assert!(t.history_bytes() > 0);
    }

    #[test]
    fn cap_evicts_latest_payloads() {
        let cap = 1 << 20;
        let mut t = TopicTree::with_limits(1, cap, usize::MAX);
        let big = "x".repeat(100 * 1024);
        for i in 0..100 {
            t.insert(&format!("t{i}"), m(&big));
        }
        assert!(
            t.history_bytes() <= cap + 100 * (STUB_KEEP + ENTRY_OVERHEAD),
            "{}",
            t.history_bytes()
        );
        for i in 0..100 {
            assert!(t.find(&format!("t{i}")).is_some());
        }
        let n = t.node(t.find("t0").unwrap());
        let l = n.latest.as_ref().unwrap();
        assert_eq!(l.truncated_from, Some(100 * 1024));
        assert_eq!(l.payload.len(), STUB_KEEP);
        assert_eq!(n.history.back().unwrap().payload.len(), STUB_KEEP);
        assert_eq!(n.history.back().unwrap().truncated_from, Some(100 * 1024));
    }

    #[test]
    fn stub_keeps_newest_topics_whole() {
        let cap = 1 << 20;
        let mut t = TopicTree::with_limits(1, cap, usize::MAX);
        let big = "x".repeat(100 * 1024);
        let whole = |t: &TopicTree, n: &str| {
            t.node(t.find(n).unwrap())
                .latest
                .as_ref()
                .unwrap()
                .truncated_from
                .is_none()
        };
        for i in 0..100 {
            t.insert(&format!("t{i}"), m(&big));
        }
        for i in 95..100 {
            assert!(whole(&t, &format!("t{i}")), "t{i}");
        }
        assert!(!whole(&t, "t1"));
        // Updating an old topic makes it the newest, so it stays whole.
        t.insert("t0", m(&big));
        assert!(whole(&t, "t0"));
        assert!(whole(&t, "t99"));
        assert!(!whole(&t, "t1"));
    }

    #[test]
    fn stub_cuts_on_a_char_boundary_and_keeps_flags() {
        let mut t = TopicTree::with_limits(1, 0, usize::MAX);
        // 'é' is two bytes: a cut at 256 would split the 128th character
        // only if the prefix were odd, so shift by one ASCII byte.
        let text = format!("a{}", "é".repeat(500));
        let mut msg = m(&text);
        msg.sparkplug = true;
        let id = t.insert("a", msg).unwrap();
        let l = t.node(id).latest.as_ref().unwrap();
        assert!(std::str::from_utf8(&l.payload).is_ok());
        assert_eq!(l.payload.len(), STUB_KEEP - 1);
        assert_eq!(l.truncated_from, Some(text.len()));
        assert!(l.sparkplug);
        assert_eq!(t.history_bytes(), STUB_KEEP - 1 + ENTRY_OVERHEAD);
    }

    #[test]
    fn order_queue_stays_bounded_below_the_cap() {
        let mut t = TopicTree::with_limits(1, usize::MAX, usize::MAX);
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
        let mut t = TopicTree::with_limits(100, 0, usize::MAX);
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
        assert_eq!(t.history_bytes(), 2000 * (1 + ENTRY_OVERHEAD));
    }

    #[test]
    fn memory_cap_evicts_oldest_globally() {
        let mut t = TopicTree::with_limits(100, 10 + 2 * ENTRY_OVERHEAD, usize::MAX);
        t.insert("a", m("aaaa"));
        t.insert("b", m("bbbb"));
        t.insert("a", m("cccc"));
        assert_eq!(payloads(&t, "a"), ["cccc"]);
        assert_eq!(payloads(&t, "b"), ["bbbb"]);
        assert_eq!(t.history_bytes(), 8 + 2 * ENTRY_OVERHEAD);
    }

    #[test]
    fn memory_cap_keeps_latest_of_each_node() {
        let mut t = TopicTree::with_limits(100, 5, usize::MAX);
        t.insert("a", m("aaaa"));
        t.insert("b", m("bbbb"));
        assert_eq!(
            t.history_bytes(),
            8 + 2 * ENTRY_OVERHEAD,
            "each node keeps its latest"
        );
        t.insert("a", m("cc"));
        assert_eq!(payloads(&t, "a"), ["cc"]);
        assert_eq!(payloads(&t, "b"), ["bbbb"]);
        assert_eq!(t.history_bytes(), 6 + 2 * ENTRY_OVERHEAD);
        // Later entries still evict once older ones are gone.
        t.insert("b", m("d"));
        assert_eq!(payloads(&t, "b"), ["d"]);
        assert_eq!(t.history_bytes(), 3 + 2 * ENTRY_OVERHEAD);
    }

    #[test]
    fn history_bytes_tracks_depth_eviction() {
        let mut t = TopicTree::with_limits(2, usize::MAX, usize::MAX);
        for p in ["1", "22", "333"] {
            t.insert("x", m(p));
        }
        assert_eq!(payloads(&t, "x"), ["22", "333"]);
        assert_eq!(t.history_bytes(), 5 + 2 * ENTRY_OVERHEAD);
        // A depth-evicted entry's queue slot is stale and skipped later.
        let mut t = TopicTree::with_limits(1, 4, usize::MAX);
        t.insert("x", m("111"));
        t.insert("x", m("22"));
        t.insert("y", m("33"));
        assert_eq!(payloads(&t, "x"), ["22"]);
        assert_eq!(t.history_bytes(), 4 + 2 * ENTRY_OVERHEAD);
    }
    #[test]
    fn latest_shares_history_buffer() {
        let mut t = TopicTree::new(5);
        let big = "x".repeat(1 << 20);
        let id = t.insert("big", m(&big)).unwrap();
        let n = t.node(id);
        assert_eq!(
            n.latest.as_ref().unwrap().payload.as_ptr(),
            n.history.back().unwrap().payload.as_ptr(),
            "latest and history must share one buffer"
        );
    }

    #[test]
    fn topic_cap_refuses_new_topics_keeps_updates() {
        // Three levels below the root: `a`, `b`, `c`.
        let mut t = TopicTree::with_limits(5, usize::MAX, 3);
        assert!(t.insert("a/b", m("1")).is_some());
        assert!(t.insert("a/c", m("2")).is_some());
        let before = t.len();
        assert_eq!(t.insert("a/d", m("3")), None);
        assert_eq!(t.insert("x/y/z", m("3")), None);
        assert_eq!(t.len(), before, "a refused insert creates no level");
        assert_eq!(t.find("x"), None);
        assert_eq!(t.dropped_topics(), 2);
        assert!(t.insert("a/b", m("4")).is_some(), "updates still land");
        assert_eq!(payloads(&t, "a/b"), ["1", "4"]);
        assert_eq!(t.node(TopicTree::ROOT).msg_count, 3);
        assert_eq!(t.dropped_topics(), 2);
        // An existing intermediate level can still take a message.
        assert!(t.insert("a", m("5")).is_some());
    }

    #[test]
    fn insert_builds_levels() {
        let mut t = TopicTree::new(10);
        let id = t
            .insert("tasmota/tele/tasmota_0C6423/SENSOR", m("{}"))
            .unwrap();
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
        assert_eq!(&n.history.back().unwrap().payload[..], b"4");
        assert_eq!(&n.latest.as_ref().unwrap().payload[..], b"4");
    }
    #[test]
    fn odd_topics_round_trip() {
        let mut t = TopicTree::new(1);
        for s in ["a//b", "/lead", "trail/", "$SYS/broker/version"] {
            let id = t.insert(s, m("v")).unwrap();
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
        let b = t.insert("a/b", m("1")).unwrap();
        let a = t.insert("a", m("2")).unwrap();
        assert_eq!(t.iter_with_message().collect::<Vec<_>>(), vec![a, b]);
        assert_eq!(t.node(TopicTree::ROOT).topic_count, 2);
    }
}
