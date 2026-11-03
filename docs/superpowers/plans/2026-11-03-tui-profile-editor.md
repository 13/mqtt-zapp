# TUI Profile Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users add, edit and delete broker profiles from the TUI picker, so they no longer need `mqtt-zapp profile add …`.

**Architecture:** The picker gets three new keymap actions: `a` adds a profile, `e` edits one and `D` deletes one. Add and edit open a new modal form (`tui/profile_form.rs`) that covers every `Profile` field. Delete reuses the existing yes/no `Confirm` card, generalized so it can also return to the picker. `App` never does I/O itself. Saving returns an `Effect`, and `Session::apply` (in `tui/mod.rs`) carries it out. It reloads `profiles.toml`, applies the change, saves the file and updates the keyring, then reports back through `App::profile_saved` or `App::profile_save_failed`.

**Tech Stack:** Rust 2024, ratatui 0.29, crossterm 0.28, keyring (optional feature). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-03-mqtt-zapp-design.md` §4 (profiles, keyring passwords). The README's "Not yet" list includes "a profile editor in the TUI". The user asked for "add/edit/remove profiles in tui" and skipped design questions, so the decisions below marked *(my call)* are open to change.

## Global Constraints

- Passwords never go into `profiles.toml`. A typed password goes only to `secrets::store_password`. No `Debug` impl or log may print it. Write `Debug` by hand, the way `PasswordState` does.
- Every profile string drawn on screen goes through `sanitize(..).replace('\n', " ")`. That covers names, hosts, users, paths and subscriptions.
- The terminal minimum stays 40×10. The form must not panic at that size.
- If `App::store_error` is `Some` (the `profiles.toml` on disk failed to parse), every add, edit and delete is refused with the note `profiles.toml has errors – fix it by hand first`. Saving would otherwise overwrite the user's broken file with an empty list.
- Saves use read-modify-write against the file on disk (`ProfileStore::load` → change → `save`), the same way the CLI does. They never write `app.picker.profiles` back wholesale.
- Key choices *(my call)*: `add_profile = "a"`, `edit_profile = "e"`, `delete_profile = "D"`. All three can be remapped in `[keys]`. None of them conflicts with an existing default.
- The CLI `profile add/list/remove` keeps working unchanged.
- CI gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`. Also run `cargo test -p mqtt-zapp --no-default-features` for the code paths without keyring.
- Every commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

**Non-goals:** editing profiles while connected (the picker is the only entry point), importing or exporting profiles, testing a connection from inside the form.

## Review Focus

