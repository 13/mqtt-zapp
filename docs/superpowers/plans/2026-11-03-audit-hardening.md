# Audit Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the 2026-11-03 audit findings (points 1–9): offline-replay and confirm safety, connection backpressure, config/clipboard/TLS hardening, clean shutdown, memory ceilings, tree/search speed, and CI supply chain.

**Architecture:** The core crate (`mqtt-zapp-core`) gets the protocol-level fixes. These are a non-blocking event emit with a dropped counter, discarding pending requests on disconnect, `Bytes` payloads with a size cap, a precomputed natural sort key, a faster search matcher and atomic private file writes. The TUI crate gets the UI-level fixes. These are the connected-only guard on destructive effects, a `y`-only confirm, a cached row list, sanitized clipboard copies, header warnings, signal handling and an awaited disconnect. CI gets cargo-deny, SHA-pinned actions and an optimized release profile.

**Tech Stack:** Rust 2024, tokio, rumqttc 0.24 (0.25 in Task 13), ratatui 0.29, crossterm 0.28, `bytes` 1, `regex` 1, cargo-deny.

**Spec:** The audit report given in-session on 2026-11-03, summarized per task below. Read it with `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md`.

**Scale note (read first):** the design spec (§2) sets the scale at *hundreds to low thousands of topics*. The audit measured the tree and search costs at 50k–150k topics. At the spec's scale they cost only a couple of milliseconds. So Tasks 1–8 (correctness, safety, security) come first. Tasks 9–11 (speed) are headroom beyond the spec and are kept lean on purpose: there is no incremental search index.

## Global Constraints

- Edition 2024. No `unsafe` in non-test code.
- The musl build (`--no-default-features`, no keyring) must still build static. Do not add dependencies with C code or that need system libraries.
- Every string from the broker shown on screen goes through `payload::sanitize`, `one_line` or `single_line`. The new clipboard path goes through `payload::clipboard_safe` (Task 4).
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test --workspace` pass after every task.
- Note and badge copy is lowercase and terse, like the existing notes ("cleared 40 retained", "copied topic").
- Commits use conventional prefixes (`fix(core):`, `feat(tui):`, `ci:`, `perf(core):`), as in the git log.
- Settings are new fields on `config::Settings`, with a default in `impl Default`. Older `settings.toml` files without the field must still parse (check that `#[serde(default)]` covers the struct).

## Review Focus

1. **A stale row list after deleting a topic.** `TopicTree::remove` must bump the shape generation, or the cached rows point at removed nodes. Test in Task 10: `rows_drop_removed_topic`.
2. **Count/Recent sort with cached rows.** Rows may be up to 1 s stale, but `j`/`k` must still move from the *selected id*, not from a stale index. Test in Task 10: `count_sort_selection_follows_id_across_rebuild`.
3. **A truncated JSON payload.** It must show as text with a `… truncated (N bytes)` marker. It must not be reported as a parse error, and the chart and diff must not panic. Test in Task 7: `truncated_json_renders_as_text_with_marker`.
4. **The dropped counter across a profile switch.** `reset_tree` must reset `dropped` to 0, so the new broker's header does not inherit it. Test in Task 2: `reset_tree_clears_dropped`.
5. **A clipboard copy of a payload with a multi-byte character at the size cap.** The cut must land on a char boundary. Test in Task 4: `clipboard_safe_cuts_on_char_boundary`.

---

### Task 1: Offline safety: no replayed clears, `y`-only confirm (audit point 3)

**Files:**
- Modify: `crates/mqtt-zapp-core/src/client.rs` (`Loop`, `run`)
- Modify: `crates/mqtt-zapp/src/tui/mod.rs` (`Session::apply`, `ClearRetained` and `Publish` arms)
- Modify: `crates/mqtt-zapp/src/tui/app.rs:536-556` (`on_confirm_key`)
- Modify: `crates/mqtt-zapp/src/tui/ui.rs:203` (confirm hint copy, if needed)
- Test: `crates/mqtt-zapp/src/tui/app/tests.rs`, `crates/mqtt-zapp-core/tests/broker.rs`

**Interfaces:**
- Produces: `Loop::drop_pending(&mut self) -> usize` (private to client.rs). `pub fn ConnState::is_connected(&self) -> bool`.

- [ ] **Step 1: Write the failing app tests**

