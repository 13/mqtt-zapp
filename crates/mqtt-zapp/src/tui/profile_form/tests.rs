use super::render::input_spans;
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mqtt_zapp_core::config::{MqttVersion, PasswordSource, Profile, Protocol, TlsConfig};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_str(f: &mut ProfileForm, s: &str) {
    for c in s.chars() {
        f.on_key(key(KeyCode::Char(c)));
    }
}

fn goto(f: &mut ProfileForm, field: PField) {
    for _ in 0..20 {
        if f.field == field {
            return;
        }
        f.on_key(key(KeyCode::Tab));
    }
    panic!("{field:?} not reachable");
}

fn sample(name: &str, pw: PasswordSource) -> Profile {
    Profile {
        name: name.into(),
        host: "h".into(),
        port: 8883,
        protocol: Protocol::Mqtts,
        mqtt_version: Default::default(),
        ws_path: "/".into(),
        client_id: Some("cid".into()),
        username: Some("u".into()),
        password: pw,
        tls: TlsConfig {
            ca_file: Some("/ca.pem".into()),
            client_cert: None,
            client_key: None,
            insecure: true,
            alpn: vec![],
        },
        subscriptions: vec!["#".into()],
        retained: true,
        keepalive_s: None,
        clean_session: None,
        session_expiry_s: None,
    }
}

/// Fill name and host so the form saves.
fn filled() -> ProfileForm {
    let mut f = ProfileForm::new(None, vec![]);
    type_str(&mut f, "home");
    goto(&mut f, PField::Host);
    type_str(&mut f, "h");
    f
}

fn save(f: &mut ProfileForm) -> Option<ProfileEdit> {
    match f.on_key(ctrl('s')) {
        ProfileOutcome::Save(e) => Some(e),
        _ => None,
    }
}

#[test]
fn line_input_edits_mid_line() {
    let mut l = LineInput::new("");
    for c in "hst".chars() {
        l.on_key(key(KeyCode::Char(c)));
    }
    l.on_key(key(KeyCode::Left));
    l.on_key(key(KeyCode::Left));
    l.on_key(key(KeyCode::Char('o')));
    assert_eq!(l.text(), "host");
    l.on_key(key(KeyCode::Home));
    l.on_key(key(KeyCode::Delete));
    assert_eq!(l.text(), "ost");
    l.on_key(key(KeyCode::End));
    l.on_key(key(KeyCode::Backspace));
    assert_eq!(l.text(), "os");
    assert!(!l.on_key(ctrl('a')), "ctrl chars are not text");
    assert_eq!(l.text(), "os");
}

#[test]
fn line_input_counts_chars_not_bytes() {
    let mut l = LineInput::new("pä");
    assert_eq!(l.cursor(), 2);
    l.on_key(key(KeyCode::Backspace));
    assert_eq!(l.text(), "p");
}

#[test]
fn new_form_defaults() {
    use PField::*;
    let f = ProfileForm::new(None, vec![]);
    assert_eq!(f.field, Name);
    assert_eq!(
        f.visible_fields(),
        [
            Name,
            Host,
            Port,
            Protocol,
            Version,
            Username,
            PasswordSource,
            ClientId,
            Subscriptions,
            Retained,
            Keepalive,
            CleanSession,
            SessionExpiry
        ]
    );
    assert_eq!(f.input(Port).unwrap().text(), "1883");
    assert_eq!(f.input(Subscriptions).unwrap().text(), "# $SYS/#");
}

#[test]
fn protocol_cycle_moves_default_port_only() {
    let mut f = ProfileForm::new(None, vec![]);
    goto(&mut f, PField::Protocol);
    f.on_key(key(KeyCode::Char(' ')));
    assert_eq!(f.protocol, Protocol::Mqtts);
    assert_eq!(f.input(PField::Port).unwrap().text(), "8883");
    assert!(f.visible_fields().contains(&PField::CaFile));
    assert!(f.visible_fields().contains(&PField::Insecure));
    goto(&mut f, PField::Port);
    f.on_key(key(KeyCode::Backspace));
    f.on_key(key(KeyCode::Backspace));
    type_str(&mut f, "00"); // 8800
    goto(&mut f, PField::Protocol);
    f.on_key(key(KeyCode::Right));
    assert_eq!(f.protocol, Protocol::Ws);
    assert_eq!(
        f.input(PField::Port).unwrap().text(),
        "8800",
        "custom port kept"
    );
    assert!(f.visible_fields().contains(&PField::WsPath));
    assert!(!f.visible_fields().contains(&PField::CaFile));
    f.on_key(key(KeyCode::Left));
    assert_eq!(f.protocol, Protocol::Mqtts);
}

