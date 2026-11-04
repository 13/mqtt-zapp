# Round Four: speed, robustness and completeness — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make memory limits real, keep the UI responsive at 200k+ topics, stop losing user state to bad files, make the connection lifecycle honest, and close the most-asked-for gaps: subscription and session options, MQTT 5 feedback, a headless `sub`/`pub` CLI, and more TLS and password sources.

**Architecture:** Core changes live in `mqtt-zapp-core`: tree accounting, state and settings loading, the client lifecycle and options, and a headless runner. TUI changes are limited to wiring, notes and form rows.
- **Performance work** stays on the UI thread. It uses incremental filtering and throttling, not new threads; the tree is not `Send`-shared.
- **New behaviour** comes in as new `Profile` fields. They use `#[serde(default, skip_serializing_if = …)]` so old and new files keep loading.

**Tech Stack:** Rust 2024; rumqttc 0.25 (v4 and v5); ratatui 0.29; tokio; serde, toml and serde_json; clap.

**Spec:** This plan has no separate spec. The authority is three surveys run on 2026-11-04 (performance, measured; robustness, partly reproduced; completeness), which are summarised under each task, plus this Goal.

## Global Constraints

- No new crates. A feature flag of an existing dependency is allowed if `Cargo.lock` gains no new package.
- CI gate, all must pass:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `cargo clippy --no-default-features --all-targets -- -D warnings`
  - `cargo test --workspace --no-default-features`
- Tests that touch config or state use `tui/test_env.rs` `ConfigDir::new(tag)` or a tempdir. Never write the real `~/.config/mqtt-zapp` or `~/.local/state/mqtt-zapp`.
- Wire strings shown to the user go through `payload::sanitize`.
- New keys go in `keymap.rs` `DEFAULTS`, the README key table and `HELP_GROUPS`.
- New settings and profile fields go in the README.
- Old `settings.toml`, `profiles.toml` and `state.toml` files must keep loading unchanged.
- Commits end with the session's `Co-Authored-By` and `Claude-Session` trailers. No push and no tag.

## Review Focus

1. **Memory accounting under adversarial traffic.** Many topics with large retained payloads, or millions of tiny messages: resident memory must track `max_history_mb` within a small factor. Tests: `cap_counts_entry_overhead` and `cap_evicts_latest_payloads` (Task 1).
2. **Incremental filtering must never show a stale match set.** A node whose new payload stops matching must leave the results. Test: `incremental_filter_drops_unmatched` (Task 4).
3. **Auth refusal must stop retrying,** while a network drop keeps retrying, and a broker kicking a duplicate client id must not flap at 1 Hz. Tests: `auth_refused_stops_retrying` (broker) and `backoff_resets_only_after_stable` (Task 6).
4. **A bad file must never cost the user data.** A corrupt or newer `state.toml` is backed up before any overwrite. Test: `corrupt_state_is_backed_up_not_wiped` (Task 3).
5. **The headless CLI must be pipe-safe:** a `SIGPIPE` or closed stdout exits quietly with status 0, and output is line-buffered and flushed. Test: `sub_exits_quietly_on_closed_stdout` (Task 12).

---

### Task 1: Real memory accounting for history

**Files:** core `payload.rs` (`Message::size`), `tree.rs` (`enforce_cap`, insert), `README.md` (settings). Tests are in `tree.rs`.

Survey evidence:
- Measured: 20k topics × 100 messages of 5 B are accounted as 7 MB but cost +434 MB of RSS.
- Reproduced: 100 topics × 100 KB under a 1000-byte cap hold 10 MB, because a topic's latest entry is never evicted.

**Interfaces:**
- `pub const ENTRY_OVERHEAD: usize = 160`. `Message::size()` returns `payload.len() + ENTRY_OVERHEAD + props bytes`.
- `enforce_cap` first evicts old history entries, as today. While the total is still above the cap, it then **stubs** the latest message of the least recently updated topics:
  - The payload is replaced by its first `STUB_KEEP = 256` bytes, cut on a UTF-8 boundary when the payload is UTF-8.
  - `truncated_from = Some(original_len)`.
  - The node, its counts, its time and its preview survive.
- Stubs reuse the existing truncation path, so re-send refuses them, the badge shows truncated, and export marks `truncated_from`.
- `history_bytes()` counts everything, latest entries included.

