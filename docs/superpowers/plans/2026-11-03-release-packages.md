# Release Packages and README Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pushing a `v*` tag makes the GitHub release carry four files plus a checksum list:
- the existing static musl binary;
- an AppImage;
- an Arch Linux `pkg.tar.zst`;
- a Debian `.deb` that installs with `apt install ./file.deb`.

The README is then redone as a modern project page: a hero with a logo and badges, a demo recording, feature highlights, per-distro install, and the reference material folded into sections.

**Architecture:** `.github/workflows/release.yml` grows from one job into a fan-out:

```
version ─┬─ musl ── appimage ─┐
         ├─ deb ──────────────┼─ release
         └─ arch ─────────────┘
```

- Each package job uploads its file as a workflow artifact.
- `release` downloads them all, writes `SHA256SUMS`, and creates the GitHub release. It does that only for tag pushes.
- A manual run (`workflow_dispatch`) builds and smoke-tests everything and keeps the artifacts, without releasing. That is the dry run.
- Packaging inputs live in a new `packaging/` directory, so each job's steps can also run locally:
  - the AppImage build script;
  - the PKGBUILD;
  - the desktop file and icon.

**Tech Stack:** GitHub Actions, cargo-deb, makepkg (in an `archlinux` container), appimagetool. Existing: Rust 2024, the musl static build.

**Spec:** the user's requests, verbatim:
- "gh ci should create on new release an appimage, a archlinux pacman file pkg.tar.zst, and a debian ready to install package";
- "continue plan make a beautiful,modern and so on README.md".

## Decisions