```rust
#[test]
fn enter_does_not_confirm_a_destructive_prompt() {
    let mut a = browse(&["a/b"]);
    a.selected = a.tree.find("a/b");
    a.handle_action(Action::DeleteRetained);
    assert!(matches!(a.handle_key(key(KeyCode::Enter)), Effect::None));
    assert_eq!(a.note_at(Instant::now()), Some(&Note::info("cancelled")));
}
```
Also update `delete_retained_needs_confirmation` (tests.rs:96-100), which currently expects Enter to confirm. Change it to `y`.

- [ ] **Step 2: Write the failing broker test**

Add `clear_while_broker_down_is_not_replayed_after_reconnect` to `broker.rs`. Follow the pattern of `reconnects_after_broker_restart`:
- publish a retained `x/keep`, then stop the broker;
- call `conn.clear_retained(&["x/keep".into()])` and expect it to time out or fail (5 s timeout);
- restart the broker on the same ports, then wait for Connected;
- connect a second client and assert it still receives the retained `x/keep`.

- [ ] **Step 3: Run the tests and check that they fail**

Run: `cargo test -p mqtt-zapp enter_does_not_confirm && cargo test -p mqtt-zapp-core --test broker clear_while_broker_down`
Expected: both FAIL. Enter returns `ClearRetained`, and the retained message is gone.

- [ ] **Step 4: Implement**
- `on_confirm_key`: only `KeyCode::Char('y')` confirms. Update the doc comment.
- `Loop::drop_pending`: clear `eventloop.pending` (a pub `VecDeque<Request>` on both the v4 and v5 `EventLoop`) and return how many requests it dropped.
- In `run`, on `Polled::Err(reason)`, call it. When n > 0, add ` · dropped {n} unsent` to the reason. Re-subscribing still happens on ConnAck, so dropping a pending subscribe is safe.
- `Session::apply`: for `ClearRetained` and `Publish`, when `!app.conn_state.is_connected()`, set `Note::error("clear failed: not connected")` or `Note::error("publish failed: not connected")`, the same text as the existing `None` branch.

- [ ] **Step 5: Run the tests and check that they pass**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 6: Commit** `fix: never replay clears/publishes queued while disconnected; only y confirms`

---

### Task 2: Keep polling under UI backpressure (audit point 2, and the editor-blocking finding)

**Files:**
- Modify: `crates/mqtt-zapp-core/src/client.rs` (`CoreEvent`, `run`)
- Modify: `crates/mqtt-zapp/src/tui/app.rs` (`App.dropped`, `apply_core_event`, `reset_tree`)
- Modify: `crates/mqtt-zapp/src/tui/ui.rs` (`header_line`)
- Test: `crates/mqtt-zapp-core/tests/broker.rs`, `crates/mqtt-zapp/src/tui/app/tests.rs`, `crates/mqtt-zapp/src/tui/ui/tests.rs`

**Interfaces:**
- Produces: `CoreEvent::Dropped(u64)`, the number of messages dropped since the last `Dropped` event. `pub dropped: u64` on `App`.

- [ ] **Step 1: Write the failing tests**
- broker.rs: `slow_consumer_stays_connected`. Connect with a keepalive of 2 s (set it on the test profile) and do not read `rx`. Publish 20_000 QoS 0 messages from a second client, then sleep 6 s. Then drain `rx` and assert:
  - no `Disconnected` event arrived after the first `Connected`;
  - at least one `Dropped(n)` with n > 0 arrived.
- app tests: `dropped_events_accumulate` (`apply_core_event(Dropped(3))` twice gives `a.dropped == 6`) and `reset_tree_clears_dropped`.
- ui tests: `header_shows_dropped_badge` (with `dropped = 42`, the header contains `42 dropped`).

- [ ] **Step 2: Run them and check that they fail**

Run: `cargo test --workspace dropped slow_consumer`
Expected: FAIL. There is no `Dropped` variant yet.

- [ ] **Step 3: Implement**

In `run`, `Polled::Publish` no longer uses `emit!`:

```rust
Polled::Publish { topic, msg } => {
    if dropped > 0 && tx.try_send(CoreEvent::Dropped(dropped)).is_ok() {
        dropped = 0;
    }
    match tx.try_send(CoreEvent::Message { topic, msg }) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => dropped += 1,
        Err(mpsc::error::TrySendError::Closed(_)) => return,
    }
}
```

`State` events keep `emit!`, since they are rare and must not be lost. `apply_core_event` adds to `dropped`, and `reset_tree` zeroes it. `header_line` appends ` · ⚠ {n} dropped` in `theme::WARN` after the connection pill when `dropped > 0`. It is the first part cut when the header is narrow.

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `fix(core): never block the poll loop on a full UI channel; count drops`