- [ ] **Step 1: Failing tests:**
  - `cap_counts_entry_overhead`: 10k one-byte messages give `history_bytes() >= 10_000 * 160`.
  - `cap_evicts_latest_payloads`: 100 topics × 100 KB under a 1 MB cap give `history_bytes() <= 1 MB + 100 * (STUB_KEEP + ENTRY_OVERHEAD)`, every topic is still present, and the oldest are `truncated_from.is_some()`.
  - `stub_keeps_newest_topics_whole`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement. Keep the LRU over topics cheap: reuse the existing `order` queue or the update stamps. Don't scan all nodes per insert.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. In the README, the `max_history_mb` comment says it covers all stored messages, and that the oldest topics keep a 256-byte stub.
- [ ] **Step 5:** Commit `fix(core): make max_history_mb bound real memory`.

### Task 2: Removed topics free their `max_topics` slot

**Files:** core `tree.rs`.

Reproduced: with `max_topics = 50`, 200 insert/remove cycles refused 175 topics while 0 were live.

**Interfaces:**
- `TopicTree` keeps a `live: usize` counter, maintained on insert and remove. The `max_topics` check uses it instead of `nodes.len() - 1`.
- A removed node drops its history, children map and name allocation, leaving an empty tombstone.
- Node ids are not reused; reuse is out of scope.

- [ ] **Step 1: Failing tests:** `churn_does_not_exhaust_max_topics` (the loop above, with no refusals) and `removed_node_frees_history`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix(core): count live topics against max_topics`.

### Task 3: Bad files never cost data or block startup

**Files:** core `state.rs`, `config.rs` (`Settings::load`); TUI `mod.rs:45`, plus the app note on startup.

Reproduced: a `state.toml` with an unknown enum value loads as defaults, and the next save wipes all favourites and `last_used`. Separately, a bad `settings.toml` makes `Settings::load()?` abort startup.

**Interfaces:**
- `AppState::load() -> (AppState, Option<String>)`. It returns the load error text when the file exists but does not parse.
  - On a parse error it copies the file to `state.toml.bak` before returning; an existing `.bak` is overwritten.
  - It then parses per top-level key: each of `picker_sort`, `last_used` and `favorites` is parsed from its own `toml::Value` subtree, so one bad key keeps the rest.
- `Settings::load_or_default() -> (Settings, Option<String>)`. It keeps the defaults for an unparseable file and never fails.
- The TUI shows the notes `state file unreadable, backed up to <path>` and `settings.toml: <error> (using defaults)` as error notes at startup.
- The existing `Settings::load() -> Result` stays for the CLI `profile` commands.

- [ ] **Step 1: Failing tests:**
  - `corrupt_state_is_backed_up_not_wiped`: a bad `picker_sort` with valid `favorites` keeps the favourites after load and save, and `.bak` holds the original.
  - `unreadable_state_is_backed_up`.
  - `bad_settings_fall_back_to_defaults`.
  - `startup_notes_bad_settings` (app).
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix: never lose state to a bad file, and start with bad settings`.

### Task 4: Responsive search at 200k topics

**Files:** TUI `app/search.rs` (`rebuild_filter`), `app.rs` (`on_tick`, `FILTER_REBUILD_EVERY`); core `search.rs` (`filter`) and `tree.rs` (touched list).

Measured at 200k nodes: about 40 ms per keystroke, and about 34 ms every 500 ms on a tick while filtering under live traffic.

**Interfaces:**
- **Keystroke debounce.** Typing schedules the rebuild `SEARCH_DEBOUNCE = 120 ms` after the last key; the tick runs it. Below `DEBOUNCE_MIN_NODES = 20_000` it rebuilds immediately, as today. `enter` and `esc` apply at once.
- **Narrowing.** When the new query is a plain, non-regex substring that extends the previous one, and the previous result is complete, only the previous matches are re-tested.
- **Incremental tick.**
  - `TopicTree` records the ids touched since the last `take_touched() -> Vec<NodeId>`, capped at `TOUCHED_MAX = 50_000`; above the cap it reports a full rebuild.
  - The tick re-tests only touched nodes. A newly matching node adds itself and its ancestors.
  - If a previously matching node no longer matches, the tick falls back to a full rebuild, so results are never stale.
- **Payload scan.** It skips the payload test when the path already matched.

- [ ] **Step 1: Failing tests:**
  - `search_debounces_on_big_trees` (counts the `filter` calls).
  - `narrowing_reuses_previous_matches`.
  - `incremental_filter_adds_new_match`.
  - `incremental_filter_drops_unmatched`.
  - `touched_overflow_forces_full_rebuild`.
  - An `#[ignore]` benchmark `perf_search_200k`: `on_search_key` stays under 2 ms.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `perf(tui): debounce, narrow and update the search filter incrementally`.

