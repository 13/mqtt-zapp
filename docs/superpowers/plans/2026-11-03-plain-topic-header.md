# Plain Topic Header Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show the header's topic path as the topic itself, `muh/wst/status`: no spaces around the `/`, and no gap before the leaf.

**Architecture:** One change in `crates/mqtt-zapp/src/tui/message_view.rs`:
- `fit_topic` joins levels with `/` instead of ` / `;
- the leaf is no longer a padded badge, but bold text in the accent colour right after the last `/`.

The shortening logic, the hit rects and the copy buttons stay as they are.

**Tech Stack:** Rust, ratatui 0.29.

**Spec:** the user's request, verbatim: "the topic header should consist of the topic like "muh/wst/status"". This replaces the spaced style from `docs/superpowers/plans/2026-11-03-switch-broker-header-copy.md` Task 2.

## Decisions

- **From the user:** the header row reads exactly like the topic, `muh/wst/status`.
- **Claude's calls** (the user can override these):
  - **Colours:** the parents and separators (`muh/wst/`) are muted. The leaf (`status`) is bold cyan text with no background block and no padding, so the row is one continuous topic.
  - **Mono / `NO_COLOR`:** the leaf stays bold, so it still stands out.
  - **Shortening** keeps the rules from the last round, with `/` as the separator:
    - first, whole middle levels are dropped: `muh/…/status`, keeping the first level plus as many of the last parent levels as fit;
    - next, only `…/status`;
    - next, only the leaf;
    - last, the leaf is cut from the left: `…atus`.
  - **Empty levels** show as they are: `a//b`, and `/` stays `/`.

## Global Constraints

- Widths are display widths (`unicode-width`).
- The width budget becomes parents + leaf ≤ width. The `+ 2` for the badge padding goes away.
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.

## Review Focus

1. **Wide characters.** A CJK topic in a narrow card keeps the leaf and never overflows: `header_row_never_overflows` (updated).
2. **Empty levels and short topics.** `/`, `a`, `a//b` and `///` at every width from 1 to 30 fit and keep the leaf: `plain_path_edge_topics`.
3. **Click to copy.** The click target still covers the whole path, the leaf included, and copies the real topic: `click_header_path_copies_topic` (updated).

---

### Task 1: Plain `a/b/c` header path

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/message_view.rs`:
  - `SEP` becomes `"/"`;
  - `fit_topic` drops the `+ 2` padding budget;
  - `leaf_badge` is replaced by `leaf_span`;
  - doc comments updated.
- Test: `message_view/tests.rs`, `ui/tests.rs`.

**Interfaces:**
- `fit_topic(topic: &str, width: usize) -> (String, String)` keeps its signature. It now guarantees `parents.width() + leaf.width() <= width`, and `fit_topic("muh/wst/status", 40) == ("muh/wst/", "status")`.
- `leaf_span(leaf: &str) -> Span<'static>`: the text is `leaf` unpadded, styled `fg(theme::ACCENT)` + bold.
- `header_lines` row 1 is `" " + parents (MUTED) + leaf_span`. In `header_compact`, the first two spans are still parents and leaf, so the compact hit-rect measurement doesn't change.

- [ ] **Step 1: Write failing tests.** Replace the spaced expectations with these:
  - `plain_path_basic`: `fit("muh/wst/status", 40) == ("muh/wst/", "status")`.
  - `plain_path_shortens_middle_levels`:
    - `fit("tasmota/tele/tasmota_A1B2C3/SENSOR", 18) == ("tasmota/…/", "SENSOR")`;
    - `fit("a/b/c/d/leaf", 11) == ("a/…/d/", "leaf")`;
    - `fit(t, 10) == ("…/", "SENSOR")`;
    - `fit(t, 7) == ("", "SENSOR")`;
    - `fit(t, 4) == ("", "…SOR")`.
  - `plain_path_edge_topics`:
    - `"a"` → `("", "a")`;
    - `"a//b"` → `("a//", "b")`;
    - `"/"` → `("/", "")`;
    - `""` → `("", "")`;
    - loop widths 1..30 over `/`, `a//b`, `///`, `x/yyyyyyyyyyyy/z`, `设备/房间/温度`. The `fit` helper asserts `p.width() + l.width() <= width`.
  - `header_card_draws_box_path_and_badges`: row 1 contains `tasmota/tele/tasmota_A1B2C3/SENSOR`.
    - At the `S` of `SENSOR`: `fg == theme::ACCENT` and bold.
    - The cell before it, the `/`, is MUTED.
  - `header_long_cjk_topic_keeps_leaf`: `parents == "设备/…/"` at width 26, and `fit_topic("a/b/c", 40) == ("a/b/", "c")`.
  - `header_row_never_overflows`: the leaf `温度` is drawn with `fg == ACCENT`, where it used to be `bg`.
  - `header_compact_when_short_or_stacked` and the `badge_row` helper look for `dev/x` instead of `dev /  x`.
  - `click_header_path_copies_topic` and `compact_header_has_copy_glyph` (in `ui/tests.rs`):
    - find `│ dev/x `;
    - a click on `d` and a click on `x` both copy `("topic", "dev/x")`.
- [ ] **Step 2:** `cargo test -p mqtt-zapp`. Expected: the tests above FAIL.
- [ ] **Step 3:** Implement as in Interfaces.
- [ ] **Step 4:** `cargo test --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.
- [ ] **Step 5:** Manual check in tmux on the local mosquitto at port 18830:
  - publish `muh/wst/status`, select it, and check that the card shows `muh/wst/status`, with `status` in cyan;
  - click `⧉ copy` and confirm the tmux buffer holds `muh/wst/status`.
- [ ] **Step 6:** Commit: `feat(tui): header shows the topic as-is (a/b/c)`
