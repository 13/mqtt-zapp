# Next Round Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** This round adds a keyring-less CI job, an app/ui/message_view module split, MQTT 5 property display, search-match highlighting in the payload, multi-series charts, Sparkplug B decoding, and a headless TUI test against Mosquitto.

**Architecture:** The core gains `Message.props` (MQTT 5 publish properties) and a `sparkplug` decoder that runs at ingest. A decoded payload is stored as JSON, so search, diff, charts and previews work on it unchanged. The TUI changes stay inside new submodules split out of `app.rs`, `ui.rs` and `message_view.rs`.

**Tech Stack:** Rust 2024, rumqttc 0.25, ratatui 0.29, crossterm 0.28. No new dependencies: the Sparkplug protobuf is decoded by hand (about 150 lines) to avoid `prost` and `protoc`.

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md` §4.3 (match highlighting), §4.4 (Sparkplug B), §4.5 (multiple series), §4.1 (MQTT 5); the "Later" list in `2026-11-03-picker-order-header-copy.md` (items 2, 3, 8).

## Global Constraints

- CI gate per task: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`, plus `cargo clippy --no-default-features --all-targets -- -D warnings` from Task 1 on.
- No new crates. musl and `--no-default-features` builds must keep working.
- The README key table is checked by `keymap.rs` (`readme_lists_every_default`); a new action needs its README row.
- Every user-facing string goes through `payload::sanitize` when it comes from the wire (property values and Sparkplug strings included).
- Conventional commit messages (`feat(tui):`, `fix(core):`, `refactor(tui):`, `ci:`, `test:`), one commit per task.

## Review Focus

1. **A v5 message with a huge user-property list** (hundreds of pairs, or kilobyte values) must not blow up the header. Show at most one properties row, ellipsized, plus `+N more`. Test: `props_row_is_one_line_and_ellipsized` (Task 3).
2. **A Sparkplug payload that is truncated, malformed or not protobuf at all** (for example an `spBv1.0/.../STATE` JSON payload) keeps its raw bytes and never panics. Tests: `decode_rejects_garbage`, `decode_truncated_returns_none`, `state_topic_is_not_decoded` (Task 6).
3. **Search highlighting with a regex that matches the empty string**, such as `a*`, must not loop or emit zero-width spans. Test: `payload_highlight_skips_empty_matches` (Task 4).
4. **Overlaid chart series whose scales differ by orders of magnitude** stay readable. All the series share one y-axis, and the legend names each one. A pinned series that is missing from some history entries just has gaps. Test: `overlay_missing_points_are_skipped` (Task 5).
5. **Search highlighting must not shift payload line numbers.** Clicking a number to chart it still hits the right line. Test: `chart_click_with_active_filter_hits_same_line` (Task 4).

---

### Task 1: CI job for the keyring-less build

**Files:** Modify `.github/workflows/ci.yml`.