### Task 5: Cheaper frames for busy topics, and an idle tick

**Files:** TUI `message_view.rs` (`body` cache), `mod.rs` (the tick timer).

Measured: a selected topic receiving 590 KB of JSON costs 15 ms per frame. The idle tick wakes 10 times a second.

**Interfaces:**
- When the selected topic's last body rebuild was under `BODY_MIN_INTERVAL = 250 ms` ago and the payload is over 64 KiB, the cached body is kept. The header's count and time still update. A `history_sel` change rebuilds at once.
- The tick interval is 100 ms while something animates: a note shown, a flash, a pending debounce, `await_reply`, a non-zero message rate, or a connecting state. Otherwise it is 1 s. Input wakes the loop as before.

- [ ] **Step 1: Failing tests:** `hot_big_topic_body_rebuild_is_throttled`, `history_change_rebuilds_body_at_once` and `idle_tick_is_slow`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `perf(tui): throttle big hot bodies and slow the idle tick`.

### Task 6: An honest connection lifecycle

**Files:** core `client.rs` (`run`, backoff, `ConnState`); TUI `mod.rs` (the channel closing) and `ui/bars.rs` (state text).

Read from the code:
- Every CONNACK resets the backoff, so a duplicate client id flaps at 1 Hz.
- Bad credentials retry forever.
- A dead event task leaves the state `Connected`.

**Interfaces:**
- The backoff resets only after the connection has stayed up for `STABLE_AFTER = 10 s`.
- A v4 `ConnectReturnCode::BadUserNamePassword`/`NotAuthorized`, or a v5 `BadUserNamePassword`/`NotAuthorized`/`Banned`, ends the loop with `ConnState::Failed { reason }`.
  - There is no retry.
  - The footer shows `authentication failed: <reason> — pick the profile again to retry`.
- When the TUI's channel closes (`None`), the state becomes `Failed { reason: "connection task ended" }`.

- [ ] **Step 1: Failing tests:**
  - `backoff_resets_only_after_stable`: a unit test with a fake clock or an injectable stable time.
  - `auth_refused_stops_retrying`: a broker test with a mosquitto config file that sets `allow_anonymous false`. Assert a single `Failed` and no further connect attempts within 3 s.
  - `closed_channel_marks_failed` (app).
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix(core): stop on auth refusal and reset backoff only when stable`.

### Task 7: Signals, copy size and the keyring off the UI thread

**Files:** TUI `mod.rs` (signals at about line 245, `Effect::Copy` at about line 105), `app/front.rs` (`load_password`).

**Interfaces:**
- **Signals.** SIGINT and SIGQUIT join SIGTERM and SIGHUP in the shutdown select, and the terminal is restored.
- **Copy.** `Effect::Copy` refuses text over `COPY_MAX = 100 * 1024` bytes with the note `too large to copy (N KiB), use w to save`.
- **Keyring.** The lookup runs in `tokio::task::spawn_blocking`. The UI shows `connecting…` and gets the result as an event. A slow keyring never blocks key handling.

- [ ] **Step 1: Failing tests:** `copy_refuses_huge_text` and `keyring_lookup_does_not_block` (inject a slow lookup closure; `handle_key` returns at once). The signal list is checked by reading the code; there is no pty test.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix(tui): handle SIGINT/SIGQUIT, cap copies, and look up the keyring off-thread`.

### Task 8: Small robustness fixes deferred from round three

**Files:** as listed.

- `export::write_new`: on a write error, remove the file just created.
- `await_reply` is cleared by `reset_tree` (reconnect or profile switch).
- A `history_sel` past the end is clamped the same way as the viewer (`p` re-send and `w` save use the clamped entry).
- Reply follow skips the jump, and keeps the note, when the target row is hidden by the starred view or the filter.

- [ ] **Step 1: Failing tests:** `failed_write_leaves_no_file`, `reconnect_clears_await_reply`, `stale_history_sel_is_clamped` and `reply_follow_respects_hidden_rows`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `fix: small robustness fixes from round three`.

### Task 9: Subscription options and `--sub`

**Files:** core `config.rs` (`Profile.subscriptions`), `client.rs` (`v4_filters`, `v5_filters`); TUI `cli.rs` and `profile_form.rs`; `README.md`.

