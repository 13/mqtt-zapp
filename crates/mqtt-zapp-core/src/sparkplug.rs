//! Sparkplug B payloads (Eclipse Tahu `Payload` protobuf) as JSON.
//!
//! A hand-written wire decoder for the few fields the viewer needs; every
//! read is bounds-checked, so no input can panic it.

use std::collections::HashMap;

use serde_json::{Map, Number, Value, json};

/// `spBv1.0/<group>/<type>/...` with a message type that carries a payload.
/// `STATE` (a plain JSON/text host status) and anything else give false.
pub fn is_sparkplug_topic(topic: &str) -> bool {
    let mut it = topic.split('/');
    it.next() == Some("spBv1.0")
        && it.next().is_some()
        && matches!(
            it.next(),
            Some("NBIRTH" | "NDEATH" | "DBIRTH" | "DDEATH" | "NDATA" | "DDATA" | "NCMD" | "DCMD")
        )
}

/// One protobuf field value, by wire type.
enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Len(&'a [u8]),
    Fixed32(u32),
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len())?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Some(s)
    }

    /// A varint of at most 10 bytes; longer or cut short is `None`.
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for i in 0..10 {
            let b = *self.buf.get(self.pos)?;
            self.pos += 1;
            if i == 9 && b > 1 {
                return None; // does not fit 64 bits
            }
            v |= u64::from(b & 0x7f) << (7 * i);
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    /// The next `(field number, value)`; groups, reserved wire types and
    /// field 0 are malformed.
    fn field(&mut self) -> Option<(u64, Wire<'a>)> {
        let key = self.varint()?;
        let num = key >> 3;
        if num == 0 {
            return None;
        }
        let w = match key & 7 {
            0 => Wire::Varint(self.varint()?),
            1 => Wire::Fixed64(u64::from_le_bytes(self.take(8)?.try_into().ok()?)),
            2 => {
                let n = usize::try_from(self.varint()?).ok()?;
                Wire::Len(self.take(n)?)
            }
            5 => Wire::Fixed32(u32::from_le_bytes(self.take(4)?.try_into().ok()?)),
            _ => return None,
        };
        Some((num, w))
    }
}

fn string(b: &[u8]) -> Option<String> {
    // Invalid UTF-8 is not a Sparkplug payload: keep the raw bytes instead.
    String::from_utf8(b.to_vec()).ok()
}