1. **Broken `profiles.toml`:** the picker shows the parse error, and `a`/`e`/`D` must not overwrite the file. Test: Task 4 `profile_keys_refused_when_store_broken`.
2. **Renaming a keyring profile:** the keyring entry is keyed by name. A rename with the password left blank must move the secret to the new name, or the next connect fails. Test: Task 5 `rename_moves_keyring_secret` (keyring calls injected).
3. **Name collisions:** adding `home` when `home` exists, or renaming `lab` to `home`, is refused in the form and never silently replaces the other profile. Tests: Task 1 `replace_refuses_collision`, Task 2 `save_rejects_taken_name`.
4. **Hostile strings and tiny terminals:** an ESC in a stored host is drawn sanitized, and the form at 40×10 scrolls to keep the focused field visible. Tests: Task 3 `form_sanitizes_values`, `form_fits_40x10_and_shows_focus`.
5. **File changed outside the app:** a profile added with the CLI while the TUI is open survives a TUI save. Test: Task 5 `save_preserves_profiles_added_on_disk`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/mqtt-zapp-core/src/config.rs` | `ProfileStore::replace` (rename-aware upsert) |
| `crates/mqtt-zapp-core/src/secrets.rs` | `delete_password` |
| `crates/mqtt-zapp/src/tui/profile_form.rs` (new) | `LineInput`, `ProfileForm` state, keys, validation, `render` |
| `crates/mqtt-zapp/src/tui/keymap.rs` | three new actions |
| `crates/mqtt-zapp/src/tui/app.rs` | `Mode::ProfileForm`, picker actions, generalized `Confirm`, profile effects |
| `crates/mqtt-zapp/src/tui/mod.rs` | `Session::apply` for the profile effects |
| `crates/mqtt-zapp/src/tui/ui.rs`, `picker.rs` | dispatch rendering, footer hints, new empty hint |
| `README.md` | document the keys; remove the item from "Not yet" |

---

### Task 1: Core store and keyring helpers

**Files:**
- Modify: `crates/mqtt-zapp-core/src/config.rs`, `crates/mqtt-zapp-core/src/secrets.rs`

**Interfaces:**
- Produces: `ProfileStore::replace(&mut self, old_name: Option<&str>, profile: Profile) -> anyhow::Result<()>`. `old_name = None` adds a new profile. `Some(n)` replaces the profile named `n` in place, keeping its position, and the profile may have a new name. It fails with `profile '<name>' already exists` when `profile.name` belongs to a profile other than `old_name`. It fails with `unknown profile '<n>'` when `old_name` is missing.
- Produces: `secrets::delete_password(profile: &str) -> anyhow::Result<()>`. A missing entry (`NoEntry`) counts as `Ok`. Without the feature it is a no-op `Ok(())`, because deleting a profile must still work when keyring support is not compiled in.

- [ ] **Step 1: Write failing tests** in `config.rs` tests:

```rust
#[test]
fn replace_renames_in_place() {
    let mut s = ProfileStore::default();
    s.upsert(sample("a")); s.upsert(sample("b"));
    s.replace(Some("a"), sample("z")).unwrap();
    assert_eq!(s.profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["z", "b"]);
    s.replace(None, sample("c")).unwrap();
    assert_eq!(s.profiles.len(), 3);
}

