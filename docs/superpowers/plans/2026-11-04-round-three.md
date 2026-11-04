# Round Three Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** This round adds the fixes, features and housekeeping the user picked on 2026-11-04, everything except shipping:
- dotted chart paths, Sparkplug wire size and alias names;
- a stable header, polish, and a scrollable help;
- quick field finding, re-send from history, MQTT 5 publish properties with reply follow;
- offline markers, favourites, saving to a file;
- file splits and shared properties memory.

**Architecture:**
- **Core** (`mqtt-zapp-core`) gets the protocol and data work: path resolution, Sparkplug alias state, outgoing v5 properties, offline detection, history export.
- **TUI** gets the keys, overlays and rendering, in the submodules split out in Task 1.
- **New keys** go through the keymap, the README table and the help overlay. The help overlay becomes scrollable first (Task 7), so it can take them.

**Tech Stack:** Rust 2024, rumqttc 0.25, ratatui 0.29, serde (gets the `rc` feature, no new crate), chrono (existing) for file timestamps.

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md`: §1 (the daily loop: find, watch, diff, publish), §4.2 tree, §4.4 Sparkplug, §4.5 charts, §4.6 publish, §4.8 settings; §5 non-goals (no persistence of received messages *across restarts*; saving to a file on request is not that).

## Global Constraints

- CI gate per task:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo clippy --no-default-features --all-targets -- -D warnings`
  - `cargo test --workspace --no-default-features`
- No new crates. Enabling a feature of an existing dependency (serde `rc`) is allowed.
- Every new action needs three things:
  - a row in `Action::DEFAULTS` (`keymap.rs`);
  - a README key-table row, which the test `readme_key_table_matches_defaults` checks;
  - an entry in a `HELP_GROUPS` group (`ui/overlays.rs`).

  Hints use `key_for(app, Action::…)`, never a literal key.
- Wire-derived strings shown on screen go through `payload::sanitize`.
- Tests that touch config, state or cwd use the shared `crate::tui::test_env` guard and never touch the real `~/.config` or `~/.local/state`. File-writing tests write into a tempdir only.
- One commit per task, conventional subject, ending with the session's attribution lines.

## Review Focus

1. **Saved file names come from topics, which are wire data** (`../../etc`, `$SYS/x`, 300-char levels, unicode, NUL). The name must stay inside the target dir, be bounded, and never overwrite an existing file. Test: `save_name_is_safe_and_unique` (Task 13).
2. **Sparkplug alias state is driven by untrusted publishers** (millions of distinct aliases, a DATA message before any BIRTH, a rebirth with new names). Memory must stay bounded, and an unknown alias stays `alias:N`. Tests: `aliases_are_capped`, `data_before_birth_keeps_alias_keys`, `rebirth_replaces_names` (Task 5).
3. **Re-send from history of a binary or truncated payload** must never publish a corrupted or cut payload silently. Test: `resend_refuses_truncated_and_binary` (Task 9).
4. **Reply follow** must not hijack the selection forever. It only fires once, within 30 s, on the exact response topic. Test: `reply_follow_expires_and_fires_once` (Task 10).
5. **Offline detection must not flag ordinary payloads.** A sensor reporting `0`, or a JSON with `"state":"OFF"`, is not offline. Test: `offline_only_for_status_topics` (Task 11).

---

### Task 1: Split the big files again (pure move)