fn float(f: f64) -> Value {
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

/// An `f32` as its shortest round-trip decimal, so 21.3 stays 21.3 rather
/// than widening to 21.299999237060547; non-finite values become null.
fn float32(f: f32) -> Value {
    if !f.is_finite() {
        return Value::Null;
    }
    float(f.to_string().parse().unwrap_or(f64::from(f)))
}

/// Most alias -> name entries kept across all nodes and devices.
const MAX_ALIASES: usize = 100_000;

/// `(group, edge node, device)`.
type NodeKey = (String, String, Option<String>);

/// Metric names learned from BIRTH messages, so a later DATA or CMD metric
/// that only carries an alias is shown under its name.
#[derive(Default)]
pub struct Aliases {
    maps: HashMap<NodeKey, HashMap<u64, String>>,
    total: usize,
}

impl Aliases {
    /// Entries held across all nodes and devices.
    pub fn len(&self) -> usize {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Decode `payload` received on `topic`. A BIRTH records its
    /// alias -> name pairs (NBIRTH replaces the node's map and its devices'
    /// maps, DBIRTH the device's); any other type resolves alias-only
    /// metrics from what was recorded.
    pub fn decode(&mut self, topic: &str, payload: &[u8]) -> Option<Value> {
        let mut it = topic.split('/');
        let (Some("spBv1.0"), Some(group), Some(kind), Some(edge)) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            return decode_inner(payload, None).map(|d| d.0);
        };
        let device = it.next();
        let key: NodeKey = (group.into(), edge.into(), device.map(Into::into));
        if matches!(kind, "NBIRTH" | "DBIRTH") {
            let (value, pairs) = decode_inner(payload, None)?;
            self.learn(kind == "NBIRTH", key, pairs);
            Some(value)
        } else {
            decode_inner(payload, self.maps.get(&key)).map(|d| d.0)
        }
    }

    fn learn(&mut self, node: bool, key: NodeKey, pairs: Vec<(String, u64)>) {
        if node {
            let (g, e) = (key.0.clone(), key.1.clone());
            let mut freed = 0;
            self.maps.retain(|k, m| {
                let hit = k.0 == g && k.1 == e;
                if hit {
                    freed += m.len();
                }
                !hit
            });
            self.total -= freed;
        } else if let Some(old) = self.maps.remove(&key) {
            self.total -= old.len();
        }
        let map: HashMap<u64, String> = pairs.into_iter().map(|(n, a)| (a, n)).collect();
        if map.is_empty() {
            return;
        }
        if self.total + map.len() > MAX_ALIASES {
            // Over the cap: record nothing new (the decode itself is fine).
            return;
        }
        self.total += map.len();
        self.maps.insert(key, map);
    }
}

/// Decode a Sparkplug B payload; `None` when it is not a well-formed one.
#[cfg(test)]
pub fn decode(payload: &[u8]) -> Option<Value> {
    decode_inner(payload, None).map(|d| d.0)
}

/// The decoded payload and the `(name, alias)` pairs its metrics carried.
/// Alias-only metrics are named from `names` when it knows them.
fn decode_inner(
    payload: &[u8],
    names: Option<&HashMap<u64, String>>,
) -> Option<(Value, Vec<(String, u64)>)> {
    if payload.is_empty() {
        return None;
    }
    let mut out = Map::new();
    let mut metrics = Map::new();
    let mut known = false;
    let mut pairs = Vec::new();
    let mut r = Reader::new(payload);
    while !r.done() {
        match r.field()? {
            (1, Wire::Varint(v)) => {
                known = true;
                out.insert("timestamp".into(), json!(v));
            }
            (2, Wire::Len(b)) => {
                known = true;
                if let Some((k, v, pair)) = metric(b, names)? {
                    metrics.insert(k, v);
                    pairs.extend(pair);
                }
            }
            (3, Wire::Varint(v)) => {
                known = true;
                out.insert("seq".into(), json!(v));
            }
            (4, Wire::Len(b)) => {
                known = true;
                out.insert("uuid".into(), Value::String(string(b)?));
            }
            // 5 is the opaque body; known fields with another wire type
            // are malformed.
            (5, Wire::Len(_)) => {}
            (1..=5, _) => return None,
            _ => {}
        }
    }
    if !known {
        return None;
    }
    // `metrics` first, so `c` charts a metric before the timestamp.
    let mut ordered = Map::new();
    if !metrics.is_empty() {
        ordered.insert("metrics".into(), Value::Object(metrics));
    }
    for k in ["timestamp", "seq", "uuid"] {
        if let Some(v) = out.remove(k) {
            ordered.insert(k.into(), v);
        }
    }
    Some((Value::Object(ordered), pairs))
}

/// One `Metric` as `(key, value, name/alias pair)`; the outer `None` is
/// malformed, the inner one a metric with neither name nor alias (skipped).
type Metric = (String, Value, Option<(String, u64)>);

fn metric(b: &[u8], names: Option<&HashMap<u64, String>>) -> Option<Option<Metric>> {
    let (mut name, mut alias, mut datatype) = (None, None, 0u64);
    let (mut null, mut raw) = (false, None);
    let mut r = Reader::new(b);
    while !r.done() {
        match r.field()? {
            (1, Wire::Len(b)) => name = Some(string(b)?),
            (2, Wire::Varint(v)) => alias = Some(v),
            (4, Wire::Varint(v)) => datatype = v,
            (7, Wire::Varint(v)) => null = v != 0,
            (n @ 10..=18, w) => raw = Some((n, w)),
            (1 | 2 | 4 | 7, _) => return None,
            _ => {}
        }
    }
    let pair = name.clone().zip(alias);
    let Some(key) = name.or_else(|| {
        alias.map(|a| {
            names
                .and_then(|m| m.get(&a).cloned())
                .unwrap_or_else(|| format!("alias:{a}"))
        })
    }) else {
        return Some(None);
    };
    let value = if null {
        Value::Null
    } else {
        match raw {
            Some((10, Wire::Varint(v))) => {
                let v = v as u32;
                if (1..=3).contains(&datatype) {
                    json!(v as i32)
                } else {
                    json!(v)
                }
            }
            Some((11, Wire::Varint(v))) => {
                if datatype == 4 {
                    json!(v as i64)
                } else {
                    json!(v)
                }
            }
            Some((12, Wire::Fixed32(v))) => float32(f32::from_bits(v)),
            Some((13, Wire::Fixed64(v))) => float(f64::from_bits(v)),
            Some((14, Wire::Varint(v))) => Value::Bool(v != 0),
            Some((15, Wire::Len(s))) => Value::String(string(s)?),
            Some((16, Wire::Len(s))) => Value::String(format!("<{} bytes>", s.len())),
            Some((17, _)) => "<dataset>".into(),
            Some((18, _)) => "<template>".into(),
            None => Value::Null,
            // A value field with the wrong wire type.
            Some(_) => return None,
        }
    };
    Some(Some((key, value, pair)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut v: u64) -> Vec<u8> {
        let mut o = Vec::new();
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                o.push(b);
                return o;
            }
            o.push(b | 0x80);
        }
    }

    /// A field with wire type `wt`; `data` is the already-encoded value.
    fn field(num: u64, wt: u64, data: &[u8]) -> Vec<u8> {
        let mut o = varint(num << 3 | wt);
        o.extend_from_slice(data);
        o
    }

    fn vf(num: u64, v: u64) -> Vec<u8> {
        field(num, 0, &varint(v))
    }

    fn lf(num: u64, b: &[u8]) -> Vec<u8> {
        let mut d = varint(b.len() as u64);
        d.extend_from_slice(b);
        field(num, 2, &d)
    }

    fn metric(parts: &[Vec<u8>]) -> Vec<u8> {
        lf(2, &parts.concat())
    }

    #[test]
    fn decode_ndata_metrics_by_name() {
        let p = [
            vf(1, 1000),
            metric(&[
                lf(1, b"Temperature"),
                vf(4, 9),
                field(12, 5, &21.5f32.to_le_bytes()),
            ]),
            metric(&[lf(1, b"On"), vf(4, 11), vf(14, 1)]),
            vf(3, 3),
        ]
        .concat();
        assert_eq!(
            decode(&p).unwrap(),
            json!({"metrics":{"Temperature":21.5,"On":true},"timestamp":1000,"seq":3})
        );
    }

    #[test]
    fn float_metrics_keep_shortest_decimal() {
        for (f, want) in [(21.3f32, 21.3f64), (0.1, 0.1)] {
            let p = metric(&[lf(1, b"x"), vf(4, 9), field(12, 5, &f.to_le_bytes())]);
            assert_eq!(decode(&p).unwrap(), json!({"metrics":{"x":want}}));
        }
        let p = metric(&[lf(1, b"x"), vf(4, 9), field(12, 5, &f32::NAN.to_le_bytes())]);
        assert_eq!(decode(&p).unwrap(), json!({"metrics":{"x":null}}));
    }

    #[test]
    fn only_unknown_fields_is_not_sparkplug() {
        assert!(decode(b"hi").is_none());
    }

    #[test]
    fn metrics_come_first() {
        let p = [
            vf(1, 1),
            metric(&[lf(1, b"x"), vf(4, 3), vf(10, 1)]),
            vf(3, 2),
        ]
        .concat();
        let keys: Vec<_> = decode(&p)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys, ["metrics", "timestamp", "seq"]);
    }

    #[test]
    fn decode_alias_only_metric() {
        let p = metric(&[vf(2, 7), vf(4, 8), vf(11, 12)]);
        assert_eq!(decode(&p).unwrap(), json!({"metrics":{"alias:7":12}}));
    }

    #[test]
    fn decode_signed_int32() {
        let p = metric(&[lf(1, b"x"), vf(4, 3), vf(10, 0xFFFF_FFFF)]);
        assert_eq!(decode(&p).unwrap(), json!({"metrics":{"x":-1}}));
        let p = metric(&[lf(1, b"x"), vf(4, 7), vf(10, 0xFFFF_FFFF)]);
        assert_eq!(decode(&p).unwrap(), json!({"metrics":{"x":4294967295u64}}));
        // The datatype may follow the value on the wire.
        let p = metric(&[lf(1, b"x"), vf(11, u64::MAX), vf(4, 4)]);
        assert_eq!(decode(&p).unwrap(), json!({"metrics":{"x":-1}}));
    }

    #[test]
    fn decode_null_and_bytes() {
        let p = [
            metric(&[lf(1, b"n"), vf(7, 1)]),
            metric(&[lf(1, b"b"), lf(16, &[1, 2, 3, 4])]),
            metric(&[lf(1, b"s"), lf(15, b"hi"), lf(1, b"s")]),
            metric(&[lf(1, b"d"), lf(17, b"")]),
        ]
        .concat();
        assert_eq!(
            decode(&p).unwrap(),
            json!({"metrics":{"n":null,"b":"<4 bytes>","s":"hi","d":"<dataset>"}})
        );
    }

    #[test]
    fn duplicate_name_later_wins() {
        let p = [
            metric(&[lf(1, b"a"), vf(10, 1)]),
            metric(&[lf(1, b"a"), vf(10, 2)]),
        ]
        .concat();
        assert_eq!(decode(&p).unwrap(), json!({"metrics":{"a":2}}));
    }

    #[test]
    fn decode_skips_unknown_fields() {
        let p = [
            vf(1, 5),
            vf(99, 1234),
            field(98, 1, &[0; 8]),
            field(97, 5, &[0; 4]),
            lf(96, b"zz"),
            lf(5, b"body"),
            metric(&[lf(1, b"a"), vf(50, 1), vf(10, 1)]),
        ]
        .concat();
        assert_eq!(
            decode(&p).unwrap(),
            json!({"metrics":{"a":1},"timestamp":5})
        );
    }

    #[test]
    fn decode_rejects_garbage() {
        assert_eq!(decode(b"{\"a\":1}"), None);
        assert_eq!(decode(b""), None);
        assert_eq!(decode(&[0x80; 20]), None, "varint overflow");
        assert_eq!(decode(&[0x08]), None, "missing value");
        assert_eq!(decode(&[0x0b]), None, "start group");
        assert_eq!(decode(&[0x00, 0x01]), None, "field 0");
        assert_eq!(decode(&lf(4, &[0xff, 0xfe])), None, "invalid utf-8");
        assert_eq!(decode(&field(1, 2, &[0])), None, "wrong wire type");
        // A length far beyond the buffer, near usize::MAX.
        let mut p = varint(2 << 3 | 2);
        p.extend(varint(u64::MAX));
        assert_eq!(decode(&p), None);
        for len in 0..64u8 {
            let junk: Vec<u8> = (0..len).map(|i| i.wrapping_mul(37) ^ 0xa5).collect();
            let _ = decode(&junk);
        }
    }

    #[test]
    fn decode_truncated_returns_none() {
        let p = [
            vf(1, 1000),
            metric(&[lf(1, b"Temperature"), vf(4, 9), vf(10, 5)]),
        ]
        .concat();
        assert!(decode(&p).is_some());
        assert_eq!(decode(&p[..p.len() - 3]), None);
    }

    fn named(name: &[u8], alias: u64) -> Vec<u8> {
        metric(&[
            lf(1, name),
            vf(2, alias),
            vf(4, 9),
            field(12, 5, &1.0f32.to_le_bytes()),
        ])
    }

    fn by_alias(alias: u64, v: f32) -> Vec<u8> {
        metric(&[vf(2, alias), vf(4, 9), field(12, 5, &v.to_le_bytes())])
    }

    fn data(m: Vec<u8>) -> Vec<u8> {
        [vf(1, 1), m].concat()
    }

    #[test]
    fn data_uses_birth_names() {
        let mut a = Aliases::default();
        a.decode("spBv1.0/g/NBIRTH/n", &data(named(b"Temperature", 7)))
            .unwrap();
        let v = a
            .decode("spBv1.0/g/NDATA/n", &data(by_alias(7, 21.5)))
            .unwrap();
        assert_eq!(v["metrics"]["Temperature"], json!(21.5));
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn device_aliases_are_separate() {
        let mut a = Aliases::default();
        a.decode("spBv1.0/g/DBIRTH/n/d1", &data(named(b"A", 1)))
            .unwrap();
        a.decode("spBv1.0/g/DBIRTH/n/d2", &data(named(b"B", 1)))
            .unwrap();
        let v = a
            .decode("spBv1.0/g/DDATA/n/d2", &data(by_alias(1, 2.0)))
            .unwrap();
        assert_eq!(v["metrics"]["B"], json!(2.0));
        let v = a
            .decode("spBv1.0/g/DDATA/n/d1", &data(by_alias(1, 2.0)))
            .unwrap();
        assert_eq!(v["metrics"]["A"], json!(2.0));
    }

    #[test]
    fn data_before_birth_keeps_alias_keys() {
        let mut a = Aliases::default();
        let v = a
            .decode("spBv1.0/g/NDATA/n", &data(by_alias(7, 1.5)))
            .unwrap();
        assert_eq!(v["metrics"]["alias:7"], json!(1.5));
        assert!(a.is_empty());
    }

    #[test]
    fn rebirth_replaces_names() {
        let mut a = Aliases::default();
        a.decode("spBv1.0/g/NBIRTH/n", &data(named(b"Temp1", 7)))
            .unwrap();
        a.decode("spBv1.0/g/NBIRTH/n", &data(named(b"Temp2", 7)))
            .unwrap();
        let v = a
            .decode("spBv1.0/g/NDATA/n", &data(by_alias(7, 3.0)))
            .unwrap();
        assert_eq!(v["metrics"]["Temp2"], json!(3.0));
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn nbirth_clears_device_maps() {
        let mut a = Aliases::default();
        a.decode("spBv1.0/g/DBIRTH/n/d1", &data(named(b"A", 1)))
            .unwrap();
        a.decode("spBv1.0/g/NBIRTH/n", &data(named(b"N", 2)))
            .unwrap();
        assert_eq!(a.len(), 1);
        let v = a
            .decode("spBv1.0/g/DDATA/n/d1", &data(by_alias(1, 2.0)))
            .unwrap();
        assert_eq!(v["metrics"]["alias:1"], json!(2.0));
    }

    #[test]
    fn name_already_in_message_is_overwritten() {
        let mut a = Aliases::default();
        a.decode("spBv1.0/g/NBIRTH/n", &data(named(b"T", 7)))
            .unwrap();
        let p = data(
            [
                metric(&[lf(1, b"T"), vf(4, 9), field(12, 5, &1.0f32.to_le_bytes())]),
                by_alias(7, 5.0),
            ]
            .concat(),
        );
        let v = a.decode("spBv1.0/g/NDATA/n", &p).unwrap();
        assert_eq!(v["metrics"]["T"], json!(5.0));
        assert_eq!(v["metrics"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn aliases_are_capped() {
        let mut a = Aliases::default();
        let batch = 40_000u64;
        for d in 0..4 {
            let p = data(
                (0..batch)
                    .map(|i| named(format!("m{i}").as_bytes(), i))
                    .collect::<Vec<_>>()
                    .concat(),
            );
            let v = a.decode(&format!("spBv1.0/g/DBIRTH/n/d{d}"), &p);
            assert!(v.is_some());
            assert!(a.len() <= MAX_ALIASES);
        }
        assert_eq!(a.len(), 80_000);
    }

    #[test]
    fn topic_types() {
        assert!(is_sparkplug_topic("spBv1.0/g/NDATA/n"));
        assert!(is_sparkplug_topic("spBv1.0/g/DBIRTH/n/dev"));
        assert!(!is_sparkplug_topic("spBv1.0/g/STATE/x"));
        assert!(!is_sparkplug_topic("spBv1.0/STATE/host"));
        assert!(!is_sparkplug_topic("tasmota/x"));
        assert!(!is_sparkplug_topic("spBv1.0"));
    }
}
