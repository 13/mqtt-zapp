# TUI Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the TUI easier to read and nicer to look at. You should always be able to see which pane has focus and which broker you are connected to, and the keys you can press should show up wherever you are.

**Architecture:** A new `tui/theme.rs` holds every colour, the pane borders, the selection style and the overlay helpers, so the look is set in one place. `ui.rs` gets a header line (app name, connection state, broker, rate) and a footer line (key hints, notes, filter). The other render functions (`message_view`, `picker`, `publish`) switch to the theme helpers. Only one behaviour changes: the payload pane can take focus, and `j`/`k` scroll it.

**Tech Stack:** ratatui 0.29 (`BorderType`, `Block::title_top`, `Line::right_aligned`, `Line::width`), crossterm 0.28. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md` (§6 says the TUI layout is open; §4.8 lists a dark/light theme setting, which this plan does **not** add). The user asked for a few things directly. You can't see which pane is active. You can't see which server you're connected to. Overall, the TUI should look more modern, more polished and easier to use.

## Global Constraints

- Use only the named ANSI 16 colours (`Color::Cyan`, `Color::DarkGray`, …) and `Color::Reset`. No RGB and no indexed colours. That way the user's terminal palette, light or dark, still decides the actual shades.
- Focus must show even without colour. A focused pane uses `BorderType::Thick` and a `●` in its title. An unfocused pane uses `BorderType::Rounded` and has no `●`.
- Every string from the broker or from a profile goes through `sanitize`/`single_line` before it is drawn. That covers topics, payloads, disconnect reasons, profile names, hosts and usernames.
- The minimum terminal size stays 40×10. At 40 columns, nothing may panic or wrap onto a second line. Header and footer segments are dropped whole, from the right, when they don't fit.
- Don't use MQTT Explorer branding or visuals.
- CI must stay green: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Existing render tests assert on old strings, glyphs and `SELECTED_BG`. Each task updates the tests its change breaks, and states the new expected value in the test. Never just delete an assertion.

## Review Focus

1. **40-column terminal:** the header, footer, picker card and overlays all fit on one line or inside the area, with no panic. Tests: Task 3 (`header_fits_40_cols`), Task 4 (`footer_drops_hints_when_narrow`), Task 7 (`overlay_fits_tiny_terminal`).
2. **Hostile profile and broker strings:** an ESC in a profile name, host or disconnect reason is never drawn raw in the header or picker. Test: Task 3 (`header_sanitizes_profile_and_reason`).
3. **Monochrome terminal:** focus is visible from glyphs alone. Test: Task 1 (`focused_pane_uses_thick_border_and_dot`).
4. **Payload focus with nothing to scroll:** Tab into the payload pane with no topic selected, or with a short payload, then press `j`/`k`/`g`/`G`. Nothing happens, and nothing panics. Test: Task 2 (`payload_focus_keys_noop_without_message`).
5. **Narrow layout flip:** below 80 columns the panes stack vertically, and the selection and scroll offsets still keep the selected row visible. Test: Task 8 (`narrow_terminal_stacks_panes`).

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/mqtt-zapp/src/tui/theme.rs` (new) | palette constants, `pane()`, `selection()`, `key_hint()`, `centered()`, `dim_backdrop()` |
| `crates/mqtt-zapp/src/tui/ui.rs` | top-level layout, header, footer, tree rows, help overlay |
| `crates/mqtt-zapp/src/tui/app.rs` | `Focus::Payload`, `ServerInfo` (replaces `profile_name`), `Note` |
| `crates/mqtt-zapp/src/tui/mod.rs` | notes channel carries `Note` |
| `crates/mqtt-zapp/src/tui/message_view.rs` | meta chips, payload/history pane chrome, empty state |
| `crates/mqtt-zapp/src/tui/picker.rs` | centred picker card, password modal, `server_url()` |
| `crates/mqtt-zapp/src/tui/publish.rs` | overlay uses `theme::centered` and `theme::pane` |

