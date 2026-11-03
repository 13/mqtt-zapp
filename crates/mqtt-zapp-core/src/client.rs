//! The broker connection: builds rumqttc options from a `Profile` (plain, TLS,
//! WebSocket), runs the event loop in a background task with reconnect backoff,
//! and reports everything as UI-agnostic `CoreEvent`s. Broker errors never panic
//! or end the task; it stops only on `disconnect()` or when the receiver drops.

use crate::config::{MqttVersion, Profile, Protocol, TlsConfig};
use crate::payload::Message;
use anyhow::{Context, Result, anyhow, bail};
use rumqttc::tokio_rustls::rustls;
use rumqttc::v5;
use rumqttc::{
    AsyncClient, Event, EventLoop, MqttOptions, Outgoing, Packet, QoS, SubscribeFilter,
    TlsConfiguration, Transport,
};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const CHANNEL_CAPACITY: usize = 10_000;
const REQUEST_CAPACITY: usize = 100;
const PUBLISH_TIMEOUT: Duration = Duration::from_secs(5);
const BACKOFF_START_MS: u64 = 1_000;
const BACKOFF_MAX_MS: u64 = 30_000;
/// Largest packet MQTT 3.1.1 can encode; rumqttc's 10 KiB default would drop
/// the connection on big payloads.
const MAX_PACKET: usize = 268_435_455;

/// QoS of the empty retained publishes that clear a topic: at least once,
/// so a clear is not silently lost the way a QoS 0 publish can be.
pub const CLEAR_QOS: u8 = 1;

#[derive(Debug, Clone, Serialize)]
pub enum ConnState {
    Connecting,
    Connected,
    Disconnected { reason: String, retry_in_ms: u64 },
}

impl ConnState {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnState::Connected)
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum CoreEvent {
    State(ConnState),
    Message {
        topic: String,
        msg: Message,
    },
    /// Messages dropped because the UI channel was full, since the last
    /// `Dropped` event.
    Dropped(u64),
}

#[derive(Clone)]
pub struct Connection {
    client: Client,
    cancel: CancellationToken,
    task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    finished: Arc<AtomicBool>,
}

/// The request side, for either protocol version.
#[derive(Clone)]
enum Client {
    V4(AsyncClient),
    V5(v5::AsyncClient),
}

impl Client {
    async fn publish(&self, topic: &str, qos: u8, retain: bool, payload: Vec<u8>) -> Result<()> {
        match self {
            Client::V4(c) => {
                let qos = rumqttc::qos(qos).map_err(|e| anyhow!("invalid qos {qos}: {e}"))?;
                Ok(c.publish(topic, qos, retain, payload).await?)
            }
            Client::V5(c) => {
                let qos = v5::mqttbytes::qos(qos).ok_or_else(|| anyhow!("invalid qos {qos}"))?;
                Ok(c.publish(topic, qos, retain, payload).await?)
            }
        }
    }

    /// Queue SUBSCRIBE without waiting; false when the request queue is full.
    fn try_subscribe_many(&self, topics: &[String]) -> bool {
        match self {
            Client::V4(c) => c.try_subscribe_many(v4_filters(topics)).is_ok(),
            Client::V5(c) => c.try_subscribe_many(v5_filters(topics)).is_ok(),
        }
    }

    async fn subscribe_many(&self, topics: &[String]) -> Result<()> {
        match self {
            Client::V4(c) => Ok(c.subscribe_many(v4_filters(topics)).await?),
            Client::V5(c) => Ok(c.subscribe_many(v5_filters(topics)).await?),
        }
    }

    fn try_disconnect(&self) {
        let _ = match self {
            Client::V4(c) => c.try_disconnect().map_err(|_| ()),
            Client::V5(c) => c.try_disconnect().map_err(|_| ()),
        };
    }
}

fn v4_filters(topics: &[String]) -> Vec<SubscribeFilter> {
    topics
        .iter()
        .map(|t| SubscribeFilter::new(t.clone(), QoS::AtMostOnce))
        .collect()
}

/// "Retain as published", so a live retained message keeps its flag (MQTT
/// 5 only; 3.1.1 brokers clear it on live delivery).
fn v5_filters(topics: &[String]) -> Vec<v5::mqttbytes::v5::Filter> {
    topics
        .iter()
        .map(|t| {
            let mut f = v5::mqttbytes::v5::Filter::new(t.clone(), v5::mqttbytes::QoS::AtMostOnce);
            f.preserve_retain = true;
            f
        })
        .collect()
}

