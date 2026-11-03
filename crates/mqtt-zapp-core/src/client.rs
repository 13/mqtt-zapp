//! The broker connection: builds rumqttc options from a `Profile` (plain, TLS,
//! WebSocket), runs the event loop in a background task with reconnect backoff,
//! and reports everything as UI-agnostic `CoreEvent`s. Broker errors never panic
//! or end the task; it stops only on `disconnect()` or when the receiver drops.

use crate::config::{Profile, Protocol, TlsConfig};
use crate::payload::Message;
use anyhow::{Context, Result, anyhow, bail};
use rumqttc::tokio_rustls::rustls;
use rumqttc::{
    AsyncClient, Event, EventLoop, MqttOptions, Outgoing, Packet, QoS, SubscribeFilter,
    TlsConfiguration, Transport,
};
use serde::Serialize;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const CHANNEL_CAPACITY: usize = 10_000;
const REQUEST_CAPACITY: usize = 100;
const BACKOFF_START_MS: u64 = 1_000;
const BACKOFF_MAX_MS: u64 = 30_000;
/// Largest packet MQTT 3.1.1 can encode; rumqttc's 10 KiB default would drop
/// the connection on big payloads.
const MAX_PACKET: usize = 268_435_455;

#[derive(Debug, Clone, Serialize)]
pub enum ConnState {
    Connecting,
    Connected,
    Disconnected { reason: String, retry_in_ms: u64 },
}

#[derive(Debug, Clone, Serialize)]
pub enum CoreEvent {
    State(ConnState),
    Message { topic: String, msg: Message },
}

#[derive(Clone)]
pub struct Connection {
    client: AsyncClient,
    cancel: CancellationToken,
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
        let opts = mqtt_options(profile, password)?;
        let (client, eventloop) = AsyncClient::new(opts, REQUEST_CAPACITY);
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        let cancel = CancellationToken::new();
        handle.spawn(run(
            eventloop,
            client.clone(),
            profile.subscriptions.clone(),
            tx,
            cancel.clone(),
        ));
        Ok((Connection { client, cancel }, rx))
    }

    pub async fn publish(
        &self,
        topic: &str,
        payload: Vec<u8>,
        qos: u8,
        retain: bool,
    ) -> Result<()> {
        let qos = rumqttc::qos(qos).map_err(|e| anyhow!("invalid qos {qos}: {e}"))?;
        self.client
            .publish(topic, qos, retain, payload)
            .await
            .context("publish failed")
    }

    /// Send DISCONNECT (best effort) and stop the background task.
    pub fn disconnect(&self) {
        let _ = self.client.try_disconnect();
        self.cancel.cancel();
    }
}

async fn run(
    mut eventloop: EventLoop,
    client: AsyncClient,
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
    emit!(CoreEvent::State(ConnState::Connecting));
    loop {
        let polled = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tx.closed() => return,
            r = eventloop.poll() => r,
        };
        match polled {
            Ok(Event::Incoming(Packet::ConnAck(_))) => {
                connected = true;
                backoff = BACKOFF_START_MS;
                if !subscriptions.is_empty() {
                    let filters = subscriptions
                        .iter()
                        .map(|t| SubscribeFilter::new(t.clone(), QoS::AtMostOnce));
                    // Non-blocking: awaiting here could deadlock, since only
                    // this loop drains the request channel.
                    let _ = client.try_subscribe_many(filters);
                }
                emit!(CoreEvent::State(ConnState::Connected));
            }
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let msg = Message {
                    payload: p.payload.to_vec(),
                    qos: p.qos as u8,
                    retain: p.retain,
                    received_at: SystemTime::now(),
                };
                emit!(CoreEvent::Message {
                    topic: p.topic,
                    msg
                });
            }
            Ok(_) => {}
            Err(e) => {
                connected = false;
                emit!(CoreEvent::State(ConnState::Disconnected {
                    reason: e.to_string(),
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
            while let Ok(ev) = eventloop.poll().await {
                if matches!(ev, Event::Outgoing(Outgoing::Disconnect)) {
                    break;
                }
            }
        })
        .await;
    }
}

pub(crate) fn mqtt_options(profile: &Profile, password: Option<String>) -> Result<MqttOptions> {
    let id = profile.effective_client_id();
    let host = &profile.host;
    let port = profile.port;
    let mut opts = match profile.protocol {
        Protocol::Mqtt | Protocol::Mqtts => MqttOptions::new(id, host, port),
        Protocol::Ws | Protocol::Wss => {
            let scheme = if profile.protocol.is_tls() {
                "wss"
            } else {
                "ws"
            };
            let host = if host.contains(':') {
                format!("[{host}]") // IPv6 literal
            } else {
                host.clone()
            };
            let path = &profile.ws_path;
            let slash = if path.starts_with('/') { "" } else { "/" };
            MqttOptions::new(id, format!("{scheme}://{host}:{port}{slash}{path}"), port)
        }
    };
    opts.set_max_packet_size(MAX_PACKET, MAX_PACKET);
    if let Some(user) = &profile.username {
        opts.set_credentials(user, password.unwrap_or_default());
    }
    let transport = match profile.protocol {
        Protocol::Mqtt => Transport::Tcp,
        Protocol::Ws => Transport::Ws,
        Protocol::Mqtts => Transport::Tls(TlsConfiguration::Rustls(tls_config(&profile.tls)?)),
        Protocol::Wss => Transport::Wss(TlsConfiguration::Rustls(tls_config(&profile.tls)?)),
    };
    opts.set_transport(transport);
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
