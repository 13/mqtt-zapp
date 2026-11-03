# Picker Order, Topic Header Card, Mouse Copy: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sort the broker picker (custom, name, last used) and reorder it with `J`/`K`, saving the order to `profiles.toml`. Replace the one-line topic header with a boxed header card. Let the mouse copy the topic and the payload.

**Architecture:** The custom order is the order of `ProfileStore::profiles` in `profiles.toml`. The picker shows a *view* (index list) over that vector, sorted by the current `PickerSort`. Data that changes on its own (last-used times, the picker sort) goes in a new core state file, never in the user's config files. The header card is drawn in `message_view.rs`, which records its clickable rects in `HitAreas` the way the panes already do. Mouse copies reuse `App::copy` and `Effect::Copy` (OSC 52).

**Tech Stack:** Rust, ratatui 0.29, crossterm, `toml`, `directories` 5 (`ProjectDirs::state_dir`).

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md` (overall design). The user skipped a separate design doc for this round, so the decisions are recorded under *Decisions* below.

## Decisions

From the user:
- Profiles can be sorted, including by a custom order.
- `J`/`K` move the selected profile down/up, and the new order is saved to `profiles.toml`.
- The topic header uses mockup C: a rounded box. Row 1 is the path, with the parents dimmed and the leaf as a cyan badge. Row 2 holds the badges (kind in cyan, size, QoS, RETAINED in magenta only when set), with the time and a live age on the right.
- The topic and the payload can be copied with the mouse.

Claude's calls (the user can override any of these):
- `s` in the picker cycles **custom → name → last used**. The picker title shows the current mode: `mqtt-zapp · pick a broker · custom`.
- `J`/`K` outside custom mode switch to custom first, keeping the selected profile, then move it.
- `last_used` (unix seconds per profile name) and `picker_sort` live in `state.toml` in the XDG state dir. Connecting never rewrites `profiles.toml` or `settings.toml`.
- The live age reads `now`, `12s ago`, `4m ago`, `3h ago` or `2d ago`, and redraws once a second.
- Header: a topic too long for the row loses its middle (`tasmota/…/SENSOR`); the leaf always stays. If the message area is shorter than 14 rows, or the panes are stacked (width < 80), the header falls back to a single unboxed row of the same parts.
- Mouse copy:
  - **left click on the header's path row** copies the topic;
  - **left click on `⧉ copy`** (right side of the payload pane's top border) copies the shown payload (history-aware, the same as `Y`);
  - **right click on a tree row** copies that row's topic.
  
  Each copy shows the existing "copied …" note. Shift-drag terminal selection stays as it is.

## Global Constraints

- The musl build has no keyring (`--no-default-features`), and everything here must build and pass `cargo test` there as well.
- Profile and topic text drawn on screen goes through `sanitize` / one-line helpers, as it does today.
- Widths are display widths (`unicode-width`), never byte or char counts.
- A failure to read or write `state.toml` must never block connecting. Fall back to defaults and show a note.
- New keys are remappable: action names `move_up` (default `K`) and `move_down` (default `J`).
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.

## Review Focus

1. Someone edits `profiles.toml` by hand while the picker is open, then presses `J`. The save must keep on-disk profiles the picker doesn't know about (append them in their file order) and drop none. Test: `persist_order_keeps_profiles_added_on_disk` (Task 2).
2. `state.toml` is missing, corrupt or unwritable. The picker opens with defaults, and connect still works. Tests: `state_corrupt_file_defaults`, `save_state_failure_is_a_note` (Tasks 1, 4).
3. A profile is renamed or deleted. Its `last_used` entry follows the rename and is dropped on delete, so last-used order never shows a ghost. Test: `last_used_follows_rename_and_delete` (Task 4).
4. A very long or wide-unicode topic at 80 columns. The header never overflows the box and keeps the leaf. Test: `header_long_cjk_topic_keeps_leaf` (Task 5).
5. A `received_at` in the future (clock skew or a retained message from a skewed box). The age shows `now` and never panics. Test: `age_future_is_now` (Task 5).

---

### Task 1: Core state file (`state.toml`)

**Files:**
- Create: `crates/mqtt-zapp-core/src/state.rs`
- Modify: `crates/mqtt-zapp-core/src/lib.rs` (add `pub mod state;`)

**Interfaces:**
- Produces:
  - `pub enum PickerSort { Custom, Name, LastUsed }`: serde `snake_case`, `Default = Custom`, with `fn name(self) -> &'static str` (`"custom"`, `"name"`, `"last used"`) and `fn next(self) -> PickerSort`.
  - `pub struct AppState { pub picker_sort: PickerSort, pub last_used: BTreeMap<String, i64> }`: `#[serde(default)]`, `Clone, Debug, Default, PartialEq`.
  - `pub fn state_dir() -> PathBuf` resolves in this order: `MQTT_ZAPP_STATE_DIR`, else `MQTT_ZAPP_CONFIG_DIR` (so the existing test helper isolates state too), else `ProjectDirs::state_dir()`, else `config::config_dir()`.
  - `impl AppState { pub fn load() -> AppState; pub fn save(&self) -> Result<()>; pub fn touch(&mut self, name: &str, now: i64); pub fn rename(&mut self, old: &str, new: &str); pub fn forget(&mut self, name: &str) }`. `load` never fails: a missing or corrupt file gives `AppState::default()`. `save` creates the dir and writes atomically (temp file + rename).