#[test]
fn version_field_cycles_and_saves() {
    let mut f = filled();
    goto(&mut f, PField::Version);
    f.on_key(key(KeyCode::Char(' ')));
    assert_eq!(f.version, MqttVersion::V5);
    assert_eq!(save(&mut f).unwrap().profile.mqtt_version, MqttVersion::V5);
    f.on_key(key(KeyCode::Left));
    assert_eq!(
        save(&mut f).unwrap().profile.mqtt_version,
        MqttVersion::V311
    );
}

#[test]
fn save_builds_profile() {
    let mut f = filled();
    goto(&mut f, PField::Subscriptions);
    f.on_key(key(KeyCode::End));
    for _ in 0.."# $SYS/#".len() {
        f.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut f, "a/#  b");
    let e = save(&mut f).expect("saves");
    assert_eq!(e.original, None);
    assert_eq!(e.password, None);
    let p = e.profile;
    assert_eq!(p.name, "home");
    assert_eq!(p.host, "h");
    assert_eq!(p.port, 1883);
    assert_eq!(p.protocol, Protocol::Mqtt);
    assert_eq!(p.subscriptions, ["a/#", "b"]);
    assert_eq!(p.username, None);
    assert_eq!(p.client_id, None);
    assert_eq!(p.ws_path, "/");
    assert_eq!(p.password, PasswordSource::None);
    assert_eq!(p.tls, TlsConfig::default());
}

#[test]
fn save_rejects_taken_name() {
    let mut f = ProfileForm::new(None, vec!["home".into()]);
    type_str(&mut f, "home");
    goto(&mut f, PField::Host);
    type_str(&mut f, "h");
    assert!(matches!(f.on_key(ctrl('s')), ProfileOutcome::Continue));
    assert_eq!(f.error.as_deref(), Some("profile 'home' already exists"));
}

#[test]
fn save_validates_in_order() {
    let mut f = ProfileForm::new(None, vec![]);
    assert!(save(&mut f).is_none());
    assert_eq!(f.error.as_deref(), Some("name is required"));
    type_str(&mut f, "n");
    assert!(save(&mut f).is_none());
    assert_eq!(f.error.as_deref(), Some("host is required"));
    goto(&mut f, PField::Host);
    type_str(&mut f, "h");
    goto(&mut f, PField::Port);
    f.on_key(key(KeyCode::End));
    for _ in 0..4 {
        f.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut f, "0");
    assert!(save(&mut f).is_none());
    assert_eq!(f.error.as_deref(), Some("port must be 1–65535"));
    f.on_key(key(KeyCode::Backspace));
    type_str(&mut f, "70000");
    assert!(save(&mut f).is_none());
    f.on_key(key(KeyCode::End));
    for _ in 0..5 {
        f.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut f, "1883");
    goto(&mut f, PField::Subscriptions);
    f.on_key(key(KeyCode::End));
    for _ in 0..10 {
        f.on_key(key(KeyCode::Backspace));
    }
    assert!(save(&mut f).is_none());
    assert_eq!(f.error.as_deref(), Some("at least one subscription"));
    f.on_key(key(KeyCode::Char('x')));
    assert_eq!(f.error, None, "a key clears the error");
}

#[cfg(feature = "keyring")] // needs the keyring source
#[test]
fn keyring_password_required_unless_already_stored() {
    // New profile: required.
    let mut f = filled();
    goto(&mut f, PField::PasswordSource);
    f.on_key(key(KeyCode::Char(' ')));
    assert_eq!(f.source, PasswordSource::Keyring);
    assert!(f.visible_fields().contains(&PField::Password));
    assert!(save(&mut f).is_none());
    assert_eq!(f.error.as_deref(), Some("type a password, or use prompt"));
    goto(&mut f, PField::Password);
    type_str(&mut f, "pw");
    assert_eq!(save(&mut f).unwrap().password.as_deref(), Some("pw"));

    // Editing a keyring profile: blank keeps the stored one.
    let p = sample("a", PasswordSource::Keyring);
    let mut f = ProfileForm::new(Some(&p), vec![]);
    assert_eq!(save(&mut f).unwrap().password, None);

    // Editing a profile that had no stored password: required.
    let p = sample("a", PasswordSource::Prompt);
    let mut f = ProfileForm::new(Some(&p), vec![]);
    goto(&mut f, PField::PasswordSource);
    f.on_key(key(KeyCode::Left));
    assert_eq!(f.source, PasswordSource::Keyring);
    assert!(save(&mut f).is_none());
}