---

### Task 3: Atomic, private config writes (audit point 6)

**Files:**
- Modify: `crates/mqtt-zapp-core/src/config.rs:223-228` (`ProfileStore::save`), plus a new fn
- Modify: `crates/mqtt-zapp-core/src/state.rs:75-83` (`AppState::save`)
- Test: `config.rs` tests module

**Interfaces:**
- Produces: `pub fn write_private(path: &Path, contents: &str) -> anyhow::Result<()>` in `config.rs`.

- [ ] **Step 1: Write the failing tests**
- `write_private_is_0600_and_replaces`: write twice, then assert the contents are the second write and `metadata.permissions().mode() & 0o777 == 0o600`.
- `write_private_leaves_no_temp_files`: after the write, the directory holds only the target file.
- `profiles_saved_private`: `ProfileStore::save` in a temp `MQTT_ZAPP_CONFIG_DIR` produces a file with mode 0600. Reuse that module's existing env lock or temp-dir helper.

- [ ] **Step 2: Run them and check that they fail**

Run: `cargo test -p mqtt-zapp-core write_private profiles_saved_private`
Expected: FAIL. `write_private` is undefined.

- [ ] **Step 3: Implement `write_private`**
- The temp file is `.{file_name}.{pid}.{rand u32}.tmp` in the same directory.
- Open it with `OpenOptions::new().write(true).create_new(true).mode(0o600)` (`std::os::unix::fs::OpenOptionsExt`, under `#[cfg(unix)]`).
- `write_all`, then `sync_all`, then `rename`. On any error, remove the temp file.
- `ProfileStore::save` and `AppState::save` call it. After `create_dir_all`, set the config dir to 0700 with `set_permissions`, but only when the dir was just created.

- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `fix(core): write profiles and state atomically with 0600`

---

### Task 4: Safe clipboard and invisible-character sanitizing (audit point 6)

**Files:**
- Modify: `crates/mqtt-zapp-core/src/payload.rs:37-48` (`sanitize`), plus a new fn
- Modify: `crates/mqtt-zapp/src/tui/app.rs:1043-1060` (`copy`), `crates/mqtt-zapp/src/tui/app/mouse.rs:~158` (the topic copy)
- Test: `payload.rs` tests, `app/tests.rs`

**Interfaces:**
- Produces:
  - `pub const CLIPBOARD_MAX: usize = 1 << 20;`
  - `pub fn clipboard_safe(s: &str, max: usize) -> String`: keeps `\n` and `\t`, drops other controls and invisible format characters, and cuts at a char boundary at or below `max` bytes.
  - `fn is_invisible_format(c: char) -> bool` (private): true for U+00AD, U+061C, U+200B, U+200C, U+200E, U+200F, U+202A–U+202E, U+2060–U+2064, U+2066–U+2069 and U+FEFF. **Not U+200D (ZWJ): emoji sequences need it.**

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn sanitize_replaces_bidi_and_zero_width() {
    assert_eq!(sanitize("x/\u{202E}evil\u{200B}"), "x/\u{fffd}evil\u{fffd}");
    assert_eq!(sanitize("👩\u{200D}💻"), "👩\u{200D}💻");
}
#[test]
fn clipboard_safe_strips_escapes_keeps_newlines() {
    assert_eq!(clipboard_safe("a\x1b[201~\nb\tc\u{202E}", 100), "a[201~\nb\tc");
}
#[test]
fn clipboard_safe_cuts_on_char_boundary() {
    assert_eq!(clipboard_safe("aé", 2), "a");
}
```
App test `copied_payload_is_sanitized`: a payload `"ok\x1b]52;c;x\x07"` copied with `Action::CopyPayload` gives text `"ok]52;c;x"`.

- [ ] **Step 2: Run them and check that they fail.** Run: `cargo test --workspace sanitize clipboard copied_payload`. Expected: FAIL.
- [ ] **Step 3: Implement.** `sanitize` replaces `is_invisible_format` characters with U+FFFD. `copy` (both branches) and the mouse topic copy pass their text through `clipboard_safe(_, CLIPBOARD_MAX)`.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS. Existing sanitize tests stay green.
- [ ] **Step 5: Commit** `fix: sanitize clipboard copies and hide bidi/zero-width characters`

---

### Task 5: Warn about unverified TLS and cleartext passwords (audit point 6)

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/app.rs:183` (`ServerInfo`), `crates/mqtt-zapp/src/tui/app/front.rs:81` (construction)
- Modify: `crates/mqtt-zapp/src/tui/ui.rs:800+` (`header_line`)
- Modify: `crates/mqtt-zapp/src/cli.rs` / `main.rs` (stderr warning)
- Test: `ui/tests.rs`, `cli.rs` tests

