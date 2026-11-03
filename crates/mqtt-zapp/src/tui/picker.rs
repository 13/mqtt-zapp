//! The connection picker and the password prompt: their state and drawing.

use super::theme;
use mqtt_zapp_core::config::{Profile, Protocol};
use mqtt_zapp_core::payload::sanitize;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::fmt;
use unicode_width::UnicodeWidthStr;

pub const EMPTY_HINT: &str = "no profiles – add one with: mqtt-zapp profile add --name … --host …";

#[derive(Debug, Default)]
pub struct PickerState {
    pub profiles: Vec<Profile>,
    pub sel: usize,
}

impl PickerState {
    pub fn new(profiles: Vec<Profile>) -> PickerState {
        PickerState { profiles, sel: 0 }
    }

    pub fn selected(&self) -> Option<&Profile> {
        self.profiles.get(self.sel)
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
    sanitize(&format!("{proto}://{user}{}:{}{path}", p.host, p.port)).replace('\n', " ")
}

/// The broker list as a centred card: `▶ name  url`, names padded to line up.
/// Returns the card and how many profiles were scrolled off its top.
pub fn render_picker(
    f: &mut Frame,
    area: Rect,
    st: &PickerState,
    error: Option<&str>,
) -> (Rect, usize) {
    let mut skipped = 0;
    let muted = Style::new().fg(theme::MUTED);
    let lines: Vec<Line> =
        if st.profiles.is_empty() {
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
                .profiles
                .iter()
                .map(|p| sanitize(&p.name).replace('\n', " "))
                .collect();
            let pad = names.iter().map(|n| n.width()).max().unwrap_or(0);
            let card = theme::centered(area, 72, st.profiles.len() as u16 + 4);
            let height = card.height.saturating_sub(4) as usize;
            let skip = (st.sel + 1).saturating_sub(height.max(1));
            skipped = skip;
            let mut v = vec![Line::default()];
            v.extend(st.profiles.iter().zip(&names).enumerate().skip(skip).map(
                |(i, (p, name))| {
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
                        Span::raw("  "),
                        Span::styled(server_url(p), muted),
                    ]);
                    if sel {
                        line.style(theme::selection(true))
                    } else {
                        line
                    }
                },
            ));
            v
        };
    let card = theme::centered(area, 72, lines.len() as u16 + 3);
    f.render_widget(
        Paragraph::new(lines).block(theme::pane("mqtt-zapp · pick a broker", true)),
        card,
    );
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
            ws_path: "/".into(),
            client_id: None,
            username: Some("alice".into()),
            password: PasswordSource::None,
            tls: TlsConfig::default(),
            subscriptions: vec![],
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

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            host: "h".into(),
            port: 1883,
            protocol: Protocol::Mqtt,
            ws_path: "/".into(),
            client_id: None,
            username: None,
            password: PasswordSource::None,
            tls: TlsConfig::default(),
            subscriptions: vec![],
        }
    }

    #[test]
    fn navigation_is_clamped() {
        let mut st = PickerState::new(vec![profile("a"), profile("b")]);
        st.up();
        assert_eq!(st.sel, 0);
        st.down();
        st.down();
        assert_eq!(st.sel, 1);
        PickerState::default().down();
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
    fn server_url_has_no_at_without_user() {
        assert_eq!(server_url(&profile("a")), "mqtt://h:1883");
    }
}