- [ ] **Step 1: Write failing tests** in `state.rs`. They follow the `MQTT_ZAPP_CONFIG_DIR` temp-dir pattern from `config.rs` tests:
  - `state_round_trip`: `touch("a", 10)`, `picker_sort = Name`, `save`, `load` gives an equal value.
  - `state_missing_file_defaults` and `state_corrupt_file_defaults` (file contains `not = [toml`): `load() == AppState::default()`.
  - `picker_sort_cycles`: `Custom.next() == Name`, `Name.next() == LastUsed`, `LastUsed.next() == Custom`.
  - `rename_and_forget`: after `touch("a",1); rename("a","b")`, `last_used == {"b":1}`; then `forget("b")` empties it.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp-core state::`. Expected: FAIL (module missing).
- [ ] **Step 3:** Implement the interfaces above.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp-core state::`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(core): state.toml for picker sort and last-used times`

### Task 2: Reorder profiles on disk

**Files:**
- Modify: `crates/mqtt-zapp-core/src/config.rs` (`impl ProfileStore`)
- Modify: `crates/mqtt-zapp/src/tui/profiles.rs`

**Interfaces:**
- Produces:
  - `ProfileStore::reorder(&mut self, names: &[String])` puts the profiles named in `names` first, in that order. Profiles not named keep their relative order after them, and unknown names are ignored.
  - `profiles::persist_order(names: &[String]) -> Result<ProfileStore>` does a fresh `ProfileStore::load()`, then `reorder`, then `save`, and returns the store. Like `persist_edit`, it refuses to overwrite a file it couldn't parse.

- [ ] **Step 1: Write failing tests:**
  - `config.rs`, `reorder_moves_named_first`: store `[a,b,c]`, `reorder(["c","a"])` gives `[c,a,b]`; `reorder(["zz"])` leaves it unchanged.
  - `profiles.rs`, `persist_order_keeps_profiles_added_on_disk`: on disk `[a,b,x]`, `persist_order(["b","a"])` gives a file order of `[b,a,x]`.
  - `profiles.rs`, `persist_order_broken_file_is_not_overwritten`: mirror `broken_file_is_not_overwritten`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp-core reorder` and `cargo test -p mqtt-zapp persist_order`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Rerun both. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: reorder profiles in profiles.toml`

### Task 3: Picker view, sort and move

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/picker.rs`

