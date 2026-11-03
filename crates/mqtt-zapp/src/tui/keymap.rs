//! Remappable key bindings: vim-style defaults, overridden per action from the
//! `[keys]` table in `settings.toml` (e.g. `toggle_diff = "D"`).

use anyhow::{Result, anyhow, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Up,
    Down,
    Collapse,
    Expand,
    ToggleExpand,
    Search,
    Publish,
    FocusNext,
    Select,
    ToggleDiff,
    Help,
    Quit,
    Cancel,
    Top,
    Bottom,
    ScrollDown,
    ScrollUp,
    CopyTopic,
    CopyPayload,
    DeleteRetained,
    ClearRetained,
}

impl Action {
    /// Every action with its settings name and default keys.
    const DEFAULTS: &'static [(Action, &'static str, &'static [&'static str])] = &[
        (Action::Up, "up", &["k", "up"]),
        (Action::Down, "down", &["j", "down"]),
        (Action::Collapse, "collapse", &["h", "left"]),
        (Action::Expand, "expand", &["l", "right"]),
        (Action::ToggleExpand, "toggle_expand", &["space"]),
        (Action::Search, "search", &["/"]),
        (Action::Publish, "publish", &["p"]),
        (Action::FocusNext, "focus_next", &["tab"]),
        (Action::Select, "select", &["enter"]),
        (Action::ToggleDiff, "toggle_diff", &["d"]),
        (Action::Help, "help", &["?"]),
        (Action::Quit, "quit", &["q"]),
        (Action::Cancel, "cancel", &["esc"]),
        (Action::Top, "top", &["g"]),
        (Action::Bottom, "bottom", &["G"]),
        (Action::ScrollDown, "scroll_down", &["ctrl-d"]),
        (Action::ScrollUp, "scroll_up", &["ctrl-u"]),
        (Action::CopyTopic, "copy_topic", &["y"]),
        (Action::CopyPayload, "copy_payload", &["Y"]),
        (Action::DeleteRetained, "delete_retained", &["x"]),
        (Action::ClearRetained, "clear_retained", &["X"]),
    ];

    /// The snake_case name used in `[keys]`.
    pub fn name(self) -> &'static str {
        Self::DEFAULTS
            .iter()
            .find(|(a, _, _)| *a == self)
            .map_or("", |(_, n, _)| n)
    }

    fn from_name(name: &str) -> Option<Action> {
        Self::DEFAULTS
            .iter()
            .find(|(_, n, _)| *n == name)
            .map(|(a, _, _)| *a)
    }
}

/// A key plus modifiers, e.g. `ctrl-s`, `j`, `G`, `esc`. Characters carry no
/// SHIFT: `G` is `Char('G')` with no modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeySpec(pub KeyCode, pub KeyModifiers);

impl KeySpec {
    pub fn parse(s: &str) -> Result<KeySpec> {
        let mut mods = KeyModifiers::NONE;
        let mut rest = s;
        loop {
            let (m, tail) = if let Some(t) = rest.strip_prefix("ctrl-") {
                (KeyModifiers::CONTROL, t)
            } else if let Some(t) = rest.strip_prefix("alt-") {
                (KeyModifiers::ALT, t)
            } else {
                break;
            };
            mods |= m;
            rest = tail;
        }
        let code = match rest {
            "esc" => KeyCode::Esc,
            "enter" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            _ => {
                let mut chars = rest.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyCode::Char(c),
                    _ => bail!("invalid key '{s}'"),
                }
            }
        };
        Ok(KeySpec(code, mods))
    }

    fn from_event(ev: KeyEvent) -> KeySpec {
        let mut mods =
            ev.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
        if let KeyCode::Char(_) = ev.code {
            // The shift is already in the character itself.
            mods.remove(KeyModifiers::SHIFT);
        }
        KeySpec(ev.code, mods)
    }
}

