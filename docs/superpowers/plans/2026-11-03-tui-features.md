# TUI Features Round 2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** This round has three parts. It fixes the minor issues left over from the TUI polish review. It adds the daily-use features from the spec: copy to clipboard, retained-message handling, sorting, and expanding or collapsing a whole subtree. Then it adds charts, a broker stats view, an `$EDITOR` and history for the publish payload, and theme and timestamp settings.

**Architecture:** Each feature gets a new `Action` in `tui/keymap.rs`, and its default key is listed in the help overlay's `HELP_GROUPS`. Side effects such as writing to stdout, publishing or running `$EDITOR` are returned as new `Effect` variants and carried out by `tui/mod.rs`. That keeps `App` free of side effects and easy to test. Logic that doesn't involve the UI (natural sort, base64, numeric extraction, `$SYS` lookup) goes in the core crate as plain functions with unit tests.

**Tech Stack:** ratatui 0.29 (`Chart`, `Dataset`, `symbols::Marker::Braille`), crossterm 0.28, `unicode-width` 0.2 (already in the dependency tree through ratatui; add it as a direct dependency of `mqtt-zapp`). No other new crates.

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md`: §4.2 (retained, expand/collapse, sort), §4.4 (copy / OSC 52), §4.5 (charts), §4.6 (payload history, `$EDITOR`), §4.7 (broker stats), §4.8 (settings). The leftovers come from the deferred-minor list of the previous plan (`2026-11-03-tui-polish.md`).

## Global Constraints

- Every string from the broker or a profile goes through `sanitize`/`single_line` before it is drawn. The OSC 52 payload is base64, so it can never contain raw control bytes.
- The minimum terminal size stays 40×10. Nothing may panic or wrap at that size.
- All colours come from `theme.rs`.
- Every remappable key goes through `Keymap`. Its `[keys]` name is snake_case.
- Deleting retained messages always asks for confirmation (spec §4.2).
- The core crate must not depend on ratatui or crossterm.
- CI must stay green: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Review Focus

1. **Clearing retained messages on a big subtree:** `X` on `$SYS` or on the root of a 1000-topic tree. The confirmation shows the count, and the publishes happen off the UI loop, so the UI never freezes. Test: Task 3 (`clear_retained_lists_every_retained_topic`, plus the off-loop publish in `mod.rs`).
2. **`$EDITOR` missing or failing:** if `EDITOR` is unset, fall back to `vi`. If the editor exits non-zero or can't be spawned, the terminal is restored and a red note says why. Test: Task 7 (`editor_failure_keeps_payload`).
3. **Chart of a payload that isn't numeric, or whose values are mixed:** non-numeric history entries are skipped. With fewer than 2 points the chart shows `not enough numeric values`. Test: Task 5.
4. **Natural sort with odd names:** empty level `∅`, `a10` vs `a9`, leading zeros, numbers longer than u64. Never panic, and the order is total. Test: Task 4.
5. **NO_COLOR or mono theme:** selection and focus stay visible (REVERSED and thick borders) once colours are stripped. Test: Task 8.

---

### Task 1: Review leftovers

**Files:** `crates/mqtt-zapp/Cargo.toml` (add `unicode-width = "0.2"`), `tui/ui.rs`, `tui/picker.rs`, `tui/app.rs`, `tui/publish.rs`

**Interfaces:**
- `ui::ellipsize(s, max_cols)` measures display columns with `UnicodeWidthStr`/`UnicodeWidthChar`. The header pill width and the picker name padding use display width too.
- Header: when the disconnect reason is cut, the cut goes into the reason, so ` · retry Ns` stays whole.
- With `focus == Payload`, `Collapse`, `Expand`, `ToggleExpand` and `Select` do nothing.
- Footer: the Browse+Tree hint order becomes search, publish, help, quit, focus, expand.
- History hint label: `older/newer`.
- Fix the stale comments ("status bar" becomes "footer", the publish overlay comment, the flash comment).

- [ ] Tests (`ui.rs`):
  - `ellipsize_counts_columns`: `ellipsize("日本語テキスト", 5)` has display width ≤ 5 and ends with `…`.
  - `header_keeps_retry_suffix`: at 40 columns with a long reason, the top line ends with `· retry 4s`.
  - `payload_focus_ignores_tree_keys`: with focus on Payload, `Expand`, `Collapse` and `Select` leave `expanded` and `selected` unchanged.
  - Update `footer_drops_hints_when_narrow`: at 40 columns, `? help` is visible.
- [ ] Watch the tests fail, implement, run `cargo test -p mqtt-zapp`, and commit `fix(tui): display-width truncation, retry suffix, payload focus keys`.

### Task 2: Copy topic / payload (OSC 52)

**Files:** `mqtt-zapp-core/src/payload.rs` (`pub fn base64(bytes: &[u8]) -> String`), `tui/keymap.rs`, `tui/app.rs`, `tui/mod.rs`, `tui/ui.rs` (help)

**Interfaces:**
- Actions: `CopyTopic` (`copy_topic`, `y`) and `CopyPayload` (`copy_payload`, `Y`), both in the help group `actions`.
- `Effect::Copy { what: &'static str, text: String }`. `mod.rs` writes `\x1b]52;c;{base64(text)}\x07` to stdout, flushes, and sets `Note::info("copied {what}")`.
- The payload copied is the message on screen (history-aware), as lossy UTF-8. The topic is `full_topic`. With nothing selected the effect is `None` and the note says `nothing to copy`.

- [ ] Tests:
  - core `base64_matches_rfc4648`: `""` gives `""`, `"f"` gives `"Zg=="`, `"fo"` gives `"Zm8="`, `"foo"` gives `"Zm9v"`, `"foobar"` gives `"Zm9vYmFy"`.
  - app `y_copies_topic_and_Y_copies_shown_payload`: covers a pinned history entry, and that nothing selected gives `Effect::None` plus a note.
- [ ] Watch the tests fail, implement, test, and commit `feat(tui): copy topic or payload to the clipboard via OSC 52`.

### Task 3: Retained marker, delete / clear retained

**Files:** `tui/keymap.rs`, `tui/app.rs` (new `Mode::Confirm`), `tui/ui.rs`, `tui/mod.rs`

**Interfaces:**
- Tree row: when `latest.retain && !latest.payload.is_empty()`, the name is followed by ` ®` in `theme::MUTED`. (An empty payload is how a broker reports a cleared retained message.)
- Actions: `DeleteRetained` (`delete_retained`, `x`) for the selected node only, and `ClearRetained` (`clear_retained`, `X`) for the selected node plus its whole subtree.
- `pub struct Confirm { pub prompt: String, pub topics: Vec<String> }`. `App.confirm: Option<Confirm>` goes with `Mode::Confirm`.
  - The prompt is `delete retained message on {topic}?` or `clear {n} retained messages under {topic}?`.
  - When nothing is retained there is no confirmation, just the note `no retained messages here`.
- In Confirm mode, `y` or `enter` returns `Effect::ClearRetained(topics)` and goes back to Browse. Any other key cancels with the note `cancelled`.
- `mod.rs`: one spawned task publishes an empty retained QoS 0 message to each topic and sends one note: `cleared N retained` or `clear failed: …`.
- Rendering: a centred `theme::pane("confirm", true)` with `dim_backdrop`, showing the prompt and `y yes  any other key: no`. The footer shows `y confirm  esc cancel`.

- [ ] Tests:
  - `retained_marker_on_row`
  - `delete_retained_needs_confirmation`: `x` gives `Mode::Confirm`, `y` gives `Effect::ClearRetained(["a/b"])`, and `n` gives `Effect::None` and Browse.
  - `clear_retained_lists_every_retained_topic`: 3 retained and 1 not under `a`; `X` on `a` lists 3.
  - `nothing_retained_notes_without_confirm`
  - `confirm_overlay_renders_at_40x10`
- [ ] Watch the tests fail, implement, test, and commit `feat(tui): retained marker; delete or clear retained messages with confirmation`.

### Task 4: Sort and subtree expand / collapse

**Files:** `mqtt-zapp-core/src/tree.rs` (`pub fn natural_cmp(a: &str, b: &str) -> Ordering`), `mqtt-zapp-core/src/config.rs` (`Settings.sort: SortOrder`), `tui/rows.rs`, `tui/keymap.rs`, `tui/app.rs`, `tui/ui.rs`

**Interfaces:**
- core: `#[derive(Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)] #[serde(rename_all = "lowercase")] pub enum SortOrder { #[default] Name, Count, Recent }` and `Settings.sort` (TOML `sort = "count"`).
- `natural_cmp` splits names into digit and non-digit runs. Digit runs compare by value: strip leading zeros, then compare length, then the digits. Non-digit runs compare case-insensitively. Fall back to plain byte order so the order is total.
- `visible_rows(tree, expanded, filter, sort: SortOrder)`:
  - `Name` uses `natural_cmp`.
  - `Count` uses `msg_count` descending, then natural.
  - `Recent` uses `last_update` descending (`None` last), then natural.
- Actions:
  - `CycleSort` (`sort`, `s`) cycles name → count → recent and sets the note `sort: {order}`.
  - `ExpandAll` (`expand_all`, `L`) expands the selected node and every descendant that has children.
  - `CollapseAll` (`collapse_all`, `H`) collapses the selected node and all its descendants.
- The header gets no sort indicator. The note is enough (YAGNI).

- [ ] Tests:
  - core `natural_cmp_orders`: `tasmota_2 < tasmota_10`, `a < B`, `x01` vs `x1` is a total order (not `Equal` unless the strings are identical), a 30-digit run doesn't panic, and `"" < "a"`.
  - core `settings_sort_parses`: `sort = "recent"`.
  - rows `sort_by_count_and_recent`
  - app `s_cycles_sort_and_notes`
  - app `expand_all_and_collapse_all_subtree`
- [ ] Watch the tests fail, implement, test, and commit `feat: natural/count/recent sort; expand or collapse a whole subtree`.

### Task 5: Charts

**Files:** `mqtt-zapp-core/src/payload.rs` (`pub fn numeric_paths(payload: &[u8]) -> Vec<String>`, `pub fn numeric_at(payload: &[u8], path: Option<&str>) -> Option<f64>`), `tui/keymap.rs`, `tui/app.rs`, `tui/message_view.rs`

**Interfaces:**
- `numeric_paths` returns the dotted paths (`ENERGY.Power`, array index `a.0`) to every numeric JSON leaf, in document order, capped at 50. A non-JSON payload gives an empty list.
- `numeric_at(p, None)` parses the whole payload, trimmed, as f64. `numeric_at(p, Some(path))` follows the path. Non-finite values give `None`.
- `App.chart: Option<Option<String>>`. `None` means off, `Some(None)` is the plain numeric value, `Some(Some(path))` is a JSON field.
- Action `Chart` (`chart`, `c`) cycles off → plain (only if the shown payload parses as a number) → each path in `numeric_paths(shown payload)` → off. The note names the series (`chart: ENERGY.Power`). Selecting another topic turns the chart off.
- While a chart is on, the payload area is split: the chart gets the top 50% (minimum 5 rows) and the payload text the rest. The chart is a `theme::pane("chart · {series}", false)` containing a ratatui `Chart`: one `Dataset`, `Marker::Braille`, `GraphType::Line`, `theme::ACCENT`. x is the history index (oldest = 0); the y bounds are min and max, padded by 5% (±1 when flat). The y axis has two labels (min and max) formatted with `{:.2}`, trailing zeros trimmed.
- Fewer than 2 numeric points: draw the muted line `not enough numeric values` instead of the chart.

- [ ] Tests:
  - core `numeric_paths_and_at`: `{"ENERGY":{"Power":12,"V":"x"},"a":[1,2.5]}` gives `["ENERGY.Power","a.0","a.1"]`, and `numeric_at(.., Some("a.1")) == Some(2.5)`. Also check that `numeric_at(b" 42 ", None) == Some(42.0)` and `numeric_at(b"nan", None) == None`.
  - app `c_cycles_series`
  - render `chart_renders_braille_series`: some cell in the chart pane has a braille symbol (U+2800..=U+28FF), and the pane title contains `chart · ENERGY.Power`.
  - render `chart_not_enough_values`
- [ ] Watch the tests fail, implement, test, and commit `feat(tui): braille chart of a numeric payload or JSON field over history`.

### Task 6: Broker stats view

**Files:** `mqtt-zapp-core/src/stats.rs` (new; `pub fn broker_stats(tree: &TopicTree) -> Vec<(&'static str, String)>`), `tui/keymap.rs`, `tui/app.rs` (`Mode::Stats`), `tui/ui.rs`

**Interfaces:**
- `broker_stats` reads the latest payload, sanitized and trimmed, of these mosquitto `$SYS` topics, in this order. A topic that is missing is left out.

  | Label | `$SYS` topic |
  |---|---|
  | version | `$SYS/broker/version` |
  | uptime | `$SYS/broker/uptime` |
  | clients connected | `$SYS/broker/clients/connected` |
  | clients total | `$SYS/broker/clients/total` |
  | msgs received/min | `$SYS/broker/load/messages/received/1min` |
  | msgs sent/min | `$SYS/broker/load/messages/sent/1min` |
  | retained | `$SYS/broker/retained messages/count` |
  | subscriptions | `$SYS/broker/subscriptions/count` |
  | bytes received/min | `$SYS/broker/load/bytes/received/1min` |
  | bytes sent/min | `$SYS/broker/load/bytes/sent/1min` |

- Action `Stats` (`stats`, `S`) opens `Mode::Stats`, a centred overlay `theme::pane("broker stats · live", true)` showing `label` (muted, padded to 20) and the value. Any key or click closes it. It updates live, because it is redrawn every frame from the tree.
- With no `$SYS` data it shows `no $SYS data – subscribe to $SYS/# (the default)`.

- [ ] Tests:
  - core `broker_stats_reads_known_topics`
  - core `broker_stats_empty_without_sys`
  - ui `stats_overlay_shows_values_and_closes_on_key`
- [ ] Watch the tests fail, implement, test, and commit `feat(tui): broker statistics overlay from $SYS`.

### Task 7: Publish – `$EDITOR` and payload history

**Files:** `tui/publish.rs`, `tui/app.rs`, `tui/mod.rs`

**Interfaces:**
- In the publish form, `ctrl-e` returns `FormOutcome::Edit(String)` with the current payload. App maps that to `Effect::EditPayload(String)`.
- `mod.rs` handles it:
  1. write the payload to a temp file in `std::env::temp_dir()` named `mqtt-zapp-{pid}.txt`
  2. leave raw mode, the alternate screen and mouse capture
  3. run `$VISUAL`, else `$EDITOR`, else `vi`, split on whitespace, with the file as the last argument
  4. restore the terminal and call `terminal.clear()`
  5. on success (exit 0) call `app.set_publish_payload(text)`, trimming one trailing `\n` that the editor added; otherwise set `Note::error("editor failed: …")` and leave the payload unchanged
  6. delete the temp file
- Payload history: `App.sent: HashMap<String, VecDeque<String>>`, up to 20 per topic, newest at the front, duplicates moved to the front. A successful `Effect::Publish` records the entry when it is issued, not when the broker confirms. In the form, with the payload field focused, `ctrl-p` and `ctrl-n` walk older and newer entries for the current topic text, replacing the payload. Walking past the newest restores the draft that was there before the walk started.
- Hint line: `ctrl-s send  ctrl-e editor  ctrl-p/n history  ctrl-f json  tab next  esc close`. Mirror it in the footer hints and the help overlay.

- [ ] Tests:
  - publish `ctrl_e_returns_edit_outcome`
  - app `set_publish_payload_replaces_text`
  - app `payload_history_walk_and_restore_draft`
  - app `history_dedups_and_caps_at_20`
  - mod/unit `editor_failure_keeps_payload`: put the run-editor logic in `fn edit_text(initial: &str, cmd: &str) -> Result<String>` and test it with `cmd = "false"` (Err) and with `cmd = "sh -c 'echo hi >'"`, which yields `hi`. Unix only, `#[cfg(unix)]`.
- [ ] Watch the tests fail, implement, test, and commit `feat(tui): edit the publish payload in $EDITOR; recall recent payloads`.

### Task 8: Settings – theme and timestamp format

**Files:** `mqtt-zapp-core/src/config.rs`, `tui/theme.rs`, `tui/ui.rs`, `tui/message_view.rs`, `tui/app.rs`

**Interfaces:**
- core `Settings`:
  - `theme: ThemeName` (`#[serde(rename_all="lowercase")] enum ThemeName { #[default] Dark, Light, Mono }`)
  - `timestamp_format: String`, default `"%H:%M:%S%.3f"`. Validate it at load: a format that chrono rejects (rendering it into a String fails) falls back to the default with a note in `store_error`'s style. Simplest: the App validates it at `App::new` and keeps the default on error.
- `theme::apply(buf: &mut Buffer, area: Rect, name: ThemeName)` is called at the end of `ui::render`.
  - Dark: no-op.
  - Light: the inactive-selection bg `DarkGray` becomes `Color::Gray`, and the black-on-accent badge text stays.
  - Mono: for every cell whose bg is not `Reset`, set REVERSED and reset fg/bg. Every other cell gets fg `Reset`.
- `NO_COLOR` set to a non-empty value forces Mono (no-color.org).
- `App.theme` and `App.timestamp_format` are taken from settings. The meta chip time uses `timestamp_format`; history rows keep `%H:%M:%S`.

- [ ] Tests:
  - core `settings_theme_and_timestamp_parse`
  - ui `mono_strips_colours_keeps_selection_and_focus`: in mono, no cell has a fg/bg other than `Reset`, the selected row has REVERSED, and `┏` is still present.
  - ui `light_theme_swaps_inactive_selection_bg`
  - ui `no_color_env_forces_mono`: test the pure helper `theme::effective(name, no_color: Option<&str>) -> ThemeName` without touching the env.
  - message_view `meta_uses_timestamp_format`: format `%H:%M` gives `HH:MM` and no seconds.
  - app `bad_timestamp_format_falls_back`
- [ ] Watch the tests fail, implement, run the full CI gate, update the README settings section (`sort`, `theme`, `timestamp_format`, new keys), and commit `feat: theme (dark/light/mono, NO_COLOR) and timestamp format settings`.

## Out of scope

Sparkplug B, multi-term and regex search, MQTT 5, auto-expand behaviour (§4.8), multiple chart series, diffing any two history entries, and persisting payload history across restarts.
