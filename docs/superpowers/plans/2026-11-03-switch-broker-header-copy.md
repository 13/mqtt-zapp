# Switch Broker, Header Copy Button, Spaced Path: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- Switch to another broker without restarting.
- Add a `⧉ copy` button to the topic header, like the payload pane's.
- Draw the header path as `muh / wsr / json`.

**Architecture:** The picker becomes reachable from a live session. The current connection stays up while you choose, `esc` goes back to it, and picking a different profile reuses the existing `Effect::Connect` path, which already disconnects, resets the tree and connects. The header changes are local to `message_view.rs`, plus one new `HitAreas` rect.

**Tech Stack:** Rust, ratatui 0.29, crossterm.

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md` (§4.1 connections, §4.4 copy). The previous round is `docs/superpowers/plans/2026-11-03-picker-order-header-copy.md`.

## Decisions

From the user:
- It must be possible to connect to another broker from a running session.
- The topic header gets the same `⧉ copy` at its top right as the payload pane, and it copies the topic.
- The header path is drawn as `muh / wsr / json`.

Claude's calls (the user can override any of these):
- **The key is `b` (action `brokers`)** and it opens the picker from browse mode. Clicking the broker name in the top bar does the same.
- **The session stays live while picking.** In the picker:
  - `esc` (or `b` again) returns to it unchanged;
  - `q` still quits;
  - `enter` on another profile switches, through the password prompt if that profile needs one;
  - `enter` on the profile that is already connected just returns, without reconnecting.
- **The picker marks the connected profile with `●`**, and its title gains `· connected: <name>`. The cursor starts on the connected profile.
- **Ad-hoc sessions (`--host`)** open the same picker of saved profiles. Nothing is marked, and `esc` returns to the ad-hoc session.
- **Path style:**
  - The real topic `muh/wsr/json` is drawn as `muh / wsr / json`: muted parents, muted ` / ` separators, and the leaf still as the cyan badge.
  - An empty level (`a//b`) is drawn as `a /  / b`.
  - Copying always gives the real topic `muh/wsr/json`.
  - Shortening keeps the leaf and puts `…` in the middle: `muh / … / json`.
- **Copy button:** in the card it is a right-aligned ` ⧉ copy ` title on the card's top border. In the compact one-row header it is a ` ⧉ ` at the right end of the row. Clicking the path row keeps working too.

## Global Constraints

- Everything that comes from the broker or from profiles is drawn through `sanitize` / one-line helpers.
- Widths are display widths (`unicode-width`).
- `esc` in the picker must never quit; `q` still does.
- New actions are remappable and must appear in the help, which must still fit 80×24 (`help_shows_every_action_at_80x24`), and in the README key table (`readme_key_table_matches_defaults`).
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`, plus `--no-default-features`.

## Review Focus

1. The user switches to a profile that needs a password, then presses `esc` at the password prompt. They must land back in the picker with the *old* session still live, not in a dead browse view. Test: `switch_password_esc_returns_to_picker_session_intact` (Task 1).
2. Messages from the old broker keep arriving while the picker is open. The tree keeps updating underneath, and returning shows them. Test: `messages_keep_arriving_while_picking` (Task 1).
3. The connected profile is renamed or deleted from the picker while connected. The `●` marker follows by name or disappears, and nothing panics. Test: `marker_follows_rename_and_delete` (Task 1).
4. A topic made of empty levels or a single level (`/`, `a`, `a//b`) in the spaced style keeps the leaf and never overflows. Test: `spaced_path_edge_topics` (Task 2).
5. The header `⧉ copy` and the payload `⧉ copy` sit in different rects. A click on one never triggers the other, and with no message shown neither copies. Test: `header_and_payload_copy_buttons_are_distinct` (Task 3).

---

### Task 1: Switch broker from a live session

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/keymap.rs`, `app.rs` (`handle_action`), `app/front.rs` (`on_picker_action`, `on_password_key`), `picker.rs` (`render_picker`), `ui.rs` (picker title and footer hints, help group, top-bar click rect), `app/mouse.rs`, `README.md`
- Test: `app/tests.rs`, `ui/tests.rs`, `picker.rs` tests

**Interfaces:**
- `Action::Brokers`, named `"brokers"`, default `b`. It goes in the help's `actions` group, or `view` if that's what fits.
- `App::in_session(&self) -> bool` is `self.server.is_some()`.
- `Action::Brokers` in browse sets `Mode::Picker`, selects the connected profile by name (`picker.set_profiles(profiles, Some(name))`) and returns `Effect::None`.
- In `on_picker_action`, when `in_session()`:
  - `Action::Cancel | Action::Brokers` → `Mode::Browse`;
  - `Action::Select` on the profile whose name equals `server.name` → `Mode::Browse` plus the note `already connected to <name>`.
- `on_password_key` `esc`: back to `Mode::Picker`, as today. The old session is untouched, because `Effect::Connect` is only returned on enter.
- `render_picker(f, area, st, error, connected: Option<&str>)` draws `●` instead of the leading space on the connected row, and `· connected: <name>` after the sort in the title.
- `HitAreas.header_broker: Rect` covers the broker name span in the top bar. A left click in browse acts like `Brokers`.
- Picker footer hints, in a session: `esc back` before `q quit`.

- [ ] **Step 1: Write failing tests:**
  - `b_opens_picker_on_connected_profile`: connect to `lab` (the second profile) via the picker, press `b`; `mode == Picker` and `picker.selected().name == "lab"`.
  - `esc_in_picker_returns_to_session`, which also covers `b` again.
  - `esc_in_picker_without_session_does_nothing`: mode stays `Picker`, and the effect is not `Quit`.
  - `enter_on_connected_profile_returns_without_reconnect`: the effect is `None`, the mode is `Browse`, and the note is `already connected to lab`.
  - `enter_on_other_profile_switches`: returns `Effect::Connect(p, _)` with `p.name == "home"`.
  - `switch_password_esc_returns_to_picker_session_intact`: the target profile uses `Prompt`, press enter, then `esc`. The mode is `Picker`, `in_session()` is still true, and `server.name` is the old one.
  - `messages_keep_arriving_while_picking`: in the picker, `apply_core_event(msg)` and then `esc`; the topic is in the tree.
  - `marker_follows_rename_and_delete` (`picker.rs` render): render with `connected = Some("lab")` and find `●` on the `lab` row. After `set_profiles` without `lab`, there is no `●`.
  - `picker_title_shows_connected` (`ui/tests.rs`): `pick a broker · custom · connected: lab`.
  - `click_broker_name_opens_picker` (`ui/tests.rs`).
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement. Add the README row `| \`b\` | \`brokers\` | Switch broker: open the picker (esc returns) |`, and one sentence in *Connecting*.
- [ ] **Step 4:** Run `cargo test --workspace && cargo test --workspace --no-default-features && cargo clippy --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Manual check: run two mosquitto instances (ports 18830 and 18831) and two profiles, then in tmux:
  - `b`, `j`, enter: the header shows the second broker;
  - `b`, `esc`: still the same broker;
  - `b`, enter on the connected profile: the note appears.
- [ ] **Step 6:** Commit: `feat(tui): switch broker from a live session (b)`

### Task 2: Spaced header path `muh / wsr / json`

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/message_view.rs` (`fit_topic`, `header_lines`, `header_compact`)
- Test: `message_view/tests.rs`