**Interfaces:**
- Produces: `ServerInfo { .., pub warning: Option<&'static str> }`, with `pub fn security_warning(p: &Profile) -> Option<&'static str>` in `front.rs`. It returns:
  - `"unverified tls"` when `p.tls.insecure` and the transport is TLS (mqtts or wss);
  - `"password in cleartext"` when the transport is plain mqtt or ws and `p.username.is_some()` and a password source is set;
  - otherwise `None`.

- [ ] **Step 1: Write the failing tests**
- unit tests of `security_warning`, one per case;
- ui test `header_shows_security_warning` (`⚠ unverified tls` appears, styled with `theme::ERR`);
- cli test `insecure_flag_warns`: the function that builds the ad-hoc profile returns the warning text, so the test needs no captured stderr.

- [ ] **Step 2: Run them and check that they fail.** Run: `cargo test -p mqtt-zapp security_warning insecure_flag`. Expected: FAIL.
- [ ] **Step 3: Implement.** The header shows `⚠ {warning}` right after the URL. It is never cut before the URL. `main.rs` prints `warning: --insecure accepts any certificate` to stderr before entering the TUI.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tui): warn on unverified TLS and cleartext passwords`

---

### Task 6: SIGTERM/SIGHUP and a DISCONNECT that reaches the broker (audit point 7)

**Files:**
- Modify: `crates/mqtt-zapp/Cargo.toml` (tokio feature `signal`)
- Modify: `crates/mqtt-zapp/src/tui/mod.rs` (`run`, `event_loop`, `Session::disconnect`)
- Modify: `crates/mqtt-zapp-core/src/client.rs` (`Connection`, `connect`)
- Test: `crates/mqtt-zapp-core/tests/broker.rs`

**Interfaces:**
- Produces: `Connection` gains the field `task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>`. Add `pub async fn close(&self, wait: Duration)`: it runs `disconnect()`, then takes the handle and awaits it under `tokio::time::timeout(wait, ..)`.
- `Session::close(&mut self)` (async) uses `conn.close(Duration::from_secs(2))`. Profile switches keep the synchronous `disconnect()`.

- [ ] **Step 1: Write the failing broker test**

`close_sends_disconnect_so_no_will_fires`:
- client A connects with a last-will on `x/will`, configured on the test profile if `Profile` supports a will; otherwise call `mqtt_options` directly;
- client B subscribes to `x/will`;
- run `A.close(2s).await`, then drop A's runtime handle;
- assert that B receives no message on `x/will` within 3 s.

If `Profile` has no will field, check a different way: assert that `close` returns only after the task has finished (`handle.is_finished()`), and that mosquitto's log shows `disconnected.` rather than `closed its connection`.

- [ ] **Step 2: Run it and check that it fails.** Run: `cargo test -p mqtt-zapp-core --test broker close_sends_disconnect`. Expected: FAIL. `close` is undefined.
- [ ] **Step 3: Implement**
- `connect` stores the `JoinHandle` that `handle.spawn` returns.
- `tui::run` calls `session.close().await` in place of `session.disconnect()`.
- `event_loop` gets a branch fed by `tokio::signal::unix::signal(SignalKind::terminate())` and `SignalKind::hangup()`. Both return `Ok(())`, so `TerminalGuard` drops and the terminal is restored.
- [ ] **Step 4: Check by hand.** Run `cargo run -- --host localhost` against a local mosquitto, then `kill <pid>` from another shell. The shell prompt comes back normal: not in raw mode, not on the alternate screen. Run `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `fix(tui): restore the terminal on SIGTERM/SIGHUP and await DISCONNECT on quit`

---

### Task 7: Memory ceilings: shared payloads, a payload cap, a topic cap (audit point 5)

**Files:**
- Modify: `crates/mqtt-zapp-core/Cargo.toml` (add `bytes = { version = "1", features = ["serde"] }`)
- Modify: `crates/mqtt-zapp-core/src/payload.rs:5-10` (`Message`)
- Modify: `crates/mqtt-zapp-core/src/client.rs:140,155` (`p.payload.to_vec()` becomes `p.payload`)
- Modify: `crates/mqtt-zapp-core/src/tree.rs` (`with_limits`, `insert`)
- Modify: `crates/mqtt-zapp-core/src/config.rs` (`Settings`)
- Modify: `crates/mqtt-zapp/src/tui/app.rs` (`App::new`, `apply_core_event`), `message_view.rs` (truncation marker)
- Modify: every `Message { payload: ... }` literal (about 40, mostly tests): use `Bytes::copy_from_slice(p.as_bytes())`, or a shared test helper.