#[test]
fn replace_refuses_collision() {
    let mut s = ProfileStore::default();
    s.upsert(sample("a")); s.upsert(sample("b"));
    assert!(s.replace(Some("a"), sample("b")).unwrap_err().to_string().contains("already exists"));
    assert!(s.replace(None, sample("a")).is_err());
    assert!(s.replace(Some("nope"), sample("x")).unwrap_err().to_string().contains("unknown profile"));
    s.replace(Some("a"), sample("a")).unwrap(); // same name: plain edit
}
```

In `secrets.rs` (`not(feature = "keyring")` tests): `assert!(super::delete_password("p").is_ok());`

- [ ] **Step 2:** `cargo test -p mqtt-zapp-core replace_ && cargo test -p mqtt-zapp-core --no-default-features secrets`. Expected: FAIL (methods missing).
- [ ] **Step 3:** Implement both. Under the feature, `delete_password` calls `keyring::Entry::new(SERVICE, profile)?.delete_credential()`. Check the keyring crate's API in `Cargo.lock`, because older versions call it `delete_password`. Map `keyring::Error::NoEntry` to `Ok`.
- [ ] **Step 4:** Rerun the commands from Step 2. Expected: PASS.
- [ ] **Step 5:** Commit `feat(core): rename-aware ProfileStore::replace and secrets::delete_password`.

---

### Task 2: Profile form state, keys and validation

**Files:**
- Create: `crates/mqtt-zapp/src/tui/profile_form.rs` (register `mod profile_form;` in `tui/mod.rs`)

**Interfaces:**
- Produces:
  - `pub struct LineInput { text: String, cursor: usize /* char index */ }`. It provides `new(&str)`, `text(&self) -> &str` and `on_key(&mut self, KeyEvent) -> bool` (true if the key was consumed). It handles printable chars (no CTRL/ALT), Backspace, Delete, Left, Right, Home and End.
  - `pub enum PField { Name, Host, Port, Protocol, WsPath, Username, PasswordSource, Password, ClientId, Subscriptions, CaFile, ClientCert, ClientKey, Insecure }`, in that order.
  - `pub struct ProfileForm { pub field: PField, pub error: Option<String>, pub original: Option<String>, /* inputs… */ }`. `original` is the name being edited, or `None` for a new profile. `Debug` is written by hand and shows the password only as `<n chars>`.
  - `ProfileForm::new(existing: Option<&Profile>, taken: Vec<String>) -> ProfileForm`. `taken` holds every other profile's name.
  - `ProfileForm::visible_fields(&self) -> Vec<PField>`. `WsPath` appears only for ws/wss, `Password` only when the source is `Keyring`, and `CaFile`/`ClientCert`/`ClientKey`/`Insecure` only when `protocol.is_tls()`.
  - `ProfileForm::on_key(&mut self, KeyEvent) -> ProfileOutcome`
  - `pub enum ProfileOutcome { Continue, Cancel, Save(ProfileEdit) }`
  - `pub struct ProfileEdit { pub original: Option<String>, pub profile: Profile, pub password: Option<String> }`. `password` is `Some` only when the source is Keyring and the user typed one. Its `Debug` is written by hand, the same way as `ProfileForm`.

**Keys:** Tab/Down and BackTab/Up move between visible fields, and Enter does too in single-line fields. Esc cancels and ctrl-s saves. On `Protocol` and `PasswordSource`, Space, Left and Right cycle the value. On `Insecure`, Space toggles it. `Subscriptions` is a single `LineInput` holding a space-separated list *(my call: MQTT filters rarely contain spaces, and this keeps every field one line high)*. When the protocol changes and the port still equals the old protocol's `default_port()`, the port changes to the new default.

**Validation on ctrl-s**, in this order, setting `error` and returning `Continue` on the first failure:
- name trimmed empty → `name is required`
- name in `taken` → `profile '<name>' already exists`
- host empty → `host is required`
- port not a `u16` in 1..=65535 → `port must be 1–65535`
- Keyring with an empty password and `original` is `None` → `type a password, or use prompt`. An edit with an empty password keeps the stored one.
- no subscriptions → `at least one subscription`

Empty optional fields become `None` (client id, username, TLS paths). An empty ws path becomes `"/"`. When `existing` is `Some`, each field starts from its value. A new profile starts as Mqtt, port 1883, source `None`, and subscriptions `# $SYS/#`.

- [ ] **Step 1: Write failing tests** in `profile_form.rs`. Add a small helper `fn type_str(f: &mut ProfileForm, s: &str)` and `fn key(c: KeyCode) -> KeyEvent`.

```rust
#[test] fn line_input_edits_mid_line() // type "hst", Left, Left, type "o" → "host"; Home, Delete → "ost"; End, Backspace → "os"
#[test] fn new_form_defaults() // visible_fields == [Name, Host, Port, Protocol, Username, PasswordSource, ClientId, Subscriptions]
#[test] fn protocol_cycle_moves_default_port_only() // port "1883" → cycle to Mqtts → "8883"; set port "9000" → cycle → stays "9000"; Mqtts shows CaFile..Insecure; Ws shows WsPath
#[test] fn save_builds_profile() // name "home", host "h", subs "a/# b" → Save(ProfileEdit{original: None, profile.subscriptions == ["a/#","b"], username None, client_id None, ws_path "/", password None})
#[test] fn save_rejects_taken_name() // taken = ["home"], name "home" → Continue, error == Some("profile 'home' already exists")
#[test] fn save_validates_in_order() // empty → "name is required"; name only → "host is required"; port "0" → "port must be 1–65535"
#[test] fn keyring_password_required_only_for_new() // new+Keyring+empty → error; edit (existing Keyring profile)+empty → Save with password None; typed "pw" → Some("pw")
#[test] fn edit_prefills_and_keeps_original() // ProfileForm::new(Some(&p), vec![]) then rename → Save with original Some("a"), profile.name "b", tls fields carried over
#[test] fn debug_hides_password() // type "s3cret" in Password; assert!(!format!("{form:?}").contains("s3cret"))
```

