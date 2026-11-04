//! `sub` and `pub`: the client without the TUI. Nothing here touches the
//! terminal mode. Exit codes: 0 done, 1 an I/O error, 2 a connect, auth or
//! publish failure (clap's usage errors are 2 as well).

use crate::cli::{Format, PubCmd, SubCmd, Target, resolve_headless};
use anyhow::{Context, Result, anyhow, bail};
use mqtt_zapp_core::client::{ConnState, Connection, CoreEvent};
use mqtt_zapp_core::config::{PasswordSource, Profile};
use mqtt_zapp_core::{export, payload, secrets};
use std::io::{ErrorKind, LineWriter, Read, Write};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

/// `eprintln!` without the panic when stderr is closed.
macro_rules! warn {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

const FAIL: i32 = 2;
/// `--retained-only` stops after this much silence.
const RETAINED_IDLE: Duration = Duration::from_secs(2);
/// How long `pub` waits for the write or the acknowledgement.
const ACK_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_WAIT: Duration = Duration::from_secs(1);

/// A payload as one line of text: invalid UTF-8 becomes `<base64:…>`;
/// backslash is `\\`, line breaks and other control characters are
/// escaped, and a space in the topic is `\x20`, so the topic ends at the
/// first space. For people: scripts should read `--format jsonl`.
fn escape(s: &str, topic: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ' ' if topic => out.push_str("\\x20"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `<topic> <payload>` without the line end.
pub fn text_line(topic: &str, body: &[u8]) -> String {
    let body = match std::str::from_utf8(body) {
        Ok(t) => escape(t, false),
        Err(_) => format!("<base64:{}>", payload::base64(body)),
    };
    format!("{} {body}", escape(topic, true))
}

/// The password a headless run can use. Prompting is impossible without a
/// terminal conversation, so the keyring, an env var or a command is read.
/// Blocking, so it runs on a blocking thread.
async fn password(p: &Profile) -> Result<Option<String>> {
    match &p.password {
        PasswordSource::None => Ok(None),
        PasswordSource::Prompt => {
            bail!("password prompt is not available in sub/pub; use keyring, env or command")
        }
        source => {
            let keyring = *source == PasswordSource::Keyring;
            let p = p.clone();
            let r = tokio::task::spawn_blocking(move || secrets::resolve_password(&p))
                .await
                .map_err(|_| anyhow!("password lookup failed"))?;
            if keyring {
                r.context("reading the password from the keyring")
            } else {
                r
            }
        }
    }
}

/// Connect and wait for the broker's CONNACK. A refusal or a failed first
/// attempt is an error: scripts want an exit code, not a retry loop.
async fn open(
    target: &Target,
    subs: &crate::cli::SubArgs,
    listen: bool,
) -> Result<(Connection, Receiver<CoreEvent>)> {
    let mut profile = resolve_headless(target, subs)?;
    if !listen {
        profile.subscriptions.clear();
    }
    let pw = password(&profile).await?;
    let (conn, mut rx) = Connection::connect(&profile, pw)?;
    loop {
        match rx.recv().await {
            Some(CoreEvent::State(ConnState::Connected)) => return Ok((conn, rx)),
            Some(CoreEvent::State(
                ConnState::Failed { reason } | ConnState::Disconnected { reason, .. },
            )) => bail!("connection failed: {reason}"),
            Some(_) => {}
            None => bail!("connection failed: {}", mqtt_zapp_core::client::TASK_ENDED),
        }
    }
}

fn report(e: &anyhow::Error) -> i32 {
    warn!("error: {e:#}");
    FAIL
}

/// Why `sub` stopped.
enum Stop {
    Done,
    Fail(i32),
}

pub async fn sub(cmd: &SubCmd) -> i32 {
    let (conn, mut rx) = match open(&cmd.target, &cmd.subs, true).await {
        Ok(v) => v,
        Err(e) => return report(&e),
    };
    let mut out = LineWriter::new(std::io::stdout());
    let stop = sub_loop(cmd, &mut rx, &mut out).await;
    let flushed = out.flush();
    conn.close(CLOSE_WAIT).await;
    match (stop, flushed) {
        (Stop::Fail(c), _) => c,
        (Stop::Done, Err(e)) if e.kind() != ErrorKind::BrokenPipe => {
            warn!("error: writing to stdout: {e}");
            1
        }
        (Stop::Done, _) => 0,
    }
}

async fn sub_loop(cmd: &SubCmd, rx: &mut Receiver<CoreEvent>, out: &mut impl Write) -> Stop {
    let mut seen = 0usize;
    if cmd.count == Some(0) {
        return Stop::Done;
    }
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    // Armed once; only a message pushes it back.
    let idle = tokio::time::sleep(RETAINED_IDLE);
    tokio::pin!(idle);
    loop {
        let ev = tokio::select! {
            ev = rx.recv() => ev,
            _ = &mut ctrl_c => return Stop::Done,
            () = &mut idle, if cmd.retained_only => return Stop::Done,
        };
        match ev {
            Some(CoreEvent::Message { topic, msg }) => {
                if cmd.retained_only && !msg.retain {
                    return Stop::Done;
                }
                let mut bytes = match cmd.format {
                    Format::Text => text_line(&topic, &msg.payload).into_bytes(),
                    Format::Jsonl => export::message_jsonl(&topic, &msg),
                };
                if cmd.format == Format::Text {
                    bytes.push(b'\n');
                }
                if let Err(e) = out.write_all(&bytes) {
                    return if e.kind() == ErrorKind::BrokenPipe {
                        Stop::Done
                    } else {
                        warn!("error: writing to stdout: {e}");
                        Stop::Fail(1)
                    };
                }
                seen += 1;
                idle.as_mut()
                    .reset(tokio::time::Instant::now() + RETAINED_IDLE);
                if cmd.count.is_some_and(|n| seen >= n) {
                    return Stop::Done;
                }
            }
            Some(CoreEvent::SubRefused { filter, reason }) => {
                warn!("warning: subscription '{filter}' refused: {reason}");
            }
            Some(CoreEvent::State(ConnState::Failed { reason })) => {
                warn!("error: connection failed: {reason}");
                return Stop::Fail(FAIL);
            }
            Some(CoreEvent::State(ConnState::Disconnected { reason, .. })) => {
                warn!("warning: disconnected: {reason}; reconnecting");
            }
            Some(_) => {}
            None => {
                warn!(
                    "error: connection failed: {}",
                    mqtt_zapp_core::client::TASK_ENDED
                );
                return Stop::Fail(FAIL);
            }
        }
    }
}

fn read_payload(cmd: &PubCmd) -> Result<Vec<u8>> {
    if let Some(m) = &cmd.message {
        Ok(m.clone().into_bytes())
    } else if let Some(f) = &cmd.file {
        std::fs::read(f).with_context(|| format!("reading {}", f.display()))
    } else {
        let mut buf = Vec::new();
        std::io::stdin()
            .lock()
            .read_to_end(&mut buf)
            .context("reading stdin")?;
        Ok(buf)
    }
}

pub async fn publish(cmd: &PubCmd) -> i32 {
    match publish_once(cmd).await {
        Ok(()) => 0,
        Err(e) => report(&e),
    }
}

async fn publish_once(cmd: &PubCmd) -> Result<()> {
    let body = read_payload(cmd)?;
    let (conn, mut rx) = open(&cmd.target, &crate::cli::SubArgs::default(), false).await?;
    let sent = async {
        conn.publish(&cmd.topic, body, cmd.qos, cmd.retain, None)
            .await?;
        // QoS 0 is done once rumqttc has written the packet; QoS 1 and 2
        // once the broker acknowledges it.
        loop {
            match rx.recv().await {
                Some(CoreEvent::PublishSent { .. }) if cmd.qos == 0 => return Ok(()),
                Some(CoreEvent::PublishAcked {
                    failure: Some(why), ..
                }) => bail!("publish refused: {why}"),
                Some(CoreEvent::PublishAcked { .. }) => return Ok(()),
                Some(CoreEvent::State(
                    ConnState::Failed { reason } | ConnState::Disconnected { reason, .. },
                )) => bail!("connection lost before the publish was done: {reason}"),
                Some(_) => {}
                None => bail!("connection lost before the publish was done"),
            }
        }
    };
    let res = tokio::time::timeout(ACK_TIMEOUT, sent)
        .await
        .unwrap_or_else(|_| Err(anyhow!("timed out waiting for the broker")));
    conn.close(CLOSE_WAIT).await;
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_line_escapes_newlines_and_binary() {
        assert_eq!(text_line("a/b", b"hello"), "a/b hello");
        assert_eq!(text_line("a/b", b"l1\nl2\r\n"), "a/b l1\\nl2\\r\\n");
        assert_eq!(text_line("a/b", b""), "a/b ");
        assert_eq!(text_line("a b/c", b"x y"), "a\\x20b/c x y");
        assert_eq!(text_line("a/b", b"c:\\dir\\n"), "a/b c:\\\\dir\\\\n");
        assert_eq!(text_line("a/b", &[0xff, 0x00, 0x01]), "a/b <base64:/wAB>");
        assert_eq!(text_line("a/b", b"\x1b[31mred"), "a/b \\u{1b}[31mred");
        assert_eq!(text_line("t\nx", b"p"), "t\\nx p");
    }
}