Run tests with `cargo test -p mqtt-zapp`. Run the whole CI gate with `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.

---

### Task 1: Theme module and visible focus

**Files:**
- Create: `crates/mqtt-zapp/src/tui/theme.rs` (register it in `tui/mod.rs`)
- Modify: `tui/ui.rs` (remove `border_style` and `SELECTED_BG`; tree pane), `tui/message_view.rs` (history pane), `tui/picker.rs` (selection)

**Interfaces:**
- Produces, in `theme.rs`:
  - `pub const ACCENT: Color = Color::Cyan;`
  - `pub const MUTED: Color = Color::DarkGray;`
  - `pub const OK: Color = Color::Green;`
  - `pub const WARN: Color = Color::Yellow;`
  - `pub const ERR: Color = Color::Red;`
  - `pub const SEL_BG: Color = Color::Blue;`
  - `pub const SEL_BG_INACTIVE: Color = Color::DarkGray;`
  - `pub fn pane(title: &str, focused: bool) -> Block<'static>`. Focused: `BorderType::Thick`, border fg `ACCENT`, title `" ● {title} "` in bold black on `ACCENT`. Unfocused: `BorderType::Rounded`, border fg `MUTED`, title `" {title} "` in fg `MUTED`. `title` is sanitized inside.
  - `pub fn selection(focused: bool) -> Style`. Focused: bg `SEL_BG`, fg `Color::White`, bold. Unfocused: bg `SEL_BG_INACTIVE`.

- [ ] **Step 1: Write failing tests** in `theme.rs`, plus one in `ui.rs`:
  - `focused_pane_uses_thick_border_and_dot`: render `pane("topics", true)` into a 20×3 `TestBackend`. Assert cell (0,0) is `┏`, the top row contains `● topics`, and cell (0,0) has fg `ACCENT`. Render `pane("topics", false)`: cell (0,0) is `╭` and the top row does not contain `●`.
  - `selection_differs_by_focus`: `selection(true).bg == Some(SEL_BG)` and `selection(false).bg == Some(SEL_BG_INACTIVE)`.
  - In `ui.rs` tests, `tab_moves_thick_border_to_history`: use `app_with(&[("a", b"1"), ("a", b"2")])`, select `a`, draw at 100×24. Find the column of the first `┏`; it must be inside the left half. Call `handle_action(Action::FocusNext)` and redraw. Now the first `┏` is in the right half and the tree pane's corner is `╭`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp theme` and the ui test. Expected: FAIL (no module).
- [ ] **Step 3: Implement** `theme.rs`. Switch `render_tree`, `render_history` and the picker to `theme::pane` and `theme::selection(focus == …)`, and delete `ui::border_style` and `ui::SELECTED_BG`. In the existing tests, replace `SELECTED_BG` with `theme::SEL_BG` (focused pane) and make sure `fresh_row_flashes_and_selection_has_background` and `picker_lists_profiles_with_selection_and_hint` still pass.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: all PASS.
- [ ] **Step 5: Commit** `feat(tui): theme module; focused pane gets thick border and title badge`

---

### Task 2: Payload pane can take focus

**Files:**
- Modify: `tui/app.rs` (`Focus`, `handle_action`), `tui/message_view.rs` (`render_payload` uses `theme::pane("payload"|"diff", app.focus == Focus::Payload)`)

**Interfaces:**
- Produces: `pub enum Focus { Tree, Payload, History }`. `FocusNext` cycles Tree → Payload → History → Tree.
- When `focus == Payload`:
  - `Up`/`Down` scroll `payload_scroll` by 1 line, clamped like `scroll_payload` (`content_len - payload_height`)
  - `Top` sets `payload_scroll = 0`
  - `Bottom` sets it to the maximum
- `ScrollDown`/`ScrollUp` keep working from any focus.