**Files:**
- `crates/mqtt-zapp/src/tui/profile_form.rs` (1041 lines): move the inline `mod tests` to `profile_form/tests.rs`, and the render half (`input_spans`, `choice`, `value_spans`, `render`) to `profile_form/render.rs`.
- `crates/mqtt-zapp/src/tui/app.rs` (1020): move `handle_key` and its per-mode key handlers (`on_help_key` and any other `on_*_key` still in `app.rs`) to `app/keys.rs`. Move `handle_action` to `app/actions.rs`.
- `crates/mqtt-zapp/src/tui/message_view.rs` (944): move the header code (`badges`, `card_title`, `header_lines`, `time_spans`, `header_compact`, `props_line`, `fit_topic`, `take_width`, `leaf_span`, `copy_label`, `copy_rect`) to `message_view/header.rs`. Move `render_history` and `fit_preview` to `message_view/history.rs`.
- `crates/mqtt-zapp/src/tui/app/tests.rs` (2201) and `ui/tests.rs` (1823): split by area into `app/tests/{mod.rs,chart.rs,search.rs,retained.rs,publish.rs,picker.rs,nav.rs}` and `ui/tests/{mod.rs,tree.rs,bars.rs,overlays.rs,mouse.rs}`. The shared helpers go in each `mod.rs`. Choose the area by the test's subject, and leave unclear ones in `mod.rs`.
- **Rules:**
  - The move is pure: no body changes, doc comments move with their items, and visibility is widened only as far as needed.
  - The test count is equal before and after.
  - Every production `.rs` file under `tui/` ends under 800 lines (`wc -l`). If one can't get there by moving only the listed items, report the counts instead of moving unlisted items.

- [ ] **Step 1:** Record the test count, summing the `test result: ok. N passed` lines.
- [ ] **Step 2:** Move the items.
- [ ] **Step 3:** Run the CI gate. Expected: PASS, with the same test count.
- [ ] **Step 4:** Commit `refactor(tui): split profile form, key handling, message header/history and the big test files`.

### Task 2: Share MQTT 5 properties between `latest` and history

**Files:** `crates/mqtt-zapp-core/Cargo.toml` (serde `features = ["derive", "rc"]`), `payload.rs`, `client.rs`, `tree.rs`, and every `props: Some(Box…)` site.

**Interfaces:**
- `Message.props: Option<std::sync::Arc<Props>>`, changed from `Box`. `latest` and the history entry then share one allocation, just as they already share the payload `Bytes`.

- [ ] **Step 1: Failing test** (`tree.rs`): `latest_and_history_share_props`. Insert a message with props, then `Arc::ptr_eq(latest.props, history.back().props)` holds.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp-core latest_and_history_share_props`. Expected: FAIL to compile (`Box` has no `ptr_eq`).
- [ ] **Step 3:** Switch to `Arc`. `Message::size()` keeps counting the props once per history entry: the history bytes are a budget, not an exact heap count, so leave the accounting as it is.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `perf(core): share MQTT 5 properties between latest and history`.

### Task 3: Chart fields whose names contain `.`

**Files:** `crates/mqtt-zapp-core/src/payload.rs` (`numeric_at`, ~line 342). Test: inline.

**Interfaces:**
- `numeric_at(payload, Some(path))` resolves `path` against the JSON value by walking it. At an object, it takes the **longest** key that equals the remaining path or is a prefix of it followed by `.`. At an array, it takes the leading numeric segment. Paths without dotted keys resolve exactly as before.
- `numeric_paths` is unchanged: it already joins keys with `.`.

- [ ] **Step 1: Failing tests:**
  - `numeric_at_dotted_key`: `{"metrics":{"Inputs.A":3}}` with path `metrics.Inputs.A` → `Some(3.0)`.
  - `numeric_at_prefers_longest_key`: `{"a.b":1,"a":{"b":2}}` with path `a.b` → `Some(1.0)`.
  - `numeric_at_array_then_dotted`: `{"x":[{"y.z":5}]}` with path `x.0.y.z` → `Some(5.0)`.
  - `every_numeric_path_resolves`: for `{"a.b":{"c":1},"d":[2,{"e.f":3}]}`, every path from `numeric_paths` gives `Some` from `numeric_at`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp-core numeric_at_ every_numeric_path`. Expected: FAIL (`None` for the dotted keys).
- [ ] **Step 3:** Implement the walk. It runs in O(depth × keys), with no new allocation per key compare.
- [ ] **Step 4:** Run the CI gate. Expected: PASS, with the existing chart tests unchanged.
- [ ] **Step 5:** Commit `fix(core): chart JSON fields whose names contain dots`.