/// The event loop for either protocol version.
enum Loop {
    V4(Box<EventLoop>),
    V5(Box<v5::EventLoop>),
}

/// What `run` needs from one poll, version-independent.
enum Polled {
    ConnAck,
    Publish { topic: String, msg: Message },
    OutgoingDisconnect,
    Other,
    Err(String),
}

/// A received publish as a `Message`, its payload copied once.
///
/// rumqttc reads packets into a shared `BytesMut` (10 KiB or more) and hands
/// out each payload as a frozen slice of it, so keeping that `Bytes` would
/// pin the whole read chunk: a 4-byte `latest` could hold megabytes alive
/// and `max_history_mb` would no longer bound real memory. The copy is the
/// only one; `latest` and history share it.
fn incoming(payload: &[u8], qos: u8, retain: bool) -> Message {
    Message {
        payload: bytes::Bytes::copy_from_slice(payload),
        qos,
        retain,
        received_at: SystemTime::now(),
        truncated_from: None,
    }
}

impl Loop {
    /// Forget the requests rumqttc kept across a failed connection (in flight
    /// or queued), so a clear or publish made while disconnected is never
    /// replayed on reconnect. Returns how many were dropped.
    fn drop_pending(&mut self) -> usize {
        match self {
            Loop::V4(l) => l.pending.drain(..).count(),
            Loop::V5(l) => l.pending.drain(..).count(),
        }
    }

    async fn poll(&mut self) -> Polled {
        match self {
            Loop::V4(l) => match l.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => Polled::ConnAck,
                Ok(Event::Incoming(Packet::Publish(p))) => Polled::Publish {
                    msg: incoming(&p.payload, p.qos as u8, p.retain),
                    topic: p.topic,
                },
                Ok(Event::Outgoing(Outgoing::Disconnect)) => Polled::OutgoingDisconnect,
                Ok(_) => Polled::Other,
                Err(e) => Polled::Err(e.to_string()),
            },
            Loop::V5(l) => match l.poll().await {
                Ok(v5::Event::Incoming(v5::Incoming::ConnAck(_))) => Polled::ConnAck,
                Ok(v5::Event::Incoming(v5::Incoming::Publish(p))) => Polled::Publish {
                    msg: incoming(&p.payload, p.qos as u8, p.retain),
                    topic: String::from_utf8_lossy(&p.topic).into_owned(),
                },
                Ok(v5::Event::Outgoing(Outgoing::Disconnect)) => Polled::OutgoingDisconnect,
                Ok(_) => Polled::Other,
                Err(e) => Polled::Err(e.to_string()),
            },
        }
    }
}

impl Connection {
    /// Start connecting in a background task. Must be called inside a tokio
    /// runtime (an error, not a panic, otherwise).
    pub fn connect(
        profile: &Profile,
        password: Option<String>,
    ) -> Result<(Connection, mpsc::Receiver<CoreEvent>)> {
        let handle = tokio::runtime::Handle::try_current()
            .context("Connection::connect needs a tokio runtime")?;
        let (client, eventloop) = match profile.mqtt_version {
            MqttVersion::V311 => {
                let (c, l) = AsyncClient::new(mqtt_options(profile, password)?, REQUEST_CAPACITY);
                (Client::V4(c), Loop::V4(Box::new(l)))
            }
            MqttVersion::V5 => {
                let opts = mqtt_options_v5(profile, password)?;
                let (c, l) = v5::AsyncClient::new(opts, REQUEST_CAPACITY);
                (Client::V5(c), Loop::V5(Box::new(l)))
            }
        };
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let cancel = CancellationToken::new();
        let finished = Arc::new(AtomicBool::new(false));
        let task = handle.spawn({
            let (client, cancel, finished) = (client.clone(), cancel.clone(), finished.clone());
            let subscriptions = profile.subscriptions.clone();
            async move {
                run(eventloop, client, subscriptions, tx, cancel).await;
                finished.store(true, Ordering::SeqCst);
            }
        });
        Ok((
            Connection {
                client,
                cancel,
                task: Arc::new(Mutex::new(Some(task))),
                finished,
            },
            rx,
        ))
    }

