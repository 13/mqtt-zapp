//! The connection picker and the password prompt: their state and drawing.

use super::theme;
use mqtt_zapp_core::config::{Profile, Protocol};
use mqtt_zapp_core::payload::sanitize;
use mqtt_zapp_core::state::{AppState, PickerSort};
use mqtt_zapp_core::tree::natural_cmp;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::collections::BTreeMap;
use std::fmt;
use unicode_width::UnicodeWidthStr;

pub const EMPTY_HINT: &str = "no profiles – press a to add one";

#[derive(Debug, Default)]
pub struct PickerState {
    /// In `profiles.toml` order, which is the custom order.
    pub profiles: Vec<Profile>,
    /// Index into the sorted view (`visible()`), not into `profiles`.
    pub sel: usize,
    pub sort: PickerSort,
    /// Profile name → unix seconds of the last connect.
    pub last_used: BTreeMap<String, i64>,
    /// A keyring lookup is in flight; drawn as a line under the list.
    pub waiting_keyring: bool,
}

impl PickerState {
    pub fn new(profiles: Vec<Profile>, state: &AppState) -> PickerState {
        PickerState {
            profiles,
            sel: 0,
            sort: state.picker_sort,
            last_used: state.last_used.clone(),
            waiting_keyring: false,
        }
    }

    /// Indices into `profiles` in display order. Computed on demand: there
    /// are a handful of profiles, and nothing can go stale.
    fn view(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.profiles.len()).collect();
        match self.sort {
            PickerSort::Custom => {}
            PickerSort::Name => {
                v.sort_by(|&a, &b| natural_cmp(&self.profiles[a].name, &self.profiles[b].name))
            }
            // Newest first; never used last, in file order (stable sort).
            PickerSort::LastUsed => v.sort_by_key(|&i| {
                std::cmp::Reverse(self.last_used.get(&self.profiles[i].name).copied())
            }),
        }
        v
    }

    pub fn visible(&self) -> impl Iterator<Item = &Profile> {
        self.view().into_iter().map(|i| &self.profiles[i])
    }

    pub fn selected(&self) -> Option<&Profile> {
        self.view().get(self.sel).map(|&i| &self.profiles[i])
    }

    fn select_index(&mut self, idx: Option<usize>) {
        if let Some(pos) = idx.and_then(|i| self.view().iter().position(|&v| v == i)) {
            self.sel = pos;
        }
    }

    fn selected_index(&self) -> Option<usize> {
        self.view().get(self.sel).copied()
    }

    pub fn cycle_sort(&mut self) {
        let cur = self.selected_index();
        self.sort = self.sort.next();
        self.select_index(cur);
    }

    /// Move the selected profile one place down (`J`) or up (`K`) in the
    /// custom order, switching to it first. False (nothing changed) at the
    /// edge or with nothing selected.
    pub fn move_selected(&mut self, down: bool) -> bool {
        let Some(i) = self.selected_index() else {
            return false;
        };
        let j = if down { i + 1 } else { i.wrapping_sub(1) };
        if j >= self.profiles.len() {
            return false;
        }
        self.sort = PickerSort::Custom;
        self.profiles.swap(i, j);
        self.sel = j;
        true
    }

    /// Profile names in custom (file) order.
    pub fn order(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.name.clone()).collect()
    }

    /// Replace the list (after a save): select `select` by name, else keep
    /// the position, clamped.
    pub fn set_profiles(&mut self, profiles: Vec<Profile>, select: Option<&str>) {
        self.profiles = profiles;
        let idx = select.and_then(|n| self.profiles.iter().position(|p| p.name == n));
        self.sel = self.sel.min(self.profiles.len().saturating_sub(1));
        self.select_index(idx);
    }

    pub fn up(&mut self) {
        self.sel = self.sel.saturating_sub(1);
    }

    pub fn down(&mut self) {
        self.sel = (self.sel + 1).min(self.profiles.len().saturating_sub(1));
    }
}

/// The profile being connected to and the password typed so far.
pub struct PasswordState {
    pub profile: Profile,
    pub input: String,
}

// Hand-written so the typed password can never reach a debug log.
impl fmt::Debug for PasswordState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordState")
            .field("profile", &self.profile.name)
            .field(
                "input",
                &format_args!("<{} chars>", self.input.chars().count()),
            )
            .finish()
    }
}

impl PasswordState {
    /// One `•` per typed character.
    pub fn masked(&self) -> String {
        "•".repeat(self.input.chars().count())
    }
}