### Task 4: Sparkplug size badge shows the wire size; a cut Sparkplug message keeps its badge

**Files:** `payload.rs` (`Message.wire_len: Option<usize>`), `client.rs` (`incoming`), `message_view` (`body`, badges), plus every `Message {…}` literal (`wire_len: None`).

**Interfaces:**
- `Message.wire_len: Option<usize>` is `Some(n)` when ingest replaced the payload, which today means Sparkplug decoding. `n` is the received length.
- The size badge shows `human_bytes(msg.wire_len.unwrap_or(payload.len()))`.
- The kind label is `SPARKPLUG` whenever `msg.sparkplug`, even when truncation turned the body into text.

- [ ] **Step 1: Failing tests:**
  - `sparkplug_badge_shows_wire_size` (message_view): a message with `sparkplug: true`, `wire_len: Some(120)` and a 600-byte JSON payload shows the badge `120 B`, not `600 B`.
  - `cut_sparkplug_keeps_badge`: the same message with `truncated_from: Some(..)` still shows `SPARKPLUG`.
  - `incoming_records_wire_len` (client.rs): a decoded NDATA records `wire_len == Some(raw.len())`; a raw payload has `None`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix: Sparkplug size badge shows the wire size and survives truncation`.

### Task 5: Sparkplug metric names from BIRTH

**Files:** `crates/mqtt-zapp-core/src/sparkplug.rs`, `client.rs` (the `run` loop owns the state and `incoming` borrows it). Test: inline.

**Interfaces:**
- `pub struct Aliases`, with `Default`, and these methods:
  - `pub fn decode(&mut self, topic: &str, payload: &[u8]) -> Option<serde_json::Value>` replaces the bare `decode` call at ingest.
  - `pub fn len(&self) -> usize` and `pub fn is_empty(&self) -> bool`.
- The key is `(group, edge_node, Option<device>)`, parsed from `spBv1.0/<group>/<TYPE>/<edge>[/<device>]`.
- **NBIRTH** replaces the node's map and clears its devices' maps. **DBIRTH** replaces the device's map. A metric that has both a name and an alias records `alias → name`. Every other type only reads the map.
- **Lookup:** in DATA and CMD messages, an alias-only metric becomes its known name. When the alias is unknown it stays `alias:N`. When that name is already a key in the same message, the decoded entry overwrites it.
- **Cap:** `const MAX_ALIASES: usize = 100_000` total. A BIRTH that would go past it records nothing new. Its own decode still works, and the cap is logged once as a note-free no-op, with no panic.
- The internal decoder returns `(Value, Vec<(String, u64)>)`, the name/alias pairs it saw, so BIRTH can learn them without a second parse.

- [ ] **Step 1: Failing tests** (fixtures with the existing test encoder):
  - `data_uses_birth_names`: NBIRTH has `Temperature` with alias 7, then NDATA has alias 7 = 21.5 → `metrics.Temperature == 21.5`.
  - `device_aliases_are_separate`: DBIRTH for d1 has alias 1 = `A`, and for d2 alias 1 = `B`. DDATA on d2 with alias 1 → key `B`.
  - `data_before_birth_keeps_alias_keys`.
  - `rebirth_replaces_names`: a second NBIRTH maps alias 7 to `Temp2`, so the next NDATA uses `Temp2`.
  - `nbirth_clears_device_maps`.
  - `aliases_are_capped`: BIRTHs totalling more than `MAX_ALIASES` give `len() <= MAX_ALIASES`, with no panic.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp-core sparkplug`. Expected: FAIL.
- [ ] **Step 3:** Implement it, and give `run` an `Aliases` that `incoming` borrows. A reconnect keeps the map: the broker's retained BIRTHs come again anyway.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. README: in the Sparkplug bullet, add "metric names from BIRTH replace aliases".
- [ ] **Step 5:** Commit `feat(core): name Sparkplug metrics from BIRTH aliases`.

