# mqtt-zapp

A terminal MQTT explorer: browse a broker's topic tree, watch payloads update
live, diff against the previous message, and publish. Inspired by the feature
set of MQTT Explorer.

The focus is a TUI that works well over SSH: a single static binary you can
copy to a headless box and run there, next to the broker.

## Install

Download the `x86_64-linux-musl` binary from the latest GitHub release, copy
it to the host and make it executable:

```sh
scp mqtt-zapp-v0.1.0-x86_64-linux-musl user@host:mqtt-zapp
ssh user@host chmod +x mqtt-zapp
```

Or build from source:

```sh
cargo install --path crates/mqtt-zapp
```

## Connecting

```sh
mqtt-zapp                                  # profile picker
mqtt-zapp --profile home                   # saved profile
mqtt-zapp --host 192.168.1.10              # ad-hoc, plain mqtt on 1883
```

### Profiles

In the picker, `a` adds a profile, `e` edits the selected one and `D` deletes
it (after a yes/no). `J`/`K` move the selected profile down/up; the order is
saved to `profiles.toml`. `s` sorts the picker by that custom order, by name or
by last used, and the choice is remembered in `state.toml` (in
`~/.local/state/mqtt-zapp/`, next to the last-used times). The form covers every field; `space` or `←`/`→` change the
protocol and password source, and `ctrl-s` saves to `profiles.toml`. A keyring
password typed there goes to the keyring, never the file. From the shell:

```sh
# plain mqtt
mqtt-zapp profile add --name home --host 192.168.1.10

# mqtts with a private CA
mqtt-zapp profile add --name secure --host broker.lan --protocol mqtts \
    --ca-file ~/ca.pem

# MQTT 5 (default is 3.1.1)
mqtt-zapp profile add --name five --host broker.lan --mqtt-version 5

# wss (websockets over TLS)
mqtt-zapp profile add --name cloud --host broker.example.com --protocol wss

# ask for the password every time / keep it in the system keyring
mqtt-zapp profile add --name a --host h --user me --password-mode prompt
mqtt-zapp profile add --name b --host h --user me --password-mode keyring
```

The websocket path defaults to `/`. Set a different one in the picker's form,
or edit `ws_path` in `profiles.toml` (for example `ws_path = "/mqtt"`).

**Passwords are never stored in the config files.** With the default `keyring`
build feature they can live in the system keyring. The static musl release is
built without that feature, so there the password is prompted for at connect
time.

## Keys

| Key | Action | What it does |
|---|---|---|
| `k`, `up` | `up` | Move up |
| `down`, `j` | `down` | Move down |
| `h`, `left` | `collapse` | Collapse, or move to the parent |
| `l`, `right` | `expand` | Expand the node |
| `space` | `toggle_expand` | Toggle expand/collapse |
| `/` | `search` | Filter the tree |
| `p` | `publish` | Open the publish form |
| `tab` | `focus_next` | Switch pane |
| `enter` | `select` | Select |
| `d` | `toggle_diff` | Diff against the previous message (or the marked one) |
| `m` | `mark_diff_base` | Mark the shown history entry as the diff base (again: unmark) |
| `?` | `help` | Show the key help |
| `q` | `quit` | Quit |
| `esc` | `cancel` | Clear the search / close |
| `g` | `top` | Jump to the top |
| `G` | `bottom` | Jump to the bottom |
| `ctrl-d` | `scroll_down` | Scroll the payload half a page down |
| `ctrl-u` | `scroll_up` | Scroll the payload half a page up |
| `y` | `copy_topic` | Copy the topic to the clipboard (OSC 52) |
| `Y` | `copy_payload` | Copy the shown payload to the clipboard (OSC 52) |
| `x` | `delete_retained` | Delete the retained message of the selected topic (asks first) |
| `X` | `clear_retained` | Clear every retained message in the subtree (asks first) |
| `s` | `sort` | Cycle the tree order: name (natural), message count, most recent; in the picker: custom, name, last used |
| `L` | `expand_all` | Expand the selected node and everything below it |
| `H` | `collapse_all` | Collapse the selected node and everything below it |
| `c` | `chart` | Chart the payload, or cycle through its numeric JSON fields |
| `S` | `stats` | Show broker statistics from `$SYS` |
| `a` | `add_profile` | In the picker: add a profile |
| `e` | `edit_profile` | In the picker: edit the selected profile |
| `D` | `delete_profile` | In the picker: delete the selected profile |
| `K` | `move_up` | In the picker: move the selected profile up (custom order) |
| `J` | `move_down` | In the picker: move the selected profile down (custom order) |

`?` shows the effective keys, including your remaps. The publish form and
search input take typed text and do not use the keymap; the help lists their
keys too.

### Search

`/` filters the tree as you type. Every space-separated word must appear,
case-insensitively, in the topic path or the latest payload (each word may be
in either); `"quoted words"` match as one phrase. `ctrl-r` switches the input
to a (case-insensitive) regular expression; an invalid one shows its error
and keeps the last valid filter.

### Settings

`settings.toml` in the config directory:

```toml
history_depth = 200      # messages kept per topic (default 100)
sort = "name"            # tree order: name (natural), count, recent; `s` cycles
theme = "dark"           # dark, light or mono; NO_COLOR forces mono
timestamp_format = "%H:%M:%S%.3f"   # strftime for the message time

[keys]                   # action name = key; replaces that action's defaults
toggle_diff = "D"
quit = "ctrl-q"
```

Keys are a single character or a name (`esc`, `enter`, `tab`, `space`, `up`,
`down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `backspace`,
`delete`), optionally prefixed with `ctrl-` or `alt-`.

## Config files

`profiles.toml` and `settings.toml` live in `~/.config/mqtt-zapp/` (the XDG
config dir). Set `MQTT_ZAPP_CONFIG_DIR` to use another directory.

### Mouse

Click a pane to focus it, a topic to select it (click the `▸` marker or the
selected row again to expand it), a history entry to pin it, and a profile in
the picker to select it (click again to connect). The wheel scrolls the pane
under the pointer. Click the topic path in the message header to copy the
topic, `⧉ copy` on the payload pane to copy the shown payload, or right-click a
tree row to copy its topic (all via OSC 52). Hold shift to select text with the
mouse.

### Publishing

In the publish form, `ctrl-e` opens the payload in `$VISUAL`/`$EDITOR` (or
`vi`), and `ctrl-p`/`ctrl-n` recall payloads sent to that topic in this
session.

## Not yet

Planned for Phase 1.x:

- Sparkplug B decoding
