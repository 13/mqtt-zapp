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

```sh
# plain mqtt
mqtt-zapp profile add --name home --host 192.168.1.10

# mqtts with a private CA
mqtt-zapp profile add --name secure --host broker.lan --protocol mqtts \
    --ca-file ~/ca.pem

# wss (websockets over TLS)
mqtt-zapp profile add --name cloud --host broker.example.com --protocol wss

# ask for the password every time / keep it in the system keyring
mqtt-zapp profile add --name a --host h --user me --password-mode prompt
mqtt-zapp profile add --name b --host h --user me --password-mode keyring
```

The websocket path defaults to `/`. For a different one, edit `ws_path` in
`profiles.toml` (for example `ws_path = "/mqtt"`).

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
| `d` | `toggle_diff` | Diff against the previous message |
| `?` | `help` | Show the key help |
| `q` | `quit` | Quit |
| `esc` | `cancel` | Clear the search / close |
| `g` | `top` | Jump to the top |
| `G` | `bottom` | Jump to the bottom |
| `ctrl-d` | `scroll_down` | Scroll the payload half a page down |
| `ctrl-u` | `scroll_up` | Scroll the payload half a page up |
| `y` | `copy_topic` | Copy the topic to the clipboard (OSC 52) |
| `Y` | `copy_payload` | Copy the shown payload to the clipboard (OSC 52) |

`?` shows the effective keys, including your remaps. The publish form and
search input take typed text and do not use the keymap; the help lists their
keys too.

### Settings

`settings.toml` in the config directory:

```toml
history_depth = 200      # messages kept per topic (default 100)

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

## Not yet

Planned for Phase 1.x:

- charts for numeric topics
- Sparkplug B decoding
- a `$SYS` stats view
- clearing retained topics
- sorting options
- MQTT 5
- copy to clipboard (OSC 52)
- a profile editor in the TUI
