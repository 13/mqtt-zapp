# History j/k Direction Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the history list, `j` moves the selection down the screen and `k` moves it up, as everywhere else.

**Architecture:** Only the key mapping is wrong; the movement helpers are fine. The fix is one swapped pair of match arms in `App::handle_action`.

The history pane is drawn newest first, with the newest entry on top. Today:
- `k` (`Action::Up`) calls `history_older`, so the highlight moves *down* the screen;
- `j` (`Action::Down`) calls `history_newer`, so it moves *up*.

The mouse wheel is already right: wheel down goes to older entries (`app/mouse.rs:184`). So is the footer hint `j/k older/newer`, which describes the fixed behaviour.

**Tech Stack:** Rust, ratatui 0.29.

**Spec:** the user's bug report, verbatim: "fix bug in history j and k are vice versa".

## Decisions

From the user:
- `j` and `k` are swapped in the history list.

Claude's calls:
- **What moves:** `j` and `↓` go to an older entry, which is down the screen. `k` and `↑` go to a newer one, which is up the screen.
- **Live view:** at the top (the newest entry), the view becomes live again, as `j` does today.
- **Footer hint:** `j/k older/newer` stays. It is right once the keys are fixed.
- **Remapped keys:** these follow automatically, since only actions change.
- **Out of scope:** `g`/`G` in the history pane.
- **Branch:** based on `main`. The unmerged `plain-topic-header` branch doesn't touch this code.

## Global Constraints

- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.

## Review Focus

1. **The keys must match the screen.** After `j`, the highlighted history row is *below* the one highlighted before. The tests should check the drawn row, not just `history_sel`: `history_j_moves_highlight_down_the_screen`.
2. **Starting from live:** with nothing pinned, `j` pins the second-newest entry, and `k` stays live: `history_k_from_live_stays_live`.
3. **Keys and wheel agree:** wheel down and `j` move the same way: `history_wheel_and_jk_agree`.

---

### Task 1: Swap j/k in the history pane

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/app.rs`, about lines 766–767:
  - `Action::Up if self.focus == Focus::History` → `self.history_newer()`;
  - `Action::Down if self.focus == Focus::History` → `self.history_older()`.
  
  Also update the doc comments on `history_older` and `history_newer` (about lines 906 and 918) to name the right keys.
- Test: `crates/mqtt-zapp/src/tui/message_view/tests.rs`, `crates/mqtt-zapp/src/tui/ui/tests.rs`.

**Interfaces:** none new.

- [ ] **Step 1: Write failing tests**, flipping the old ones:
  - `history_navigation_up_down` (`message_view/tests.rs:574`): swap every `Action::Up` and `Action::Down` in it; the expected `history_sel` values stay.
  - Swap the same way in:
    - `history_up_with_single_entry_stays_live`, renamed `history_down_with_single_entry_stays_live` (`:594`);
    - the test at `:611`–`:614`;
    - every other test that drives the history pane with `Action::Up`/`Down` after `Focus::History` or `hist(...)`. Check `message_view/tests.rs:347, 547–554` and `ui/tests.rs:161, 302` and keep the ones whose focus is the tree.
  - New `history_j_moves_highlight_down_the_screen` (`ui/tests.rs`):
    - set up `dev/x` with payloads `p1`..`p4`, focus History, and draw at 100×30;
    - press `j` via `handle_key`, draw, and find the row with the history selection background;
    - press `j` again and draw: the highlighted row's y is one *greater* than before;
    - press `k`: back to the first y.
  - New `history_k_from_live_stays_live`: with `history_sel == None`, `k` leaves it `None`, and `j` gives `Some(len - 2)`.
  - New `history_wheel_and_jk_agree`: one wheel-down over the history pane gives the same `history_sel` as one `j` on a fresh app.
- [ ] **Step 2:** `cargo test -p mqtt-zapp history`. Expected: the flipped and new tests FAIL.
- [ ] **Step 3:** Swap the two match arms and fix the two doc comments.
- [ ] **Step 4:** `cargo test --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.
- [ ] **Step 5:** Manual check in tmux on the local mosquitto at port 18830:
  - publish 4 messages to one topic, select it, and `tab` twice to the history pane;
  - `j` should move the highlight down to older entries;
  - `k` should move it back up, and at the top the view should be live again.
- [ ] **Step 6:** Commit: `fix(tui): j/k in the history list move down/up the screen`