- [ ] **Step 1:** Add a job `no-default-features` modelled on `test`, but with no `libdbus-1-dev` install (that proves dbus isn't needed). It installs only `mosquitto`, then runs `cargo clippy --locked --no-default-features --all-targets -- -D warnings` and `cargo test --locked --workspace --no-default-features`. Reuse the SHA-pinned actions already in the file.
- [ ] **Step 2:** Run both commands locally. Expected: PASS. If `mqtt-zapp-core` pulls keyring through a workspace-level feature, fix that by building per package (`-p mqtt-zapp -p mqtt-zapp-core`).
- [ ] **Step 3:** Commit `ci: check the keyring-less build on every push`.

### Task 2: Split `app.rs`, `ui.rs` and `message_view.rs`

Pure move: no behaviour change. The test count before and after must be equal (`cargo test --workspace 2>&1 | grep -E '^test result' `, sum of passed).

**Files:**
- Create `crates/mqtt-zapp/src/tui/app/chart.rs`: `cycle_chart`, `chart_series`, `chart_line`, `chart_click`, `charted_body`, `scroll_to_chart_line`.
- Create `crates/mqtt-zapp/src/tui/app/publish.rs`: `on_publish_key`, `set_publish_payload`, `remember_sent`, `walk_sent`, `open_publish`.
- Create `crates/mqtt-zapp/src/tui/app/retained.rs`: `on_confirm_key`, `retained_under`, `ask_delete_topic`, `delete_topic`, `ask_clear_retained`.
- Create `crates/mqtt-zapp/src/tui/app/search.rs`: `on_search_key`, `clear_search`, `reveal_selected`, `rebuild_filter`, `set_filter`.
- Create `crates/mqtt-zapp/src/tui/ui/overlays.rs`: `render_stats`, `render_confirm`, `fit_width`, `wrap_words`, `HELP_GROUPS`, `HELP_FIXED`, `render_help`, `match_in`, `help_matches`, `highlight_ranges`, and the `CONFIRM_*` consts.
- Create `crates/mqtt-zapp/src/tui/ui/bars.rs`: `key_for`, `hints`, `push_hints`, `footer_line`, `spans_width`, `ellipsize`, `REASON_MIN`, `header_line`.
- Create `crates/mqtt-zapp/src/tui/message_view/chart.rs`: `chart_split`, `HistoryKey`, `ChartKey`, `Points`, `ChartMeta`, `ChartCache`, `history_key`, `chart_key`, `chart_meta`, `chart_points`, `fmt_num`, `render_chart`.
- Each new file follows `app/front.rs`: `use super::*;` and an `impl App` block (or free fns). Widen visibility only as far as `pub(super)` needs. `MATCH_MODIFIER` becomes `pub(super)`, because Task 4 uses it from `message_view`.

- [ ] **Step 1:** Record the test count.
- [ ] **Step 2:** Move the items. Keep the doc comments with them.
- [ ] **Step 3:** Run the CI gate. Expected: PASS, with the same test count. `app.rs`, `ui.rs` and `message_view.rs` each end up under 800 lines (`wc -l`).
- [ ] **Step 4:** Commit `refactor(tui): split chart, publish, retained, search, overlays and bars into submodules`.

### Task 3: MQTT 5 publish properties in the message header

**Files:**
- Modify `crates/mqtt-zapp-core/src/payload.rs` (the struct and its tests), `crates/mqtt-zapp-core/src/client.rs` (`incoming`, `Loop::poll`), every `Message { … }` literal (18 sites, `grep -rn "truncated_from: None" crates`), and `crates/mqtt-zapp/src/tui/message_view.rs` (header).
- Test: `payload.rs` tests, `message_view/tests.rs`, `crates/mqtt-zapp-core/tests/broker.rs`.

**Interfaces:**
- Produces, in `payload.rs`:
  ```rust
  #[derive(Clone, Debug, Default, PartialEq, Serialize)]
  pub struct Props {
      pub content_type: Option<String>,
      pub response_topic: Option<String>,
      pub correlation_data: Option<Bytes>,
      pub message_expiry: Option<u32>,      // seconds
      pub payload_utf8: Option<bool>,       // payload_format_indicator == 1
      pub user: Vec<(String, String)>,
  }
  impl Props { pub fn is_empty(&self) -> bool; pub fn summary(&self) -> Vec<String>; }
  // Message gains:  pub props: Option<std::sync::Arc<Props>>,
  ```
  `summary()` returns display chips in this order: `type <content_type>`, `reply <response_topic>`, `corr <hex, first 8 bytes, … if longer>`, `expiry <n>s`, `utf8`, then `k=v` for each user pair. Every string goes through `sanitize`. `None` when the publish has no properties or all of them are empty (topic alias and subscription ids are not shown).

- [ ] **Step 1: Failing tests** (`payload.rs`):
  - `props_summary_order`: `Props{content_type:Some("application/json"), response_topic:Some("r/t"), correlation_data:Some(Bytes::from_static(&[1,2,3,4,5,6,7,8,9])), message_expiry:Some(60), payload_utf8:Some(true), user:vec![("a","1")]}` → `["type application/json","reply r/t","corr 0102030405060708…","expiry 60s","utf8","a=1"]`.
  - `props_summary_sanitizes`: `user:[("k","x\u{202e}y")]` → the chip doesn't contain `\u{202e}`.
  - `empty_props_is_empty`: `Props::default().is_empty()`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp-core props_`. Expected: FAIL (`Props` not defined).
- [ ] **Step 3:** Implement `Props` and the `Message.props` field. Set `props: None` at all literal sites. In `Loop::poll` V5, map `p.properties` → `Props`, and store `Some(Arc)` only when it isn't `is_empty()`. `incoming` gains a `props: Option<Arc<Props>>` parameter.
- [ ] **Step 4: Failing TUI tests** (`message_view/tests.rs`):
  - `props_row_shown_in_card`: a message with `content_type="text/plain"` and user `("dev","42")`, rendered in a card layout (≥ `HEADER_CARD_MIN_HEIGHT`), gives a buffer with a row containing `type text/plain` and `dev=42`. Without props the card is 3 rows tall, exactly as before (existing tests stay green).
  - `props_row_is_one_line_and_ellipsized`: 300 user pairs, width 80 → one props row, ending in `+N more` where N is the number of chips that didn't fit, and no wrap.
  - `props_row_in_compact_header`: the stacked layout shows the props as a second row under the compact header.
- [ ] **Step 5:** Implement `fn props_line(props: &Props, width: u16) -> Line<'static>` in `message_view.rs`. It renders chips joined by `  ` in `theme::MUTED`, fills the row greedily, and the tail is ` +N more`. In `render_message`, `header_rows` becomes `if card {3} else {1} + props_present as u16`. The card draws the badges row and then the props row inside the same block, and `HEADER_CARD_MIN_HEIGHT` stays the same.
- [ ] **Step 6: Broker test** (`broker.rs`, v5 profile): `v5_properties_arrive`. A raw rumqttc v5 client publishes with `content_type="application/json"` and a user property `("src","test")`. The received `msg.props` contains both. Skipped without mosquitto, like the others.
- [ ] **Step 7:** Run the CI gate. Expected: PASS. Add a sentence under *Features* (transport bullet) in the README: "MQTT 5 properties (content type, response topic, user properties …) show under the message header."
- [ ] **Step 8:** Commit `feat: show MQTT 5 publish properties under the message header`.

### Task 4: Highlight search matches in the payload pane

**Files:** Modify `crates/mqtt-zapp/src/tui/message_view.rs` (`render_payload`). Test: `message_view/tests.rs`.

**Interfaces:**
- Consumes `app.filter: Option<Filter>` and `Filter::ranges(&self, haystack: &str) -> Vec<Range<usize>>` (core `search.rs`), and `ui::MATCH_MODIFIER` (made `pub(super)` in Task 2).
- Produces `fn mark_matches(line: Line<'static>, ranges: &[Range<usize>]) -> Line<'static>` in `message_view.rs`. It splits the line's spans at the range boundaries (byte offsets into the concatenated line text) and adds `MATCH_MODIFIER` to the pieces inside a range. Empty ranges are skipped.

- [ ] **Step 1: Failing tests:**
  - `payload_highlights_filter_terms`: the filter is `wzr`, and the payload is `{"id":"WZR-7"}`. The line holding `WZR` has the cells `W`,`Z`,`R` with `MATCH_MODIFIER`, and the other cells don't.
  - `payload_highlight_keeps_json_colours`: the matched cells keep their JSON colour (the `fg` is unchanged and only the modifier is added).
  - `payload_highlight_skips_empty_matches`: regex mode, `a*`, gives a render that terminates and has no panic. Cells outside a real `a` have no modifier.
  - `payload_highlight_off_in_diff_view`: with `show_diff` on, nothing is highlighted.
  - `chart_click_with_active_filter_hits_same_line`: with a filter active, a click on a number's line charts that field (the same assertion as the existing chart-click test).
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp payload_highlight`. Expected: FAIL.
- [ ] **Step 3:** Implement `mark_matches`. In `render_payload`, apply it to each `Json` and `Plain` line before `mark(...)`, when `app.filter` is `Some` and `!app.show_diff`. Also skip empty ranges in core `Query::ranges` if the test shows they come from there.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. In the README *Search* section, add the bullet "Matches are underlined in the tree and in the payload."
- [ ] **Step 5:** Commit `feat(tui): underline search matches in the payload`.

### Task 5: Overlay several chart series

**Files:** Modify `crates/mqtt-zapp/src/tui/app/chart.rs`, `crates/mqtt-zapp/src/tui/message_view/chart.rs`, `crates/mqtt-zapp/src/tui/keymap.rs`, `crates/mqtt-zapp/src/tui/ui/overlays.rs` (`HELP_GROUPS` "view") and `README.md`. Test: `app/tests.rs`, `message_view/tests.rs`.

**Interfaces:**
- Produces:
  - `Action::ChartPin`, named `chart_pin`, default key `C`. The help text is "Pin the charted series so the next `c` overlays another (again: unpin)".
  - `App.chart_pinned: Vec<Option<String>>`, with at most `MAX_PINNED = 3`.
  - `pub fn chart_lines(&self) -> Vec<usize>` returns the payload lines of the current series and each pinned one. It replaces `chart_line()`'s use in `render_payload`, and `chart_line` stays for scrolling.
  - `const SERIES_COLORS: [Color; 4] = [theme::ACCENT, Color::Magenta, Color::Yellow, Color::Green]`. Index 0 is the current series, then each pinned one in pin order.
- Rules:
  - `C` with the chart off sets the note `nothing to pin`.
  - `C` on a series that is already pinned unpins it.
  - A fourth pin replaces the oldest and sets the note `pinned: <name> (dropped <oldest>)`.
  - `c` cycles only the current series and skips paths that are already pinned.
  - Turning the chart off (`c` past the end, or clicking the charted number) keeps the pins. Selecting another topic, or `reset_tree`, clears them, the same places that clear `chart` today (`app.rs` lines near `self.chart = None`).

- [ ] **Step 1: Failing tests** (`app/tests.rs`):
  - `chart_pin_overlays_next_series`: payload `{"a":1,"b":2}` × 3 history entries, then `c`, `C` and `c` → `chart == Some(Some("b"))` and `chart_pinned == [Some("a")]`.
  - `chart_pin_again_unpins`.
  - `chart_pin_caps_at_three`: the fourth pin drops the oldest, and the note text is exact.
  - `chart_pins_cleared_on_select`.
  - `chart_pin_without_chart_notes`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp chart_pin`. Expected: FAIL.
- [ ] **Step 3:** Implement the action, the keymap row, the help-group entry and the README row (`| \`C\` | \`chart_pin\` | … |`, right after `c`).
- [ ] **Step 4: Failing render tests** (`message_view/tests.rs`):
  - `overlay_draws_legend_and_colours`: with one pin, the chart title is `chart · b + a`. The legend shows both names, and the cells of each line use its `SERIES_COLORS` colour.
  - `overlay_missing_points_are_skipped`: the pinned field is missing in 2 of 5 entries. The render has no panic, and that dataset has 3 points.
  - `overlay_highlights_all_charted_lines`: both `a` and `b` lines in the payload are selected-styled.
- [ ] **Step 5:** Implement it in `render_chart`. Build one `Dataset` per series (current first) with `.name(path_or_value)`, using `chart_points` for each. The single-entry cache in `ChartCache.slot` becomes a small `Vec` keyed by path, capped at `1 + MAX_PINNED`, so overlays don't thrash it. Bounds are the min/max over all the datasets. Set `.legend_position(Some(LegendPosition::TopLeft))` and hide the legend when there is only one series (the current look). "not enough numeric values" shows only when every dataset has fewer than 2 points.
- [ ] **Step 6:** Run the CI gate. Expected: PASS (the existing single-series chart tests stay unchanged).
- [ ] **Step 7:** Commit `feat(tui): pin chart series to overlay several fields`.

### Task 6: Sparkplug B decoding

**Files:** Create `crates/mqtt-zapp-core/src/sparkplug.rs`. Modify `lib.rs`, `payload.rs` (`Message.sparkplug: bool`), `client.rs` (`incoming`), the `message_view.rs` badge, and `README.md` (Features, plus remove the Roadmap item). Test: inline in `sparkplug.rs`, `message_view/tests.rs`.

**Interfaces:**
- Produces:
  - `pub fn is_sparkplug_topic(topic: &str) -> bool`: `spBv1.0/<group>/<type>/…` where `<type>` is one of `NBIRTH NDEATH DBIRTH DDEATH NDATA DDATA NCMD DCMD`. `STATE` and anything else give false.
  - `pub fn decode(payload: &[u8]) -> Option<serde_json::Value>`: a hand-written protobuf wire decoder (varint, 64-bit, length-delimited, 32-bit; unknown fields skipped; any out-of-bounds read → `None`).
  - Output shape:
    ```json
    {"timestamp": 1700000000000, "seq": 3, "uuid": "…",
     "metrics": {"Temperature": 21.5, "Node Control/Rebirth": false, "alias:7": 12}}
    ```
    The metric key is `name`, or `alias:<n>` when the name is absent. The value comes from whichever of fields 10–16 is set (int/long as integers, float/double as f64, bool, string; bytes as `"<N bytes>"`). `is_null` gives `null`, and dataset/template give `"<dataset>"`/`"<template>"`. A duplicate name means the later one wins. Keys are absent when their field is absent.
  - Field numbers. Payload: 1 timestamp (u64), 2 metrics (repeated Metric), 3 seq (u64), 4 uuid (string), 5 body (bytes; ignored). Metric: 1 name, 2 alias, 3 timestamp, 4 datatype, 7 is_null, 10 int_value (u32), 11 long_value (u64), 12 float_value (fixed32), 13 double_value (fixed64), 14 boolean_value, 15 string_value, 16 bytes_value, 17 dataset_value, 18 template_value.
  - The datatype (field 4) decides signedness for int/long. Int8/16/32 (datatypes 1–3) are sign-extended from the u32, and Int64 (4) is reinterpreted as i64. Unsigned types stay unsigned.
- Ingest: in `client.rs`, `incoming(topic, …)`. When `is_sparkplug_topic(topic)` and `decode` returns `Some(v)`, the payload becomes `serde_json::to_vec(&v)` and `sparkplug = true`. Otherwise the raw bytes are kept. Decoding happens before the `max_payload_kb` truncation (in `App::apply_core_event_inner`), so the cap applies to the JSON.
- TUI: the kind badge reads `SPARKPLUG` instead of `JSON` when `msg.sparkplug`. Everything else (pretty print, diff, chart paths like `metrics.Temperature`, search, preview) works unchanged on the JSON.

- [ ] **Step 1: Failing tests** (`sparkplug.rs`). Build the fixtures with a tiny test-only encoder (`fn varint`, `fn field`) in the test module, so they're readable:
  - `decode_ndata_metrics_by_name`: timestamp 1000, seq 3, metrics `Temperature` (datatype 9 Float, 21.5), `On` (Boolean true) → `{"timestamp":1000,"seq":3,"metrics":{"Temperature":21.5,"On":true}}`.
  - `decode_alias_only_metric`: alias 7 and long 12 → key `alias:7`.
  - `decode_signed_int32`: datatype 3, int_value `0xFFFFFFFF` → `-1`.
  - `decode_null_and_bytes`: `is_null` → `null`, bytes of length 4 → `"<4 bytes>"`.
  - `decode_skips_unknown_fields`.
  - `decode_rejects_garbage`: `b"{\"a\":1}"` → `None`. Neither panics nor returns `Some`.
  - `decode_truncated_returns_none`: a valid payload with its last 3 bytes cut → `None`.
  - `topic_types`: `spBv1.0/g/NDATA/n` → true, `spBv1.0/g/STATE/x` → false, `spBv1.0/STATE/host` → false, `tasmota/x` → false.
  - `state_topic_is_not_decoded` (in `client.rs` tests or a pure helper): `incoming("spBv1.0/STATE/h", b"{\"online\":true}", …)` keeps the bytes and `sparkplug == false`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp-core sparkplug`. Expected: FAIL.
- [ ] **Step 3:** Implement `sparkplug.rs`, the `Message.sparkplug` field (`false` at every literal site), and the ingest hook.
- [ ] **Step 4: Failing TUI test:** `sparkplug_badge_and_chart_path`. A message with `sparkplug: true` and the decoded JSON shows the badge `SPARKPLUG`, and `chart_series()` contains `Some("metrics.Temperature")`.
- [ ] **Step 5:** Implement the badge. Run the CI gate. Expected: PASS.
- [ ] **Step 6:** README: add a Features bullet ("🏭 **Sparkplug B:** `spBv1.0/…` payloads are decoded to JSON, so you can search, diff and chart their metrics.") and delete the Roadmap section, which is now empty.
- [ ] **Step 7:** Commit `feat(core): decode Sparkplug B payloads to JSON`.

### Task 7: Headless TUI test against Mosquitto

**Files:** Create `crates/mqtt-zapp/src/tui/e2e_tests.rs` (`#[cfg(test)] mod e2e_tests;` in `tui/mod.rs`). It reuses the broker spawner through `#[path = "../../../mqtt-zapp-core/tests/support/mod.rs"] mod support;`.

**Interfaces:**
- Consumes `App::new`, `App::startup_effect`, `Session::new` / `Session::apply`, `App::apply_core_event`, `App::handle_key`, `App::on_tick`, and `ui::render` with `ratatui::backend::TestBackend`. It is the same sequence as `event_loop` in `tui/mod.rs`, minus the terminal input and signals.
- The test helper is `async fn pump_until(app, session, pred: impl Fn(&App) -> bool, secs: u64)`. It awaits `session.rx` events (with a timeout), applies them, and calls `on_tick` until `pred` holds, and panics on timeout.

- [ ] **Step 1: Write the test** `tui_against_mosquitto`, which returns early (skips) when `support::start(false)` is `None`:
  1. A side `Connection` publishes retained `zapp/e2e/temp` = `{"t":21.5}` and `zapp/e2e/other` = `x`.
  2. Build an `App` for an ad-hoc profile on the test port with `subscriptions = ["zapp/#"]` and a temp `MQTT_ZAPP_CONFIG_DIR` (as existing app tests do), then apply its `startup_effect` through the `Session`.
  3. Run `pump_until` until both topics are in `app.tree`.
  4. Send the keys `/`, `t`, `e`, `m`, `p`, `enter`, select the `temp` row, and render to an 80×24 `TestBackend`. The buffer contains `temp` and `21.5`, and doesn't contain `other`.
  5. Press `x` then `y` (delete retained), and pump until the side connection, on a fresh subscribe, sees no retained message on `zapp/e2e/temp` within 2 s.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp tui_against_mosquitto -- --nocapture`. Expected: PASS with mosquitto installed. Temporarily breaking the filter (for example skipping `rebuild_filter`) should make it FAIL. Check this once, then revert.
- [ ] **Step 3:** Run the CI gate, including `--no-default-features`. Expected: PASS.
- [ ] **Step 4:** Commit `test(tui): headless end-to-end run against mosquitto`.

---

## Out of scope

- Setting MQTT 5 properties when publishing (display only for now).
- Encoding or publishing Sparkplug payloads, and Sparkplug STATE/host-application semantics.
- Per-series y-axes or normalised charts.
- Pushing and tagging (item 1 of the previous list): that waits for the user's go-ahead.