#[cfg(feature = "keyring")] // needs the keyring source
#[test]
fn password_dropped_unless_keyring() {
    let mut f = filled();
    goto(&mut f, PField::PasswordSource);
    f.on_key(key(KeyCode::Char(' ')));
    goto(&mut f, PField::Password);
    type_str(&mut f, "pw");
    goto(&mut f, PField::PasswordSource);
    f.on_key(key(KeyCode::Char(' '))); // -> prompt
    let e = save(&mut f).unwrap();
    assert_eq!(e.profile.password, PasswordSource::Prompt);
    assert_eq!(e.password, None);
}

#[test]
fn edit_prefills_and_keeps_original() {
    let p = sample("a", PasswordSource::None);
    let mut f = ProfileForm::new(Some(&p), vec!["other".into()]);
    assert_eq!(f.input(PField::Name).unwrap().text(), "a");
    assert_eq!(f.input(PField::Port).unwrap().text(), "8883");
    assert_eq!(f.input(PField::CaFile).unwrap().text(), "/ca.pem");
    type_str(&mut f, "b");
    let e = save(&mut f).unwrap();
    assert_eq!(e.original.as_deref(), Some("a"));
    let mut want = p.clone();
    want.name = "ab".into();
    assert_eq!(e.profile, want);
}

#[test]
fn esc_cancels_and_enter_moves_on() {
    let mut f = ProfileForm::new(None, vec![]);
    f.on_key(key(KeyCode::Enter));
    assert_eq!(f.field, PField::Host);
    f.on_key(key(KeyCode::BackTab));
    assert_eq!(f.field, PField::Name);
    f.on_key(key(KeyCode::BackTab));
    assert_eq!(f.field, PField::SessionExpiry, "wraps");
    assert!(matches!(
        f.on_key(key(KeyCode::Esc)),
        ProfileOutcome::Cancel
    ));
}

#[test]
fn insecure_toggles() {
    let p = sample("a", PasswordSource::None);
    let mut f = ProfileForm::new(Some(&p), vec![]);
    goto(&mut f, PField::Insecure);
    f.on_key(key(KeyCode::Char(' ')));
    assert!(!save(&mut f).unwrap().profile.tls.insecure);
}

#[test]
fn form_rejects_bad_qos() {
    let mut f = filled();
    goto(&mut f, PField::Subscriptions);
    type_str(&mut f, " a@3");
    assert!(save(&mut f).is_none());
    assert_eq!(f.error.as_deref(), Some("bad QoS in \"a@3\""));
    let mut f = filled();
    goto(&mut f, PField::Subscriptions);
    type_str(&mut f, " a/#@2 user@host/#");
    assert!(save(&mut f).is_some());
}

#[test]
fn retained_toggles_and_is_marked_v5_only() {
    let mut f = filled();
    goto(&mut f, PField::Retained);
    f.on_key(key(KeyCode::Char(' ')));
    assert!(!save(&mut f).unwrap().profile.retained);
    assert!(draw(&f, 80, 30).join("\n").contains("(MQTT 5 only)"));
}

fn draw(f: &ProfileForm, w: u16, h: u16) -> Vec<String> {
    use ratatui::{Terminal, backend::TestBackend};
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|fr| render(fr, fr.area(), f)).unwrap();
    let buf = t.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_owned()).collect())
        .collect()
}

#[test]
fn form_sanitizes_values() {
    let mut p = sample("x", PasswordSource::None);
    p.host = "evil\x1b[31m".into();
    let rows = draw(&ProfileForm::new(Some(&p), vec![]), 80, 24);
    let all = rows.join("\n");
    assert!(!all.contains('\x1b'));
    assert!(all.contains("evil"));
}

