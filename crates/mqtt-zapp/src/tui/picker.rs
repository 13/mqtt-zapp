//! The connection picker and the password prompt: their state and drawing.

use super::ui::SELECTED_BG;
use mqtt_zapp_core::config::Profile;
use mqtt_zapp_core::payload::sanitize;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use std::fmt;

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

/// `name  protocol://host:port  user`, as in `profile list`.
fn profile_line(p: &Profile) -> String {
    let proto = format!("{:?}", p.protocol).to_lowercase();
    sanitize(&format!(
        "{}  {}://{}:{}  {}",
        p.name,
        proto,
        p.host,
        p.port,
        p.username.as_deref().unwrap_or("")
    ))
    .replace('\n', " ")
}

pub fn render_picker(f: &mut Frame, area: Rect, st: &PickerState, error: Option<&str>) {
    let block = Block::bordered().title("mqtt-zapp – pick a broker");
    let lines: Vec<Line> = if st.profiles.is_empty() {
        let mut v = Vec::new();
        if let Some(e) = error {
            v.push(Line::styled(
                sanitize(e).replace('\n', " "),
                Style::new().fg(Color::Red),
            ));
        }
        v.push(Line::raw(EMPTY_HINT));
        v
    } else {
        let height = block.inner(area).height as usize;
        let skip = (st.sel + 1).saturating_sub(height);
        st.profiles
            .iter()
            .enumerate()
            .skip(skip)
            .map(|(i, p)| {
                let line = Line::raw(profile_line(p));
                if i == st.sel {
                    line.style(Style::new().bg(SELECTED_BG))
                } else {
                    line
                }
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines).block(block), area);
}

pub fn render_password(f: &mut Frame, area: Rect, st: &PasswordState) {
    let title = format!(
        "password for {}",
        sanitize(&st.profile.name).replace('\n', " ")
    );
    let line = Line::raw(format!("{}▏", st.masked()));
    f.render_widget(
        Paragraph::new(line).block(Block::bordered().title(title)),
        area,
    );
}

#[cfg(test)]
mod tests {
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
    fn profile_line_has_empty_user_when_none() {
        assert_eq!(profile_line(&profile("a")), "a  mqtt://h:1883  ");
    }
}