    pub async fn publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    ) -> Result<()> {
        // The request queue only drains while connected; don't hang the caller
        // for a whole outage.
        tokio::time::timeout(
            PUBLISH_TIMEOUT,
            self.client.publish(topic, qos, retain, payload),
        )
        .await
        .map_err(|_| anyhow!("publish timed out (not connected?)"))?
        .context("publish failed")
    }

    /// Clear the retained message of each topic: an empty retained publish,
    /// the standard way in MQTT 3.1.1 and 5. Sent at QoS 1 ([`CLEAR_QOS`]) so
    /// the client keeps retrying until the broker acknowledges it. Stops at
    /// the first failure; returns how many were sent.
    pub async fn clear_retained(&self, topics: &[String]) -> Result<usize> {
        for (i, t) in topics.iter().enumerate() {
            self.publish(t, Vec::new(), CLEAR_QOS, true)
                .await
                .with_context(|| format!("clear failed on {t} ({i} of {} done)", topics.len()))?;
        }
        Ok(topics.len())
    }

    /// Send DISCONNECT (best effort) and stop the background task.
    pub fn disconnect(&self) {
        self.client.try_disconnect();
        self.cancel.cancel();
    }

    /// Like `disconnect`, then wait up to `wait` for the background task to
    /// finish, which it does once DISCONNECT has been written to the socket.
    pub async fn close(&self, wait: Duration) {
        self.disconnect();
        let handle = self.task.lock().ok().and_then(|mut t| t.take());
        if let Some(handle) = handle {
            let _ = tokio::time::timeout(wait, handle).await;
        }
    }

    /// Whether the background task has ended (for tests).
    #[doc(hidden)]
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
}

async fn run(
    mut eventloop: Loop,
    client: Client,
    subscriptions: Vec<String>,
    tx: mpsc::Sender<CoreEvent>,
    cancel: CancellationToken,
) {
    // Every send doubles as the exit check: a closed receiver means stop.
    macro_rules! emit {
        ($ev:expr) => {
            if tx.send($ev).await.is_err() {
                return;
            }
        };
    }
    let mut backoff = BACKOFF_START_MS;
    let mut connected = false;
    let mut dropped: u64 = 0;
    emit!(CoreEvent::State(ConnState::Connecting));
    loop {
        let polled = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tx.closed() => return,
            r = eventloop.poll() => r,
        };
        match polled {
            Polled::ConnAck => {
                connected = true;
                backoff = BACKOFF_START_MS;
                subscribe(&client, &subscriptions, &tx, &cancel);
                emit!(CoreEvent::State(ConnState::Connected));
            }
            Polled::Publish { topic, msg } => {
                // Never wait on a full UI channel: the loop must keep polling
                // (keepalive pings, acks). Count what does not fit instead.
                if dropped > 0 && tx.try_send(CoreEvent::Dropped(dropped)).is_ok() {
                    dropped = 0;
                }
                match tx.try_send(CoreEvent::Message { topic, msg }) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => dropped += 1,
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                }
            }
            Polled::OutgoingDisconnect | Polled::Other => {}
            Polled::Err(reason) => {
                connected = false;
                let unsent = eventloop.drop_pending();
                let reason = if unsent > 0 {
                    format!("{reason} · dropped {unsent} unsent")
                } else {
                    reason
                };
                emit!(CoreEvent::State(ConnState::Disconnected {
                    reason,
                    retry_in_ms: backoff,
                }));
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    _ = tx.closed() => return,
                    _ = tokio::time::sleep(Duration::from_millis(backoff)) => {}
                }
                backoff = (backoff * 2).min(BACKOFF_MAX_MS);
                emit!(CoreEvent::State(ConnState::Connecting));
            }
        }
    }
    // Cancelled: give a queued DISCONNECT a moment to reach the broker.
    if connected {
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match eventloop.poll().await {
                    Polled::OutgoingDisconnect | Polled::Err(_) => break,
                    _ => {}
                }
            }
        })
        .await;
    }
}

