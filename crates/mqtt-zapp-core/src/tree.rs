use crate::Message;
use serde::Serialize;
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

#[cfg(test)]
mod tests {
    use super::*;
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