**Interfaces:**
- Each subscription string is `filter[@qos]`, e.g. `sensors/#@1`; the default QoS is 0. The type stays `Vec<String>` for compatibility. A parser `pub fn parse_sub(s: &str) -> Result<(String, QoS), String>` reports the error `bad QoS in "<s>"`.
- `Profile.retained: bool`, with a default of `true` and `skip_serializing_if` set at the default. When it is false:
  - v5 subscribes with retain handling "do not send", and the `$SYS` defaults are unaffected;
  - v4 ignores it, and the profile form shows `(MQTT 5 only)`.
- `--sub <filter[@qos]>` is repeatable on `tui` and on the new `sub` command. It replaces the profile's list for this session.
- The profile form validates every line with `parse_sub`.

- [ ] **Step 1: Failing tests:**
  - `parse_sub_cases`: `a/#` → QoS 0; `a/#@2` → QoS 2; `a@3` → error; `a@x` → error.
  - `v5_filters_honour_qos_and_retained`.
  - `cli_sub_overrides_profile`.
  - `form_rejects_bad_qos`.
  - The broker test `v5_retained_false_skips_retained`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `feat: per-subscription QoS, skip-retained, and --sub`.

### Task 10: Session options

**Files:** core `config.rs` (Profile), `client.rs` (`mqtt_options`, `mqtt_options_v5`); TUI `cli.rs` and `profile_form.rs`; `README.md`.

**Interfaces:** These optional `Profile` fields use `skip_serializing_if = "Option::is_none"`. Each has a CLI flag and a form row.

| Field | CLI flag | Meaning |
|---|---|---|
| `keepalive_s: Option<u16>` | `--keepalive` | The rumqttc default (5 s) applies when unset. |
| `clean_session: Option<bool>` | `--no-clean` | Sets false. On v5 this maps to clean start. |
| `session_expiry_s: Option<u32>` | `--session-expiry` | v5 only; ignored on v4, and the form says so. |

The validation errors are `keepalive: 0..=65535 s` and `session expiry needs MQTT 5`; the second applies when it is set on a v4 profile. The second is a warning note, not an error.

- [ ] **Step 1: Failing tests:**
  - `options_apply_keepalive_and_clean`, which checks rumqttc `MqttOptions` getters.
  - `v5_session_expiry_in_connect_props`.
  - `old_profiles_load_without_session_fields`.
  - `form_session_rows_round_trip`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `feat: keepalive, clean session and session expiry per profile`.

### Task 11: Show MQTT 5 broker feedback

**Files:** core `client.rs` (`poll`, `Polled`, `CoreEvent`); TUI `app.rs` (notes) and `ui/bars.rs`.

**Interfaces:**
- `CoreEvent::ConnAckInfo { session_present: bool, assigned_client_id: Option<String>, reason: Option<String> }`:
  - From v5, reason, assigned id and session-present.
  - From v4, session-present only.
  - The TUI notes `session resumed` when the session was present, and `client id assigned: <id>` when one was assigned.
- `CoreEvent::SubRefused { filter: String, reason: String }` comes from SUBACK failure codes in v4 and v5. The TUI shows the error note `subscription refused: <filter> (<reason>)`.
- `CoreEvent::ServerDisconnect { reason: String }` comes from a v5 DISCONNECT with its reason code and reason string. The state text shows it.
- All strings are sanitized.

- [ ] **Step 1: Failing tests:**
  - Unit tests mapping rumqttc packets: `suback_failure_becomes_sub_refused` and `v5_disconnect_reason_shown`.
  - The broker test `refused_subscription_is_reported`: a mosquitto ACL denies a topic.
  - The app test `sub_refused_note`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `feat: show MQTT 5 connack, suback and disconnect feedback`.

### Task 12: Headless `sub` and `pub` commands

**Files:** TUI crate `cli.rs` (new `Cmd::Sub` and `Cmd::Pub`); a new `crates/mqtt-zapp/src/headless.rs`; core `export.rs` (a line with the topic); `README.md` (CLI section).

**Interfaces:**
- **`mqtt-zapp sub [conn args] [--sub F@q]… [--format text|jsonl] [-n COUNT] [--retained-only]`**
  - It connects with the same `ConnArgs` and profile resolution as the TUI.
  - `text` prints `<topic> <payload>`, one line each. Binary payloads print as `<base64:…>`, and newlines are escaped as `\n`.
  - `jsonl` prints the `history_jsonl` line shape with a leading `"topic"` field (`export::message_jsonl(topic, msg) -> Vec<u8>`).
  - `-n` exits after COUNT messages. `--retained-only` exits at the first non-retained message or after 2 s of silence.
  - The default subscription is the profile's list, or `#` when there is no profile.
  - Exit codes: 0 on success, 2 on a connect or auth failure (the message goes to stderr).
