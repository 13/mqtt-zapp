//! Remappable key bindings: vim-style defaults, overridden per action from the
//! `[keys]` table in `settings.toml` (e.g. `toggle_diff = "D"`).

use anyhow::{Result, anyhow, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeMap, HashMap};

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
}

impl Action {
    /// Every action with its settings name and default keys.
    const DEFAULTS: [(Action, &'static str, &'static [&'static str]); 17] = [
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
    ];

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

pub struct Keymap {
    bindings: HashMap<KeySpec, Action>,
}

impl Keymap {
    /// Defaults plus `overrides` (action name -> key). An override replaces all
    /// default keys of that action and takes its key away from any other action.
    pub fn from_settings(overrides: &BTreeMap<String, String>) -> Result<Keymap> {
        let mut bindings = HashMap::new();
        for (action, _, keys) in Action::DEFAULTS {
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
}