- [ ] **Step 1: Write failing tests** in `app.rs`:
  - `focus_cycles_tree_payload_history`: three `FocusNext` calls go back to `Tree`, passing through `Payload` and then `History`.
  - `payload_focus_jk_scrolls_one_line`: payload of 50 JSON lines, `payload_height.set(10)`, focus Payload. `Down` twice gives `payload_scroll == 2`. `Up` gives 1. `Bottom` gives 40. `Top` gives 0. The tree selection doesn't change.
  - `payload_focus_keys_noop_without_message`: no selection, focus Payload. `Down`, `Up`, `Top` and `Bottom` leave `payload_scroll == 0` and don't panic.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp focus`. Expected: FAIL.
- [ ] **Step 3: Implement.** Add a private `fn scroll_payload_by(&mut self, delta: isize)` next to `scroll_payload`, and route the match arms through it.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS. Existing `FocusNext` tests that expected Tree→History in one press now need two presses. Update them.
- [ ] **Step 5: Commit** `feat(tui): payload pane is focusable; j/k/g/G scroll it`

---

### Task 3: Header bar with connection and broker

**Files:**
- Modify: `tui/app.rs` (`ServerInfo`), `tui/picker.rs` (`server_url`), `tui/ui.rs` (`render_browse` layout and `header_line`)

**Interfaces:**
- Produces, in `picker.rs`: `pub fn server_url(p: &Profile) -> String`. It returns `proto://[user@]host:port`, sanitized and on one line, e.g. `mqtts://alice@broker.lan:8883`. A ws/wss profile with `ws_path` other than `/` gets the path appended.
- Produces, in `app.rs`:
  ```rust
  pub struct ServerInfo { pub name: String, pub url: String, pub subscriptions: Vec<String> }
  ```
  `App.server: Option<ServerInfo>` replaces `profile_name`. It is set in `connect_effect`. Update the two `profile_name` assertions (around app.rs:986 and :1028) to `a.server.as_ref().map(|s| s.name.as_str())`.
- Produces, in `ui.rs`: `fn header_line(app: &App, width: u16) -> Line<'static>`. Browse layout becomes `[header 1, main Min(1), footer 1]`. The header is built from these segments, left to right:
  1. ` mqtt-zapp ` in bold black on `ACCENT`
  2. the state pill: `● connected` in `OK`, `◌ connecting` in `WARN`, or `✕ {reason}` / `✕ {reason} · retry {n}s` in `ERR`
  3. `name`, in bold
  4. `url`, in `MUTED`
  5. right-aligned: `{rate:.1} msg/s · {n} topics`, in `MUTED`

  When the line is too wide, drop segments in the order 4, then 5, then 3. Segments 1 and 2 always stay. Truncate segment 2's reason with `…` if needed.
- The rate, topic count and connection state move from the footer to the header.