- **`mqtt-zapp pub [conn args] -t TOPIC (-m MSG | -f FILE | --stdin) [-q 0|1|2] [-r]`**
  - It publishes once, waits for the PUBACK or PUBCOMP at QoS above 0, then disconnects.
  - Exit codes: 0 on success, 2 on failure.
- **Output.** It is written with `LineWriter` on stdout. `BrokenPipe` gives a quiet exit 0. There is no TUI and no terminal mode change.
- **Sparkplug.** It decodes the same way as the TUI.

- [ ] **Step 1: Failing tests:**
  - Broker tests running the binary through `assert_cmd`-free `std::process::Command` (`env!("CARGO_BIN_EXE_mqtt-zapp")`):
    - `pub_then_sub_retained_round_trip`
    - `sub_jsonl_shape`
    - `sub_count_exits`
    - `sub_exits_quietly_on_closed_stdout`
    - `pub_auth_failure_exits_2`
  - Unit tests: `text_line_escapes_newlines_and_binary`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS. In the README, add a "Scripting" section with three examples.
- [ ] **Step 5:** Commit `feat: headless sub and pub commands`.

### Task 13: TLS ALPN, CLI cert flags and more password sources

**Files:** core `config.rs` (`TlsConfig`, `PasswordSource`), `client.rs` (`tls_config`); TUI `cli.rs` and `profile_form.rs`; `README.md`.

**Interfaces:**
- **ALPN.** `TlsConfig.alpn: Vec<String>` (default empty, skipped when empty) sets `rustls::ClientConfig::alpn_protocols`. The flag is `--alpn <proto>`, repeatable.
- **CLI cert flags.** `--cert <file>` and `--key <file>` mirror the profile's existing client cert and key fields.
- **Password sources.** `PasswordSource::Env(String)` reads that variable. `PasswordSource::Command(String)` runs `sh -c <cmd>` with stdin closed and a 10 s timeout, and uses the first line of stdout. Failures:
  - `password env var <NAME> not set`
  - `password command failed: <status/stderr first line>`

  Both run off the UI thread, through Task 7's path.
- The form gets a source selector with the values none / keyring / prompt / env / command, plus a value field.

- [ ] **Step 1: Failing tests:**
  - `alpn_applied`, which inspects the built `ClientConfig`.
  - `cli_cert_key_flags`.
  - `password_from_env`.
  - `password_from_command`, using `printf secret`.
  - `password_command_timeout`, using `sleep 20` with an injected short timeout.
  - `old_password_source_values_load`.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `feat: TLS ALPN, --cert/--key, and env or command passwords`.

### Task 14: Leaner tree nodes

**Files:** core `tree.rs` (`TopicNode`) and `payload.rs`. Measured: about 900 B per node, and 272 B for `TopicNode` alone.

**Interfaces:**
- Drop `TopicNode.latest`. Add `fn latest(&self) -> Option<&Message>` returning `history.back()`, and update all readers.
  - The history always keeps at least the latest entry.
  - Task 1's stub lives in the history.
- Store `name` as `Box<str>`.
- Children are `Vec<(Box<str>, NodeId)>`, kept sorted by name and binary-searched, replacing `BTreeMap<String, NodeId>`. Use whatever ordering is needed today.
- Compute `sort_key` lazily: the natural key is computed when sorting by name, and cached in the rows cache, not per node.

- [ ] **Step 1: Failing tests:** `bytes_per_node_under_budget` (`size_of::<TopicNode>() <= 160`) and an `#[ignore]` RSS benchmark `perf_nodes_100k`. All existing tree tests stay green unchanged.
- [ ] **Step 2:** Run them. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the CI gate. Expected: PASS.
- [ ] **Step 5:** Commit `perf(core): shrink topic nodes`.

---

## Out of scope

- Payload decoders: gzip needs a crate. CBOR and MessagePack (hand-written, about 150 lines each) are next round.
- Proxy support: the rumqttc `proxy` feature adds a crate.
- Per-topic message rate, a retained-only view, and custom colour palettes.
- Moving search to another thread.
- Reusing node ids.
- Shipping (push or tag).