**Interfaces:**
- Produces:
  - `Message { pub payload: bytes::Bytes, pub qos: u8, pub retain: bool, pub received_at: SystemTime, pub truncated_from: Option<usize> }`.
  - `impl Message { pub fn truncate_to(&mut self, max: usize) }`: when the payload is longer than `max`, it sets `truncated_from = Some(len)` and `payload = Bytes::copy_from_slice(&payload[..max])`. It copies so the large buffer is freed.
  - `Settings { pub max_payload_kb: usize /* default 16384 */, pub max_topics: usize /* default 200_000 */ }`.
  - `TopicTree::with_limits(history_depth, max_bytes, max_topics)`.
  - `TopicTree::insert(&mut self, topic: &str, msg: Message) -> Option<NodeId>`: it returns `None` when the topic is new and the tree already holds `max_topics` nodes. Update its callers.
  - `pub fn dropped_topics(&self) -> u64` counts those refusals.

- [ ] **Step 1: Write the failing tests**
- `tree.rs`: `latest_shares_history_buffer`. Insert a 1 MiB payload, then assert `node.latest.payload.as_ptr() == node.history.back().payload.as_ptr()`.
- `topic_cap_refuses_new_topics_keeps_updates`: with `max_topics = 3`, inserting `a/b`, `a/c` and `a/d` makes `a/d` return `None`, while an update to `a/b` still returns `Some`.
- `payload.rs`: `truncate_to_marks_original_len`.
- `message_view/tests.rs`: `truncated_json_renders_as_text_with_marker`. A 2 KB JSON payload truncated to 1 KB renders as text, its last line reads `… truncated (2048 bytes)`, and chart cycling (`c`) does not panic.
- The header shows `⚠ N topics refused` when `dropped_topics > 0`, using the Task 2 badge slot (join it with `·`).

- [ ] **Step 2: Run them and check that they fail.** Run: `cargo test --workspace latest_shares topic_cap truncate truncated_json`. Expected: FAIL.
- [ ] **Step 3: Implement.** `apply_core_event` calls `msg.truncate_to(max_payload_kb * 1024)` before `tree.insert`. `detect` and `pretty_lines` work on the (truncated) payload as they do now. `build_body` appends the marker line when `truncated_from.is_some()`.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `perf(core): share payload buffers, cap payload size and topic count`

---

### Task 8: Payload view: pretty-print cap and cached chart parsing (audit point 5)

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/message_view.rs` (`build_body`, `ChartCache`, `chart_points`)
- Modify: `crates/mqtt-zapp/src/tui/app.rs:942-966` (`chart_series`, `chart_line`)
- Test: `message_view/tests.rs`

**Interfaces:**
- Produces: `const PRETTY_MAX: usize = 1 << 20;`. Above it, `build_body` skips the JSON parse and shows the text lines. `ChartCache` also memoises `series: Vec<Option<String>>` and `line: Option<usize>` under the same key as the points, which is `(message stamp, chart path)`.

- [ ] **Step 1: Write the failing tests**
- `large_json_skips_pretty_print`: a 1.5 MiB JSON payload, sanitized, renders as raw text lines.
- `chart_series_parsed_once_per_message`: render 5 frames with no new message, then assert `chart_cache.builds.get() == 1`, using the existing `#[cfg(test)] builds` counter.
- [ ] **Step 2: Run them and check that they fail.** Run: `cargo test -p mqtt-zapp large_json chart_series_parsed`. Expected: FAIL.
- [ ] **Step 3: Implement.** `chart_series` and `chart_line` read from the cache, and the cache fills them in when the key changes.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `perf(tui): cap pretty-printing and cache chart parsing`

---

### Task 9: Precomputed natural sort key (audit point 1, part 1)

**Files:**
- Modify: `crates/mqtt-zapp-core/src/tree.rs` (`TopicNode`, `TopicNode::new`, `natural_cmp`)
- Modify: `crates/mqtt-zapp/src/tui/rows.rs:28-36` (comparator)