/// Subscribe to `topics` at QoS 0 without blocking the poll loop, which is the
/// only thing draining the request channel. If the channel is full (requests
/// queued during an outage), a spawned task waits for room while the loop keeps
/// draining; if even that fails, the reason is reported as `Disconnected`.
fn subscribe(
    client: &Client,
    topics: &[String],
    tx: &mpsc::Sender<CoreEvent>,
    cancel: &CancellationToken,
) {
    if topics.is_empty() || client.try_subscribe_many(topics) {
        return;
    }
    let (client, tx, cancel) = (client.clone(), tx.clone(), cancel.clone());
    let topics = topics.to_vec();
    tokio::spawn(async move {
        if let Err(e) = client.subscribe_many(&topics).await
            && !cancel.is_cancelled()
        {
            let _ = tx
                .send(CoreEvent::State(ConnState::Disconnected {
                    reason: format!("subscribe failed: {e:#}"),
                    retry_in_ms: 0,
                }))
                .await;
        }
    });
}

/// (client id, host or websocket URL, port) for either options type.
fn endpoint(profile: &Profile) -> (String, String, u16) {
    let id = profile.effective_client_id();
    let host = &profile.host;
    let port = profile.port;
    let host = match profile.protocol {
        Protocol::Mqtt | Protocol::Mqtts => host.clone(),
        Protocol::Ws | Protocol::Wss => {
            let scheme = if profile.protocol.is_tls() {
                "wss"
            } else {
                "ws"
            };
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]") // IPv6 literal
            } else {
                host.clone()
            };
            let path = &profile.ws_path;
            let slash = if path.starts_with('/') { "" } else { "/" };
            format!("{scheme}://{host}:{port}{slash}{path}")
        }
    };
    (id, host, port)
}

fn transport(profile: &Profile) -> Result<Transport> {
    Ok(match profile.protocol {
        Protocol::Mqtt => Transport::Tcp,
        Protocol::Ws => Transport::Ws,
        Protocol::Mqtts => Transport::Tls(TlsConfiguration::Rustls(tls_config(&profile.tls)?)),
        Protocol::Wss => Transport::Wss(TlsConfiguration::Rustls(tls_config(&profile.tls)?)),
    })
}

pub(crate) fn mqtt_options(profile: &Profile, password: Option<String>) -> Result<MqttOptions> {
    let (id, host, port) = endpoint(profile);
    let mut opts = MqttOptions::new(id, host, port);
    opts.set_max_packet_size(MAX_PACKET, MAX_PACKET);
    if let Some(user) = &profile.username {
        opts.set_credentials(user, password.unwrap_or_default());
    }
    opts.set_transport(transport(profile)?);
    Ok(opts)
}

fn mqtt_options_v5(profile: &Profile, password: Option<String>) -> Result<v5::MqttOptions> {
    let (id, host, port) = endpoint(profile);
    let mut opts = v5::MqttOptions::new(id, host, port);
    // Incoming limit, announced to the broker in CONNECT.
    opts.set_max_packet_size(Some(MAX_PACKET as u32));
    if let Some(user) = &profile.username {
        opts.set_credentials(user, password.unwrap_or_default());
    }
    opts.set_transport(transport(profile)?);
    Ok(opts)
}

fn tls_config(tls: &TlsConfig) -> Result<Arc<rustls::ClientConfig>> {
    let builder = rustls::ClientConfig::builder();
    let builder = if tls.insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(insecure::AcceptAnyCert::new()))
    } else {
        builder.with_root_certificates(root_store(tls)?)
    };
    let config = match (&tls.client_cert, &tls.client_key) {
        (None, None) => builder.with_no_client_auth(),
        (Some(cert), Some(key)) => {
            let certs = read_pem(cert, |r| rustls_pemfile::certs(r).collect())?;
            let key = read_pem(key, rustls_pemfile::private_key)?
                .with_context(|| format!("no private key in {}", key.display()))?;
            builder
                .with_client_auth_cert(certs, key)
                .context("invalid client certificate or key")?
        }
        _ => bail!("TLS client auth needs both client_cert and client_key"),
    };
    Ok(Arc::new(config))
}

fn root_store(tls: &TlsConfig) -> Result<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    let certs = match &tls.ca_file {
        Some(path) => read_pem(path, |r| rustls_pemfile::certs(r).collect())?,
        None => rustls_native_certs::load_native_certs().context("loading system CA roots")?,
    };
    roots.add_parsable_certificates(certs);
    if roots.is_empty() {
        match &tls.ca_file {
            Some(p) => bail!("no usable CA certificates in {}", p.display()),
            None => bail!("no usable system CA certificates found"),
        }
    }
    Ok(roots)
}