- [ ] **Step 1: Write failing tests** in `ui.rs`:
  - `header_shows_broker_and_state`: give the app `server` = `ServerInfo{name:"home", url:"mqtts://alice@broker.lan:8883", ..}`, state Connected, and draw at 120×24. Row 0 contains `mqtt-zapp`, `● connected`, `home` and `mqtts://alice@broker.lan:8883`, and ends with `topics` (after `trim_end`). The cell under `●` has fg `OK`.
  - `header_disconnected_shows_reason_and_retry`: reason `connection refused`, retry 3200 ms. Row 0 contains `✕ connection refused · retry 4s` in `ERR`.
  - `header_fits_40_cols`: draw at 40×10 with a long reason. Row 0 still starts with ` mqtt-zapp ` and contains `✕`, and row 1 is the pane border (`┏` or `╭`), so nothing wrapped.
  - `header_sanitizes_profile_and_reason`: name `"h\x1b[31m"`, reason `"\x1b[2J"`. No cell contains `\x1b`.
  - In `picker.rs`: `server_url_formats`: covers mqtts with a user, and wss with path `/mqtt` (`wss://h:443/mqtt`).
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement.** Rewrite the existing footer tests that asserted state, rate and name (`status_bar_shows_state_and_rate`, `disconnected_state_in_status_bar`, `connected_and_connecting_markers`) so they check row 0 instead of the last line. `connected_and_connecting_markers` changes from `starts_with("● home")` to `contains("● connected")` plus `contains("home")`.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tui): header bar with connection state, broker url and rate`

---

### Task 4: Footer with key hints, notes and filter chip

**Files:**
- Modify: `tui/theme.rs`, `tui/ui.rs` (replace `status_line` and `mode_hint` with `footer_line`), `tui/app.rs` (`Note`), `tui/mod.rs` (notes channel type)

**Interfaces:**
- Produces, in `theme.rs`: `pub fn key_hint(key: &str, label: &str) -> [Span<'static>; 2]`. The key is bold `ACCENT`; the label is `" {label}  "` in `MUTED`.
- Produces, in `app.rs`:
  ```rust
  pub struct Note { pub text: String, pub error: bool }
  ```
  - `set_note(note: Note, now: Instant)`
  - `note_at(now) -> Option<&Note>`
  - the `status_note: Option<(Note, Instant)>` field
  - `mod.rs` sends `Note { error: true, .. }` for `publish failed: …` and `error: false` for success. Change the channel to `UnboundedSender<Note>`.
- Produces, in `ui.rs`: `fn footer_line(app: &App, width: u16) -> Line<'static>`, built from the parts below:
  - **SearchInput mode:** `/` in bold `ACCENT`, then the query, then `▏`. Right-aligned: `key_hint("enter","keep")`, `key_hint("esc","clear")`.
  - **Otherwise:** an optional filter chip first: ` filter: {q} ({n}) ` in black on `WARN`, followed by `key_hint("esc","clear")`. Then the note, if there is one: in `ERR` when `error`, otherwise in `OK`, truncated to `NOTE_MAX_CHARS`. The note replaces the hints. Otherwise the hints for the context. Keys always come from `key_for` (keymap remaps), never literals:
    - **Browse + Tree:** search, publish, `enter` expand, focus_next "focus", help, quit
    - **Browse + Payload:** `j/k` scroll, toggle_diff "diff", focus_next "focus", help
    - **Browse + History:** `j/k` "older/newer", toggle_diff "diff", focus_next "focus", help
    - **Picker:** `enter` connect, `j/k` move, quit
    - **Password:** `enter` connect, `esc` back/quit (keep the current rule about whether profiles exist)
  - Drop hints whole, from the right, until the line fits `width`.

- [ ] **Step 1: Write failing tests** in `ui.rs`:
  - `footer_hints_follow_focus`: in Tree focus the last line contains `/ search` and `p publish`. After `FocusNext`, it contains `d diff` and `scroll`.
  - `footer_drops_hints_when_narrow`: at 40×10 the last line's `chars().count() <= 40`, and the line has no partial hint (every visible key is followed by its label).
  - `error_note_is_red`: `set_note(Note{text:"publish failed: x".into(), error:true}, now)`. The first cell of the note has fg `ERR`.
  - Update `browse_hint_follows_keymap_remaps` to the new labels (`P publish` after a remap). In `search_highlights_matches_and_shows_status`, the last line now starts with `/wzr▏`; use `starts_with` instead of equality. Update `filter_segment_coexists_with_state_and_note` so it checks the filter chip and note in the footer and the state in the header.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement**, including the changes to the `Note` callers in `mod.rs` and `app.rs`.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tui): context key hints, coloured notes and filter chip in footer`

---

### Task 5: Tree rows and empty states

**Files:**
- Modify: `tui/ui.rs` (`row_spans`, `render_tree`)

**Interfaces:**
- Row layout: `{indent}{marker} {name}  {preview}` on the left, and the count right-aligned to the pane's inner width.
  - The marker (`▸`/`▾`) is in `ACCENT`.
  - The name is bold when the node has a `latest` message.
  - The preview is in `MUTED`.
  - The count is `{msgs}` for a leaf and `{msgs} · {topics}t` for a branch, in `MUTED`.
  - If the right-aligned count doesn't fit, drop it.
  - The ` = ` separator goes; there are two spaces instead.
- The flash (`FLASH_FG`/`FLASH_MODIFIER`) applies only to the name span.
- `row_spans` gets a new parameter `width: usize` so it can pad the count. Use `Line::width()` to measure.
- Empty states are drawn centred and in `MUTED` inside the tree pane:
  - no rows and no filter: `waiting for messages…`, with `subscribed to {subs joined by ", "}` on the next line, taken from `app.server` (omit that line when `server` is `None`)
  - no rows with a filter: `no topics match "{q}"`

- [ ] **Step 1: Write failing tests:**
  - Update `tree_row_shows_marker_name_preview_and_count`. The line containing `▾ a` ends with `3 · 2t│` or `3 · 2t┃`, depending on the border. The line with `b  world` ends with `2` followed by the border.
  - `tree_empty_state_lists_subscriptions`: no messages, `server.subscriptions = ["#", "$SYS/#"]`. The text contains `waiting for messages…` and `subscribed to #, $SYS/#`.
  - `tree_filter_no_match_message`: apply the filter `zzz`. The text contains `no topics match "zzz"`.
  - Fix up `search_highlights_matches_and_shows_status`, which finds `"esp-WZR1 ="`, to the new separator.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tui): cleaner tree rows with right-aligned counts; empty states`

---

### Task 6: Message pane chips and titles

**Files:**
- Modify: `tui/message_view.rs`

**Interfaces:**
- `fn human_bytes(n: usize) -> String` returns `"512 B"`, `"1.2 KiB"` or `"3.4 MiB"` (one decimal, base 1024).
- `metadata` returns `Line<'static>` chips instead of a `String`, in this order:
  - the full topic, bold `ACCENT`
  - ` QoS {n} `, reversed `MUTED`
  - ` RETAINED `, black on `Color::Magenta`, only when retained
  - the kind chip ` JSON ` / ` TEXT ` / ` BINARY `, from `detect`
  - `human_bytes(len)`, in `MUTED`
  - the time `%H:%M:%S%.3f`, in `MUTED`

  Chips are separated by one space.
