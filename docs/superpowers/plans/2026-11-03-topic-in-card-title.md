# Topic in the Header Card's Title Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The topic card above the payload shows the topic in its top border, the way the payload pane shows `payload` in its top border:

```
╭ muh/sensors/200/json ───────────────────────────── ⧉ copy ╮
│  JSON   7 B   QoS 0   RETAINED       15:13:23.782 · 7s ago │
╰─────────────────────────────────────────────────────────────╯
╭ payload ─────────────────────────────────────────── ⧉ copy ╮
```

**Architecture:** All of it is in `render_message` / `header_lines` in `crates/mqtt-zapp/src/tui/message_view.rs`. The path row inside the card moves into the block's left title. The card shrinks from 4 rows to 3: top border with the topic, the badge row, and the bottom border. The payload gets the freed row.

**Tech Stack:** Rust, ratatui 0.29.

**Spec:** the user's request, verbatim: "above the payload box is the topic box and there should be the topic like "muh/sensors/200/json" as in the payload box "--- payload ---"".

**Base:** branch `plain-topic-header`, not yet merged. It already draws the plain `muh/sensors/200/json` path, and this plan builds on its `fit_topic` and `leaf_span`.

## Decisions

- **From the user:** the topic sits in the card's top border, like `payload` in the payload pane's.
- **Claude's calls** (the user can override these):
  - **Title look.** The title is ` muh/sensors/200/json `, with one space on each side as in the other pane titles. `muh/sensors/200/` is muted, like the payload title, and `json` stays bold cyan, so the last level still stands out.
  - **Copy button.** `⧉ copy` stays at the right end of the same top border. A click on the topic title copies the topic too.
  - **Fitting.** The title gets `card width − 2 (corners) − 1 (gap) − copy label width` columns. `fit_topic` shortens a long topic in the middle, as before (`muh/…/json`).
  - **Card size.** The card is 3 rows tall. `HEADER_CARD_MIN_HEIGHT` (14) stays.
  - **One-row header unchanged.** The compact header, used when the panes are stacked or the area is short, stays as it is: path, badges, time and ` ⧉ ` in one row.

## Global Constraints

- Everything from the broker is drawn through `one_line`/`sanitize`. Widths are display widths.
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.

## Review Focus

1. **Topic and copy button share one border.** A long topic must never run under `⧉ copy` or past the corner: `card_title_never_reaches_copy_button` (long topic, card widths 20..80).
2. **Narrow cards.** When the card is too narrow for both, `⧉ copy` wins and the title shrinks to its leaf, then to nothing. Neither panics: `narrow_card_keeps_copy_button`.
3. **Click targets.** The title rect covers exactly the drawn title, and the copy rect sits right of it. Neither covers the bare border between them: `click_card_title_copies_topic`.

---

### Task 1: Topic in the card's top border

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/message_view.rs`:
  - `header_lines` returns only the badge row;
  - `render_message` sets the card title and `header_rows` becomes `3`;
  - the `header_topic` rect moves to the title span on the border.
- Test: `crates/mqtt-zapp/src/tui/message_view/tests.rs`, `crates/mqtt-zapp/src/tui/ui/tests.rs`.

**Interfaces:**
- `header_lines(s, kind, time_fmt, now, width) -> Line<'static>`: the badge row only. The `topic` parameter goes away.
- `card_title(topic: &str, room: usize) -> Line<'static>` is new. It returns `" " + parents (MUTED) + leaf_span(leaf) + " "`, or an empty line when `room < 3`. The leaf comes from `fit_topic(topic, room - 2)`.
- In card mode, `HitAreas.header_topic` is `Rect { x: meta.x + 1, y: meta.y, width: title width, height: 1 }`. `header_copy` is unchanged.

- [ ] **Step 1: Write failing tests:**
  - `header_card_draws_box_path_and_badges`, rewritten:
    - row 0 starts with `╭ tasmota/tele/tasmota_A1B2C3/SENSOR ─`, and `S` is ACCENT and bold;
    - row 0 also contains `⧉ copy ╮`;
    - row 1 contains ` JSON `, ` QoS 1 ` and ` RETAINED `;
    - row 2 starts with `╰`;
    - the payload pane's top border is on row 3.
  - `card_title_never_reaches_copy_button`: for widths 20..80, with topic `a/very/long/topic/path/with/many/levels/leaf`:
    - row 0 ends with `╮`;
    - it contains `⧉ copy` exactly once;
    - the title text ends before the copy label starts;
    - `leaf` is visible whenever `width ≥ 2 + 1 + 8 + 6`.
  - `narrow_card_keeps_copy_button`: at a card width of 12, row 0 contains `⧉ copy` and there is no panic.
  - `click_card_title_copies_topic` (`ui/tests.rs`) for `dev/x` at 100×24:
    - `╭ dev/x ` is on the message area's first row;
    - a click on `d` or on `x` gives `("topic", "dev/x")`;
    - a click on the bare `─` halfway between the title and `⧉ copy` gives `Effect::None`.
  - Update every test that looked for the path inside the card:
    - `click_header_path_copies_topic`: find `╭ dev/x `;
    - the `badge_row` helper: the badge row is the row right after the `╭ dev/x` row;
    - `metadata_line`, `meta_uses_timestamp_format`;
    - `click_header_copy_copies_topic`: still finds the first `⧉ copy` on the card's border row;
    - `header_row_never_overflows`: the CJK leaf `温度` sits on row 0, and row 0 ends with `╮`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp`. Expected: the new and updated tests FAIL.
- [ ] **Step 3:** Implement as in Interfaces. The block keeps its rounded MUTED border. It gets `.title(card_title(..))` on the left and the existing right-aligned `⧉ copy` title.
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.
- [ ] **Step 5:** Manual check in tmux on the local mosquitto at port 18830:
  - publish `muh/sensors/200/json`, select it;
  - the card reads `╭ muh/sensors/200/json ──… ⧉ copy ╮` above the badge row, and `payload` starts on the next row;
  - clicking the title or `⧉ copy` puts `muh/sensors/200/json` in the tmux buffer.
- [ ] **Step 6:** Commit: `feat(tui): topic in the header card's top border`
