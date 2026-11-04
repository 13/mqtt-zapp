# Esc Closes the Chart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One key closes the chart, however many numeric fields the payload has. Today `c` is the only way out: it steps through every field and only closes after the last one, so a payload with 50 numbers takes 50 presses.

**Architecture:** The `cancel` action (`esc`) closes the chart before it does anything else, and only clears the search once no chart is open. The chart pane shows the hint, so the way out is visible.

**Tech Stack:** Rust, ratatui 0.29 (existing).

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md` §4.5 (charts) and §6 (vim-style keys, remappable). The user reported the problem on 2026-11-04: "when i c chart and a json has 50 elements i have to cycle through all until chart closes".

## Global Constraints

- `esc` stays the `cancel` action, so a remap of `cancel` in `[keys]` moves this behaviour with it. Hints use `key_for(app, Action::Cancel)`, not a literal `esc`.
- The README key table is checked by the test at `keymap.rs` ~396. If the `cancel` help text changes, the README row changes with it.
- CI gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`, plus `cargo clippy --no-default-features --all-targets -- -D warnings`.

## Review Focus

1. **Chart open and a search filter active.** The first `esc` closes only the chart and keeps the filter. The second `esc` clears the filter. Test: `esc_closes_chart_before_clearing_search`.
2. **Pinned series.** `esc` is "close the chart". It clears the pins too, so the next `c` starts a fresh single chart instead of bringing back an overlay the user just dismissed. Turning the chart off with `c` keeps the pins, as it does today. Test: `esc_clears_chart_pins`.
3. **Footer truthfulness.** While the chart is open, the filter chip doesn't show `esc clear`, because `esc` would close the chart instead. Test: `filter_hint_hidden_while_chart_open`.
4. **A remapped cancel key** shows up in the chart hint. Test: `chart_hint_uses_cancel_key`.
5. **No chart open.** `esc` behaves exactly as before, and the existing search tests stay green unchanged.

---

### Task 1: `esc` closes the chart, with a visible hint

**Files:**
- Modify `crates/mqtt-zapp/src/tui/app.rs` (the `Action::Cancel` arm, ~line 793).
- Modify `crates/mqtt-zapp/src/tui/app/chart.rs`: new `close_chart`, and the `cycle_chart` doc comment.
- Modify `crates/mqtt-zapp/src/tui/message_view/chart.rs`: `render_chart` gets a right-aligned hint title.
- Modify `crates/mqtt-zapp/src/tui/ui/bars.rs`: the filter chip hint (~line 152).
- Modify `crates/mqtt-zapp/src/tui/keymap.rs`: the `cancel` help text.
- Modify `README.md`: the `cancel` row, and one sentence in the *Keys* intro or under charts.
- Test: `app/tests.rs`, `message_view/tests.rs`, `ui/tests.rs`.

**Interfaces:**
- Produces `pub(super) fn close_chart(&mut self) -> bool` in `app/chart.rs`. It returns `false` and does nothing when `self.chart.is_none() && self.chart_pinned.is_empty()`. Otherwise it sets `chart = None`, clears `chart_pinned` and sets the note `chart off`, then returns `true`.
- `Action::Cancel` becomes `if !self.close_chart() { self.clear_search() }`.
- The `cancel` help text becomes `Close the chart, else clear the search / close`.

- [ ] **Step 1: Failing tests** in `app/tests.rs`:
  - `esc_closes_chart`: a payload with 50 numeric fields (`{"f0":0,…,"f49":49}`) × 2 history entries. Press `c`, then `esc` → `chart == None`, and the note is `chart off`.
  - `esc_closes_chart_before_clearing_search`: a filter is set and the chart is on. After the first `esc`, `chart == None` and `filter.is_some()`. After the second `esc`, `filter.is_none()`.
  - `esc_clears_chart_pins`: `c`, `C`, `c`, then `esc` → `chart == None` and `chart_pinned.is_empty()`.
  - `esc_without_chart_still_clears_search`: no chart and a filter set; `esc` → `filter.is_none()`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp esc_`. Expected: FAIL. `esc_closes_chart` fails because `chart` is still `Some`, and the search test fails because the filter is cleared first.
- [ ] **Step 3:** Implement `close_chart` and the `Cancel` arm. Update the `cycle_chart` doc comment to mention that `esc` closes.
- [ ] **Step 4: Failing render tests:**
  - `chart_title_shows_close_hint` (`message_view/tests.rs`): with the chart open, the chart's top border row contains `esc close`.
  - `chart_hint_uses_cancel_key`: with `cancel = "ctrl-q"` in the keymap, the border shows `ctrl-q close`. Build the keymap the way the existing remap tests do.
  - `filter_hint_hidden_while_chart_open` (`ui/tests.rs`): with a filter and the chart open, the footer has the filter chip but not `esc clear`. With the chart off, `esc clear` is back.
- [ ] **Step 5:** Implement the hint. In `render_chart`, add `.title(Line::styled(format!(" {} close ", key_for(app, Action::Cancel)), muted).right_aligned())` to the block, using the same muted style as the payload's `⧉ copy` label. In `bars.rs`, push the filter chip's `clear` hint only when `app.chart.is_none()`.
- [ ] **Step 6:** Update the `cancel` help text in `keymap.rs` and the README `cancel` row to match. Add a README sentence under *Keys*: "`c` cycles the chart through the numeric fields; `esc` closes it at any point."
- [ ] **Step 7:** Run the CI gate. Expected: PASS, with the existing chart and search tests unchanged.
- [ ] **Step 8:** Commit `fix(tui): esc closes the chart instead of cycling through every field`.

## Out of scope

- A reverse `c` (previous field) or a field picker for big payloads. Possible follow-ups if 50 fields are still awkward to *find*, not just to leave.
- Making the `close` hint clickable.