**Interfaces:**
- Produces:
  - `pub struct NaturalKey(Box<[KeyPart]>)` with `impl Ord`;
  - `enum KeyPart { Num { value: Box<str> /* digits, leading zeros stripped */, width: usize }, Text(Box<str> /* lowercased */) }`;
  - `pub fn natural_key(s: &str) -> NaturalKey`;
  - `TopicNode.sort_key: NaturalKey`, set in `TopicNode::new`.
- The order must equal `natural_cmp` exactly, **including the tie-breaks**: compare all parts first, then shorter digit runs, then plain byte order of the name. Express that last tie-break as `key.cmp(..).then_with(|| a.name.cmp(&b.name))` at the call site. `natural_cmp` stays and becomes `natural_key(a).cmp(&natural_key(b)).then(a.cmp(b))`.

- [ ] **Step 1: Write the failing test**

`natural_key_agrees_with_natural_cmp`: for every pair from
`["n10","n9","n2","x01","x1","Tasmota_2","tasmota_10","a","A","","0","00","b-1","b_1","é9","é10"]`,
assert `natural_key(a).cmp(&natural_key(b)).then(a.cmp(b)) == old_natural_cmp(a, b)`. Keep the old fn as `#[cfg(test)] fn old_natural_cmp` for this test.
- [ ] **Step 2: Run it and check that it fails.** Run: `cargo test -p mqtt-zapp-core natural_key`. Expected: FAIL. `natural_key` is undefined.
- [ ] **Step 3: Implement.** In `visible_rows`, `by_name` becomes `x.sort_key.cmp(&y.sort_key).then_with(|| x.name.cmp(&y.name))`.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS. `natural_cmp_orders` and `sort_by_name_count_and_recent` stay green.
- [ ] **Step 5: Commit** `perf(core): precompute natural sort keys per node`

---

### Task 10: Cached tree rows (audit point 1, part 2, plus the point 9 benchmark)

**Files:**
- Modify: `crates/mqtt-zapp-core/src/tree.rs` (`shape_gen`)
- Create: `crates/mqtt-zapp/src/tui/expanded.rs`, which holds the `Expanded` wrapper
- Modify: `crates/mqtt-zapp/src/tui/app.rs` (the `expanded` field type, the `rows()` signature, every `self.filter =` and `self.sort =` site listed below), `app/mouse.rs:136,155`, `ui.rs:446`, `rows.rs`
- Test: `app/tests.rs`, `rows.rs` tests, and a timing test in `app/tests.rs`

**Interfaces:**
- Consumes: `TopicNode.sort_key` from Task 9.
- Produces:
  - `TopicTree::shape_gen(&self) -> u64`, bumped whenever a node is created or `remove` runs.
  - `pub struct Expanded { set: HashSet<NodeId>, generation: u64 }`, with `contains`, `insert -> bool`, `remove -> bool`, `clear`, `generation() -> u64` and `as_set() -> &HashSet<NodeId>`. Every mutating method bumps `generation`.
  - `App.filter_gen: u64`, bumped by a new `fn set_filter(&mut self, f: Option<Filter>)`. That replaces the direct assignments at app.rs:396, 796 and 819.
  - `pub fn rows(&self) -> std::rc::Rc<[Row]>`, cached in `rows_cache: RefCell<Option<(RowsKey, Rc<[Row]>)>>`, where `struct RowsKey { shape: u64, expanded: u64, filter: u64, sort: SortOrder, order_second: Option<u64> }`. `order_second` is `None` for Name sort. For Count and Recent sort it is the whole seconds since `App` was created, so those orders refresh at most once a second.
- Call sites keep working through `Rc<[Row]>`: `.iter()`, `.get()`, `.len()`, and `.iter().copied()` in place of `.into_iter()`.