### Task 6: Stable message header

**Files:** `crates/mqtt-zapp/src/tui/message_view.rs` and `message_view/header.rs` (from Task 1). Test: `message_view` tests.

**Interfaces:**
- `fn props_row_shown(node: &TopicNode, msg: &Message) -> bool` is true when `msg` or **any** entry of `node.history` has props. `render_message`, `history_rows` and `settle_history_focus` all use it, so the boxes don't move while you browse history. An entry without props shows the row as ` no properties`, muted.
- `props_line`: when not even the first chip fits beside ` +N more`, the first chip is cut with `…` to fill the room.

- [ ] **Step 1: Failing tests:**
  - `header_height_stable_across_history`: in a history where only entry 1 of 3 has props, the payload box's top row is the same for every `history_sel`.
  - `entry_without_props_says_so`.
  - `oversized_first_chip_is_cut`: `content_type` is 1 KB at width 60, and the row contains `type ` and `…` and stays within 60 columns.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix(tui): keep the message header height stable across history`.

### Task 7: Polish batch, and a scrollable help

**Files:** `message_view.rs` (`render_payload`), `message_view/chart.rs` (title), `app/chart.rs` (`close_chart` note), `ui/overlays.rs` (`render_help`), `app/keys.rs` (help keys), `README.md`.

**Interfaces and rules:**
- **No underline in hex dumps:** `render_payload` skips `mark_matches` when the body's kind label is `BINARY`.
- **Coloured overlay title:** with more than one series, the title's names are drawn as spans styled with each series' `SERIES_COLORS` entry. The ` + ` separators stay muted. The text is unchanged (`chart · b + a`).
- **`close_chart` note:** it reads `chart off, pins cleared` when pins were dropped, otherwise `chart off`.
- **README:** replace "`esc` closes it at any point" with "`esc` closes it (in the search input, `esc` clears the search first)".
- **Help scrolls:** when the help content is taller than the overlay, `up`/`down` and `pageup`/`pagedown` scroll it (not `j`/`k`: typing filters the help). The bottom border shows `N-M/T` like the payload pane. Typing still filters, and a filter resets the scroll to 0. The overlay keeps its current size.
- **The help test changes:** `help_shows_every_action_at_80x24` becomes `help_reaches_every_action_at_80x24`. Scrolling to the end must reveal every action name. This makes room for the six new actions in Tasks 8–13.

- [ ] **Step 1: Failing tests:**
  - `hex_dump_not_underlined`.
  - `overlay_title_names_coloured`.
  - `esc_note_says_pins_cleared`.
  - `help_scrolls_with_arrows`.
  - `help_filter_resets_scroll`.
  - `help_reaches_every_action_at_80x24`, replacing the old one.
- [ ] **Step 2:** Run them. Expected: FAIL. The help test fails only once the scrolling assertions exist. The old test is deleted in this step.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix(tui): polish (hex underline, overlay colours, esc note) and a scrollable help`.

### Task 8: Find a field fast: chart backwards and a field picker

**Files:** `keymap.rs`, `app/chart.rs`, a new `crates/mqtt-zapp/src/tui/field_picker.rs` (the overlay state and render), `app.rs` (`Mode::FieldPicker`), `app/keys.rs`, `ui.rs` (draw the overlay), `ui/bars.rs` (hints), `ui/overlays.rs` (`HELP_GROUPS` "view"), `README.md`.

**Interfaces:**
- `Action::ChartPrev`, name `chart_prev`, default key `alt-c`. It steps the current series backwards, with the same skipping of pinned paths as `c`. From off it goes to the **last** free series, and past the first it goes to off.
- `Action::ChartPick`, name `chart_pick`, default key `v`. It opens `Mode::FieldPicker` over the shown payload's `chart_series()`. Empty series → note `no numeric values to chart`.
- `pub struct FieldPicker { query: String, sel: usize }`, with these behaviours:
  - Typing filters the list case-insensitively by substring of the display name (`value` for `None`).
  - `up`/`down` and `ctrl-p`/`ctrl-n` move the selection. These are fixed keys in the overlay, like the search input.
  - `enter` sets `chart` to the selected series and closes the overlay.
  - `esc` closes it without change.
  - `backspace` edits the query.
  - Each row shows the name and the field's current value. Matches are underlined with `MATCH_MODIFIER`.