- **From the user:** a new release produces an AppImage, an Arch `pkg.tar.zst` and a ready-to-install Debian package.
- **Claude's calls** (the user can override these):
  - **Trigger:** unchanged, a pushed `v*` tag. The release job fails first if the tag is not `v` + the workspace `version` from `Cargo.toml`, because every package name carries that version.
  - **Keyring:**
    - The `.deb` and the Arch package are normal glibc builds with the default `keyring` feature. They depend on the system's dbus library: `libdbus-1-3` on Debian, `dbus` on Arch.
    - The AppImage wraps the existing static musl binary, without keyring, so it runs on any x86_64 distro with nothing to bundle. Passwords are prompted for, as with the musl binary today.
  - **Debian target:** built on `ubuntu-22.04` (glibc 2.35), so the `.deb` installs on Debian 12+ and Ubuntu 22.04+. The dependencies come from `dpkg-shlibdeps` (cargo-deb's `$auto`).
  - **Arch package:** a source PKGBUILD in `packaging/arch/PKGBUILD`, built from a `git archive` tarball of the tagged commit inside an `archlinux:latest` container. The same PKGBUILD is a starting point for the AUR later; publishing to the AUR is out of scope.
  - **Architecture:** x86_64 only, as today. aarch64 is out of scope.
  - **File names:**
    - `mqtt-zapp-v{V}-x86_64-linux-musl` (unchanged);
    - `mqtt-zapp-{V}-x86_64.AppImage`;
    - `mqtt-zapp_{V}-1_amd64.deb`;
    - `mqtt-zapp-{V}-1-x86_64.pkg.tar.zst`;
    - `SHA256SUMS`.
  - **Package contents:** `/usr/bin/mqtt-zapp`, the README and the LICENSE.
    - The repo has no LICENSE file, though `Cargo.toml` says MIT, and Debian and Arch packages need one. Add an MIT `LICENSE` with the copyright line "Copyright (c) 2026 mqtt-zapp contributors".
  - **Maintainer field** (deb and PKGBUILD): the repo's git identity, `13 <13@users.noreply.github.com>`.
  - **Desktop entry:** the AppImage needs a `.desktop` file and an icon. Add `packaging/mqtt-zapp.desktop` (`Terminal=true`, `Categories=Network;Utility;`) and a small `packaging/mqtt-zapp.svg`. The native packages install neither, because it is a terminal tool.
  - **Smoke tests:** every package is installed in a clean container of its target distro, and `mqtt-zapp --version` must print `mqtt-zapp {V}`:
    - the `.deb` with `apt install` in `debian:12`;
    - the Arch package with `pacman -U` in `archlinux`;
    - the AppImage run in `ubuntu:22.04` with `--appimage-extract-and-run`.
  - **README look** (Task 3):
    - **Hero:** centred, with the new icon, the name, a one-line tagline, and badges for CI, the latest release, MIT and Rust.
    - **Demo:** a GIF recorded with [VHS](https://github.com/charmbracelet/vhs) from a committed `docs/demo/demo.tape`, so it can be re-recorded after UI changes. It drives the real app against a local mosquitto that a committed `docs/demo/publish.sh` feeds made-up home-sensor data, so no real broker or topics appear. A still `docs/demo/screenshot.png` comes from the same tape.
    - **Body:** a short feature list with one emoji per line, a quick start in three commands, an install section with one block per format, then Connecting, Keys, Search, Settings, Mouse, Publishing and Config.
    - **Folding:** the long reference parts (the full key table, settings, config paths) go in `<details>` blocks so the page stays scannable.
    - **Roadmap:** "Not yet" becomes "Roadmap". The page ends with License.
    - **Wording:** the content stays the facts the README states today. The redesign changes layout and wording, not claims.
  - **Pinned tools:** appimagetool and its type-2 runtime are downloaded from a fixed release with a checked sha256. cargo-deb is installed with `--locked` at a fixed version.

## Global Constraints

- Every action and tool version is pinned. `actions/checkout@v5`, `dtolnay/rust-toolchain@stable` and `Swatinem/rust-cache@v2` match ci.yml.
- `actionlint` passes on both workflows.
- Builds use `--locked`.
- The existing musl binary and its name are unchanged, because the README's `scp` example uses it.
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace` still pass. The `[package.metadata.deb]` table must not change the build.

- README: the key table rows keep the exact form `` | `key`, `key` | `action` | ``, because `readme_key_table_matches_defaults` (keymap.rs) checks them. No other line in the README may start with `` | ` ``, because the test counts those rows. Only GitHub-flavoured Markdown and the HTML GitHub allows (`<p align>`, `<img>`, `<details>`) are used.

## Review Focus

1. **Tag and version mismatch:** a tag `v0.2.0` on a commit whose Cargo version is still `0.1.0` must fail the `version` job before anything is built or released, and not publish `0.1.0` packages under a `v0.2.0` release. Test: run the check script locally with a mismatched tag.
2. **Missing runtime libraries:** a `.deb` that installs but fails to start because it lacks `libdbus-1-3`, or an Arch package missing `dbus`. Only an install in a clean container catches this, so the smoke tests run in bare `debian:12` and `archlinux` images, not on the build host.
3. **AppImage without FUSE:** CI runners and many containers have no FUSE. The build must not need it, and the smoke test uses `--appimage-extract-and-run`.
4. **A partial release:** if one package job fails, no release is created. `release` needs every job, and creating the release is its last step.
5. **README claims drifting from the code:** every key, command, flag and path the new README shows must exist. The key table is pinned by its test, and the rest needs a pass that checks each command example against `mqtt-zapp --help` and `mqtt-zapp profile add --help`.
6. **makepkg as root:** makepkg refuses to run as root, so the Arch job builds as an unprivileged user in the container and installs with pacman as root.

---

### Task 1: Packaging inputs

**Files:**
- Create:
  - `LICENSE` (MIT);
  - `packaging/mqtt-zapp.desktop`;
  - `packaging/mqtt-zapp.svg`;
  - `packaging/appimage.sh`;
  - `packaging/arch/PKGBUILD`;
  - `packaging/check-version.sh`.
- Modify: `crates/mqtt-zapp/Cargo.toml` (`description`, `license`, `repository`, `[package.metadata.deb]`).

**Interfaces:**
- `packaging/check-version.sh TAG` exits 0 when `TAG == "v$(cargo metadata version of mqtt-zapp)"` and prints the version. Otherwise it exits 1 with `tag vX does not match Cargo version Y`.
- `packaging/appimage.sh BINARY VERSION OUTDIR` builds `OUTDIR/mqtt-zapp-VERSION-x86_64.AppImage`. It:
  - makes an AppDir with `usr/bin/mqtt-zapp`, an `AppRun` that execs it with `"$@"`, the desktop file and the icon;
  - runs appimagetool with `--runtime-file`.

  Both are downloaded into a cache directory, pinned and sha256-checked. The script needs no FUSE: it runs appimagetool with `--appimage-extract-and-run`.
- `packaging/arch/PKGBUILD`:
  - `pkgname=mqtt-zapp`, `pkgver` from the tag, `pkgrel=1`, `arch=(x86_64)`, `license=(MIT)`;
  - `depends=(dbus gcc-libs glibc)`, `makedepends=(cargo git)`;
  - `source=("mqtt-zapp-$pkgver.tar.gz")`, produced by `git archive --prefix=mqtt-zapp-$pkgver/`, with `sha256sums=(SKIP)`;
  - `prepare` runs `cargo fetch --locked`, `build` runs `cargo build --release --locked --frozen -p mqtt-zapp`, `check` runs `cargo test --frozen -p mqtt-zapp`, and `package` installs the binary, LICENSE and README.
- `[package.metadata.deb]`:
  - `maintainer`, `copyright`, `section = "net"`, `depends = "$auto"`;
  - `assets` = the release binary to `usr/bin/`, README.md to `usr/share/doc/mqtt-zapp/`, LICENSE to `usr/share/doc/mqtt-zapp/copyright`.

- [ ] **Step 1: Failing checks first**
  - `packaging/check-version.sh v0.1.0` → exit 0, prints `0.1.0`.
  - `packaging/check-version.sh v9.9.9` → exit 1 with the mismatch message.
  - Write these as `packaging/test.sh`, which runs both and asserts the exit codes. Run it before writing check-version.sh. Expected: FAIL (no script).
- [ ] **Step 2:** Implement check-version.sh, using `cargo metadata --no-deps --format-version 1` and jq. Run `packaging/test.sh`. Expected: PASS.
- [ ] **Step 3: Local deb**
  - `cargo install cargo-deb --locked --version <pinned>`, then `cargo deb -p mqtt-zapp --locked`.
  - Then, in `docker run --rm -v "$PWD/target/debian:/p" debian:12`: `apt-get update && apt-get install -y /p/*.deb && mqtt-zapp --version`.
  - Expected: prints `mqtt-zapp 0.1.0`, and `dpkg -I` shows `Depends:` with `libdbus-1-3`.
- [ ] **Step 4: Local Arch**
  - `git archive --prefix=mqtt-zapp-0.1.0/ -o packaging/arch/mqtt-zapp-0.1.0.tar.gz HEAD`, then `makepkg` in `packaging/arch` (as the user, not root).
  - Install in `docker run archlinux` with `pacman -U`, then `mqtt-zapp --version`.
  - Expected: prints `mqtt-zapp 0.1.0`, and `pacman -Qi` lists `dbus`.
  - Add `packaging/arch/*.tar.gz`, `*.pkg.tar.zst`, `src/` and `pkg/` to `.gitignore`.
- [ ] **Step 5: Local AppImage**
  - Build the musl binary as release.yml does, then run `packaging/appimage.sh <bin> 0.1.0 dist`.
  - In `docker run ubuntu:22.04`, run `dist/mqtt-zapp-0.1.0-x86_64.AppImage --appimage-extract-and-run --version`.
  - Expected: prints `mqtt-zapp 0.1.0`.
- [ ] **Step 6:** `cargo test --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.
- [ ] **Step 7:** Commit: `build: packaging inputs for AppImage, pacman and deb`

### Task 2: Release workflow

**Files:**
- Modify: `.github/workflows/release.yml`.

**Interfaces:** consumes the Task 1 scripts, with the names and outputs above. The jobs:
- `version`: checkout, then `packaging/check-version.sh "$GITHUB_REF_NAME"` on tag pushes. On a manual run, the version comes from Cargo. Outputs `version`.
- `musl`: the current build, verify and strip steps. Uploads the artifact `musl`.
- `appimage` (needs `musl`, `version`): downloads the binary, runs `packaging/appimage.sh`, smoke-tests in `ubuntu:22.04`, and uploads.
- `deb` (on `ubuntu-22.04`, needs `version`): installs libdbus-1-dev and pkg-config, runs `cargo deb --locked`, smoke-tests in `debian:12`, and uploads.
- `arch` (`container: archlinux:latest`, needs `version`):
  - `pacman -Syu --noconfirm base-devel rust git dbus`, then creates the user `builder`;
  - the `git archive` tarball, then `makepkg` as `builder`, then `pacman -U` plus the smoke test, then uploads.
  - Checkout needs `git config --global --add safe.directory '*'` inside the container.
- `release` (needs every job; tag pushes only): downloads all artifacts into `dist/`, runs `sha256sum * > SHA256SUMS`, then `gh release create "$GITHUB_REF_NAME" dist/* --generate-notes`.

- [ ] **Step 1:** Write the workflow. Run `actionlint .github/workflows/release.yml`. Expected: no findings.
- [ ] **Step 2:** Commit: `ci: release AppImage, pacman and deb packages`
- [ ] **Step 3: Dry run on GitHub.** This needs a push, so ask the user first. Push the branch, then run `gh workflow run release.yml --ref release-packages` and `gh run watch`.
  - Expected: every job is green, five artifacts are uploaded, and no release is created.
  - If the user declines the push, record that the workflow was only checked locally and with actionlint.

### Task 3: Modern README

**Files:**
- Modify: `README.md`.
- Create:
  - `docs/demo/demo.tape`;
  - `docs/demo/publish.sh`;
  - `docs/demo/demo.gif`;
  - `docs/demo/screenshot.png`;
  - `docs/demo/profiles.toml` (one demo profile, on port 18830).

**Interfaces:**
- Consumes the Task 1 icon (`packaging/mqtt-zapp.svg`) and the Task 2 file names for the install section.
- `docs/demo/publish.sh PORT` publishes retained, made-up data to `127.0.0.1:PORT`, then keeps updating it about once a second until killed:
  - `home/livingroom/climate` with JSON `temperature` / `humidity`;
  - `home/kitchen/plug` with `ENERGY.Power` / `Voltage`;
  - `home/garden/weather`;
  - `home/hall/door` with a text payload.
- `docs/demo/demo.tape`:
  - starts mosquitto on 18830 and `publish.sh`;
  - runs `MQTT_ZAPP_CONFIG_DIR=docs/demo mqtt-zapp` at 120×34;
  - connects;
  - expands the tree and selects `home/kitchen/plug`;
  - presses `c` twice (a chart with the highlight), `d` (diff) and `/` + `garden`;
  - writes `demo.gif` and, mid-way, `screenshot.png`.
- README outline, in order:
  1. Hero, inside `<p align="center">`: the icon at 96px, `# mqtt-zapp`, the tagline "A fast terminal MQTT explorer — browse, watch, diff, chart and publish, right next to your broker.", and badges:
     - `https://github.com/13/mqtt-zapp/actions/workflows/ci.yml/badge.svg`;
     - `https://img.shields.io/github/v/release/13/mqtt-zapp`;
     - `https://img.shields.io/badge/license-MIT-blue`;
     - `https://img.shields.io/badge/rust-2024-orange`.
  2. The demo GIF.
  3. **Features:** about 8 lines, each with an emoji, covering:
     - the live topic tree with counts;
     - pretty JSON and the diff;
     - charts of numeric fields, click to chart;
     - publishing;
     - retained management and topic delete;
     - profiles, TLS, WebSocket, MQTT 5;
     - mouse and clipboard copy;
     - one static binary for SSH boxes.
  4. **Quick start:** install, `mqtt-zapp profile add …`, `mqtt-zapp`.
  5. **Install:** one `###` per format (Arch, Debian/Ubuntu, AppImage, static binary, from source), each with its command block. The note that the AppImage and the static binary prompt for passwords (no keyring) is a GitHub `> [!NOTE]` callout.
  6. **Connecting / Profiles:** the current content, with the examples in one block.
  7. **Keys:** a one-paragraph intro, then the full table in `<details open>`. Its rows are unchanged in form.
  8. **Search, Mouse, Publishing:** short sections.
  9. **Settings and Config files:** each in `<details>`.
  10. **Roadmap**, then **License** (MIT, a link to LICENSE).

- [ ] **Step 1:** Install VHS. On Arch it is `pacman -S vhs`, which pulls in ttyd and ffmpeg. This is a system install, so ask the user first. If they decline, make the still with a tmux capture and [freeze](https://github.com/charmbracelet/freeze) and leave the GIF out.
- [ ] **Step 2:** Write `publish.sh`, the demo profile and `demo.tape`. Run `vhs docs/demo/demo.tape`. Expected:
  - `demo.gif` (under 3 MB) and `screenshot.png` exist;
  - looking at the GIF (Read the PNG) shows the tree, the highlighted chart line and the diff;
  - no real hostnames or topics appear.
- [ ] **Step 3:** Rewrite README.md per the outline. Run `cargo test -p mqtt-zapp readme_key_table_matches_defaults`. Expected: PASS.
- [ ] **Step 4:** Check the claims:
  - run every command in the README's code blocks under `--help` or a dry run against the local mosquitto;
  - `grep -n '^| \`' README.md | wc -l` equals the number of actions;
  - every relative link and image path resolves (`test -e` for each).

  Expected: all OK.
- [ ] **Step 5:** Render check. Run `gh markdown-preview`, or failing that `pandoc README.md -o "$S/readme.html"`, and look at it: the hero is centred, the badges sit on one row, the GIF loads, and the details blocks fold.
- [ ] **Step 6:** `cargo test --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.
- [ ] **Step 7:** Commit: `docs: modern README with demo, features and per-distro install`