Write each test's assertions as the comment describes, using exactly those strings.

- [ ] **Step 2:** `cargo test -p mqtt-zapp profile_form`. Expected: FAIL to compile.
- [ ] **Step 3:** Implement the module as specified above.
- [ ] **Step 4:** Run the Step 2 command again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(tui): profile form state and validation`.

---

### Task 3: Render the profile form

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/profile_form.rs`

**Interfaces:**
- Consumes: `ProfileForm` from Task 2. `theme::{centered, pane, dim_backdrop, ERR, MUTED, ACCENT}`.
- Produces: `pub fn render(f: &mut Frame, area: Rect, form: &ProfileForm)`.

**Layout:** a card from `theme::centered(area, 72, visible_fields().len() as u16 + 4)` titled `add profile` or `edit profile · <name>`. If `error` is set, the title shows the error in `theme::ERR`, the same way `publish::render` does. Each row is a label padded to 13 columns, then the value. The focused row gets a `▶ ` marker and a reversed-cell cursor at `LineInput::cursor`. Enum fields render as `‹ mqtts ›`, and the toggle as `[x]`/`[ ]`. The password row shows `•` for each char. When editing a keyring profile and the password is empty, the row shows the muted text `(unchanged)`. The last row is the muted hint `ctrl-s save  tab next  space change  esc cancel`. When the card is shorter than the field list, rows scroll so the focused one stays visible, using the same skip formula as `render_picker`. Each value goes through `sanitize(..).replace('\n', " ")`, character by character so the cursor index still matches. This mirrors `publish::topic_line`.

- [ ] **Step 1: Write failing tests**, using the `TestBackend` pattern the `ui.rs` tests already use:
  - `form_sanitizes_values`: a profile with host `"evil\x1b[31m"`, rendered at 80×24. The buffer contains no `'\x1b'` cell and does contain `evil`.
  - `form_fits_40x10_and_shows_focus`: render at 40×10 with the focus on `Subscriptions`. No panic, and the buffer contains `▶` on the same row as `subscriptions`.
  - `form_masks_password`: Keyring source, password `"abc"`. The buffer contains `•••` and not `abc`.
  - `edit_title_and_unchanged_hint`: editing keyring profile `home`. The buffer contains `edit profile · home` and `(unchanged)`.
- [ ] **Step 2:** `cargo test -p mqtt-zapp profile_form`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement `render`.
- [ ] **Step 4:** Run the Step 2 command again. Expected: PASS.
- [ ] **Step 5:** Commit `feat(tui): draw the profile form`.

---

### Task 4: Wire the form and delete into the picker

**Files:**
- Modify: `tui/keymap.rs`, `tui/app.rs`, `tui/ui.rs`, `tui/picker.rs`

**Interfaces:**
- Consumes: `ProfileForm`, `ProfileOutcome`, `ProfileEdit`, `profile_form::render` (Tasks 2–3).
- Produces:
  - `Action::{AddProfile, EditProfile, DeleteProfile}` named `add_profile`/`edit_profile`/`delete_profile` with defaults `a`/`e`/`D`.
  - `Mode::ProfileForm`, `App::profile_form: Option<ProfileForm>`.
  - `Confirm` becomes `pub struct Confirm { pub prompt: String, pub action: ConfirmAction }`, with `pub enum ConfirmAction { ClearRetained(Vec<String>), DeleteProfile(String) }`. After any key in `Mode::Confirm`, the mode goes back to `Mode::Picker` for `DeleteProfile` and to `Mode::Browse` otherwise.
  - New `Effect` variants: `SaveProfile(ProfileEdit)` and `DeleteProfile(String)`.
  - `App::profile_saved(&mut self, profiles: Vec<Profile>, select: Option<&str>, note: &str)`. It replaces `picker.profiles`, selects `select` by name (or clamps `sel` when it is `None`), closes the form, returns to `Mode::Picker` and sets an info note.
  - `App::profile_save_failed(&mut self, err: String)`. If the form is open, it sets `form.error = Some(err)` and keeps the form open. Otherwise it sets an error note.