- Payload pane: `theme::pane("payload" | "diff", focus == Payload)`. When the content is longer than the inner height, the bottom border has a right-aligned `{first}-{last}/{total}` title (`Block::title_bottom(Line::from(..).right_aligned())`).
- History pane: the title is `history · live` when `history_sel` is `None`. Otherwise it is `history · paused (j to newest)`, using the user's key for Down. Rows: the time in `MUTED`, then the preview. The selected row uses `theme::selection(focus == History)`.
- Empty state (no topic selected): a centred `MUTED` line, `select a topic with a message`. Keep the `HINT` text so the existing tests still pass.

- [ ] **Step 1: Write failing tests** in `message_view.rs`:
  - `human_bytes_units`: 0 gives `"0 B"`, 1023 gives `"1023 B"`, 1536 gives `"1.5 KiB"`, 5·1024·1024 gives `"5.0 MiB"`.
  - `meta_chips_show_kind_and_retained`: a retained JSON payload. The meta row contains ` QoS 1 `, ` RETAINED ` and ` JSON `. The cell under `R` of RETAINED has bg `Color::Magenta`. Update the existing assertions on `qos 1`, `retained` and `5 B` to `QoS 1`, `RETAINED` and `5 B`.
  - `payload_scroll_indicator`: 50 lines at height 10 after one `ScrollDown`. The text contains `/50` on the payload's bottom border.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp message_view`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tui): message metadata chips, payload scroll indicator, history titles`

---

### Task 7: Picker card and overlays

**Files:**
- Modify: `tui/theme.rs`, `tui/picker.rs`, `tui/ui.rs` (`render_help`), `tui/publish.rs` (`overlay_area` gives way to `theme::centered`)

**Interfaces:**
- Produces, in `theme.rs`:
  - `pub fn centered(area: Rect, width: u16, height: u16) -> Rect` clamps to `area`, minus a 1-cell margin when there is room.
  - `pub fn dim_backdrop(f: &mut Frame, area: Rect)` sets `Modifier::DIM` on every cell in `area`. Call it before `Clear` and the overlay.