**Interfaces:**
- Consumes: `PickerSort`, `AppState` (Task 1), `natural_cmp` from `mqtt_zapp_core::tree`.
- Produces (on `PickerState`):
  - Fields: `profiles: Vec<Profile>` (file order, unchanged), `sel` (now an index into `view`), `sort: PickerSort`, `last_used: BTreeMap<String, i64>`, private `view: Vec<usize>`.
  - `fn new(profiles: Vec<Profile>, state: &AppState) -> PickerState`.
  - `fn visible(&self) -> impl Iterator<Item = &Profile>` iterates in view order. `selected()` keeps its signature.
  - `fn cycle_sort(&mut self)` keeps the same profile selected.
  - `fn move_selected(&mut self, down: bool) -> bool` switches to `Custom` if needed, swaps within `profiles`, rebuilds the view and keeps the selection on the moved profile. It returns `false` (and changes nothing) at the edge.
  - `fn order(&self) -> Vec<String>` returns profile names in file order.
  - `fn set_profiles(&mut self, profiles: Vec<Profile>, select: Option<&str>)` rebuilds the view and selects `select` by name, else keeps the index, clamped.
- Sorting: `Name` uses `natural_cmp` on the name. `LastUsed` puts the newest first; profiles never used go last, ordered by file position.

- [ ] **Step 1: Write failing tests:**
  - `sort_by_name_is_natural`: names `[b10, a, b2]` in name mode show `[a, b2, b10]`.
  - `sort_by_last_used_newest_first_unused_last`
  - `cycle_sort_keeps_selection`
  - `move_down_in_name_mode_switches_to_custom`: asserts `sort == Custom`, the moved profile is still selected, and `order()` changed.
  - `move_at_edge_is_noop`: `K` on the first or `J` on the last returns `false` and leaves `order()` unchanged.
  - `set_profiles_selects_by_name`
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp picker::`. Expected: FAIL.
- [ ] **Step 3:** Implement. Update `render_picker` to iterate `visible()` and append ` · {sort.name()}` to the title. Update `app.rs:317`, `profile_saved` (`app.rs:465`) and `mouse.rs:73` to use the new API.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS, including the existing picker click tests.
- [ ] **Step 5:** Commit: `feat(tui): picker sort modes and in-memory reorder`

### Task 4: Wire keys, effects and last-used

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/keymap.rs`, `app.rs`, `mod.rs`, `ui.rs` (picker footer hints and help), `README.md` (keys table, Profiles section)

**Interfaces:**
- Consumes: Tasks 1–3.
- Produces:
  - `Action::MoveUp` / `Action::MoveDown`, named `"move_up"` / `"move_down"`, defaults `K` / `J`. They are picker-only and ignored in browse mode.
  - `Effect::SaveOrder(Vec<String>)`: `mod.rs` calls `profiles::persist_order`. On `Ok(store)` it calls `app.profile_saved(store.profiles, selected_name, "order saved")`. On `Err` it calls `app.profile_save_failed(..)`, which reloads the order from disk.
  - `Effect::SaveState(AppState)`: `mod.rs` calls `AppState::save`. On `Err` it sets a warning note `could not save state: …`, and nothing else happens.
  - `CycleSort` in `Mode::Picker` calls `picker.cycle_sort()` and returns `SaveState`.
  - On `Effect::Connect` for a profile whose name is in `picker.profiles`, `mod.rs` calls `touch(name, now)` and saves state before connecting, ignoring errors apart from the note. Ad-hoc `--host` connects record nothing.
  - On a successful `SaveProfile`, the state follows the rename via `rename(old, new)`. On `DeleteProfile` it calls `forget(name)`.