/// The text form `parse` accepts: `ctrl-s`, `G`, `esc`, `space`.
impl fmt::Display for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.1.contains(KeyModifiers::CONTROL) {
            f.write_str("ctrl-")?;
        }
        if self.1.contains(KeyModifiers::ALT) {
            f.write_str("alt-")?;
        }
        match self.0 {
            KeyCode::Esc => f.write_str("esc"),
            KeyCode::Enter => f.write_str("enter"),
            KeyCode::Tab => f.write_str("tab"),
            KeyCode::BackTab => f.write_str("backtab"),
            KeyCode::Char(' ') => f.write_str("space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Backspace => f.write_str("backspace"),
            KeyCode::Delete => f.write_str("delete"),
            KeyCode::Up => f.write_str("up"),
            KeyCode::Down => f.write_str("down"),
            KeyCode::Left => f.write_str("left"),
            KeyCode::Right => f.write_str("right"),
            KeyCode::Home => f.write_str("home"),
            KeyCode::End => f.write_str("end"),
            KeyCode::PageUp => f.write_str("pageup"),
            KeyCode::PageDown => f.write_str("pagedown"),
            other => write!(f, "{other:?}"),
        }
    }
}

pub struct Keymap {
    bindings: HashMap<KeySpec, Action>,
}

impl Keymap {
    /// Defaults plus `overrides` (action name -> key). An override replaces all
    /// default keys of that action and takes its key away from any other action.
    pub fn from_settings(overrides: &BTreeMap<String, String>) -> Result<Keymap> {
        let mut bindings = HashMap::new();
        for &(action, _, keys) in Action::DEFAULTS {
            for k in keys {
                bindings.insert(KeySpec::parse(k)?, action);
            }
        }
        let mut taken = HashMap::new();
        for (name, key) in overrides {
            let action = Action::from_name(name)
                .ok_or_else(|| anyhow!("unknown action '{name}' in [keys]"))?;
            let spec = KeySpec::parse(key).map_err(|e| anyhow!("[keys] {name}: {e}"))?;
            if let Some(other) = taken.insert(spec, name) {
                bail!("[keys] '{key}' is bound to both '{other}' and '{name}'");
            }
            bindings.retain(|_, a| *a != action);
            bindings.insert(spec, action);
        }
        Ok(Keymap { bindings })
    }

    /// Every action with its keys, in `Action` declaration order; each
    /// action's keys sorted by display string. An action whose keys were all
    /// taken by overrides has an empty list.
    pub fn bindings(&self) -> Vec<(Action, Vec<KeySpec>)> {
        Action::DEFAULTS
            .iter()
            .map(|(action, _, _)| {
                let mut keys: Vec<KeySpec> = self
                    .bindings
                    .iter()
                    .filter(|(_, a)| *a == action)
                    .map(|(k, _)| *k)
                    .collect();
                keys.sort_by_cached_key(|k| k.to_string());
                (*action, keys)
            })
            .collect()
    }