- The README rows go after `chart_pin`. The Keys intro sentence becomes "`c`/`alt-c` step the chart through the numeric fields, `v` picks one by name; `esc` closes it …".

- [ ] **Step 1: Failing tests:**
  - `chart_prev_steps_back`: payload `{"a":1,"b":2,"c":3}`. `alt-c` from off → `c`, again → `b`, again → `a`, again → off.
  - `chart_prev_skips_pinned`.
  - `picker_filters_and_charts`: 50 fields `f0`…`f49`. Press `v`, type `f4`, `down`, `enter` → `chart == Some(Some("f40"))`, because the list in order is f4, f40…f49.
  - `picker_esc_changes_nothing`.
  - `picker_empty_payload_notes`.
  - `picker_renders_values_and_underlines`: render test, with the row `f40` and its value `40` drawn.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `feat(tui): chart backwards with alt-c and pick a field by name with v`.

### Task 9: Re-send from history

**Files:** `app/publish.rs` (`open_publish`), `publish.rs` (`PublishForm::prefilled`), `README.md` (Publishing section). Test: `app/tests/publish.rs`.

**Interfaces:**
- `PublishForm::prefilled(topic: &str, payload: &str, qos: u8, retain: bool) -> PublishForm`.
- When `p` is pressed with the history focused **and** an entry selected (`history_sel.is_some()`), the form is prefilled with that entry's topic, payload, QoS and retain.
- Otherwise `p` behaves as today.
- Refusals: a truncated entry gives the note `can't re-send: payload was truncated`, and a payload that is not UTF-8 gives `can't re-send: binary payload`. In both cases the form opens with the topic only.
- A Sparkplug-decoded entry prefills its JSON, with the note `re-sending Sparkplug as JSON`. The re-send publishes JSON, not protobuf, and the note says so.

- [ ] **Step 1: Failing tests:**
  - `resend_prefills_from_history`.
  - `resend_needs_history_focus`: tree focus → empty payload, as today.
  - `resend_refuses_truncated_and_binary`.
  - `resend_sparkplug_notes_json`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. Add a README sentence: "In the history pane, `p` opens the form with that message filled in."
- [ ] **Step 5:** Commit `feat(tui): re-send a history entry from the publish form`.

### Task 10: MQTT 5 properties when publishing, and reply follow

**Files:**
- Core: `client.rs` (`Client::publish` and `Connection::publish` take props).
- TUI: `publish.rs` (form fields), `app.rs` (`Effect::Publish.props`, reply state), `mod.rs` (`Session::apply` passes props), `app/publish.rs`, `README.md`.

**Interfaces:**
- **Core:** `Connection::publish(&self, topic, payload, qos, retain, props: Option<&Props>) -> Result<()>`. On V5 it maps `content_type`, `response_topic`, `correlation_data` and `user` to rumqttc `PublishProperties`, and on V4 it ignores them. Update every caller; `clear_retained` passes `None`.
- **Form:** `PublishForm::new(topic, v5: bool)`. With `v5` it adds three single-line fields after the payload:
  - `content type`;
  - `response topic`;
  - `user props`, typed as `k=v, k2=v2`. Split on `,` then on the first `=`, and trim. An item without `=` is a form error: `user props: expected k=v`.

  The fields are hidden on 3.1.1. `Send` carries `props: Option<Props>`, which is `None` when every field is empty.
- **Reply follow:** sending with a response topic sets `App.await_reply = Some((topic, Instant::now()))`. On the first message to exactly that topic within `REPLY_WINDOW = 30 s`, `select` that node (expanding its ancestors), set the note `reply on <topic>`, and clear `await_reply`. After the window it is cleared silently.
- The form's v5 flag comes from the connected profile's `mqtt_version`.

