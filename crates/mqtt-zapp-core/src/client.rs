//! The broker connection: builds rumqttc options from a `Profile` (plain, TLS,
//! WebSocket), runs the event loop in a background task with reconnect backoff,
//! and reports everything as UI-agnostic `CoreEvent`s. Broker errors never panic
//! or end the task; it stops only on `disconnect()` or when the receiver drops.

use crate::config::{MqttVersion, Profile, Protocol, TlsConfig};
use crate::payload::{Message, Props};
use crate::sparkplug;
use anyhow::{Context, Result, anyhow, bail};
use rumqttc::tokio_rustls::rustls;
use rumqttc::v5;
use rumqttc::{
    AsyncClient, Event, EventLoop, MqttOptions, Outgoing, Packet, QoS, SubscribeFilter,
    TlsConfiguration, Transport,
};
use rustls::pki_types::pem::{self, PemObject};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
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
    async fn publish(
        &self,
        topic: &str,
        qos: u8,
        retain: bool,
        payload: Vec<u8>,
        props: Option<&Props>,
    ) -> Result<()> {
        match self {
            Client::V4(c) => {
                let qos = rumqttc::qos(qos).map_err(|e| anyhow!("invalid qos {qos}: {e}"))?;
                Ok(c.publish(topic, qos, retain, payload).await?)
            }
            Client::V5(c) => {
                let qos = v5::mqttbytes::qos(qos).ok_or_else(|| anyhow!("invalid qos {qos}"))?;
                match props {
                    Some(p) => {
                        let props = v5::mqttbytes::v5::PublishProperties {
                            content_type: p.content_type.clone(),
                            response_topic: p.response_topic.clone(),
                            correlation_data: p.correlation_data.clone(),
                            user_properties: p.user.clone(),
                            ..Default::default()
                        };
                        Ok(
                            c.publish_with_properties(topic, qos, retain, payload, props)
                                .await?,
                        )
                    }
                    None => Ok(c.publish(topic, qos, retain, payload).await?),
                }
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

/// Largest payload decoded as Sparkplug: the JSON form is many times larger
/// than the protobuf, and decoding happens before the payload cap.
const SPARKPLUG_MAX: usize = 1 << 20;

/// A received publish as a `Message`, its payload copied once.
///
/// rumqttc reads packets into a shared `BytesMut` (10 KiB or more) and hands
/// out each payload as a frozen slice of it, so keeping that `Bytes` would
/// pin the whole read chunk: a 4-byte `latest` could hold megabytes alive
/// and `max_history_mb` would no longer bound real memory. The copy is the
/// only one; `latest` and history share it.
///
/// A Sparkplug B payload (by topic) is stored as its JSON form, so the
/// payload cap (applied in `App::apply_core_event_inner`) covers that; one
/// that does not decode, or exceeds `SPARKPLUG_MAX`, stays raw.
fn incoming(
    topic: &str,
    payload: &[u8],
    qos: u8,
    retain: bool,
    props: Option<Arc<Props>>,
    aliases: &mut sparkplug::Aliases,
) -> Message {
    let decoded = (sparkplug::is_sparkplug_topic(topic) && payload.len() <= SPARKPLUG_MAX)
        .then(|| aliases.decode(topic, payload))
        .flatten()
        .and_then(|v| serde_json::to_vec(&v).ok());
    let decoded_len = decoded.is_some().then_some(payload.len());
    Message {
        sparkplug: decoded.is_some(),
        payload: decoded.map_or_else(
            || bytes::Bytes::copy_from_slice(payload),
            bytes::Bytes::from,
        ),
        qos,
        retain,
        received_at: SystemTime::now(),
        truncated_from: None,
        props,
        wire_len: decoded_len,
    }
}

/// The properties the header shows, taken from a v5 publish.
fn props_from_v5(p: v5::mqttbytes::v5::PublishProperties) -> Props {
    Props {
        content_type: p.content_type,
        response_topic: p.response_topic,
        correlation_data: p.correlation_data,
        message_expiry: p.message_expiry_interval,
        payload_utf8: (p.payload_format_indicator == Some(1)).then_some(true),
        user: p.user_properties,
        dropped_user: 0,
    }
    .capped()
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

    async fn poll(&mut self, aliases: &mut sparkplug::Aliases) -> Polled {
        match self {
            Loop::V4(l) => match l.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => Polled::ConnAck,
                Ok(Event::Incoming(Packet::Publish(p))) => Polled::Publish {
                    msg: incoming(&p.topic, &p.payload, p.qos as u8, p.retain, None, aliases),
                    topic: p.topic,
                },
                Ok(Event::Outgoing(Outgoing::Disconnect)) => Polled::OutgoingDisconnect,
                Ok(_) => Polled::Other,
                Err(e) => Polled::Err(e.to_string()),
            },
            Loop::V5(l) => match l.poll().await {
                Ok(v5::Event::Incoming(v5::Incoming::ConnAck(_))) => Polled::ConnAck,
                Ok(v5::Event::Incoming(v5::Incoming::Publish(p))) => {
                    let topic = String::from_utf8_lossy(&p.topic).into_owned();
                    Polled::Publish {
                        msg: incoming(
                            &topic,
                            &p.payload,
                            p.qos as u8,
                            p.retain,
                            p.properties
                                .map(props_from_v5)
                                .filter(|p| !p.is_empty())
                                .map(Arc::new),
                            aliases,
                        ),
                        topic,
                    }
                }
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

    /// `props` (content type, response topic, correlation data, user
    /// properties) are sent on MQTT 5 and ignored on 3.1.1.
    pub async fn publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
        props: Option<&Props>,
    ) -> Result<()> {
        // The request queue only drains while connected; don't hang the caller
        // for a whole outage.
        tokio::time::timeout(
            PUBLISH_TIMEOUT,
            self.client.publish(topic, qos, retain, payload, props),
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
            self.publish(t, Vec::new(), CLEAR_QOS, true, None)
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

/// Report the messages dropped so far if the UI channel has room; a full
/// (or closed) channel keeps the count pending for the next try.
fn flush_dropped(tx: &mpsc::Sender<CoreEvent>, dropped: &mut u64) {
    if *dropped > 0 && tx.try_send(CoreEvent::Dropped(*dropped)).is_ok() {
        *dropped = 0;
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
    // Kept across reconnects; the broker's retained BIRTHs come again anyway.
    let mut aliases = sparkplug::Aliases::default();
    emit!(CoreEvent::State(ConnState::Connecting));
    loop {
        let polled = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tx.closed() => return,
            r = eventloop.poll(&mut aliases) => r,
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
                flush_dropped(&tx, &mut dropped);
                match tx.try_send(CoreEvent::Message { topic, msg }) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => dropped += 1,
                    Err(mpsc::error::TrySendError::Closed(_)) => return,
                }
            }
            // Pings and acks: a quiet broker still gets the count out.
            Polled::Other => flush_dropped(&tx, &mut dropped),
            Polled::OutgoingDisconnect => {}
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
                match eventloop.poll(&mut aliases).await {
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
    // An explicit provider: rustls 0.23 would otherwise need a process-wide
    // default, and we build only ring (see Cargo.toml).
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .context("TLS protocol versions")?;
    let builder = if tls.insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(insecure::AcceptAnyCert::new(&provider)))
    } else {
        builder.with_root_certificates(root_store(tls)?)
    };
    let config = match (&tls.client_cert, &tls.client_key) {
        (None, None) => builder.with_no_client_auth(),
        (Some(cert), Some(key)) => {
            let certs = read_pem(cert, |b| {
                CertificateDer::pem_slice_iter(b).collect::<Result<Vec<_>, _>>()
            })?;
            let key = read_pem(key, |b| match PrivateKeyDer::from_pem_slice(b) {
                Ok(k) => Ok(Some(k)),
                Err(pem::Error::NoItemsFound) => Ok(None),
                Err(e) => Err(e),
            })?
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
    match &tls.ca_file {
        Some(path) => {
            let certs = read_pem(path, |b| {
                CertificateDer::pem_slice_iter(b).collect::<Result<Vec<_>, _>>()
            })?;
            roots.add_parsable_certificates(certs);
            if roots.is_empty() {
                bail!("no usable CA certificates in {}", path.display());
            }
        }
        None => {
            // 0.8 returns what it could load plus per-source errors; a few
            // unreadable stores are fine as long as something loaded.
            let native = rustls_native_certs::load_native_certs();
            roots.add_parsable_certificates(native.certs);
            if roots.is_empty() {
                if native.errors.is_empty() {
                    bail!("no usable system CA certificates found");
                }
                let errors: Vec<String> = native.errors.iter().map(|e| e.to_string()).collect();
                bail!(
                    "no usable system CA certificates found: {}",
                    errors.join("; ")
                );
            }
        }
    }
    Ok(roots)
}

fn read_pem<T>(
    path: &std::path::Path,
    parse: impl FnOnce(&[u8]) -> std::result::Result<T, pem::Error>,
) -> Result<T> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&bytes).with_context(|| format!("parsing PEM in {}", path.display()))
}

mod insecure {
    //! `tls.insecure = true`: accept any server certificate, but still check
    //! handshake signatures so the session itself is sound.

    use super::rustls;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{
        CryptoProvider, WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature,
    };
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{DigitallySignedStruct, Error, SignatureScheme};

    #[derive(Debug)]
    pub(super) struct AcceptAnyCert(WebPkiSupportedAlgorithms);

    impl AcceptAnyCert {
        /// Verifies handshake signatures with `provider`'s algorithms.
        pub(super) fn new(provider: &CryptoProvider) -> Self {
            Self(provider.signature_verification_algorithms)
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
    fn flush_dropped_waits_for_room_then_resets() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut dropped = 0;
        flush_dropped(&tx, &mut dropped);
        assert!(rx.try_recv().is_err(), "nothing to report");
        tx.try_send(CoreEvent::Dropped(0)).unwrap(); // fill the channel
        dropped = 5;
        flush_dropped(&tx, &mut dropped);
        assert_eq!(dropped, 5, "a full channel keeps the count pending");
        rx.try_recv().unwrap();
        flush_dropped(&tx, &mut dropped);
        assert_eq!(dropped, 0);
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::Dropped(5))));
    }

    #[test]
    fn incoming_payload_does_not_alias_the_read_buffer() {
        // rumqttc hands out the payload as a slice of its shared read chunk.
        let chunk = bytes::Bytes::from(vec![7u8; 64 * 1024]);
        let p = chunk.slice(100..104);
        let m = incoming("t", &p, 1, true, None, &mut Default::default());
        assert_eq!(&m.payload[..], &[7u8; 4][..]);
        assert_eq!((m.qos, m.retain, m.truncated_from), (1, true, None));
        let range = chunk.as_ptr_range();
        assert!(
            !range.contains(&m.payload.as_ptr()),
            "the stored payload must not pin the read chunk"
        );
    }

    #[test]
    fn sparkplug_payload_is_stored_as_json() {
        let p = [0x08, 0xe8, 0x07, 0x18, 0x03]; // timestamp 1000, seq 3
        let m = incoming(
            "spBv1.0/g/NDATA/n",
            &p,
            0,
            false,
            None,
            &mut Default::default(),
        );
        assert!(m.sparkplug);
        assert_eq!(&m.payload[..], br#"{"timestamp":1000,"seq":3}"#);
        let m = incoming(
            "spBv1.0/g/NDATA/n",
            b"not protobuf {",
            0,
            false,
            None,
            &mut Default::default(),
        );
        assert!(!m.sparkplug);
        assert_eq!(&m.payload[..], b"not protobuf {");
    }

    #[test]
    fn incoming_records_wire_len() {
        let p = [0x08, 0xe8, 0x07, 0x18, 0x03];
        let m = incoming(
            "spBv1.0/g/NDATA/n",
            &p,
            0,
            false,
            None,
            &mut Default::default(),
        );
        assert_eq!(m.wire_len, Some(p.len()));
        let m = incoming("t", b"raw", 0, false, None, &mut Default::default());
        assert_eq!(m.wire_len, None);
    }

    #[test]
    fn oversize_sparkplug_payload_stays_raw() {
        // A valid payload: field 5 (body) of 1 MiB + 1 bytes after a timestamp.
        let mut p = vec![0x08, 0x01, 0x2a, 0xfb, 0xff, 0x3f];
        p.resize((1 << 20) + 1, 0);
        let m = incoming(
            "spBv1.0/g/NDATA/n",
            &p,
            0,
            false,
            None,
            &mut Default::default(),
        );
        assert!(!m.sparkplug);
        assert_eq!(m.payload.len(), (1 << 20) + 1);
    }

    #[test]
    fn state_topic_is_not_decoded() {
        let m = incoming(
            "spBv1.0/STATE/h",
            b"{\"online\":true}",
            0,
            true,
            None,
            &mut Default::default(),
        );
        assert!(!m.sparkplug);
        assert_eq!(&m.payload[..], b"{\"online\":true}");
    }

    #[test]
    fn v5_props_are_capped_at_ingestion() {
        let wire = v5::mqttbytes::v5::PublishProperties {
            user_properties: (0..300)
                .map(|i| (format!("k{i}"), "v".repeat(1024)))
                .collect(),
            ..Default::default()
        };
        let p = props_from_v5(wire);
        assert_eq!(p.user.len(), 32);
        assert_eq!(p.dropped_user, 268);
        assert!(p.user.iter().all(|(_, v)| v.len() <= 256));
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

    /// A self-signed cert and its key as PEM files in a temp dir.
    fn pem_files() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("c.pem");
        let key = dir.path().join("c.key");
        std::fs::write(&cert, ck.cert.pem()).unwrap();
        std::fs::write(&key, ck.signing_key.serialize_pem()).unwrap();
        (dir, cert, key)
    }

    fn tls_err(tls: &TlsConfig) -> String {
        format!("{:#}", tls_config(tls).unwrap_err())
    }

    #[test]
    fn pem_ca_file_and_client_auth_load() {
        let (_dir, cert, key) = pem_files();
        let tls = TlsConfig {
            ca_file: Some(cert.clone()),
            client_cert: Some(cert),
            client_key: Some(key),
            insecure: false,
        };
        let config = tls_config(&tls).unwrap();
        assert!(config.client_auth_cert_resolver.has_certs());
    }

    #[test]
    fn pem_errors_keep_their_messages() {
        let (dir, cert, key) = pem_files();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "not pem\n").unwrap();

        let tls = TlsConfig {
            ca_file: Some(empty.clone()),
            ..Default::default()
        };
        assert!(tls_err(&tls).starts_with("no usable CA certificates in"));

        let tls = TlsConfig {
            client_cert: Some(cert.clone()),
            client_key: Some(cert.clone()), // a cert, not a key
            insecure: true,
            ..Default::default()
        };
        assert!(tls_err(&tls).starts_with("no private key in"));

        for (c, k) in [(Some(cert), None), (None, Some(key))] {
            let tls = TlsConfig {
                client_cert: c,
                client_key: k,
                insecure: true,
                ..Default::default()
            };
            assert_eq!(
                tls_err(&tls),
                "TLS client auth needs both client_cert and client_key"
            );
        }

        let tls = TlsConfig {
            ca_file: Some(dir.path().join("missing.pem")),
            ..Default::default()
        };
        assert!(tls_err(&tls).starts_with("reading "));
    }
}