    pub fn lookup(&self, ev: KeyEvent) -> Option<Action> {
        self.bindings.get(&KeySpec::from_event(ev)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn defaults() {
        let k = Keymap::from_settings(&Default::default()).unwrap();
        assert_eq!(k.lookup(key('j')), Some(Action::Down));
        assert_eq!(k.lookup(key('/')), Some(Action::Search));
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(k.lookup(ctrl('d')), Some(Action::ScrollDown));
        assert_eq!(k.lookup(ctrl('u')), Some(Action::ScrollUp));
    }

    #[test]
    fn override_rebinds() {
        let k = Keymap::from_settings(&[("toggle_diff".into(), "D".into())].into()).unwrap();
        assert_eq!(k.lookup(key('D')), Some(Action::ToggleDiff));
        assert_eq!(k.lookup(key('d')), None);
    }

    #[test]
    fn unknown_action_errors() {
        assert!(Keymap::from_settings(&[("nope".into(), "x".into())].into()).is_err());
    }

    #[test]
    fn parse_ctrl() {
        assert_eq!(
            KeySpec::parse("ctrl-s").unwrap(),
            KeySpec(KeyCode::Char('s'), KeyModifiers::CONTROL)
        );
    }

    #[test]
    fn parse_named_and_bad_keys() {
        assert_eq!(
            KeySpec::parse("esc").unwrap(),
            KeySpec(KeyCode::Esc, KeyModifiers::NONE)
        );
        assert_eq!(
            KeySpec::parse("enter").unwrap(),
            KeySpec(KeyCode::Enter, KeyModifiers::NONE)
        );
        assert_eq!(
            KeySpec::parse("tab").unwrap(),
            KeySpec(KeyCode::Tab, KeyModifiers::NONE)
        );
        assert_eq!(
            KeySpec::parse("up").unwrap(),
            KeySpec(KeyCode::Up, KeyModifiers::NONE)
        );
        assert_eq!(
            KeySpec::parse("G").unwrap(),
            KeySpec(KeyCode::Char('G'), KeyModifiers::NONE)
        );
        assert!(KeySpec::parse("").is_err());
        assert!(KeySpec::parse("ctrl-").is_err());
        assert!(KeySpec::parse("bogus").is_err());
    }

    #[test]
    fn shifted_char_from_terminal_matches_plain_spec() {
        // Terminals report `G` as Char('G') + SHIFT.
        let k = Keymap::from_settings(&Default::default()).unwrap();
        let ev = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        assert_eq!(k.lookup(ev), Some(Action::Bottom));
        assert_eq!(
            k.lookup(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
            None
        );
    }

    #[test]
    fn override_steals_key_from_default_owner() {
        let k = Keymap::from_settings(
            &[("quit".into(), "x".into()), ("top".into(), "q".into())].into(),
        )
        .unwrap();
        assert_eq!(k.lookup(key('q')), Some(Action::Top));
        assert_eq!(k.lookup(key('x')), Some(Action::Quit));
        assert_eq!(k.lookup(key('g')), None);
    }

    #[test]
    fn bad_key_in_override_errors() {
        assert!(Keymap::from_settings(&[("quit".into(), "ctrl-".into())].into()).is_err());
    }

    #[test]
    fn default_bindings_round_trip_through_display_and_parse() {
        let k = Keymap::from_settings(&Default::default()).unwrap();
        let all = k.bindings();
        assert_eq!(all.len(), Action::DEFAULTS.len());
        for (action, keys) in &all {
            assert!(!keys.is_empty(), "{}", action.name());
            for spec in keys {
                let shown = spec.to_string();
                assert_eq!(KeySpec::parse(&shown).unwrap(), *spec, "{shown}");
            }
        }
        let shown = |a: Action| -> Vec<String> {
            all.iter()
                .find(|(x, _)| *x == a)
                .unwrap()
                .1
                .iter()
                .map(|s| s.to_string())
                .collect()
        };
        assert_eq!(shown(Action::Up), ["k", "up"]);
        assert_eq!(shown(Action::ScrollDown), ["ctrl-d"]);
        assert_eq!(shown(Action::ToggleExpand), ["space"]);
        assert_eq!(shown(Action::Bottom), ["G"]);
        assert_eq!(shown(Action::Help), ["?"]);
        // Declaration order.
        assert_eq!(all[0].0, Action::Up);
        assert_eq!(all[16].0, Action::ScrollUp);
    }

    #[test]
    fn bindings_reflect_overrides() {
        let k = Keymap::from_settings(&[("quit".into(), "x".into())].into()).unwrap();
        let quit = k.bindings().into_iter().find(|(a, _)| *a == Action::Quit);
        assert_eq!(quit.unwrap().1, vec![KeySpec::parse("x").unwrap()]);
    }

    /// The README key table is generated from the defaults; keep it in sync.
    #[test]
    fn readme_key_table_matches_defaults() {
        let readme = include_str!("../../../../README.md");
        let k = Keymap::from_settings(&Default::default()).unwrap();
        for (action, keys) in k.bindings() {
            let keys: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
            let prefix = format!("| {} | `{}` |", keys.join(", "), action.name());
            assert!(
                readme.lines().any(|l| l.starts_with(&prefix)),
                "README row missing: {prefix}"
            );
        }
        let rows = readme.lines().filter(|l| l.starts_with("| `")).count();
        assert_eq!(rows, Action::DEFAULTS.len());
    }
}
