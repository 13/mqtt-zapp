# mqtt-zapp Phase 1 (TUI essentials) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust MQTT explorer TUI that can:
- connect to saved brokers (mqtt/mqtts/ws/wss, user/password, TLS)
- show a live topic tree with plain-text search over topics and payloads
- show live messages with formatting, history and a diff to the previous message
- publish messages

**Architecture:** One Cargo workspace with two crates:
- `mqtt-zapp-core` is a UI-agnostic library. It holds the connection, topic tree, history, search, diff, payload decoding and config. Its public state and events derive `serde::Serialize`.
- `mqtt-zapp` is the binary. It holds the CLI and a ratatui front end.

The core pushes `CoreEvent`s over a tokio channel. The TUI drains them in batches and redraws on a 100 ms tick.

**Tech Stack:** The plan uses these major versions. A newer compatible minor version is fine.
- Rust stable (1.98)
- tokio 1
- rumqttc 0.24, features `use-rustls` and `websocket`
- ratatui 0.29, crossterm 0.28 with `event-stream`, tui-textarea 0.7
- clap 4 with `derive`
- serde
- serde_json with `preserve_order`, so key order is kept when pretty-printing
- toml 0.8, directories 5, similar 2
- rpassword 7, rand 0.8
- anyhow / thiserror
- keyring 3, optional and behind a feature
- dev dependencies: rcgen, tempfile

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md`. Read it first. This plan covers only the **[core]** items. Every unmarked item is out of scope.

## Global Constraints

- Release target: a single static binary, `x86_64-unknown-linux-musl`. Only x86_64 is supported.
- Use rustls with the **`ring`** backend. `cargo tree -i aws-lc-sys` must print nothing.
- Keyring support sits behind cargo feature `keyring`. It is on by default and **off in the musl build** (`--no-default-features`). Without it, mqtt-zapp prompts for passwords.
- Never write passwords to `profiles.toml` or `settings.toml`.
- Config paths come from XDG: `~/.config/mqtt-zapp/profiles.toml` and `~/.config/mqtt-zapp/settings.toml`. Override the directory with env `MQTT_ZAPP_CONFIG_DIR`, which tests use.
- Default subscriptions are `["#", "$SYS/#"]`. Each profile can override them in TOML.
- MQTT version is 3.1.1 only. MQTT 5 is Phase 1.x.
- The client id defaults to `mqtt-zapp-` + 6 random lowercase hex characters.
- Default keys are vim-style and remappable through the `[keys]` table in `settings.toml`.
- Core public event and state types derive `serde::Serialize`. The core crate must not depend on ratatui or crossterm.
- Search is plain, case-insensitive substring matching on the full topic path **or** the latest payload. No prefix syntax.
- CI runs on GitHub Actions: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
- Do not use MQTT Explorer branding or visuals.
- Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

Each item below is the expected behaviour for one risk:

1. **Hostile or odd payload bytes.** Payloads may contain ANSI escape sequences, other control characters, invalid UTF-8, or be 1 MB or more. The terminal must never interpret them (SSH terminal injection). Binary shows as hex, previews are truncated, and nothing panics. Tests: Task 1, Task 9.
2. **Broker problems.** The broker may be unreachable, reject the password, or drop the connection mid-session. Show `ConnState::Disconnected{reason}` in the status bar, retry with backoff (1 s up to a 30 s maximum), keep the UI responsive, and never exit. Test: Task 6.
3. **Message bursts.** Expect a retained-message burst on subscribe (hundreds of messages at once) plus about 500 msg/min. Input must stay responsive. The TUI drains at most 2000 events per loop iteration and redraws at most every 100 ms. Test: Task 8.
4. **Odd topic shapes.** These include `a//b`, a leading `/x`, a trailing `x/`, `$SYS/...`, and very deep or wide topics. The tree keeps empty levels as nodes named `""`, shown as `∅`. A topic round-trips exactly through `TopicTree::full_topic`. Test: Task 2.
5. **Terminal restore.** On a panic, Ctrl-C, or `q`, leave raw mode and the alternate screen. A terminal smaller than 40×10 shows "terminal too small" instead of panicking. Test: Task 8.

---

## File Structure

```
Cargo.toml                         workspace
crates/mqtt-zapp-core/
  Cargo.toml
  src/lib.rs                       re-exports
  src/payload.rs                   Message, PayloadKind detection, pretty/hex, sanitize
  src/tree.rs                      TopicTree arena, history ring, counts
  src/search.rs                    Filter (visible set) over TopicTree
  src/diff.rs                      JSON-key diff + text line diff
  src/config.rs                    Profile, Protocol, TlsConfig, Settings, ProfileStore
  src/secrets.rs                   password storage/resolution (keyring feature)
  src/client.rs                    Connection: rumqttc options, event loop task, CoreEvent
  tests/broker.rs                  integration tests (spawn mosquitto)
  tests/support/mod.rs             mosquitto spawner + rcgen certs
crates/mqtt-zapp/
  Cargo.toml
  src/main.rs                      clap entry, dispatch
  src/cli.rs                       Args, ProfileCmd, ad-hoc → Profile
  src/tui/mod.rs                   run(): terminal setup/restore, event loop
  src/tui/keymap.rs                Action, KeySpec parsing, Keymap
  src/tui/app.rs                   App state machine, handle_action, apply_core_event
  src/tui/rows.rs                  flatten tree → visible rows
  src/tui/ui.rs                    layout + render dispatch, status bar, too-small
  src/tui/message_view.rs          payload/metadata/history/diff rendering
  src/tui/picker.rs                profile picker + password prompt widgets
  src/tui/publish.rs               publish form state + render
.github/workflows/ci.yml
.github/workflows/release.yml
README.md
```

---

### Task 1: Workspace scaffold, payload decoding, CI

**Files:**
- Create: `Cargo.toml`, `crates/mqtt-zapp-core/{Cargo.toml,src/lib.rs,src/payload.rs}`, `crates/mqtt-zapp/{Cargo.toml,src/main.rs}` (main only prints `--version` for now), `.github/workflows/ci.yml`, `.gitignore` (`/target`)
- Test: unit tests in `src/payload.rs`