- [ ] **Step 1: Write failing tests** (`app.rs` test module and `keymap.rs`):
  - `keymap::move_defaults`: `J` maps to `MoveDown` and `K` maps to `MoveUp`.
  - `picker_j_returns_save_order`: with profiles `[a,b]`, `J` gives `Effect::SaveOrder(["b","a"])`.
  - `picker_j_at_bottom_returns_none`
  - `picker_s_cycles_and_saves_state`: the effect is `SaveState` with `picker_sort == Name`.
  - `save_state_failure_is_a_note`: `app.state_save_failed("x")` sets a warn note.
  - `last_used_follows_rename_and_delete`: drive `profile_saved` after a rename edit, then after a delete, and assert `picker.last_used`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement. Add `J`/`K` and `s` to the picker footer hints and to the help group for the picker. Add the rows to the README table and one sentence in *Profiles* ("`J`/`K` move a profile; `s` sorts by custom order, name or last used").
- [ ] **Step 4:** Run `cargo test --workspace && cargo test --workspace --no-default-features && cargo clippy --all-targets -- -D warnings`. Expected: all PASS.
- [ ] **Step 5:** Manual check: `MQTT_ZAPP_CONFIG_DIR=$(mktemp -d) cargo run -- profile add --name b --host h` (do the same for `a`), then run `cargo run`. `J` reorders, the file order changes, `s` cycles the title, and a restart keeps the sort.
- [ ] **Step 6:** Commit: `feat(tui): J/K reorder profiles, s sorts the picker, remember last used`

