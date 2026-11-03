# Chart Field Highlight and Click Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- While `c` cycles through the numeric fields to chart, the payload pane highlights the line of the field being charted.
- A mouse click on a numeric line charts that field.

**Architecture:**
- The payload pane shows JSON pretty-printed by `serde_json::to_string_pretty`, with key order preserved. `numeric_paths` walks the same `serde_json::Value` to find the chartable fields.
- A new core function, `numeric_lines`, walks that value the way the pretty printer lays it out. It returns which pretty line holds each numeric field.
- The TUI then:
  - highlights the active field's line;
  - scrolls it into view while cycling;
  - maps a click on the payload pane back to a line, then to a field.

**Tech Stack:** Rust, serde_json (`preserve_order`), ratatui 0.29.

**Spec:** the user's request, verbatim: "when chart cycling through highlight which value is active and make it also clickable with mouse".

## Decisions

- **From the user:**
  - Show which value the chart is on while cycling.
  - Make the values clickable to chart them.
- **Claude's calls** (the user can override these):
  - **Highlight:** the charted field's line in the payload pane gets the inactive selection background (`theme::select(line, false)`).
    - In mono / `NO_COLOR` this becomes reverse video, and the light theme already adjusts that colour.
    - For a payload that is a plain number (path `None`), the highlight is its single line.
  - **Scrolling:** after each `c`, the payload scrolls just enough to show the highlighted line.
  - **Chart title:** shows the position as well, `chart · ENERGY.Power · 2/5`, so you can see how far the cycle has got.
  - **Click:** a left click on a payload line that holds a number charts that field. If that field is already charted, the click turns the chart off.
    - The click also focuses the payload pane, as any payload click does now.
    - A click on any other line only focuses, as today.
    - Click targets come from the shown pretty JSON only, so diff view and binary/text payloads have none. A plain number payload's one line is clickable.
  - **Keys:** no new key. `c` keeps its cycle: off → value → each field → off.

## Global Constraints

- The line mapping must match `pretty_lines` exactly for every JSON shape, including:
  - nested objects and arrays;
  - empty `{}` and `[]`;
  - arrays of numbers;
  - numbers at the root.
- `numeric_lines` keeps the same field order and the same `MAX_NUMERIC_PATHS` cap as `numeric_paths`.
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.

## Review Focus

1. **A line mapping that drifts** would highlight or chart the wrong field. A property-style test checks every mapped line against the pretty text, over many shapes: `numeric_lines_match_pretty_output`.
2. **Clicks while scrolled** must still land on the right field: `click_numeric_line_when_scrolled`.
3. **Payload changes:** when a new message arrives with different fields, the highlight follows the path, not a stale line number. If the field is gone, nothing is highlighted: `highlight_follows_path_across_messages`.
4. **Pinned history entries:** with one pinned, the highlight and the click use the shown (pinned) payload, not the latest one: `click_uses_pinned_payload`.

---

### Task 1: `numeric_lines` in core

**Files:**
- Modify: `crates/mqtt-zapp-core/src/payload.rs`.

**Interfaces:**
- `pub fn numeric_lines(payload: &[u8]) -> Vec<(usize, Option<String>)>` maps each chartable value to its 0-based line in `pretty_lines(&detect(payload), payload)`:
  - for JSON, `(line, Some(path))`, in `numeric_paths` order with the same cap;
  - for a payload that parses as a plain number, `[(0, None)]`;
  - otherwise empty.