**Interfaces:**
- Produces:
  - `pub struct Message { pub payload: Vec<u8>, pub qos: u8, pub retain: bool, pub received_at: std::time::SystemTime }` derives `Clone`, `Debug`, `Serialize`.
  - `pub enum PayloadKind { Json(serde_json::Value), Text(String), Binary }`
  - `pub fn detect(payload: &[u8]) -> PayloadKind`
  - `pub fn pretty_lines(kind: &PayloadKind, payload: &[u8]) -> Vec<String>`. This gives pretty JSON lines, text split on `\n`, or the hex dump. Every line is sanitized.
  - `pub fn hex_dump(payload: &[u8]) -> Vec<String>`. Each row has 16 bytes: an `00000000` offset, the hex bytes, and an ASCII gutter.
  - `pub fn preview(payload: &[u8], max_chars: usize) -> String`. This is a single line, sanitized, with `\n` → `⏎`. When it is truncated it ends with `…`.
  - `pub fn sanitize(s: &str) -> String`. It keeps `\n`, turns `\t` into 4 spaces, and replaces any other char where `char::is_control()` is true (including ESC `\x1b`) with `�`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn detects_json_object() { assert!(matches!(detect(br#"{"a":1}"#), PayloadKind::Json(_))); }
#[test] fn detects_multiline_pretty_json() { assert!(matches!(detect(b"{\n  \"UPSNAME\": \"apc-ups\"\n}\n"), PayloadKind::Json(_))); }
#[test] fn plain_number_is_text_not_json() { assert!(matches!(detect(b"23.5"), PayloadKind::Text(_))); } // scalars stay Text; only objects/arrays are Json
#[test] fn invalid_utf8_is_binary() { assert!(matches!(detect(&[0xff, 0xfe, 0x00]), PayloadKind::Binary)); }
#[test] fn nul_bytes_are_binary() { assert!(matches!(detect(b"ab\0cd"), PayloadKind::Binary)); }
#[test] fn sanitize_strips_escape() { assert_eq!(sanitize("a\x1b[31mb"), "a�[31mb"); }
#[test] fn sanitize_keeps_newline_expands_tab() { assert_eq!(sanitize("a\n\tb"), "a\n    b"); }
#[test] fn pretty_keeps_key_order() { let p=br#"{"z":1,"a":2}"#; assert_eq!(pretty_lines(&detect(p),p), vec!["{","  \"z\": 1,","  \"a\": 2","}"]); }
#[test] fn preview_truncates_and_flattens() { assert_eq!(preview(b"line1\nline2-long", 10), "line1⏎lin…"); }
#[test] fn hex_dump_row() { assert_eq!(hex_dump(b"AB")[0], "00000000  41 42                                             AB"); }
#[test] fn huge_payload_preview_is_bounded() { let p=vec![b'x'; 1_000_000]; assert_eq!(preview(&p, 80).chars().count(), 80); }
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp-core payload`.** Expected: FAIL to compile, because the functions are missing.
- [ ] **Step 3: Implement `payload.rs`** to the interfaces above. `detect` follows these rules:
  - Invalid UTF-8, or any `\0`, gives `Binary`.
  - Otherwise, if the trimmed text starts with `{` or `[` and `serde_json` parses it, the result is `Json`.
  - Otherwise the result is `Text`.

  In `preview`, cap the work: only read the first `max_chars*4` bytes. Write `ci.yml` with these jobs on `ubuntu-latest`:
  - `fmt`
  - `clippy -D warnings`
  - `test`, which runs `sudo apt-get install -y mosquitto` first and then `cargo test --workspace`
- [ ] **Step 4: Run `cargo test -p mqtt-zapp-core payload && cargo fmt --check && cargo clippy --all-targets -- -D warnings`.** Expected: all pass.
- [ ] **Step 5: Commit** with message `feat(core): workspace scaffold and payload decoding`.

---

### Task 2: Topic tree with history

**Files:**
- Create: `crates/mqtt-zapp-core/src/tree.rs`
- Test: unit tests in the same file

**Interfaces:**
- Consumes: `Message` (Task 1)
- Produces:
  - `pub type NodeId = usize;` The root is `TopicTree::ROOT` (= 0) and has no name.
  - `pub struct TopicNode` with these fields:
    - `pub name: String`
    - `pub parent: Option<NodeId>`
    - `pub children: BTreeMap<String, NodeId>`
    - `pub latest: Option<Message>`
    - `pub history: VecDeque<Message>`, newest at the back, including latest
    - `pub msg_count: u64`, the count for this subtree
    - `pub topic_count: u32`, the number of descendants with ≥1 message
    - `pub last_update: Option<Instant>`, which is `#[serde(skip)]`
  - `pub struct TopicTree` with these methods:
    - `new(history_depth: usize)`
    - `insert(&mut self, topic: &str, msg: Message) -> NodeId`
    - `node(&self, id) -> &TopicNode`
    - `full_topic(&self, id) -> String`
    - `find(&self, topic: &str) -> Option<NodeId>`
    - `len(&self) -> usize`, the number of nodes
    - `iter_with_message(&self) -> impl Iterator<Item = NodeId>`

  Both structs derive `Serialize`.

- [ ] **Step 1: Write the failing tests**

```rust
fn m(p:&str)->Message{ Message{payload:p.into(),qos:0,retain:false,received_at:SystemTime::now()} }
#[test] fn insert_builds_levels() { let mut t=TopicTree::new(10); let id=t.insert("tasmota/tele/tasmota_0C6423/SENSOR",m("{}")); assert_eq!(t.full_topic(id),"tasmota/tele/tasmota_0C6423/SENSOR"); assert_eq!(t.len(),5); }
#[test] fn counts_propagate() { let mut t=TopicTree::new(10); t.insert("a/b",m("1")); t.insert("a/b",m("2")); t.insert("a/c",m("3")); let a=t.find("a").unwrap(); assert_eq!(t.node(a).msg_count,3); assert_eq!(t.node(a).topic_count,2); }
#[test] fn history_is_bounded_newest_last() { let mut t=TopicTree::new(3); for i in 0..5 { t.insert("x",m(&i.to_string())); } let n=t.node(t.find("x").unwrap()); assert_eq!(n.history.len(),3); assert_eq!(n.history.back().unwrap().payload,b"4"); assert_eq!(n.latest.as_ref().unwrap().payload,b"4"); }
#[test] fn odd_topics_round_trip() { let mut t=TopicTree::new(1); for s in ["a//b","/lead","trail/","$SYS/broker/version"] { let id=t.insert(s,m("v")); assert_eq!(t.full_topic(id),s); } }
#[test] fn intermediate_node_has_no_message() { let mut t=TopicTree::new(1); t.insert("a/b",m("1")); assert!(t.node(t.find("a").unwrap()).latest.is_none()); }
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp-core tree`.** Expected: FAIL to compile.
- [ ] **Step 3: Implement `tree.rs`.** Store the nodes in a `Vec<TopicNode>` arena and split topics with `split('/')`. Increment `topic_count` on the ancestors only when a node receives its first message.
- [ ] **Step 4: Run `cargo test -p mqtt-zapp-core tree`.** Expected: PASS.
- [ ] **Step 5: Commit** with message `feat(core): topic tree with bounded history`.

---

### Task 3: Search filter

**Files:**
- Create: `crates/mqtt-zapp-core/src/search.rs`
- Test: unit tests in the same file

**Interfaces:**
- Consumes: `TopicTree`, `NodeId` (Task 2)
- Produces:
  - `pub struct Filter { pub query: String, pub matches: HashSet<NodeId>, pub visible: HashSet<NodeId> }`. `visible` holds the matches, all their ancestors, and ROOT.
  - `pub fn filter(tree: &TopicTree, query: &str) -> Option<Filter>`. It returns `None` when the trimmed query is empty, which means there is no filter.
  - `pub fn match_ranges(haystack: &str, query: &str) -> Vec<Range<usize>>`. These are case-insensitive byte ranges, used for highlighting.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn plain_substring_matches_topic_anywhere() { let t=tree_with(&[("muh/esp/esp32-WZR1/LWT","online")]); let f=filter(&t,"wzr").unwrap(); assert!(f.matches.contains(&t.find("muh/esp/esp32-WZR1").unwrap())); }
#[test] fn matches_latest_payload() { let t=tree_with(&[("tasmota/tele/tasmota_A/STATE",r#"{"DeviceName":"wzr-lamp"}"#)]); let f=filter(&t,"WZR").unwrap(); assert!(f.matches.contains(&t.find("tasmota/tele/tasmota_A/STATE").unwrap())); }
#[test] fn ancestors_are_visible_non_matches_hidden() { let t=tree_with(&[("a/wzr","1"),("b/c","2")]); let f=filter(&t,"wzr").unwrap(); assert!(f.visible.contains(&t.find("a").unwrap())); assert!(!f.visible.contains(&t.find("b").unwrap())); }
#[test] fn empty_query_is_no_filter() { assert!(filter(&tree_with(&[("a","1")]),"  ").is_none()); }
#[test] fn no_prefix_syntax_needed() { let t=tree_with(&[("x/device:wzr","1")]); assert!(!filter(&t,"wzr").unwrap().matches.is_empty()); }
#[test] fn binary_payload_does_not_panic() { let mut t=TopicTree::new(1); t.insert("bin",Message{payload:vec![0xff,0x00],qos:0,retain:false,received_at:SystemTime::now()}); assert!(filter(&t,"zz").unwrap().matches.is_empty()); }
#[test] fn match_ranges_case_insensitive() { assert_eq!(match_ranges("aWZRbwzr","wzr"), vec![1..4,5..8]); }
#[test] fn fast_enough_at_spec_scale() { // 5000 topics × 1 KB payloads
    let mut t=TopicTree::new(1); let p="x".repeat(1024); for i in 0..5000 { t.insert(&format!("tasmota/tele/dev_{i}/SENSOR"),m(&p)); }
    let s=Instant::now(); filter(&t,"dev_4999"); assert!(s.elapsed()<Duration::from_millis(200), "{:?}",s.elapsed()); }
```

Put the `tree_with(&[(&str,&str)]) -> TopicTree` test helper in the test module.

- [ ] **Step 2: Run `cargo test -p mqtt-zapp-core search`.** Expected: FAIL to compile.
- [ ] **Step 3: Implement it as a linear scan.** Lowercase the query once. For each node, match on `full_topic(id).to_lowercase()` or on `String::from_utf8_lossy(latest.payload).to_lowercase()`. Intermediate nodes match on topic only. Then walk parents to fill `visible`. Do not add an index (spec §7 allows a scan at this scale).
- [ ] **Step 4: Run `cargo test -p mqtt-zapp-core search`.** Expected: PASS.
- [ ] **Step 5: Commit** with message `feat(core): plain substring search over topics and payloads`.

---

### Task 4: Diff

**Files:**
- Create: `crates/mqtt-zapp-core/src/diff.rs`
- Test: unit tests in the same file

**Interfaces:**
- Consumes: `detect`, `PayloadKind`, `pretty_lines` (Task 1)
- Produces:
  - `pub enum Diff { Json(Vec<JsonChange>), Text(Vec<LineChange>), Binary { equal: bool } }`
  - `pub struct JsonChange { pub path: String, pub kind: ChangeKind, pub old: Option<String>, pub new: Option<String> }`
  - `pub enum ChangeKind { Added, Removed, Changed }`
  - `pub struct LineChange { pub tag: LineTag, pub text: String }` with `LineTag { Same, Added, Removed }`
  - `pub fn diff(prev: &[u8], cur: &[u8]) -> Diff`

  All of these derive `Serialize`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn json_key_changes() { let d=diff(br#"{"ENERGY":{"Power":10},"A":1}"#, br#"{"ENERGY":{"Power":12},"B":2}"#);
  let Diff::Json(c)=d else{panic!()}; 
  assert!(c.iter().any(|x| x.path=="ENERGY.Power" && matches!(x.kind,ChangeKind::Changed) && x.old.as_deref()==Some("10") && x.new.as_deref()==Some("12")));
  assert!(c.iter().any(|x| x.path=="A" && matches!(x.kind,ChangeKind::Removed)));
  assert!(c.iter().any(|x| x.path=="B" && matches!(x.kind,ChangeKind::Added))); }
#[test] fn array_index_paths() { let Diff::Json(c)=diff(br#"{"a":[1,2]}"#,br#"{"a":[1,3]}"#) else{panic!()}; assert_eq!(c[0].path,"a[1]"); }
#[test] fn identical_json_has_no_changes() { let Diff::Json(c)=diff(br#"{"a":1}"#,br#"{"a":1}"#) else{panic!()}; assert!(c.is_empty()); }
#[test] fn text_line_diff() { let Diff::Text(l)=diff(b"online",b"offline") else{panic!()}; assert_eq!(l.iter().filter(|x| matches!(x.tag,LineTag::Removed)).count(),1); }
#[test] fn mixed_json_and_text_falls_back_to_text() { assert!(matches!(diff(br#"{"a":1}"#,b"oops"), Diff::Text(_))); }
#[test] fn binary() { assert!(matches!(diff(&[0xff],&[0xff]), Diff::Binary{equal:true})); }
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp-core diff`.** Expected: FAIL to compile.
- [ ] **Step 3: Implement it.** Use a recursive walk over `serde_json::Value` to build JSON diffs, with path segments joined by `.` and `[i]` for array indices. Print leaf values compactly with `to_string()`. For text diffs, use `similar::TextDiff::from_lines` over `pretty_lines` joined by `\n`.
- [ ] **Step 4: Run `cargo test -p mqtt-zapp-core diff`.** Expected: PASS.
- [ ] **Step 5: Commit** with message `feat(core): json and text diff`.

---

### Task 5: Config (profiles, settings) and secrets

**Files:**
- Create: `crates/mqtt-zapp-core/src/config.rs`, `crates/mqtt-zapp-core/src/secrets.rs`
- Modify: `crates/mqtt-zapp-core/Cargo.toml`. Add the `[features] default=["keyring"]` and `keyring=["dep:keyring"]` features, with `keyring = { version="3", optional=true, features=["sync-secret-service","crypto-rust"] }`.
- Test: unit tests in both files, using `tempfile` for `MQTT_ZAPP_CONFIG_DIR`

**Interfaces:**
- Produces:
  - `pub enum Protocol { Mqtt, Mqtts, Ws, Wss }`. It uses serde lowercase names. Its methods:
    - `default_port()`: 1883, 8883, 80 or 443, in that order
    - `is_tls()`
  - `pub struct TlsConfig { pub ca_file: Option<PathBuf>, pub client_cert: Option<PathBuf>, pub client_key: Option<PathBuf>, pub insecure: bool }`. All fields default.
  - `pub enum PasswordSource { None, Keyring, Prompt }`. It uses serde lowercase names and defaults to `None`.
  - `pub struct Profile` with these fields:
    - `pub name: String`
    - `pub host: String`
    - `pub port: u16`
    - `pub protocol: Protocol`
    - `pub ws_path: String`, default `"/"`
    - `pub client_id: Option<String>`
    - `pub username: Option<String>`
    - `pub password: PasswordSource`
    - `pub tls: TlsConfig`
    - `pub subscriptions: Vec<String>`, default `["#","$SYS/#"]`

    It has a method `effective_client_id(&self) -> String`.
  - `pub struct ProfileStore { pub profiles: Vec<Profile> }`. On disk it is `[[profile]]` tables. Its methods:
    - `load() -> Result<Self>`. A missing file means an empty store.
    - `save(&self) -> Result<()>`
    - `get(&self, name) -> Option<&Profile>`
    - `upsert(&mut self, Profile)`
    - `remove(&mut self, name) -> bool`
  - `pub struct Settings { pub history_depth: usize, pub keys: BTreeMap<String, String> }`. `history_depth` defaults to 100, and `keys` is empty by default, meaning the defaults apply. Its method is `load() -> Result<Self>`, and a missing file gives the defaults.
  - `pub fn config_dir() -> PathBuf`. It returns `MQTT_ZAPP_CONFIG_DIR`, or failing that `directories::ProjectDirs::from("", "", "mqtt-zapp").config_dir()`.
  - Secrets:
    - `pub fn store_password(profile: &str, pw: &str) -> Result<()>`. Without the feature it returns `Err("keyring support not compiled in; use password = \"prompt\"")`.
    - `pub fn load_password(profile: &str) -> Result<Option<String>>`

    Both use service `"mqtt-zapp"` and user = the profile name.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn defaults_fill_in() { let p:Profile=toml::from_str(r#"name="home"
host="192.168.22.5"
port=1883
protocol="mqtt""#).unwrap(); assert_eq!(p.subscriptions,vec!["#","$SYS/#"]); assert_eq!(p.ws_path,"/"); assert!(matches!(p.password,PasswordSource::None)); }
#[test] fn store_round_trip_and_never_contains_password() { let d=tempfile::tempdir().unwrap(); std::env::set_var("MQTT_ZAPP_CONFIG_DIR",d.path());
  let mut s=ProfileStore::default(); s.upsert(sample("home")); s.save().unwrap();
  let text=std::fs::read_to_string(d.path().join("profiles.toml")).unwrap();
  for l in text.lines().filter(|l| l.trim_start().starts_with("password")) { assert!(["\"none\"","\"keyring\"","\"prompt\""].iter().any(|v| l.trim_end().ends_with(v)), "{l}"); }
  assert_eq!(ProfileStore::load().unwrap().get("home").unwrap().host,"192.168.22.5"); }
#[test] fn client_id_generated_format() { let id=sample("x").effective_client_id(); assert!(id.starts_with("mqtt-zapp-") && id.len()==16); }
#[test] fn settings_missing_file_defaults() { /* empty tempdir */ assert_eq!(Settings::load().unwrap().history_depth,100); }
#[test] fn upsert_replaces_by_name() { let mut s=ProfileStore::default(); s.upsert(sample("a")); let mut b=sample("a"); b.port=1; s.upsert(b); assert_eq!(s.profiles.len(),1); assert_eq!(s.get("a").unwrap().port,1); }
```

Tests that set the env var must be serialized, with a `static LOCK: Mutex<()>`. `sample(name)` builds a profile for `192.168.22.5:1883` with `username=Some("u")` and `password=PasswordSource::Keyring`. The only key named `password` in the file may hold `none`, `keyring` or `prompt`. Its type has no way to hold a secret.

- [ ] **Step 2: Run `cargo test -p mqtt-zapp-core config`.** Expected: FAIL to compile.
- [ ] **Step 3: Implement both files** to the interfaces above.
- [ ] **Step 4: Run these checks:**
  - `cargo test -p mqtt-zapp-core` passes.
  - `cargo build -p mqtt-zapp-core --no-default-features` succeeds.
  - `cargo tree -p mqtt-zapp-core --no-default-features -i libdbus-sys` prints nothing.
- [ ] **Step 5: Commit** with message `feat(core): profiles, settings and keyring-backed secrets`.

---

### Task 6: MQTT connection + integration tests

**Files:**
- Create: `crates/mqtt-zapp-core/src/client.rs`, `crates/mqtt-zapp-core/tests/broker.rs`, `crates/mqtt-zapp-core/tests/support/mod.rs`
- Test: `tests/broker.rs`

**Interfaces:**
- Consumes: `Profile`, `TlsConfig`, `Protocol` (Task 5) and `Message` (Task 1)
- Produces:
  - `pub enum ConnState { Connecting, Connected, Disconnected { reason: String, retry_in_ms: u64 } }`
  - `pub enum CoreEvent { State(ConnState), Message { topic: String, msg: Message } }`
  - `pub struct Connection`, which is `Clone` and wraps a `rumqttc::AsyncClient` and a cancel token. Its methods:
    - `pub fn connect(profile: &Profile, password: Option<String>) -> Result<(Connection, tokio::sync::mpsc::Receiver<CoreEvent>)>`. This spawns the poll task. The channel capacity is 10_000.
    - `pub async fn publish(&self, topic: &str, payload: Vec<u8>, qos: u8, retain: bool) -> Result<()>`
    - `pub fn disconnect(&self)`
  - `pub(crate) fn mqtt_options(profile: &Profile, password: Option<String>) -> Result<rumqttc::MqttOptions>`

  `ConnState` and `CoreEvent` derive `Serialize`.

The poll loop must behave like this:
- Emit `Connecting`.
- On `ConnAck`, emit `Connected` and subscribe to `profile.subscriptions` at QoS 0.
- On `Publish`, emit `Message`.
- On error, emit `Disconnected{reason: err.to_string(), retry_in_ms}`, sleep for the backoff, and poll again. The backoff is 1 s, doubling up to 30 s, and resets after a `ConnAck`.
- It never panics. It exits when the receiver drops or after `disconnect()`.

TLS rules:
- Use `rumqttc::tokio_rustls::rustls` (the re-export) so the versions match.
- With no `ca_file`, load the system roots via `rustls-native-certs`.
- With a `ca_file`, load that PEM.
- A client cert and key enable client authentication.
- With `insecure=true`, use a `ServerCertVerifier` that accepts any certificate.

WebSocket rules:
- The broker address passed to `MqttOptions::new` is `ws://host:port{ws_path}` or `wss://...`.
- Use `Transport::Ws` or `Transport::Wss(tls_config)`.

- [ ] **Step 1: Write `tests/support/mod.rs`.** It provides `pub struct Broker { pub port: u16, pub ws_port: u16, pub tls_port: u16, pub ca_pem_path: PathBuf, child: Child, dir: TempDir }` and `impl Drop` kills the child. `pub fn start(auth: bool) -> Broker` does the following:
  - picks free ports
  - generates a CA and a `localhost` server cert with `rcgen`
  - writes a `mosquitto.conf` with three listeners: `listener <port>`, `listener <ws_port>` + `protocol websockets`, and `listener <tls_port>` + `cafile/certfile/keyfile`
  - writes `allow_anonymous false` + `password_file` (created with `mosquitto_passwd -b -c`, user `zapp`, pw `zapp-pw`) when `auth`, otherwise `allow_anonymous true`
  - spawns `mosquitto -c` and waits until the TCP port accepts connections, for up to 5 s
  - skips the test (prints `SKIP` and returns early through a `Option<Broker>`) if `mosquitto` isn't on PATH. In CI it is installed, so the tests run there.

- [ ] **Step 2: Write the failing integration tests**

```rust
#[tokio::test] async fn receives_published_message_plain() { /* connect two Connections; A publishes "zapp/t" "hi"; B gets CoreEvent::Message{topic=="zapp/t", msg.payload==b"hi"} within 5s */ }
#[tokio::test] async fn retained_message_delivered_on_subscribe_with_retain_flag() { /* publish retain=true first, then connect; first Message for that topic has msg.retain==true */ }
#[tokio::test] async fn auth_ok_and_bad_password_reports_disconnected() { /* auth broker; good pw → Connected; bad pw → State(Disconnected{reason}) and the receiver stays open (no panic) */ }
#[tokio::test] async fn tls_with_ca_file() { /* Protocol::Mqtts, host "localhost", tls.ca_file = ca_pem_path → Connected */ }
#[tokio::test] async fn tls_insecure_without_ca() { /* insecure=true, no ca → Connected */ }
#[tokio::test] async fn websocket() { /* Protocol::Ws on ws_port → round-trip message */ }
#[tokio::test] async fn reconnects_after_broker_restart() { /* Connected → drop Broker → Disconnected → start new broker on same port → Connected again within 10s */ }
```

Use a helper `async fn wait_for(rx, pred, secs) -> CoreEvent` that panics on timeout.

- [ ] **Step 3: Run `cargo test -p mqtt-zapp-core --test broker`.** Expected: FAIL to compile.
- [ ] **Step 4: Implement `client.rs`** to the interfaces above.
- [ ] **Step 5: Run `cargo test -p mqtt-zapp-core --test broker` and `cargo tree -i aws-lc-sys`.** Expected: all 7 tests pass, and the tree command prints nothing. If rumqttc pulls in aws-lc, set `default-features=false` and select the ring feature of its rustls dependency.
- [ ] **Step 6: Commit** with message `feat(core): mqtt connection with tls, websockets, reconnect`.

---

### Task 7: CLI

**Files:**
- Create: `crates/mqtt-zapp/src/cli.rs`
- Modify: `crates/mqtt-zapp/src/main.rs`
- Test: unit tests in `cli.rs`

**Interfaces:**
- Consumes: `Profile`, `ProfileStore`, `store_password` (Task 5)
- Produces:
  - `#[derive(Parser)] pub struct Args` with `#[command(subcommand)] cmd: Option<Cmd>` plus these options:
    - `--profile <NAME>`
    - `--host`
    - `--port`
    - `--protocol <mqtt|mqtts|ws|wss>`
    - `--user`
    - `--ask-password` (a bool flag)
    - `--ca-file`
    - `--insecure`
  - `pub enum Cmd { Tui, Profile(ProfileCmd) }`. No subcommand means `Tui`.
  - `pub enum ProfileCmd`:
    - `Add` with every connection flag plus `--name` and `--password-mode <none|keyring|prompt>`. With `keyring`, it reads the password through `rpassword` and calls `store_password`.
    - `List`, which prints `name  protocol://host:port  user`
    - `Remove { name }`
  - `pub enum Launch { Picker, Connect(Profile) }`
  - `pub fn resolve_launch(args: &Args, store: &ProfileStore) -> Result<Launch>`. It works like this:
    - `--host` builds an ad-hoc `Profile` named `"adhoc"` with `password=Prompt` if `--ask-password`.
    - `--profile` looks the profile up and errors with `unknown profile 'x'` if it's missing.
    - Otherwise it returns `Picker`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn no_args_is_picker() { assert!(matches!(resolve_launch(&Args::parse_from(["mqtt-zapp"]),&store()).unwrap(),Launch::Picker)); }
#[test] fn profile_flag_selects() { let Launch::Connect(p)=resolve_launch(&Args::parse_from(["mqtt-zapp","--profile","home"]),&store()).unwrap() else{panic!()}; assert_eq!(p.host,"192.168.22.5"); }
#[test] fn unknown_profile_errors() { assert!(resolve_launch(&Args::parse_from(["mqtt-zapp","--profile","nope"]),&store()).unwrap_err().to_string().contains("unknown profile 'nope'")); }
#[test] fn adhoc_wss_defaults_port() { let Launch::Connect(p)=resolve_launch(&Args::parse_from(["mqtt-zapp","--host","b.example","--protocol","wss","--user","u","--ask-password"]),&store()).unwrap() else{panic!()}; assert_eq!(p.port,443); assert!(matches!(p.password,PasswordSource::Prompt)); }
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp cli`.** Expected: FAIL to compile.
- [ ] **Step 3: Implement `cli.rs` and dispatch it from `main.rs`.** `Tui` calls `tui::run(launch)`, which is a stub returning `Ok(())` until Task 8.
- [ ] **Step 4: Run `cargo test -p mqtt-zapp cli`.** Expected: PASS. Also run `cargo run -- profile add --name t --host localhost && cargo run -- profile list` with a temporary `MQTT_ZAPP_CONFIG_DIR`. Expected: it prints `t  mqtt://localhost:1883`.
- [ ] **Step 5: Commit** with message `feat(cli): profiles subcommands and launch resolution`.

---

### Task 8: TUI shell: terminal lifecycle, event loop, keymap, tree pane

**Files:**
- Create: `crates/mqtt-zapp/src/tui/{mod.rs,keymap.rs,app.rs,rows.rs,ui.rs}`
- Test: unit tests in `keymap.rs`, `rows.rs` and `app.rs`, and render tests with `ratatui::backend::TestBackend`

**Interfaces:**
- Consumes: `TopicTree`, `NodeId`, `Filter` (Tasks 2–3), `Connection`, `CoreEvent`, `ConnState` (Task 6), `Settings` (Task 5), `Launch` (Task 7)
- Produces:
  - `pub enum Action` with these variants:
    - `Up`, `Down`
    - `Collapse`, `Expand`, `ToggleExpand`
    - `Search`
    - `Publish`
    - `FocusNext`
    - `Select`
    - `ToggleDiff`
    - `Help`
    - `Quit`
    - `Cancel`
    - `Top`, `Bottom`
  - `pub struct KeySpec(KeyCode, KeyModifiers)` with `KeySpec::parse("ctrl-s" | "j" | "G" | "esc" | "enter" | "tab" | "up") -> Result<KeySpec>`
  - `pub struct Keymap`:
    - `Keymap::from_settings(&BTreeMap<String,String>) -> Result<Keymap>`. Overrides are keyed by the snake_case action name, for example `toggle_diff = "D"`. An unknown action name is an error.
    - `Keymap::lookup(&self, KeyEvent) -> Option<Action>`
  - Default bindings:

    | Key | Action |
    |-----|--------|
    | `j`/`down` | Down |
    | `k`/`up` | Up |
    | `h`/`left` | Collapse |
    | `l`/`right` | Expand |
    | `enter` | Select |
    | `space` | ToggleExpand |
    | `/` | Search |
    | `p` | Publish |
    | `tab` | FocusNext |
    | `d` | ToggleDiff |
    | `?` | Help |
    | `q` | Quit |
    | `esc` | Cancel |
    | `g` | Top |
    | `G` | Bottom |

  - `pub struct Row { pub id: NodeId, pub depth: u16, pub expanded: bool, pub has_children: bool }`
  - `pub fn visible_rows(tree: &TopicTree, expanded: &HashSet<NodeId>, filter: Option<&Filter>) -> Vec<Row>`. This is a depth-first walk in BTreeMap order that skips ROOT. With a filter, it shows only `filter.visible` and treats every visible node as expanded.
  - `pub enum Mode { Picker, Password, Browse, SearchInput, Publish, Help }`
  - `pub struct App` with these fields:
    - `tree`
    - `expanded`
    - `selected: Option<NodeId>`
    - `filter: Option<Filter>`
    - `search: String`
    - `mode`
    - `conn_state: ConnState`
    - `dirty: bool`
    - `msg_rate`, a 10 s sliding window
  - App methods:
    - `apply_core_event(&mut self, CoreEvent)`
    - `handle_action(&mut self, Action) -> Effect`
    - `pub enum Effect { None, Quit, Publish{topic,payload,qos,retain}, Connect(Profile, Option<String>) }`
  - `pub fn run(launch: Launch) -> anyhow::Result<()>` in `tui/mod.rs`

The event loop is a `tokio::select!` over three sources:
- the crossterm `EventStream`
- `rx.recv()`. After the first event, drain with `try_recv` up to 2000 more events.
- a 100 ms interval tick

On each tick, render if `dirty`, then rebuild the filter with `search::filter` if a filter is active. Before entering raw mode, install a panic hook that restores the terminal: `disable_raw_mode`, `LeaveAlternateScreen`, and show the cursor. A terminal smaller than 40×10 renders the text `terminal too small`.

The tree row text is `▸`/`▾` + name (`∅` for an empty name) + ` = ` + `preview(latest, 60)` + ` (msg_count)`. A node updated within the last 1 s is drawn with a reversed or highlighted style, which is the flash.

- [ ] **Step 1: Write the failing tests**

```rust
// keymap.rs
#[test] fn defaults() { let k=Keymap::from_settings(&Default::default()).unwrap(); assert_eq!(k.lookup(key('j')),Some(Action::Down)); assert_eq!(k.lookup(key('/')),Some(Action::Search)); }
#[test] fn override_rebinds() { let k=Keymap::from_settings(&[("toggle_diff".into(),"D".into())].into()).unwrap(); assert_eq!(k.lookup(key('D')),Some(Action::ToggleDiff)); assert_eq!(k.lookup(key('d')),None); }
#[test] fn unknown_action_errors() { assert!(Keymap::from_settings(&[("nope".into(),"x".into())].into()).is_err()); }
#[test] fn parse_ctrl() { assert_eq!(KeySpec::parse("ctrl-s").unwrap(),KeySpec(KeyCode::Char('s'),KeyModifiers::CONTROL)); }
// rows.rs
#[test] fn collapsed_hides_children() { /* tree a/b, a/c; expanded={} → rows [a]; expanded={a} → [a,b,c] with depth 0,1,1 */ }
#[test] fn filter_forces_expand_and_hides_others() { /* a/wzr, b/c; filter "wzr" → rows [a, wzr] */ }
// app.rs
#[test] fn burst_of_retained_messages_keeps_selection_stable() { /* select "a"; apply 2000 CoreEvent::Message for other topics; selected still == id of "a" */ }
#[test] fn down_up_moves_selection_over_visible_rows() {}
// ui.rs
#[test] fn too_small_terminal_renders_notice() { /* TestBackend 30x8 → buffer contains "terminal too small" */ }
#[test] fn escape_sequence_in_payload_is_not_rendered_raw() { /* insert payload "\x1b[2J"; render 80x24; no cell symbol contains '\x1b' */ }
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp tui`.** Expected: FAIL to compile.
- [ ] **Step 3: Implement `keymap.rs`, `rows.rs`, `app.rs` (Browse mode, tree navigation, `apply_core_event`), `ui.rs`, and `mod.rs`.** The layout is:
  - the tree on the left, 50% wide
  - the right pane as a placeholder for Task 9
  - a status bar at the bottom
- [ ] **Step 4: Run `cargo test -p mqtt-zapp`.** Expected: PASS. Then run `cargo run -- --host 192.168.22.5` manually. Expected:
  - the tree fills up
  - `j`/`k`/`l` work
  - `q` restores the terminal
  - forcing a panic, for example with a temporary `panic!` behind an env var, also restores the terminal
- [ ] **Step 5: Commit** with message `feat(tui): terminal lifecycle, keymap, live topic tree`.

---

### Task 9: Message pane: payload, metadata, history, diff

**Files:**
- Create: `crates/mqtt-zapp/src/tui/message_view.rs`
- Modify: `crates/mqtt-zapp/src/tui/{app.rs,ui.rs}`
- Test: `TestBackend` render tests in `message_view.rs`

**Interfaces:**
- Consumes: `pretty_lines`, `detect`, `PayloadKind` (Task 1), `diff`, `Diff` (Task 4), `App` (Task 8)
- Produces:
  - App fields:
    - `focus: Focus { Tree, History }`
    - `history_sel: Option<usize>`. This is an index into `node.history`, where `None` means the latest.
    - `show_diff: bool`
    - `payload_scroll: u16`
  - `pub fn render_message(f: &mut Frame, area: Rect, app: &App)`

The right pane is split vertically:
- The metadata line: `topic · qos N · retained? · <size> B · HH:MM:SS.mmm` (local time, via `chrono`, which you may add).
- The payload. Pretty lines are scrollable, and JSON gets syntax colours from a simple tokenizer: keys cyan, strings green, numbers yellow, bool/null magenta. With `show_diff`, the payload area instead shows `diff(prev, cur)`, where `prev` is the history entry before the selected one. JSON changes appear as `~ path: old → new`, `+ path: new` and `- path: old`, coloured yellow/green/red. Text diffs use `+`/`-` line prefixes. If there is no previous message, show `no previous message`.
- The history list, newest first: `HH:MM:SS  preview(40)`.

Behaviour:
- `tab` moves focus between the tree and the history list.
- `j`/`k` in History moves `history_sel`.
- When `history_sel` is `None`, new messages update the view live. When an entry is selected, the view stays frozen on it.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn shows_multiline_pretty_json_as_multiple_lines() { /* payload "{\n  \"UPSNAME\": \"apc-ups\",\n  \"WATTS\": 120\n}"; render 100x30; rows contain "\"UPSNAME\": \"apc-ups\"" and "\"WATTS\": 120" on separate lines */ }
#[test] fn binary_shows_hex() { /* payload [0xff,0x00,0x41] → buffer contains "ff 00 41" */ }
#[test] fn diff_shows_changed_key() { /* two msgs {"ENERGY":{"Power":10}} then {"ENERGY":{"Power":12}}; show_diff=true → buffer contains "ENERGY.Power: 10 → 12" */ }
#[test] fn diff_without_previous() { /* one msg, show_diff → "no previous message" */ }
#[test] fn history_selection_freezes_view() { /* select history[0] (older), apply new message → payload area still shows older payload */ }
#[test] fn metadata_marks_retained() { /* retain=true → buffer contains "retained" */ }
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp message_view`.** Expected: FAIL.
- [ ] **Step 3: Implement `message_view.rs`** and wire `Focus`, `ToggleDiff` and `FocusNext` into `App::handle_action`.
- [ ] **Step 4: Run `cargo test -p mqtt-zapp`.** Expected: PASS.
- [ ] **Step 5: Commit** with message `feat(tui): message view with history and diff`.

---

### Task 10: Live search mode

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/{app.rs,ui.rs,rows.rs}`
- Test: tests in `app.rs` and `ui.rs`

**Interfaces:**
- Consumes: `search::filter`, `match_ranges` (Task 3), `visible_rows` (Task 8)
- Produces: `App::on_search_key(&mut self, KeyEvent)`.
  - `Search` enters `Mode::SearchInput` and shows `/query▏` in the status bar.
  - Every character or backspace rebuilds `app.filter` immediately.
  - `enter` returns to Browse and keeps the filter.
  - `esc` clears the query and the filter.
  - After a rebuild, if the selection isn't visible, select the first match.

  Matching substrings in tree rows get a bold, underlined style via `match_ranges`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn typing_filters_each_keystroke() { /* topics muh/esp/esp32-WZR1/LWT, tasmota/tele/x/STATE; press '/', 'w','z' → filter Some, rows include esp32-WZR1; press 'r' → still */ }
#[test] fn payload_match_shows_topic() { /* payload {"DeviceName":"wzr-lamp"} under tasmota/.../STATE → visible after typing wzr */ }
#[test] fn new_matching_topic_appears_while_filtered() { /* filter "wzr" active; apply Message for "new/wzr2"; tick → rows include wzr2 */ }
#[test] fn esc_clears_filter() {}
#[test] fn selection_jumps_to_first_match_when_hidden() {}
```

The tick handler from Task 8 rebuilds the filter. In tests, call `app.on_tick()`, which you extract in this task if Task 8 inlined it.

- [ ] **Step 2: Run `cargo test -p mqtt-zapp search`.** Expected: FAIL.
- [ ] **Step 3: Implement it** as described above.
- [ ] **Step 4: Run `cargo test -p mqtt-zapp`.** Expected: PASS.
- [ ] **Step 5: Commit** with message `feat(tui): live plain-text search`.

---

### Task 11: Connection picker, password prompt, status bar

**Files:**
- Create: `crates/mqtt-zapp/src/tui/picker.rs`
- Modify: `crates/mqtt-zapp/src/tui/{app.rs,mod.rs,ui.rs}`
- Test: tests in `picker.rs` and `app.rs`

**Interfaces:**
- Consumes: `ProfileStore`, `PasswordSource`, `load_password` (Task 5), `Connection::connect` (Task 6), `Launch` (Task 7), `Effect::Connect` (Task 8)
- Produces:
  - `pub struct PickerState { pub profiles: Vec<Profile>, pub sel: usize }`
  - `pub struct PasswordState { pub profile: Profile, pub input: String }`. Its render masks the input with `•`.
  - `App::new(launch: Launch, store: ProfileStore, settings: Settings) -> App`. `Launch::Picker` starts in `Mode::Picker`, and `Launch::Connect` resolves the password at once.

Password resolution:
- `PasswordSource::None` connects without a password.
- `Keyring` calls `load_password`. On `Err` or `None`, it falls back to the prompt.
- `Prompt` goes to `Mode::Password`. Enter returns `Effect::Connect`.

`mod.rs` handles `Effect::Connect` like this: it drops any old `Connection`, creates the new one, and replaces `rx` and the tree. The status bar shows:
- `●` connected (green), `◌` connecting (yellow), `✕ <reason> – retry in Ns` (red)
- the profile name
- `msg/s`, `N topics`
- the mode hint, for example `/ search  p publish  ? help`

An empty picker shows: `no profiles – add one with: mqtt-zapp profile add --name … --host …`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn picker_enter_on_none_password_connects() { /* store with profile password=None; Select → Effect::Connect(p, None) */ }
#[test] fn picker_prompt_goes_to_password_mode_then_connects() { /* password=Prompt; Select → Mode::Password; type "pw", enter → Effect::Connect(p, Some("pw")) */ }
#[test] fn password_render_is_masked() { /* buffer contains "••" and not "pw" */ }
#[test] fn disconnected_state_in_status_bar() { /* apply State(Disconnected{reason:"connection refused",retry_in_ms:4000}) → buffer contains "connection refused" and "retry in 4s" */ }
#[test] fn empty_picker_hint() {}
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp picker`.** Expected: FAIL.
- [ ] **Step 3: Implement it** as described above.
- [ ] **Step 4: Run `cargo test -p mqtt-zapp`.** Expected: PASS. Manual check: `mqtt-zapp profile add --name home --host 192.168.22.5`, then `mqtt-zapp`, pick `home`, and the tree fills up. Stop the network or set the wrong port, and the red status shows retries.
- [ ] **Step 5: Commit** with message `feat(tui): connection picker, password prompt, status bar`.

---

### Task 12: Publish form

**Files:**
- Create: `crates/mqtt-zapp/src/tui/publish.rs`
- Modify: `crates/mqtt-zapp/src/tui/{app.rs,mod.rs,ui.rs}`
- Test: tests in `publish.rs`

**Interfaces:**
- Consumes: `Effect::Publish` (Task 8), `Connection::publish` (Task 6)
- Produces:
  - `pub struct PublishForm { pub topic: tui_textarea::TextArea<'static>, pub payload: TextArea<'static>, pub qos: u8, pub retain: bool, pub field: Field }` with `Field { Topic, Payload, Qos, Retain }`
  - `PublishForm::new(prefill_topic: &str)`
  - `PublishForm::on_key(&mut self, KeyEvent) -> FormOutcome` with `FormOutcome { Continue, Cancel, Send{topic,payload,qos,retain} }`
  - `PublishForm::format_json(&mut self) -> Result<(), String>`

Behaviour:
- `p` in Browse opens the form as a bottom overlay, about 40% of the height. The topic is pre-filled with `full_topic(selected)`.
- Keys inside the form:
  - `tab`/`shift-tab` change the field
  - in Qos, `0`/`1`/`2` set the QoS
  - in Retain, `space` toggles it
  - `ctrl-s` sends
  - `ctrl-f` pretty-formats the payload JSON, or shows the parse error in the form's title
  - `esc` cancels
- An empty topic, or a topic containing `+` or `#`, blocks sending with the inline error `invalid topic`.

  The overlay stays open after a send, so the user can test repeatedly. A send emits `Effect::Publish`. `mod.rs` spawns `conn.publish(...)`, and any error shows in the status bar.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn prefilled_topic() { assert_eq!(PublishForm::new("tasmota/cmnd/x/POWER").topic.lines()[0],"tasmota/cmnd/x/POWER"); }
#[test] fn ctrl_s_sends_multiline_payload() { /* type "{\n\"a\":1}" into payload; ctrl-s → Send{payload == b"{\n\"a\":1}", qos 0, retain false} */ }
#[test] fn qos_and_retain_fields() { /* tab to Qos, press '1'; tab to Retain, space → Send{qos:1,retain:true} */ }
#[test] fn wildcard_topic_rejected() { /* topic "a/#" → ctrl-s returns Continue and error "invalid topic" */ }
#[test] fn format_json_pretty_prints_or_errors() { /* "{\"a\":1}" → lines ["{","  \"a\": 1","}"]; "{oops" → Err(..) */ }
#[test] fn esc_cancels() {}
```

- [ ] **Step 2: Run `cargo test -p mqtt-zapp publish`.** Expected: FAIL.
- [ ] **Step 3: Implement it** as described above.
- [ ] **Step 4: Run `cargo test -p mqtt-zapp`.** Expected: PASS. Manual check against a local `mosquitto` instance, **not** the user's broker, to avoid toggling real devices. Publish `zapp/test` `{"x":1}` with retain, and it appears in the tree with the retained mark.
- [ ] **Step 5: Commit** with message `feat(tui): publish form`.

---

### Task 13: Help overlay, release build, README

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/ui.rs`, which gets the help overlay listing the effective keymap
- Create: `.github/workflows/release.yml`, `README.md`
- Test: a help render test, plus the static-binary check

**Interfaces:**
- Consumes: `Keymap` (Task 8)
- Produces:
  - `Keymap::bindings(&self) -> Vec<(Action, Vec<KeySpec>)>`, used by the help overlay
  - `impl Display for KeySpec`

- [ ] **Step 1: Write the failing test.** Remap `quit` to `"x"`. The help overlay buffer should contain `x` next to `quit` and should not show `q` for quit.
- [ ] **Step 2: Run `cargo test -p mqtt-zapp help`.** Expected: FAIL.
- [ ] **Step 3: Implement the help overlay.** Then write `release.yml`, which triggers on `push: tags: ['v*']` and does the following:
  - runs `rustup target add x86_64-unknown-linux-musl`
  - installs `musl-tools`
  - runs `cargo build --release --target x86_64-unknown-linux-musl -p mqtt-zapp --no-default-features`
  - uploads `mqtt-zapp-${tag}-x86_64-linux-musl` to a GitHub release with `gh release create`

  Then write the README. It covers:
  - installation by `scp`-ing the binary
  - `profile add` examples for mqtt/mqtts/wss with a CA file
  - the `settings.toml` `[keys]` example
  - the key table
  - a note that passwords live in the keyring or are prompted for
- [ ] **Step 4: Verify.**
  - `cargo test --workspace` passes.
  - Run `cargo build --release --target x86_64-unknown-linux-musl -p mqtt-zapp --no-default-features && file target/x86_64-unknown-linux-musl/release/mqtt-zapp`. Expected: `statically linked`. Install the target and `musl` locally with `pacman -S musl` if they're missing.
  - End to end against `192.168.22.5`, read-only:
    - connect
    - type `/` then `tasmota_0C6423`, and the tree narrows
    - select `.../SENSOR`, and the JSON updates live
    - press `d`, and the diff shows the changed keys
    - press `q`, and the terminal is clean
- [ ] **Step 5: Commit** with message `feat: help overlay, release workflow, README`. Push `main`. CI must be green.

---

## Self-review notes

Spec coverage for each [core] item:

| Spec item | Task |
|-----------|------|
| §4.1 profiles | Tasks 5 and 7 |
| §4.1 TLS | Task 6 |
| §4.1 picker and `--profile` | Tasks 7 and 11 |
| §4.1 reconnect | Task 6 |
| §4.1 keyring/prompt | Tasks 5 and 11 |
| §4.2 live tree, counts, flash | Task 8 |
| §4.3 search | Tasks 3 and 10 |
| §4.4 metadata, formatting, hex, history | Tasks 1 and 9 |
| §4.4 diff to previous | Tasks 4 and 9 |
| §4.6 publish | Task 12 |
| §6 tests and CI | Tasks 1, 6 and 13 |
| §6 musl, ring, keyring feature | Tasks 5, 6 and 13 |
| §6 remappable keys | Tasks 8 and 13 |
| §6 Serialize hedge | Tasks 1, 2, 4 and 6 |

Deliberately excluded because they are non-core (Phase 1.x):
- yank/copy (OSC 52)
- delete retained
- sort options
- charts
- Sparkplug
- `$SYS` view
- settings beyond `history_depth` and `keys`
- MQTT 5
- per-topic publish history
- `$EDITOR`
- profile editing inside the TUI. The CLI `profile add` covers creating profiles for now.

Deviation from the spec: the integration tests **spawn** mosquitto (installed with apt in CI) instead of using a GitHub service container, because the TLS and WebSocket listeners need generated config and certs. Spawning also works the same way locally.