### Task 5: Topic header card

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/message_view.rs` (replace `metadata`), `app.rs` (`on_tick`)

**Interfaces:**
- Produces:
  - `pub fn age(received: SystemTime, now: SystemTime) -> String` returns `now` below 1 s and for future times; otherwise the largest whole unit among `s/m/h/d`, plus ` ago`.
  - `fn fit_topic(topic: &str, width: usize) -> (String, String)` returns (dimmed parents, leaf) fitted to `width` display columns. It shortens the parents in the middle with `…`, and as a last resort truncates the leaf from the left with a leading `…`.
  - `fn header_lines(s: &Shown, topic: &str, kind: &str, time_fmt: &str, now: SystemTime, width: u16) -> [Line<'static>; 2]` gives the two inner rows of the card.
  - `fn header_compact(...) -> Line<'static>` gives the one-row fallback: leaf badge, kind, size, QoS, RETAINED and age.
  - `pub const HEADER_CARD_MIN_HEIGHT: u16 = 14;`
  - In `render_message`, the card is `Constraint::Length(4)` with a rounded `Block` (`BorderType::Rounded`, `theme::MUTED` border), or `Length(1)` compact when `area.height < HEADER_CARD_MIN_HEIGHT` or the panes are stacked. Pass `stacked: bool` in from `ui.rs` (`main.width < STACK_BELOW`).
  - Styles: parents use `theme::MUTED`; the leaf badge is black on `ACCENT`, bold. The kind badge is black on `ACCENT`; size and QoS keep today's reversed-muted chip; RETAINED stays black on magenta. Time and age are muted and right-aligned on row 2. Mono and `NO_COLOR` need no extra work, because `theme::apply` already handles them.
  - `on_tick`: set `dirty` when a message is shown and the whole second changed since the last draw (store `last_age_second: u64` on `App`).

- [ ] **Step 1: Write failing tests** in `message_view.rs` (render to `TestBackend` like the existing `metadata_line` tests):
  - `age_formats`: 0 s → `now`, 12 s → `12s ago`, 240 s → `4m ago`, 3 h → `3h ago`, 2 d → `2d ago`.
  - `age_future_is_now`
  - `header_card_draws_box_path_and_badges` at 100×30: row 1 contains `tasmota/tele/tasmota_A1B2C3/` and ` SENSOR `, row 2 contains ` JSON `, `QoS 0` and `RETAINED`, and the box uses `╭`/`╰`.
  - `header_no_retained_badge_when_not_retained`
  - `header_long_cjk_topic_keeps_leaf`: topic `设备/房间/很长很长很长很长很长很长很长很长/温度` in a 40-wide card; the row's display width is ≤ inner width and the row ends with the ` 温度 ` badge.
  - `header_compact_when_short_or_stacked`: at 79×24 and at 100×12, there is no `╭`, and a single row contains the leaf and `JSON`.
  - Update `metadata_line`, `meta_uses_timestamp_format` and `meta_shows_json_kind` to the new functions. Keep what they assert.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp message_view::`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS, including `stacked_payload_keeps_room_at_79x24` and `help fits 80x24`.
- [ ] **Step 5:** Manual check: `cargo run -- --host 192.168.22.5`; select a Tasmota SENSOR topic and confirm the age ticks. Repeat in an 80×24 terminal and with `NO_COLOR=1`.
- [ ] **Step 6:** Commit: `feat(tui): boxed topic header with path, badges and live age`

### Task 6: Copy with the mouse

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/app/mouse.rs`, `message_view.rs` (record rects, draw `⧉ copy`), `app.rs` (`copy` becomes `pub(super)`, plus a new `copy_topic_of(id)`), `README.md` (Mouse section)

**Interfaces:**
- Consumes: `App::copy(topic: bool) -> Effect` and the header from Task 5.
- Produces:
  - `HitAreas` gains `header_topic: Rect` (the path row, in both card and compact form) and `payload_copy: Rect` (the cells of `⧉ copy`).
  - The payload pane's block gets a right-aligned title `Line::from(" ⧉ copy ").right_aligned()` in `MUTED`; `ACCENT` when the payload is focused.
  - `browse_click` returns `Effect`. A click inside `header_topic` returns `self.copy(true)`; a click inside `payload_copy` returns `self.copy(false)`. Both are checked before the pane-focus branches.
  - A right click (`MouseEventKind::Down(MouseButton::Right)`) on a tree row returns `Effect::Copy { what: "topic", text: full_topic(row) }` and doesn't change the selection.

- [ ] **Step 1: Write failing tests** in the `ui.rs` click tests (they already render, then click through `HitAreas`):
  - `click_header_path_copies_topic`
  - `click_copy_button_copies_payload`
  - `click_copy_button_copies_pinned_history_entry`: pin an older entry and expect its payload.
  - `right_click_tree_row_copies_that_topic_without_selecting`
  - `click_header_with_nothing_shown_is_none`: no selection gives `Effect::None` and no copy effect.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp click_`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Manual check over `ssh localhost` in a terminal with OSC 52 (kitty/foot/wezterm). Click the path and paste, then click `⧉ copy` and paste.
- [ ] **Step 6:** Commit: `feat(tui): copy topic and payload with the mouse`

### Task 7: Split `app.rs` and `ui.rs` (pure move)

Added at the user's request ("do 3,4,5,6,7" from *Later*). There is no behaviour change, so the existing tests are the gate. The test count before and after must be equal.

**Files:**
- `crates/mqtt-zapp/src/tui/app.rs` keeps `App`, `Mode`, `Focus`, `Effect`, `Note`, `new`, `handle_key`, `handle_action` and `on_tick`. Moved out:
  - `app/picker.rs`: picker, password and profile-form handlers (`on_picker_action` … `on_password_key`, `profile_saved`, `profile_save_failed`);
  - `app/publish.rs`: `on_publish_key` … `open_publish`;
  - `app/search.rs`: `on_search_key` … `rebuild_filter`;
  - `app/retained.rs`: `on_confirm_key`, `ask_clear_retained`;
  - `app/tests.rs`: the `mod tests` body.
- `crates/mqtt-zapp/src/tui/ui.rs` becomes `ui/mod.rs` (`draw` and the layout). Moved out:
  - `ui/header.rs` (`header_line`);
  - `ui/tree.rs` (tree pane, `row_spans`);
  - `ui/footer.rs` (status bar, hints, `ellipsize`, `single_line`);
  - `ui/help.rs` (the help overlay);
  - `ui/tests.rs`.
  
  Keep any picker or stats drawing that already lives in its own module where it is.

- [ ] **Step 1:** Run `cargo test -p mqtt-zapp 2>&1 | grep 'test result'` and record the count.
- [ ] **Step 2:** Move the code, widening visibility only to `pub(super)` / `pub(crate)` where it's needed.
- [ ] **Step 3:** Run `cargo test --workspace && cargo clippy --all-targets -- -D warnings`. Expected: PASS with the same count. No file under `tui/` exceeds about 900 lines apart from `tests.rs`.
- [ ] **Step 4:** Commit: `refactor(tui): split app.rs and ui.rs by responsibility`

### Task 8: MQTT 5

**Files:**
- Modify: `crates/mqtt-zapp-core/src/config.rs` (`Profile`), `client.rs`, `crates/mqtt-zapp/src/cli.rs`, `tui/profile_form.rs`, `tui/picker.rs` (`server_url`), `README.md`
- Test: `crates/mqtt-zapp-core/tests/broker.rs`

**Interfaces:**
- `pub enum MqttVersion { V311, V5 }`, serde `"3.1.1"` / `"5"`, `Default = V311`. `Profile.mqtt_version` uses `#[serde(default, skip_serializing_if = "is_default")]`, so existing files round-trip unchanged.
- `client.rs` gets an internal `enum Client { V4(rumqttc::AsyncClient), V5(rumqttc::v5::AsyncClient) }` and `enum Loop { V4(EventLoop), V5(v5::EventLoop) }`, with `async fn poll(&mut self) -> Polled`, where `enum Polled { ConnAck, Publish { topic: String, payload: Vec<u8>, qos: u8, retain: bool }, Disconnect, Other, Err(String) }`. The `run` loop is shared and only works on `Polled`. `Connection::publish`, `subscribe` and `disconnect` dispatch on `Client`.
- v5 options mirror the v4 ones: credentials, transport and TLS reuse `tls_config`, and the max packet size is set to `MAX_PACKET`.
- CLI: `--mqtt-version <3.1.1|5>` for ad-hoc connects and `profile add`. The form gets a `version` cycle field next to protocol. `server_url` appends ` (v5)` for v5 profiles.

- [ ] **Step 1: Write failing tests:**
  - `config.rs`, `mqtt_version_defaults_and_round_trips`: a file without the key gives `V311`, and saving a `V311` profile leaves out `mqtt_version`.
  - `broker.rs`, `receives_published_message_v5` and `reconnects_after_broker_restart_v5`, both following the v4 tests.
  - `cli.rs`, `mqtt_version_flag_parses`.
  - `profile_form.rs`, `version_field_cycles_and_saves`.
- [ ] **Step 2:** Run `cargo test --workspace`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test --workspace && cargo test --workspace --no-default-features && cargo clippy --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: MQTT 5 connections`

### Task 9: Diff against a marked history entry

**Interfaces:**
- `Action::MarkDiffBase`, named `mark_diff_base`, default `m`. It is active when history is focused (or the payload is focused with an entry pinned). It marks the shown entry as the diff base. Pressing `m` on the marked entry unmarks it.
- `App.diff_base: Option<Stamp>`, where `Stamp` identifies a history entry by `received_at` so it survives the ring buffer moving. It is cleared on `select` of another topic, and when the entry falls out of the ring.
- With diff on and a base marked, the diff is base → shown, and the payload title reads `diff vs 12:00:01.234`. Without a base it behaves as today (previous → shown). The marked history row shows `◆`.

- [ ] **Step 1: Write failing tests** in `message_view.rs` / `app` tests:
  - `diff_uses_marked_base`
  - `m_twice_unmarks`
  - `base_cleared_on_topic_change`
  - `base_cleared_when_evicted` (`history_depth = 3`, push 4)
  - `title_names_base_time`
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement. Add help and README rows.
- [ ] **Step 4:** Run `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(tui): diff against a marked history entry`

### Task 10: Search terms (AND) and opt-in regex

**Interfaces (core `search.rs`):**
- `pub fn terms(query: &str) -> Vec<String>` splits on whitespace; `"double quoted"` is one term, and an unclosed quote runs to the end.
- `pub enum QueryMode { Plain, Regex }`. Signatures:
  - `filter(tree, query, mode) -> Result<Option<Filter>, String>`, where `Err` carries a regex error;
  - `match_ranges(haystack, query, mode) -> Vec<Range<usize>>`, which merges overlapping ranges.
- Plain matching: every term must match, case-insensitive, in the full path *or* the latest payload. Each term may match in a different one. A single term behaves exactly as today.
- Regex matching: the whole query is one case-insensitive `regex::Regex`. Add the `regex` crate (pure Rust).
- TUI: `ctrl-r` in the search input toggles regex. The filter chip reads `filter: … (N)` or `regex: … (N)`. An invalid regex turns the chip red with the error and keeps the last valid filter.

- [ ] **Step 1: Write failing tests:**
  - `terms_split_and_quotes`
  - `and_terms_across_path_and_payload` (`wzr` in the path, `online` in the payload)
  - `and_requires_all`
  - `single_term_unchanged` (existing tests stay)
  - `regex_mode_matches`
  - `invalid_regex_is_err`
  - `match_ranges_multi_term_merged`
  - TUI: `ctrl_r_toggles_regex_chip` and `invalid_regex_keeps_last_filter`
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement. Extend the search benchmark-style test `search_5k_topics_under_50ms` (release-agnostic bound: 200 ms in debug).
- [ ] **Step 4:** Run `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(search): AND terms, quoted phrases, opt-in regex (ctrl-r)`

### Task 11: Total memory cap and `auto_expand`

**Interfaces:**
- Settings:
  - `max_history_mb: usize`, default `256`. It caps the bytes of payloads held in history across all topics. A node's latest message is never evicted.
  - `auto_expand: usize`, default `0` (today's behaviour). New nodes at depth < N expand when they first appear. Depth 1 is a top-level node.
- `TopicTree::with_limits(history_depth, max_bytes)`, `history_bytes(&self) -> usize`. On insert, once the total goes over the cap, it drops the globally oldest history entry, using a FIFO `VecDeque<(NodeId, Stamp)>` and skipping stale entries, and repeats until it is under the cap. It never removes the last remaining entry of a node.
- `App` applies `auto_expand` in `apply_core_event` for newly created nodes only, so collapsing one by hand sticks.

- [ ] **Step 1: Write failing tests:**
  - `tree.rs`, `memory_cap_evicts_oldest_globally`: cap 10 B, payloads of 4 B on `a`, `b`, `a`; the oldest `a` entry goes first.
  - `tree.rs`, `memory_cap_keeps_latest_of_each_node`
  - `tree.rs`, `history_bytes_tracks_depth_eviction`
  - `config.rs`, `settings_limits_parse_and_default`
  - `app`, `auto_expand_opens_new_nodes_to_depth`
  - `app`, `auto_expand_respects_manual_collapse`
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement. Add both to the README *Settings* block.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat: total history memory cap and auto_expand setting`

---

## Later (not in this plan; candidates for the next round)

In suggested order:

1. **Ship:** `main` is 54 commits ahead of `origin` and has no tags. Push, tag `v0.1.0`, and smoke-test the musl binary on a headless box (only with the user's go-ahead).
2. **CI:** add a `cargo build/test --no-default-features` job, so the release variant is checked on every push.
3. **Split `app.rs` (≈2000 lines) and `ui.rs` (≈1770)** into per-overlay submodules (stats, chart, confirm, help, picker), following `app/mouse.rs`.
4. **MQTT 5** (spec §4.1): a profile `version` field and the rumqttc `v5` event loop.
5. **Diff against the pinned history entry** (spec §4.4, 1.x).
6. **Search: space-separated AND terms**, plus an opt-in regex mode (spec §4.3).
7. **A total memory cap** across all topics' history (spec §7), plus an `auto_expand` setting (spec §4.8).
8. **A search benchmark test** at about 5k topics × 1 KB, plus one headless TUI test against Mosquitto.
9. **Sparkplug B decoding and multi-series charts**, only if they're needed.
