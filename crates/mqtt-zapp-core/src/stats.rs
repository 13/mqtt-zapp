//! Broker statistics from the `$SYS` tree (mosquitto's topic names).

use crate::TopicTree;
use crate::payload::sanitize;

/// Label and `$SYS` topic of each statistic, in display order.
const STATS: [(&str, &str); 10] = [
    ("version", "$SYS/broker/version"),
    ("uptime", "$SYS/broker/uptime"),
    ("clients connected", "$SYS/broker/clients/connected"),
    ("clients total", "$SYS/broker/clients/total"),
    (
        "msgs received/min",
        "$SYS/broker/load/messages/received/1min",
    ),
    ("msgs sent/min", "$SYS/broker/load/messages/sent/1min"),
    ("retained", "$SYS/broker/retained messages/count"),
    ("subscriptions", "$SYS/broker/subscriptions/count"),
    ("bytes received/min", "$SYS/broker/load/bytes/received/1min"),
    ("bytes sent/min", "$SYS/broker/load/bytes/sent/1min"),
];

/// The known statistics the tree has a value for, sanitized and trimmed.
pub fn broker_stats(tree: &TopicTree) -> Vec<(&'static str, String)> {
    STATS
        .iter()
        .filter_map(|&(label, topic)| {
            let m = tree.node(tree.find(topic)?).latest.as_ref()?;
            let text = String::from_utf8_lossy(&m.payload);
            Some((label, sanitize(text.trim()).replace('\n', " ")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bytes, Message};
    use std::time::SystemTime;

    fn put(t: &mut TopicTree, topic: &str, p: &str) {
        t.insert(
            topic,
            Message {
                payload: Bytes::copy_from_slice(p.as_bytes()),
                qos: 0,
                retain: true,
                received_at: SystemTime::now(),
                truncated_from: None,
                sparkplug: false,
                props: None,
            },
        );
    }

    #[test]
    fn broker_stats_reads_known_topics() {
        let mut t = TopicTree::new(1);
        put(&mut t, "$SYS/broker/clients/connected", " 3\n");
        put(&mut t, "$SYS/broker/version", "mosquitto version 2.0.18");
        put(&mut t, "$SYS/broker/retained messages/count", "\x1b[2J42");
        put(&mut t, "$SYS/broker/unknown", "x");
        let s = broker_stats(&t);
        assert_eq!(
            s,
            vec![
                ("version", "mosquitto version 2.0.18".to_owned()),
                ("clients connected", "3".to_owned()),
                ("retained", "\u{fffd}[2J42".to_owned()),
            ]
        );
    }

    #[test]
    fn broker_stats_empty_without_sys() {
        let mut t = TopicTree::new(1);
        put(&mut t, "a/b", "1");
        assert!(broker_stats(&t).is_empty());
    }
}