- [ ] **Step 1: Write the failing tests**
- `rows_are_cached_between_calls`: call `rows()` twice with no change and assert `Rc::ptr_eq`.
- `rows_rebuild_on_expand_filter_sort_and_new_topic`: each kind of change gives a pointer that is not equal.
- `rows_drop_removed_topic`: after `delete_topic` confirmation and `tree.remove`, the rows do not contain the removed id.
- `count_sort_selection_follows_id_across_rebuild`: Count sort, select `b`, then publish to `a` until it outranks `b`. Pressing `j` moves to the row after `b`'s *current* position.
- Timing test `rows_fast_at_50k`, gated `#[cfg_attr(debug_assertions, ignore = "timing: run with --release")]` like `fast_enough_at_spec_scale`: 50_000 topics `dev_{i}/x` all expanded. The first `rows()` call takes < 60 ms, and a second cached call takes < 1 ms.
- [ ] **Step 2: Run them and check that they fail.** Run: `cargo test -p mqtt-zapp rows_`. Expected: FAIL to compile (the `Rc` and `Expanded` types do not exist yet).
- [ ] **Step 3: Implement.** Replace the `expanded: HashSet<NodeId>` field with `Expanded`. The 11 mutation sites in app.rs keep their method names. `visible_rows` takes `&HashSet` through `as_set()`. In the two test files that mutate `.expanded` directly, switch to the wrapper's methods.
- [ ] **Step 4: Run** `cargo test --workspace && cargo test --release -p mqtt-zapp rows_fast_at_50k`. Expected: PASS.
- [ ] **Step 5: Add the benchmark to CI.** In `.github/workflows/ci.yml`, add the step `cargo test --release -p mqtt-zapp rows_fast_at_50k` next to the search-speed step.
- [ ] **Step 6: Commit** `perf(tui): cache the flattened tree rows`

---

### Task 11: Faster search matcher (audit point 4)

**Files:**
- Modify: `crates/mqtt-zapp-core/src/search.rs` (`Query`, `Query::parse`, `Query::matches`, `filter`)
- Modify: `crates/mqtt-zapp/src/tui/app.rs:450-453` (`on_tick` throttle)
- Test: `search.rs` tests, `app/tests.rs`

**Interfaces:**
- Produces:
  - `const SEARCH_PAYLOAD_MAX: usize = 64 * 1024;`
  - `Query::Terms` holds `Vec<regex::bytes::Regex>` (each term escaped with `regex::escape` and built case-insensitive), alongside the original `Vec<String>` that `ranges` uses.
  - `Query::Regex` adds a `regex::bytes::Regex` built from the same pattern.
  - `fn matches(&self, path: &str, payload: &[u8]) -> bool` works on raw bytes. It never lowercases and never allocates.
  - `filter` walks the tree depth-first with one reused `String` path buffer instead of calling `full_topic` per node, and passes `&payload[..min(len, SEARCH_PAYLOAD_MAX)]`.
  - `App.last_filter_rebuild: Instant`. `on_tick` rebuilds the live filter at most every 500 ms.
- [ ] **Step 1: Write the failing tests**
- `match_beyond_payload_cap_is_ignored`: a term placed only after 64 KiB does not match.
- `non_utf8_payload_matches_ascii_term`: the payload `b"\xff\xfeTEMP=21"` matches `temp`.
- Update `fast_enough_at_spec_scale` with a second case: 50_000 topics with 1 KB payloads, three queries < 150 ms in release.
- app: `on_tick_throttles_filter_rebuild`. Two ticks 100 ms apart with tree changes give one rebuild. Add a `#[cfg(test)] filter_builds: Cell<u32>` counter.
- [ ] **Step 2: Run them and check that they fail.** Run: `cargo test -p mqtt-zapp-core search && cargo test --release -p mqtt-zapp-core --lib fast_enough_at_spec_scale`. Expected: FAIL on the cap, non-UTF-8 and 50k timing cases.
- [ ] **Step 3: Implement** to the interfaces above. `ranges` and `match_ranges` (used for highlighting) stay on `&str`.
- [ ] **Step 4: Run** `cargo test --workspace && cargo test --release -p mqtt-zapp-core --lib fast_enough_at_spec_scale`. Expected: PASS.
- [ ] **Step 5: Commit** `perf(core): byte-level case-insensitive search with a payload cap`

---

### Task 12: Supply chain, CI and release profile (audit point 8)