**Behaviour:**
- Picker: `AddProfile` opens `ProfileForm::new(None, all names)`. `EditProfile` opens it on the selected profile with `taken` = all names except that one, and does nothing when the list is empty. `DeleteProfile` asks `delete profile <name>?`. All three are refused with the store-error note from Global Constraints when `store_error.is_some()`.
- `Mode::ProfileForm` keys bypass the keymap and go to `form.on_key`. `Cancel` returns to `Mode::Picker`. `Save(edit)` returns `Effect::SaveProfile(edit)` and leaves the form open until `profile_saved` or `profile_save_failed` is called.
- `ui.rs`: `Mode::ProfileForm` and a `Confirm` for profile deletion are drawn on the front screen. The front screen draws the picker, a dimmed backdrop, then the form or the confirm card. Footer hints: the picker gains `(k(AddProfile), "add")`, `(k(EditProfile), "edit")` and `(k(DeleteProfile), "delete")` after "connect". `Mode::ProfileForm` gets `ctrl-s save`, `tab next`, `esc cancel`.
- `picker::EMPTY_HINT` becomes `no profiles – press a to add one`. Update the `empty_picker_hint` test to that string.

- [ ] **Step 1: Write failing tests** in `app.rs`, using the existing `picker_app` helper:
  - `add_key_opens_form_and_save_emits_effect`: press `a`, so the mode is `ProfileForm`. Type the name `n` and host `h` through `handle_key`, then press ctrl-s. Assert `matches!(e, Effect::SaveProfile(ref ed) if ed.profile.name == "n")` and that the mode is still `ProfileForm`.
  - `profile_saved_closes_form_and_selects`: call `a.profile_saved(vec![profile("a",N), profile("n",N)], Some("n"), "saved n")`. The mode is `Picker`, `picker.sel == 1` and the note text is `saved n`.
  - `profile_save_failed_keeps_form`: the error shows in `profile_form.error`.
  - `edit_on_empty_picker_does_nothing`: the mode stays `Picker`.
  - `delete_confirms_then_returns_to_picker`: press `D`, then `y`. The effect is `Effect::DeleteProfile("a")` and the mode is `Picker`. Press `D` then `n`: no effect, the mode is `Picker`, and the note is `cancelled`.
  - `profile_keys_refused_when_store_broken`: set `store_error = Some(..)` and press `a`, `e` and `D`. The mode stays `Picker` and the note contains `profiles.toml has errors`.
  - Update the existing `ClearRetained` confirm tests to use `ConfirmAction::ClearRetained`.
  - `keymap.rs`: `profile_actions_have_defaults`, checking that `a`, `e` and `D` look up to the three actions.
  - `ui.rs`: `picker_footer_shows_profile_keys` checks that the footer contains `add` and `delete`. `profile_form_drawn_over_picker` checks that the buffer contains `add profile` after pressing `a`.