#[test]
fn error_title_sanitizes_profile_name() {
    let p = sample("x\x1b[2J", PasswordSource::None);
    let mut f = ProfileForm::new(Some(&p), vec![]);
    goto(&mut f, PField::Host);
    f.on_key(key(KeyCode::Backspace));
    save(&mut f);
    assert_eq!(f.error.as_deref(), Some("host is required"));
    for (w, h) in [(80, 24), (40, 10)] {
        let all = draw(&f, w, h).join("\n");
        assert!(!all.contains('\x1b'), "{w}x{h}: {all}");
    }
}

#[test]
fn form_fits_40x10_and_shows_focus() {
    let mut f = ProfileForm::new(None, vec![]);
    goto(&mut f, PField::Subscriptions);
    let rows = draw(&f, 40, 10);
    assert!(
        rows.iter()
            .any(|r| r.contains('▶') && r.contains("subscriptions")),
        "{rows:#?}"
    );
    // The first row has scrolled away.
    assert!(!rows.iter().any(|r| r.contains(" name  ")), "{rows:#?}");
}

#[cfg(feature = "keyring")] // needs the keyring source
#[test]
fn form_masks_password() {
    let mut f = filled();
    goto(&mut f, PField::PasswordSource);
    f.on_key(key(KeyCode::Char(' ')));
    goto(&mut f, PField::Password);
    type_str(&mut f, "abc");
    let all = draw(&f, 80, 24).join("\n");
    assert!(all.contains("•••"));
    assert!(!all.contains("abc"));
    assert!(all.contains("‹ keyring ›"));
}

#[test]
fn edit_title_and_unchanged_hint() {
    let p = sample("home", PasswordSource::Keyring);
    let all = draw(&ProfileForm::new(Some(&p), vec![]), 80, 24).join("\n");
    assert!(all.contains("edit profile · home"), "{all}");
    assert!(all.contains("(unchanged)"));
    assert!(all.contains("[x]"), "insecure shown");
    assert!(
        all.contains("subscriptions #"),
        "label and value apart: {all}"
    );
    let all = draw(&ProfileForm::new(None, vec![]), 80, 24).join("\n");
    assert!(all.contains("add profile"));
}

#[test]
fn error_shows_in_title() {
    let mut f = ProfileForm::new(None, vec![]);
    save(&mut f);
    let all = draw(&f, 80, 24).join("\n");
    assert!(all.contains("name is required"), "{all}");
}

#[test]
fn wide_chars_keep_cursor_in_view() {
    use unicode_width::UnicodeWidthStr;
    let spans = input_spans(&"中".repeat(30), Some(30), 10);
    let cols: usize = spans.iter().map(|s| s.content.width()).sum();
    assert!(cols <= 10, "{cols} columns");
    let last = spans.last().unwrap();
    assert!(
        last.style.add_modifier.contains(Modifier::REVERSED),
        "cursor shown"
    );
}

#[cfg(not(feature = "keyring"))]
#[test]
fn keyring_not_offered_without_feature() {
    let mut f = ProfileForm::new(None, vec![]);
    goto(&mut f, PField::PasswordSource);
    for _ in 0..6 {
        f.on_key(key(KeyCode::Char(' ')));
        assert_ne!(f.source, PasswordSource::Keyring);
    }
}

#[cfg(feature = "keyring")] // needs the keyring source
#[test]
fn debug_hides_password() {
    let mut f = filled();
    goto(&mut f, PField::PasswordSource);
    f.on_key(key(KeyCode::Char(' ')));
    goto(&mut f, PField::Password);
    type_str(&mut f, "s3cret");
    assert!(!format!("{f:?}").contains("s3cret"));
    let e = save(&mut f).unwrap();
    assert!(!format!("{e:?}").contains("s3cret"));
}