**Files:**
- Create: `deny.toml`
- Modify: `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `Cargo.toml` (`[profile.release]`)

- [ ] **Step 1: Release profile.** Set `[profile.release]` to `panic = "abort"`, `lto = "fat"`, `codegen-units = 1`, `strip = true`.

  Check: `cargo build --release && ls -l target/release/mqtt-zapp` shows a smaller binary than 9.7 MB, and `cargo test --release -p mqtt-zapp rows_fast_at_50k` passes. Then check the PKGBUILD's `options=(!lto)` comment still holds. That option controls makepkg's C LTO, not Cargo's, so leave it. Run `makepkg` in the Arch CI job (Step 5) to confirm.
- [ ] **Step 2: cargo-deny.** Create `deny.toml` with:
  - `[advisories]` default (deny vulnerabilities, warn on unmaintained);
  - `[licenses] allow = ["MIT","Apache-2.0","Apache-2.0 WITH LLVM-exception","BSD-2-Clause","BSD-3-Clause","ISC","Unicode-3.0","Zlib","CDLA-Permissive-2.0","MPL-2.0"]`;
  - `[bans] multiple-versions = "warn"`;
  - `[sources] unknown-registry = "deny"`, `unknown-git = "deny"`.

  Run: `cargo install cargo-deny --locked && cargo deny check`. Expected: `advisories ok, bans ok, licenses ok, sources ok`. If a needed licence is missing, add it to `allow` with a comment naming the crate. If there is an advisory, do not ignore it without writing the reason in `deny.toml`.
- [ ] **Step 3: CI job and permissions.**
  - In `ci.yml`, add top-level `permissions: { contents: read }` and a `deny` job using `EmbarkStudios/cargo-deny-action`.
  - Add `--locked` to `cargo clippy` and `cargo test`.
  - Remove `Swatinem/rust-cache` from the `musl` and `deb` release jobs. Release builds start from a clean cache, so a poisoned PR cache cannot reach a shipped binary.
- [ ] **Step 4: Pin actions to SHAs.** For each `uses:` (actions/checkout@v5, dtolnay/rust-toolchain@stable, Swatinem/rust-cache@v2, actions/upload-artifact@v7, actions/download-artifact@v8, EmbarkStudios/cargo-deny-action@v2):
  - get the SHA with `gh api repos/<owner>/<repo>/commits/<ref> --jq .sha`;
  - write `uses: owner/repo@<sha> # <ref>`;
  - for `dtolnay/rust-toolchain`, pin the SHA of the `stable` branch and keep the `toolchain: stable` input.

  Add `.github/dependabot.yml` with `package-ecosystem: github-actions` (weekly) and `cargo` (weekly), so the pins don't go stale.
- [ ] **Step 5: Provenance.** In `release.yml`'s `release` job, add `permissions: { contents: write, id-token: write, attestations: write }` and an `actions/attest-build-provenance` step (SHA-pinned) with `subject-path: dist/*` before "Create release". Run the workflow manually (`gh workflow run release.yml`) and confirm every job is green. A manual run does not publish.
- [ ] **Step 6: Commit** `ci: cargo-deny, SHA-pinned actions, --locked, provenance; optimized release profile`

---

### Task 13: Dependency refresh: rumqttc 0.25, rustls-native-certs 0.8 (audit point 8)

**Files:**
- Modify: `crates/mqtt-zapp-core/Cargo.toml`, `crates/mqtt-zapp-core/src/client.rs` (the TLS config builder, `Loop::poll` and `drop_pending` if the API moved)

- [ ] **Step 1:** Bump `rumqttc = "0.25"` and `rustls-native-certs = "0.8"`, then run `cargo update -p rumqttc -p rustls-native-certs`.
- [ ] **Step 2:** Run `cargo build`, then fix the API changes. Check that `rustls-native-certs` 0.8's `load_native_certs()` returns a `CertificateResult` (`.certs` and `.errors`). Keep the "refuse an empty root store" behaviour, and report `.errors` only when the store is empty. Check that `EventLoop.pending` is still public; Task 1 depends on it. If it isn't, use the replacement API and keep the `drop_pending` signature. Check the `insecure` verifier still compiles against the rustls 0.23 `ServerCertVerifier` trait.
- [ ] **Step 3:** Run `cargo tree -i rustls -e normal | head -1`. Expected: `rustls v0.23.x`. Run `cargo test --workspace` (needs mosquitto for the TLS, ws and v5 broker tests). Expected: PASS, with no SKIP lines when mosquitto is installed. Run `cargo deny check`. Expected: ok.
- [ ] **Step 4: Commit** `build(core): rumqttc 0.25 (rustls 0.23), rustls-native-certs 0.8`

---

## Deliberately out of scope

- **`Zeroizing<String>` for passwords (point 6, optional):** rumqttc keeps its own plain `String` copy for reconnects, so zeroizing our buffers would not remove the secret from memory. Revisit only if rumqttc supports zeroizing credentials.
- **An incremental or prefix-narrowing search index (point 4):** it isn't needed at the spec's scale (§2, §7). Task 11 plus the 500 ms throttle already gives headroom to 50k topics.
- **Re-reading the subtree on delete confirm (robustness finding 6, second half):** this is intentional and tested (`retained_arriving_during_the_confirm_is_cleared_too`).
- **Freeing removed node slots in `TopicTree`:** the topic cap (Task 7) bounds the memory they can use.