- [ ] **Step 2:** `cargo test -p mqtt-zapp`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement as described. In `mod.rs`, `Session::apply` gets temporary arms `Effect::SaveProfile(_) | Effect::DeleteProfile(_) => {}` so the crate compiles. Task 5 fills them in.
- [ ] **Step 4:** `cargo test -p mqtt-zapp && cargo clippy --all-targets -- -D warnings`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(tui): add, edit and delete profiles from the picker`.

---

### Task 5: Persist changes (profiles.toml and keyring)

**Files:**
- Modify: `crates/mqtt-zapp/src/tui/mod.rs`, `README.md`

**Interfaces:**
- Consumes: `ProfileStore::{load, save, replace, remove}`, `secrets::{store_password, load_password, delete_password}`, `App::{profile_saved, profile_save_failed}`.
- Produces: a pure, testable core in `mod.rs`:

```rust
/// Keyring calls, injected so tests never touch the real keyring.
pub(crate) trait Secrets {
    fn store(&self, name: &str, pw: &str) -> Result<()>;
    fn load(&self, name: &str) -> Result<Option<String>>;
    fn delete(&self, name: &str) -> Result<()>;
}
pub(crate) fn save_profile(store: &mut ProfileStore, edit: ProfileEdit, s: &dyn Secrets) -> Result<()>;
pub(crate) fn delete_profile(store: &mut ProfileStore, name: &str, s: &dyn Secrets) -> Result<()>;
```

**`save_profile` order:**
1. `store.replace(edit.original.as_deref(), edit.profile.clone())?`
2. If the source is Keyring:
   - When `edit.password` is `Some(pw)`, call `s.store(new_name, pw)`.
   - When it is `None` and the profile was renamed, call `s.load(old)`. If that returns `Some(pw)`, call `s.store(new, pw)` and then `s.delete(old)`.
3. If the profile was Keyring before and is now another source, or was renamed and the secret moved, call `s.delete(old)`. Ignore a delete error, but still save.

The caller then runs `store.save()`.

**`delete_profile`:** `store.remove(name)`, or fail with `unknown profile '<name>'`. If the removed profile used Keyring, call `s.delete(name)` and ignore its error.

**`Session::apply`:** run `ProfileStore::load()`, then `save_profile` or `delete_profile` with a `RealSecrets` that wraps `secrets::*`, then `store.save()`. On `Ok`, call `app.profile_saved(store.profiles, Some(&name) /* None for delete */, "saved <name>" / "deleted <name>")`. On `Err(e)`, call `app.profile_save_failed(format!("{e:#}"))`. This runs synchronously on the UI thread, because it is small local file I/O.

- [ ] **Step 1: Write failing tests** in `mod.rs` using a `FakeSecrets(RefCell<HashMap<String,String>>)`:
  - `rename_moves_keyring_secret`: the fake holds `{"a": "pw"}`. Save an edit renaming `a` → `b` (Keyring, password `None`). The fake becomes `{"b": "pw"}`.
  - `new_password_stored_under_name`: a new profile `k` with password `Some("x")` makes the fake hold `{"k": "x"}`.
  - `switching_off_keyring_deletes_secret`: the entry for `a` is removed.
  - `delete_removes_profile_and_secret`, plus `delete_unknown_errors` with the message `unknown profile 'zz'`.
  - `save_preserves_profiles_added_on_disk`: under `MQTT_ZAPP_CONFIG_DIR` set to a tempdir (serialize with a static `Mutex`, the way `config.rs` tests do), save a store containing `x` to disk. Then run the load → `save_profile(new "y")` → save path through a helper `persist_edit(edit, &dyn Secrets) -> Result<ProfileStore>`, which `Session::apply` also uses. The file then contains both `x` and `y`.
- [ ] **Step 2:** `cargo test -p mqtt-zapp tui::tests` (or the module path you chose). Expected: FAIL.
- [ ] **Step 3:** Implement `Secrets`, `RealSecrets`, `save_profile`, `delete_profile`, `persist_edit` and `persist_delete`, and replace the temporary arms from Task 4.
- [ ] **Step 4:** Run the full gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && cargo test -p mqtt-zapp --no-default-features`. Expected: all PASS.
- [ ] **Step 5:** README: under the picker section, add "In the picker, `a` adds a profile, `e` edits the selected one and `D` deletes it." Remove "a profile editor in the TUI" from "Not yet". Add `add_profile`, `edit_profile` and `delete_profile` to the `[keys]` action list if the README has one.
- [ ] **Step 6:** Manual smoke test: run `MQTT_ZAPP_CONFIG_DIR=$(mktemp -d) cargo run`, then add a profile, edit it, rename it and delete it. Check `profiles.toml` after each step.
- [ ] **Step 7:** Commit `feat(tui): save profile edits to profiles.toml and the keyring`.