/// `proto://[user@]host:port[/path]`, sanitized onto one line. The path is
/// only shown for websocket profiles that set one.
pub fn server_url(p: &Profile) -> String {
    let proto = format!("{:?}", p.protocol).to_lowercase();
    let user = p
        .username
        .as_deref()
        .map(|u| format!("{u}@"))
        .unwrap_or_default();
    let path = match p.protocol {
        Protocol::Ws | Protocol::Wss if p.ws_path != "/" => p.ws_path.as_str(),
        _ => "",
    };
    let version = match p.mqtt_version {
        mqtt_zapp_core::config::MqttVersion::V5 => " (v5)",
        mqtt_zapp_core::config::MqttVersion::V311 => "",
    };
    sanitize(&format!(
        "{proto}://{user}{}:{}{path}{version}",
        p.host, p.port
    ))
    .replace('\n', " ")
}

/// The broker list as a centred card: `▶ name  url`, names padded to line up.
/// The profile the session is on (`connected`) gets a `●` and is named in
/// the title. Returns the card and how many profiles were scrolled off its top.
pub fn render_picker(
    f: &mut Frame,
    area: Rect,
    st: &PickerState,
    error: Option<&str>,
    connected: Option<&str>,
) -> (Rect, usize) {
    let mut skipped = 0;
    let muted = Style::new().fg(theme::MUTED);
    let lines: Vec<Line> = if st.profiles.is_empty() {
        let mut v = vec![Line::default()];
        if let Some(e) = error {
            v.push(Line::styled(
                format!(" {}", sanitize(e).replace('\n', " ")),
                Style::new().fg(theme::ERR),
            ));
        }
        v.push(Line::styled(format!(" {EMPTY_HINT}"), muted));
        v
    } else {
        let names: Vec<String> = st
            .visible()
            .map(|p| sanitize(&p.name).replace('\n', " "))
            .collect();
        let pad = names.iter().map(|n| n.width()).max().unwrap_or(0);
        let card = theme::centered(area, 72, st.profiles.len() as u16 + 4);
        let height = card.height.saturating_sub(4) as usize;
        let skip = (st.sel + 1).saturating_sub(height.max(1));
        skipped = skip;
        let mut v = vec![Line::default()];
        v.extend(
            st.visible()
                .zip(&names)
                .enumerate()
                .skip(skip)
                .map(|(i, (p, name))| {
                    let sel = i == st.sel;
                    let line = Line::from(vec![
                        Span::styled(
                            if sel { " ▶ " } else { "   " },
                            Style::new().fg(theme::ACCENT),
                        ),
                        Span::styled(
                            format!("{name}{}", " ".repeat(pad - name.width())),
                            Style::new().add_modifier(Modifier::BOLD),
                        ),
                        if connected == Some(p.name.as_str()) {
                            Span::styled(" ● ", Style::new().fg(theme::OK))
                        } else {
                            Span::raw("   ")
                        },
                        Span::styled(server_url(p), muted),
                    ]);
                    if sel { theme::select(line, true) } else { line }
                }),
        );
        v
    };
    let mut lines = lines;
    if st.waiting_keyring {
        lines.push(Line::styled(" connecting… (waiting for keyring)", muted));
    }
    let card = theme::centered(area, 72, lines.len() as u16 + 3);
    let mut title = format!("mqtt-zapp · pick a broker · {}", st.sort.name());
    if let Some(name) = connected.filter(|n| st.profiles.iter().any(|p| p.name == *n)) {
        title.push_str(&format!(
            " · connected: {}",
            sanitize(name).replace('\n', " ")
        ));
    }
    f.render_widget(Paragraph::new(lines).block(theme::pane(&title, true)), card);
    (card, skipped)
}