- Picker: a card `centered(area, 72, profiles + 4)` with the focused `pane` titled `mqtt-zapp`. Each row is `▶ ` (selected) or `  `, then the name, bold and padded to the longest name, then two spaces, then `server_url(p)` in `MUTED`. The selected row uses `selection(true)`. The empty state is a centred card with the error in `ERR` above `EMPTY_HINT`.
- Password: a modal `centered(area, 50, 5)`, pane title `password · {name}`. Line 1 is the `server_url` in `MUTED`; line 2 is `› ` followed by the masked input and `▏`.
- Help: `centered(area, 76, 22)` with `dim_backdrop`. The left column is grouped under the bold `ACCENT` headings below, each line `{name:<14}{keys}` (keep that format; the existing test depends on it):
  - `navigate`: up, down, collapse, expand, toggle_expand, top, bottom, select
  - `view`: focus_next, scroll_down, scroll_up, toggle_diff
  - `actions`: search, publish, help, cancel, quit

  The right column is unchanged. Add a `debug_assert` that every `Action` is in exactly one group.
- Publish: `centered(area, 80, 18)` with `dim_backdrop`, a focused `pane("publish")`, and the error state keeps its red title and border.

- [ ] **Step 1: Write failing tests:**
  - `centered_clamps`: `centered(Rect::new(0,0,40,10), 76, 22)` returns a rect inside `0..40 × 0..10`.
  - `overlay_fits_tiny_terminal`: open help at 40×10, then the publish form at 40×10. Neither panics, and the top-left corner is a border glyph.
  - Update `picker_lists_profiles_with_selection_and_hint`. The selected row contains `▶ lab` and `mqtts://alice@broker.lan:8883`, and its bg is `theme::SEL_BG`.
  - `password_modal_shows_server`: the text contains `password · home` and `mqtts://alice@broker.lan:8883`. Keep the masking assertions in `password_render_is_masked`.
  - `help_groups_actions`: the text contains `navigate`, `view` and `actions`. `help_overlay_lists_effective_keymap` still passes.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(tui): centred picker card, password modal, grouped help, dimmed backdrop`

---

### Task 8: Stack panes on narrow terminals

**Files:**
- Modify: `tui/ui.rs` (`render_browse`)

**Interfaces:**
- When `main.width < 80`, split vertically: tree `Percentage(45)` on top, message `Percentage(55)` below. Otherwise split horizontally 40/60 (the tree gets less than today's 50/50, because payloads need the width).

- [ ] **Step 1: Write failing tests:**
  - `narrow_terminal_stacks_panes`: at 60×30, row 1 has a single pane top border that spans the full width (`┏` at col 0 and `┓` at col 59), and a second top border appears further down. With 40 topics and the selection on the last one, the selected row is visible.
  - `wide_terminal_splits_40_60`: at 100×24 the second pane's top-left corner is at col 40.
- [ ] **Step 2:** Run the tests. Expected: FAIL.
- [ ] **Step 3: Implement.** Re-check the existing tests that slice the "tree pane" to the first 40 columns at width 80. `search_highlights_matches_and_shows_status` changes to 32 columns, which is 40% of 80.
- [ ] **Step 4:** Run the full CI gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`. Expected: all clean.
- [ ] **Step 5: Manual check.** Run `cargo run -p mqtt-zapp` against a broker at 120×40, 80×24 and 40×10. Tab through the panes, open help and the publish form, and trigger a disconnect (stop the broker). Confirm the header turns red and the panes are still readable.
- [ ] **Step 6: Commit** `feat(tui): stack panes vertically below 80 columns; 40/60 split`

---

## Out of scope

- A configurable or light theme (spec §4.8). The ANSI-16 palette follows whatever terminal theme is set.
- `NO_COLOR` support, mouse support, and new key bindings beyond payload focus.
- Broker statistics or any other new view.