- [ ] **Step 1: Failing tests:**
  - `form_v5_fields_build_props`.
  - `form_user_props_parse_error`.
  - `form_v311_has_no_prop_fields`.
  - `reply_follow_selects_reply` (app).
  - `reply_follow_expires_and_fires_once` (app). Fake the elapsed time by setting the stored `Instant` in the past.
  - `v5_publish_sends_properties` (`broker.rs`): publish with content type and a user property, and a v5 subscriber sees both.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. README Publishing section: describe the v5 fields and reply follow.
- [ ] **Step 5:** Commit `feat: MQTT 5 properties when publishing, and follow the reply`.

### Task 11: Offline and stale markers in the tree

**Files:**
- Core: `crates/mqtt-zapp-core/src/payload.rs` (`says_offline`), `config.rs` (`Settings.stale_after_s`).
- TUI: `ui.rs` (`row_spans`) and `README.md` (settings block, and a tree bullet).

**Interfaces:**
- `pub fn says_offline(leaf: &str, payload: &[u8]) -> bool`. It is true only for a **status leaf**: `LWT`, `lwt`, `online`, `status`, `state`, `connected`, `availability`, case-insensitively. The trimmed payload must also match, case-insensitively, one of these:
  - `offline`, `disconnected`, `dead`, `lost`;
  - `false` or `0`, but only for `online` and `connected`;
  - a JSON object whose `online` or `connected` field is `false`.

  Everything else is false.
- **Row marker:** a row whose own latest payload `says_offline`, or that has a direct child for which it does, gets a red ` offline` chip after the name. The node is offline; its parent (the device) is marked through the child rule.
- **Stale:** `Settings.stale_after_s: u64` (default `0`, off). When it is above 0, a row with a message whose `last_update` is older than that is drawn dim, and the chip ` stale` is muted. Offline wins over stale.

- [ ] **Step 1: Failing tests:**
  - `says_offline_cases`: `("LWT","Offline")` → true; `("online","false")` → true; `("status","{\"online\":false}")` → true.
  - `offline_only_for_status_topics`: these are all false:
    - `("SENSOR","0")`
    - `("POWER","OFF")`
    - `("state","{\"state\":\"OFF\"}")`
    - `("online","true")`
    - `("temperature","offline")`
  - `device_row_marked_offline` (ui render): `tasmota/x/LWT = Offline`, so the `x` row has ` offline`.
  - `stale_rows_dimmed`.
  - `stale_off_by_default`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement. The check runs only on drawn rows and is O(children) per row.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. README: add a Features or tree note, and `stale_after_s = 300 # dim topics silent this long (default 0: off)`.
- [ ] **Step 5:** Commit `feat(tui): mark offline devices and, optionally, stale topics`.

### Task 12: Favourite topics

**Files:**
- Core: `state.rs` (`AppState.favorites`).
- TUI: `keymap.rs`, `app.rs`, `rows.rs` (starred-only view), `ui.rs` (star marker), `ui/bars.rs` (header or footer chip), `ui/overlays.rs`, `README.md`.

**Interfaces:**
- `AppState.favorites: BTreeMap<String, BTreeSet<String>>`, mapping profile name to full topic strings. It uses `#[serde(default)]`, so old `state.toml` files still load.
- `Action::ToggleStar`, name `star`, default key `*`. It toggles the selected topic for the connected profile and saves the state right away, as the picker sort does. The notes are `starred <topic>` and `unstarred <topic>`.
- `Action::StarredOnly`, name `starred_only`, default key `'`. It toggles a view showing only starred topics and their ancestors, expanded. Combined with a search filter, it shows the intersection (starred matches plus ancestors).
  - While the view is on, the footer shows a chip ` ★ starred (N) `.
  - With nothing starred, it sets the note `no starred topics` and doesn't switch.