/// A small modal under the picker: the broker and the masked input.
pub fn render_password(f: &mut Frame, area: Rect, st: &PasswordState) {
    let title = format!("password · {}", st.profile.name);
    let lines = vec![
        Line::styled(
            format!(" {}", server_url(&st.profile)),
            Style::new().fg(theme::MUTED),
        ),
        Line::default(),
        Line::from(vec![
            Span::styled(" › ", Style::new().fg(theme::ACCENT)),
            Span::raw(format!("{}▏", st.masked())),
        ]),
    ];
    let modal = theme::centered(area, 50, 5);
    f.render_widget(Clear, modal);
    f.render_widget(
        Paragraph::new(lines).block(theme::pane(&title, true)),
        modal,
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn server_url_formats() {
        use mqtt_zapp_core::config::{PasswordSource, Profile, Protocol, TlsConfig};
        let mut p = Profile {
            name: "x".into(),
            host: "broker.lan".into(),
            port: 8883,
            protocol: Protocol::Mqtts,
            mqtt_version: Default::default(),
            ws_path: "/".into(),
            client_id: None,
            username: Some("alice".into()),
            password: PasswordSource::None,
            tls: TlsConfig::default(),
            subscriptions: vec![],
            retained: true,
            keepalive_s: None,
            clean_session: None,
            session_expiry_s: None,
        };
        assert_eq!(super::server_url(&p), "mqtts://alice@broker.lan:8883");
        p.protocol = Protocol::Wss;
        p.host = "h".into();
        p.port = 443;
        p.ws_path = "/mqtt".into();
        p.username = None;
        assert_eq!(super::server_url(&p), "wss://h:443/mqtt");
        p.protocol = Protocol::Mqtt;
        assert_eq!(super::server_url(&p), "mqtt://h:443", "path only for ws");
    }

    use super::*;
    use mqtt_zapp_core::config::{PasswordSource, Protocol, TlsConfig};
    use mqtt_zapp_core::state::{AppState, PickerSort};

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            host: "h".into(),
            port: 1883,
            protocol: Protocol::Mqtt,
            mqtt_version: Default::default(),
            ws_path: "/".into(),
            client_id: None,
            username: None,
            password: PasswordSource::None,
            tls: TlsConfig::default(),
            subscriptions: vec![],
            retained: true,
            keepalive_s: None,
            clean_session: None,
            session_expiry_s: None,
        }
    }

    #[test]
    fn navigation_is_clamped() {
        let mut st = PickerState::new(vec![profile("a"), profile("b")], &AppState::default());
        st.up();
        assert_eq!(st.sel, 0);
        st.down();
        st.down();
        assert_eq!(st.sel, 1);
        PickerState::default().down();
    }

    fn picker(names: &[&str]) -> PickerState {
        PickerState::new(
            names.iter().map(|n| profile(n)).collect(),
            &AppState::default(),
        )
    }

    fn shown(st: &PickerState) -> Vec<&str> {
        st.visible().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn sort_by_name_is_natural() {
        let mut st = picker(&["b10", "a", "b2"]);
        assert_eq!(shown(&st), ["b10", "a", "b2"], "custom = file order");
        st.cycle_sort();
        assert_eq!(st.sort, PickerSort::Name);
        assert_eq!(shown(&st), ["a", "b2", "b10"]);
    }

    #[test]
    fn sort_by_last_used_newest_first_unused_last() {
        let mut state = AppState {
            picker_sort: PickerSort::LastUsed,
            ..Default::default()
        };
        state.touch("b", 10);
        state.touch("d", 20);
        let st = PickerState::new(
            ["a", "b", "c", "d"].iter().map(|n| profile(n)).collect(),
            &state,
        );
        assert_eq!(shown(&st), ["d", "b", "a", "c"]);
    }

    #[test]
    fn cycle_sort_keeps_selection() {
        let mut st = picker(&["b", "c", "a"]);
        st.sel = 1; // "c"
        st.cycle_sort();
        assert_eq!(st.selected().unwrap().name, "c");
        assert_eq!(st.sel, 2);
    }

    #[test]
    fn move_down_in_name_mode_switches_to_custom() {
        let mut st = picker(&["b", "a", "c"]);
        st.cycle_sort(); // name: a b c
        st.sel = 0; // "a"
        assert!(st.move_selected(true));
        assert_eq!(st.sort, PickerSort::Custom);
        assert_eq!(st.order(), ["b", "c", "a"]);
        assert_eq!(st.selected().unwrap().name, "a");
    }

    #[test]
    fn move_up_swaps_in_custom_order() {
        let mut st = picker(&["a", "b", "c"]);
        st.sel = 2;
        assert!(st.move_selected(false));
        assert_eq!(st.order(), ["a", "c", "b"]);
        assert_eq!(st.sel, 1);
    }

    #[test]
    fn move_at_edge_is_noop() {
        let mut st = picker(&["a", "b"]);
        assert!(!st.move_selected(false));
        st.sel = 1;
        assert!(!st.move_selected(true));
        assert_eq!(st.order(), ["a", "b"]);
        assert!(!PickerState::default().move_selected(true));
    }

    #[test]
    fn set_profiles_selects_by_name() {
        let mut st = picker(&["a", "b"]);
        st.set_profiles(vec![profile("x"), profile("b"), profile("a")], Some("a"));
        assert_eq!(st.selected().unwrap().name, "a");
        st.set_profiles(vec![profile("x")], None);
        assert_eq!(st.sel, 0, "clamped");
    }

    #[test]
    fn title_names_sort() {
        let st = picker(&["a"]);
        let backend = ratatui::backend::TestBackend::new(80, 10);
        let mut t = ratatui::Terminal::new(backend).unwrap();
        t.draw(|f| {
            render_picker(f, f.area(), &st, None, None);
        })
        .unwrap();
        let text: String = t
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("pick a broker · custom"), "{text}");
    }

    #[test]
    fn masked_counts_chars_not_bytes() {
        let st = PasswordState {
            profile: profile("a"),
            input: "pä".into(),
        };
        assert_eq!(st.masked(), "••");
    }

    #[test]
    fn server_url_marks_v5() {
        let mut p = profile("a");
        p.mqtt_version = mqtt_zapp_core::config::MqttVersion::V5;
        assert_eq!(server_url(&p), "mqtt://h:1883 (v5)");
    }

    #[test]
    fn server_url_has_no_at_without_user() {
        assert_eq!(server_url(&profile("a")), "mqtt://h:1883");
    }
}