#[test]
fn form_session_rows_round_trip() {
    let mut p = sample("s", PasswordSource::None);
    p.mqtt_version = MqttVersion::V5;
    p.keepalive_s = Some(30);
    p.clean_session = Some(false);
    p.session_expiry_s = Some(3600);
    let mut f = ProfileForm::new(Some(&p), vec![]);
    let e = save(&mut f).unwrap().profile;
    assert_eq!(
        (e.keepalive_s, e.clean_session, e.session_expiry_s),
        (Some(30), Some(false), Some(3600))
    );
    // Defaults stay unset; emptied rows clear; the toggle flips back.
    let mut f = ProfileForm::new(Some(&p), vec![]);
    goto(&mut f, PField::Keepalive);
    for _ in 0..3 {
        f.on_key(key(KeyCode::Backspace));
    }
    goto(&mut f, PField::CleanSession);
    f.on_key(key(KeyCode::Char(' ')));
    goto(&mut f, PField::SessionExpiry);
    for _ in 0..5 {
        f.on_key(key(KeyCode::Backspace));
    }
    let e = save(&mut f).unwrap().profile;
    assert_eq!(
        (e.keepalive_s, e.clean_session, e.session_expiry_s),
        (None, None, None)
    );
}

#[test]
fn form_keepalive_range_and_expiry_warning() {
    let mut f = filled();
    goto(&mut f, PField::Keepalive);
    type_str(&mut f, "70000");
    assert!(save(&mut f).is_none());
    assert_eq!(f.error.as_deref(), Some("keepalive: 0..=65535 s"));
    let mut f = filled();
    goto(&mut f, PField::Keepalive);
    type_str(&mut f, "x");
    assert!(save(&mut f).is_none());
    let mut f = filled();
    goto(&mut f, PField::SessionExpiry);
    type_str(&mut f, "60");
    // 3.1.1: a warning in the hint line, not an error.
    assert!(
        draw(&f, 80, 30)
            .join("\n")
            .contains("session expiry needs MQTT 5")
    );
    assert!(save(&mut f).is_some());
    goto(&mut f, PField::Version);
    f.on_key(key(KeyCode::Char(' ')));
    assert!(
        !draw(&f, 80, 30)
            .join("\n")
            .contains("session expiry needs MQTT 5")
    );
}

#[test]
fn env_and_command_sources_take_a_value() {
    let mut f = filled();
    goto(&mut f, PField::PasswordSource);
    assert!(!f.visible_fields().contains(&PField::SourceValue));
    // none -> (keyring ->) prompt -> env
    for _ in 0..4 {
        if matches!(f.source, PasswordSource::Env(_)) {
            break;
        }
        f.on_key(key(KeyCode::Char(' ')));
    }
    assert!(matches!(f.source, PasswordSource::Env(_)));
    assert!(f.visible_fields().contains(&PField::SourceValue));
    assert!(!f.visible_fields().contains(&PField::Password));
    assert!(save(&mut f).is_none(), "an empty variable name is refused");
    assert_eq!(f.error.as_deref(), Some("env: type a variable name"));
    goto(&mut f, PField::SourceValue);
    type_str(&mut f, "MQTT_PW");
    assert_eq!(
        save(&mut f).unwrap().profile.password,
        PasswordSource::Env("MQTT_PW".into())
    );
    goto(&mut f, PField::PasswordSource);
    f.on_key(key(KeyCode::Char(' ')));
    assert!(matches!(f.source, PasswordSource::Command(_)));
    goto(&mut f, PField::SourceValue);
    f.on_key(key(KeyCode::Home));
    for _ in 0.."MQTT_PW".len() {
        f.on_key(key(KeyCode::Delete));
    }
    assert!(save(&mut f).is_none());
    type_str(&mut f, "pass show mqtt");
    let e = save(&mut f).unwrap();
    assert_eq!(
        e.profile.password,
        PasswordSource::Command("pass show mqtt".into())
    );
    assert_eq!(e.password, None, "nothing for the keyring");
    // Editing keeps the value and the ALPN list.
    let mut p = e.profile;
    p.protocol = Protocol::Mqtts;
    p.tls.alpn = vec!["mqtt".into()];
    let mut f = ProfileForm::new(Some(&p), vec![]);
    let again = save(&mut f).unwrap().profile;
    assert_eq!(again.password, p.password);
    assert_eq!(again.tls.alpn, vec!["mqtt".to_owned()]);
}

#[test]
fn paste_fills_the_focused_text_field_only() {
    let mut f = ProfileForm::new(None, vec![]);
    goto(&mut f, PField::Name);
    f.paste("my broker\x1b\nrest");
    assert_eq!(f.name.text(), "my broker\u{fffd}");
    goto(&mut f, PField::CleanSession);
    let before = f.clean_session;
    f.paste(" ");
    assert_eq!(f.clean_session, before, "a toggle ignores a paste");
}