fn read_pem<T>(
    path: &std::path::Path,
    parse: impl FnOnce(&mut dyn std::io::BufRead) -> std::io::Result<T>,
) -> Result<T> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&mut bytes.as_slice()).with_context(|| format!("parsing PEM in {}", path.display()))
}

mod insecure {
    //! `tls.insecure = true`: accept any server certificate, but still check
    //! handshake signatures so the session itself is sound.

    use super::rustls;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{
        WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature,
    };
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct AcceptAnyCert(WebPkiSupportedAlgorithms);

    impl AcceptAnyCert {
        pub(super) fn new() -> Self {
            Self(rustls::crypto::ring::default_provider().signature_verification_algorithms)
        }
    }

    impl ServerCertVerifier for AcceptAnyCert {
        fn verify_server_cert(
            &self,
            _end_entity: &CertificateDer<'_>,
            _intermediates: &[CertificateDer<'_>],
            _server_name: &ServerName<'_>,
            _ocsp_response: &[u8],
            _now: UnixTime,
        ) -> Result<ServerCertVerified, Error> {
            Ok(ServerCertVerified::assertion())
        }

        fn verify_tls12_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            verify_tls12_signature(message, cert, dss, &self.0)
        }

        fn verify_tls13_signature(
            &self,
            message: &[u8],
            cert: &CertificateDer<'_>,
            dss: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, Error> {
            verify_tls13_signature(message, cert, dss, &self.0)
        }

        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            self.0.supported_schemes()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PasswordSource;

    #[test]
    fn incoming_payload_does_not_alias_the_read_buffer() {
        // rumqttc hands out the payload as a slice of its shared read chunk.
        let chunk = bytes::Bytes::from(vec![7u8; 64 * 1024]);
        let p = chunk.slice(100..104);
        let m = incoming(&p, 1, true);
        assert_eq!(&m.payload[..], &[7u8; 4][..]);
        assert_eq!((m.qos, m.retain, m.truncated_from), (1, true, None));
        let range = chunk.as_ptr_range();
        assert!(
            !range.contains(&m.payload.as_ptr()),
            "the stored payload must not pin the read chunk"
        );
    }

    fn profile(host: &str, protocol: Protocol, ws_path: &str) -> Profile {
        Profile {
            name: "t".into(),
            host: host.into(),
            port: 9001,
            protocol,
            mqtt_version: Default::default(),
            ws_path: ws_path.into(),
            client_id: Some("id".into()),
            username: None,
            password: PasswordSource::None,
            tls: TlsConfig::default(),
            subscriptions: vec![],
        }
    }

    fn addr(p: &Profile) -> String {
        mqtt_options(p, None).unwrap().broker_address().0
    }

    #[test]
    fn ws_url_adds_missing_leading_slash() {
        assert_eq!(
            addr(&profile("host", Protocol::Ws, "mqtt")),
            "ws://host:9001/mqtt"
        );
        assert_eq!(
            addr(&profile("host", Protocol::Ws, "/mqtt")),
            "ws://host:9001/mqtt"
        );
        let o = mqtt_options(&profile("host", Protocol::Ws, "/"), None).unwrap();
        assert!(matches!(o.transport(), Transport::Ws));
    }

    #[test]
    fn wss_selects_wss_transport() {
        let mut p = profile("host", Protocol::Wss, "/mqtt");
        p.tls.insecure = true; // avoid depending on system roots
        let o = mqtt_options(&p, None).unwrap();
        assert_eq!(o.broker_address().0, "wss://host:9001/mqtt");
        assert!(matches!(o.transport(), Transport::Wss(_)));
    }

    #[test]
    fn ipv6_host_bracketed_once() {
        assert_eq!(addr(&profile("::1", Protocol::Ws, "/")), "ws://[::1]:9001/");
        assert_eq!(
            addr(&profile("[::1]", Protocol::Ws, "/")),
            "ws://[::1]:9001/"
        );
    }

    #[test]
    fn plain_and_tls_use_bare_host() {
        let o = mqtt_options(&profile("h", Protocol::Mqtt, "/"), None).unwrap();
        assert_eq!(o.broker_address(), ("h".into(), 9001));
        assert!(matches!(o.transport(), Transport::Tcp));
        let mut p = profile("h", Protocol::Mqtts, "/");
        p.tls.insecure = true;
        let o = mqtt_options(&p, None).unwrap();
        assert!(matches!(o.transport(), Transport::Tls(_)));
    }
}