**Interfaces:**
- `fit_topic(topic, width) -> (String, String)` keeps its signature. The first part is now the spaced parents including the trailing ` / `, for example `"muh / wsr / "`. The leaf is unchanged. The width budget is the same: spaced parents + leaf + 2 ≤ width.
- Shortening: when the spaced parents don't fit, drop whole middle levels and replace them with `…`, keeping the first level, as in `muh / … / `. If even that doesn't fit, use `… / `. With no room for that either, drop the parents, then cut the leaf from the left (today's last resort).
- Parents and separators are drawn muted, as today; only the leaf badge changes colour.

- [ ] **Step 1: Write failing tests** (update the `fit_topic` and header tests that assert `tasmota/tele/tasmota_A1B2C3/`):
  - `spaced_path_basic`: `fit_topic("muh/wsr/json", 40) == ("muh / wsr / ", "json")`.
  - `spaced_path_shortens_middle_levels`: `fit_topic("tasmota/tele/tasmota_A1B2C3/SENSOR", 22)` gives `("tasmota / … / ", "SENSOR")`. Narrower than that gives `("… / ", "SENSOR")`.
  - `spaced_path_edge_topics`: `"a"` → `("", "a")`; `"a//b"` → `("a /  / ", "b")`; `"/"` → `(" / ", "")`. Each has width ≤ budget.
  - `header_card_draws_box_path_and_badges`: row 1 contains `tasmota / tele / tasmota_A1B2C3 /  SENSOR `.
  - `header_long_cjk_topic_keeps_leaf`: same assertions, now with the spaced output.
  - `copy_still_gives_real_topic`: click the path; `Effect::Copy` text is `tasmota/tele/tasmota_A1B2C3/SENSOR`.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp message_view`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run `cargo test -p mqtt-zapp`. Expected: PASS.
- [ ] **Step 5:** Commit: `feat(tui): header path drawn as "a / b / c"`

### Task 3: `⧉ copy` on the topic header

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/message_view.rs` (`render_message`), `app/mouse.rs` (`HitAreas`, `browse_click`), `README.md` (Mouse section)
- Test: `ui/tests.rs`

**Interfaces:**
- `HitAreas.header_copy: Rect`.
- Card: `Block` title `Line::from(" ⧉ copy ").right_aligned()` in `MUTED`, on the card's top border. Its rect uses the same arithmetic as `payload_copy`: `x = right - 1 - w`, `y = card.y`.
- Compact: the row ends with ` ⧉ `, right-aligned at `meta.right() - 3`, and the rect covers those 3 cells. The badges and time are fitted so they never run under it.
- `browse_click` checks `header_copy` (→ `self.copy(true)`) before `header_topic` and `payload_copy`.
- `header_copy` is reset to `Rect::default()` on the nothing-shown path, as `header_topic` is.

- [ ] **Step 1: Write failing tests:**
  - `click_header_copy_copies_topic`: find `⧉ copy` on the card's border row (row 0 of the message area). Clicking it gives `("topic", "dev/x")`.
  - `header_and_payload_copy_buttons_are_distinct`: there are two `⧉ copy` on screen. The first (header) gives a topic and the second (payload) gives the payload. With no selection, clicking at either old position gives `Effect::None`.
  - `compact_header_has_copy_glyph`: at 79×24 (stacked), the header row ends with `⧉`, and clicking it gives a topic.
- [ ] **Step 2:** Run `cargo test -p mqtt-zapp click_`. Expected: FAIL.
- [ ] **Step 3:** Implement. In the README *Mouse* section, change it to "click `⧉ copy` on the header or the payload pane".
- [ ] **Step 4:** Run `cargo test --workspace && cargo clippy --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Manual check in tmux with SGR mouse sequences and `set-clipboard on`: the header `⧉ copy` puts the topic in the tmux buffer.
- [ ] **Step 6:** Commit: `feat(tui): copy button on the topic header`