- A starred row shows `★` before the name, in the accent colour.
- A favourite whose topic hasn't arrived yet in this session is simply not shown. Nothing is created for it.

- [ ] **Step 1: Failing tests:**
  - `star_toggles_and_persists`: under `test_env`, star → `state.toml` contains the topic; unstar → it doesn't.
  - `starred_only_shows_ancestors`.
  - `starred_only_with_search_intersects`.
  - `starred_only_empty_notes`.
  - `favorites_are_per_profile`.
  - `old_state_without_favorites_loads`.
  - `star_marker_rendered`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. Add the README key rows and a sentence under the tree features.
- [ ] **Step 5:** Commit `feat(tui): star favourite topics and show only starred`.

### Task 13: Save to a file

**Files:**
- Core: a new `crates/mqtt-zapp-core/src/export.rs`.
- TUI: `keymap.rs`, `app.rs` (actions), `ui/overlays.rs`, `README.md`.

**Interfaces:**
- `pub fn file_stem(topic: &str, now: chrono::DateTime<chrono::Local>) -> String`:
  - Every char outside `[A-Za-z0-9._-]` becomes `_`.
  - Runs of `_` collapse, and leading `.` and `_` are trimmed.
  - The topic part is cut to 80 bytes, and empty becomes `topic`.
  - It ends with `-YYYYmmdd-HHMMSS`.
- `pub fn write_new(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<PathBuf>` creates the file with `create_new`, so it never overwrites. On `AlreadyExists` it tries `stem-1.ext`, `stem-2.ext` and so on up to `-99`. Files are mode `0600`, like the config files.
- `pub fn history_jsonl(msgs: &[&Message]) -> Vec<u8>`. Each line is `{"time": RFC 3339 with ms, "qos": n, "retain": b, "payload": …}`.
  - `payload` is the JSON value when the payload is a JSON object or array, the string when it is UTF-8, and `{"base64": "…"}` otherwise.
  - It adds `"truncated_from": n` when the entry was cut, and `"sparkplug": true` when the payload was decoded.
- `Action::SavePayload`, name `save_payload`, default key `w`: writes the shown payload's raw bytes into the current directory. The extension is `json`, `txt` or `bin` by kind.
- `Action::SaveHistory`, name `save_history`, default key `W`: writes the topic's whole history (oldest first) as `jsonl`.
- Notes: `saved <path>` on success, `save failed: <error>` on failure. Nothing to save → `nothing to save`.

- [ ] **Step 1: Failing tests** (core, all writing into a tempdir):
  - `save_name_is_safe_and_unique`. The topics `../../etc/passwd`, `$SYS/broker`, a 300-char level, `a\0b` and `ü/ä` all give stems that:
    - contain no `/`, `\0` or `..` prefix;
    - are at most 80 + 16 bytes long;
    - give two different paths when written twice.
  - `history_jsonl_shapes`: one each of JSON, text, binary, truncated and Sparkplug.
  - `write_new_never_overwrites`.
- [ ] **Step 2: Failing app tests:**
  - `w_saves_shown_payload`: under `test_env`, with cwd changed to a tempdir inside the guard and restored on drop.
  - `W_saves_history`.
  - `nothing_to_save_notes`.
- [ ] **Step 3:** Run them. Expected: FAIL.
- [ ] **Step 4:** Implement. If changing cwd in tests proves fragile, give the App a `save_dir: PathBuf` field, defaulting to `.`, that tests set. Record that as a ruling.
- [ ] **Step 5:** Run the CI gate. Expected: PASS. Add the README key rows and a sentence: "`w` saves the shown payload and `W` the topic's history (JSON lines) into the current directory."
- [ ] **Step 6:** Commit `feat: save the shown payload or a topic's history to a file`.

---

## Out of scope

- Shipping (push and tag), as the user asked.
- Encoding Sparkplug for re-send (it re-sends JSON, and says so).
- A favourites section pinned above the tree, as opposed to the starred-only toggle.
- A configurable save directory (cwd only, unless Task 13's ruling adds an internal field).