- The layout it mirrors, from `to_string_pretty`:
  - a non-empty object or array opens on its own line (the key's line for nested ones), puts each child on following lines, and closes on its own line;
  - empty `{}` and `[]`, and scalars, take exactly one line.

- [ ] **Step 1: Failing tests**
  - `numeric_lines_basic`: `{"a":1,"b":{"c":2,"d":"x"},"e":[3,4]}` gives `[(1,"a"),(3,"b.c"),(7,"e.0"),(8,"e.1")]`. Check these against the actual `to_string_pretty` lines in the test itself.
  - `numeric_lines_match_pretty_output` runs over roughly 10 payloads, covering nested arrays of objects, empty containers, a root array, deep nesting, and a key containing `.`. For each `(line, Some(path))` result:
    - the last path segment appears on that pretty line, as a key or as the array index position;
    - the line's value parses to `numeric_at(payload, Some(path))`.
    
    The returned paths, in order, equal `numeric_paths(payload)`.
  - `numeric_lines_plain_number_and_text`: `b"21.5"` gives `[(0, None)]`; `b"hello"` and binary give `[]`.
- [ ] **Step 2:** `cargo test -p mqtt-zapp-core numeric_lines`. Expected: FAIL (not defined).
- [ ] **Step 3:** Implement with a recursive walker. It mirrors `walk_numbers`, carrying a line counter.
- [ ] **Step 4:** `cargo test -p mqtt-zapp-core`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(core): numeric_lines maps chartable values to pretty lines`

### Task 2: Highlight, scroll, title position, click

**Files:**
- Modify:
  - `crates/mqtt-zapp/src/tui/message_view.rs` (`render_payload`, `render_chart`);
  - `crates/mqtt-zapp/src/tui/app.rs` (`cycle_chart`; new `chart_line`, `chart_click`);
  - `crates/mqtt-zapp/src/tui/app/mouse.rs` (`browse_click`, the payload branch);
  - `README.md` (the Mouse section and the chart key row).
- Test: `crates/mqtt-zapp/src/tui/message_view/tests.rs`, `crates/mqtt-zapp/src/tui/ui/tests.rs`, `crates/mqtt-zapp/src/tui/app/tests.rs`.

**Interfaces:**
- Consumes `numeric_lines` from Task 1.
- `App::chart_line(&self) -> Option<usize>` returns the pretty line of `self.chart` in the shown payload. It is `None` when the chart is off, the diff view is on, or the field is not in this payload.
- `App::chart_click(&mut self, line: usize)` charts the field on `line`, or turns the chart off if that field is already charted. It does nothing for a line without a number.
- In `render_payload`, when not in diff view and `chart_line()` is `Some(l)` with `l` among the drawn lines, line `l` is drawn through `theme::select(line, false)`.
- `cycle_chart` sets `payload_scroll` so `chart_line()` lies within `[scroll, scroll + payload_height)`. It uses the smallest change.
- The `render_chart` title is `chart · {name} · {i}/{n}`, where `i` is the 1-based index of the series in `c`'s cycle list and `n` its length.
- In `browse_click`, a click inside the payload pane's inner area at row `y` maps to `line = payload_scroll + (y - inner.y)`. The click then focuses the payload, calls `chart_click(line)`, and returns `Effect::None`.

- [ ] **Step 1: Failing tests**
  - `chart_highlights_active_line`:
    - payload `{"t":21.5,"h":40}`, press `c` once, and draw;
    - the payload row containing `"t": 21.5` has bg `SEL_BG_INACTIVE` and the `"h"` row does not;
    - press `c` again: now `"h"` is highlighted.
  - `chart_title_shows_position`: `chart · t · 1/2`, then `chart · h · 2/2`.
  - `cycle_scrolls_highlight_into_view`:
    - payload of 60 numeric keys, payload height about 10;
    - press `c` 30 times, and after each press `chart_line()` lies within the visible window.
  - `click_numeric_line_charts_it`: in `ui/tests.rs`, click the `"h"` line, and `app.chart == Some(Some("h"))`. Click it again: `app.chart == None`. Click the `{` line: the chart is unchanged and the payload is focused.
  - `click_numeric_line_when_scrolled`: 60 keys, scroll to 20, click the third visible row, and `chart == Some(Some("k22"))`, matching that row's key.
  - `highlight_follows_path_across_messages`:
    - chart `h`, then a new message `{"x":1,"h":2}` arrives;
    - the highlighted row is the `"h": 2` row (line 2).
    - Then `{"x":1}` arrives: no row is highlighted, and nothing panics.
  - `click_uses_pinned_payload`:
    - pin the older entry `{"a":1}`, where the latest is `{"b":1,"a":2}`;
    - click line 1, and `chart == Some(Some("a"))`.
  - `diff_view_has_no_highlight_or_click_targets`.
- [ ] **Step 2:** `cargo test -p mqtt-zapp chart click`. Expected: FAIL.
- [ ] **Step 3:** Implement as in Interfaces.
  - Compute `numeric_lines` from the shown message's payload. Recomputing it per call is fine: it is linear and runs only on `c`, on clicks, and once per frame while the chart is on.
  - README: the Mouse section gets "click a number in the payload to chart it (again: off)", and the `c` row mentions the highlight.
- [ ] **Step 4:** `cargo test --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.
- [ ] **Step 5:** Manual check in tmux on the local mosquitto at port 18830:
  - publish `{"ENERGY":{"Power":12,"Voltage":230},"t":21.5}` a few times with changing values;
  - `c` cycles, and the highlight moves from line to line;
  - clicking the `"Voltage"` line charts it, and clicking it again turns the chart off.
- [ ] **Step 6:** Commit: `feat(tui): highlight the charted field; click a number to chart it`
