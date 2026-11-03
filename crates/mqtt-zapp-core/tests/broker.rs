//! Integration tests against a real mosquitto (skipped when it is not installed).

mod support;

use mqtt_zapp_core::client::{ConnState, Connection, CoreEvent};
use mqtt_zapp_core::config::{PasswordSource, Profile, Protocol, TlsConfig};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

fn profile(host: &str, port: u16, protocol: Protocol) -> Profile {
    Profile {
        name: "test".into(),
        host: host.into(),
        port,
        protocol,
        ws_path: "/".into(),
        client_id: None,
        username: None,
        password: PasswordSource::None,
        tls: TlsConfig::default(),
        subscriptions: vec!["zapp/#".into()],
    }
}

/// Next event matching `pred`; panics after `secs` or if the channel closes.
async fn wait_for(
    rx: &mut Receiver<CoreEvent>,
    pred: impl Fn(&CoreEvent) -> bool,
    secs: u64,
) -> CoreEvent {
    tokio::time::timeout(Duration::from_secs(secs), async {
        loop {
            match rx.recv().await {
                Some(ev) if pred(&ev) => return ev,
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out after {secs}s waiting for event"))
}

fn is_connected(ev: &CoreEvent) -> bool {
    matches!(ev, CoreEvent::State(ConnState::Connected))
}

fn is_disconnected(ev: &CoreEvent) -> bool {
    matches!(ev, CoreEvent::State(ConnState::Disconnected { .. }))
}

fn is_message(ev: &CoreEvent, topic: &str) -> bool {
    matches!(ev, CoreEvent::Message { topic: t, .. } if t == topic)
}

/// Wait for `Connected`, then prove the subscription is live by receiving our
/// own publish: the broker handles SUBSCRIBE before a later PUBLISH from the
/// same client, so no sleeps are needed.
async fn ready(conn: &Connection, rx: &mut Receiver<CoreEvent>, marker: &str) {
    wait_for(rx, is_connected, 5).await;
    conn.publish(marker, b"ready".to_vec(), 1, false)
        .await
        .unwrap();
    wait_for(rx, |e| is_message(e, marker), 5).await;
}

#[tokio::test]
async fn receives_published_message_plain() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    let (conn_b, mut rx_b) = Connection::connect(&p, None).unwrap();
    ready(&conn_b, &mut rx_b, "zapp/ready/b").await;

    let (conn_a, mut rx_a) = Connection::connect(&p, None).unwrap();
    wait_for(&mut rx_a, is_connected, 5).await;
    conn_a
        .publish("zapp/t", b"hi".to_vec(), 1, false)
        .await
        .unwrap();

    let CoreEvent::Message { topic, msg } =
        wait_for(&mut rx_b, |e| is_message(e, "zapp/t"), 5).await
    else {
        unreachable!()
    };
    assert_eq!(topic, "zapp/t");
    assert_eq!(msg.payload, b"hi");
}

#[tokio::test]
async fn retained_message_delivered_on_subscribe_with_retain_flag() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    let (conn_a, mut rx_a) = Connection::connect(&p, None).unwrap();
    ready(&conn_a, &mut rx_a, "zapp/ready/a").await;
    conn_a
        .publish("zapp/r", b"kept".to_vec(), 1, true)
        .await
        .unwrap();
    // Seeing it come back means the broker has stored it.
    wait_for(&mut rx_a, |e| is_message(e, "zapp/r"), 5).await;

    let (_conn_b, mut rx_b) = Connection::connect(&p, None).unwrap();
    let CoreEvent::Message { msg, .. } = wait_for(&mut rx_b, |e| is_message(e, "zapp/r"), 5).await
    else {
        unreachable!()
    };
    assert!(msg.retain);
    assert_eq!(msg.payload, b"kept");
}

#[tokio::test]
async fn auth_ok_and_bad_password_reports_disconnected() {
    let Some(b) = support::start(true) else {
        return;
    };
    let mut p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    p.username = Some("zapp".into());

    let (_good, mut rx_good) = Connection::connect(&p, Some("zapp-pw".into())).unwrap();
    wait_for(&mut rx_good, is_connected, 5).await;

    let (_bad, mut rx_bad) = Connection::connect(&p, Some("wrong".into())).unwrap();
    let CoreEvent::State(ConnState::Disconnected {
        reason,
        retry_in_ms,
    }) = wait_for(&mut rx_bad, is_disconnected, 5).await
    else {
        unreachable!()
    };
    assert!(!reason.is_empty());
    assert_eq!(retry_in_ms, 1000);
    // Still alive: it retries after the backoff and reports again.
    let CoreEvent::State(ConnState::Disconnected { retry_in_ms, .. }) =
        wait_for(&mut rx_bad, is_disconnected, 5).await
    else {
        unreachable!()
    };
    assert_eq!(retry_in_ms, 2000);
}

#[tokio::test]
async fn tls_with_ca_file() {
    let Some(b) = support::start(false) else {
        return;
    };
    let mut p = profile("localhost", b.tls_port, Protocol::Mqtts);
    p.tls.ca_file = Some(b.ca_pem_path.clone());
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    ready(&conn, &mut rx, "zapp/ready/tls").await;
}

#[tokio::test]
async fn tls_insecure_without_ca() {
    let Some(b) = support::start(false) else {
        return;
    };
    let mut p = profile("localhost", b.tls_port, Protocol::Mqtts);
    p.tls.insecure = true;
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    ready(&conn, &mut rx, "zapp/ready/insecure").await;
}

#[tokio::test]
async fn websocket() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p = profile("127.0.0.1", b.ws_port, Protocol::Ws);
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    ready(&conn, &mut rx, "zapp/ready/ws").await;
    conn.publish("zapp/ws", b"over ws".to_vec(), 0, false)
        .await
        .unwrap();
    let CoreEvent::Message { msg, .. } = wait_for(&mut rx, |e| is_message(e, "zapp/ws"), 5).await
    else {
        unreachable!()
    };
    assert_eq!(msg.payload, b"over ws");
}

#[tokio::test]
async fn reconnects_after_broker_restart() {
    let Some(b) = support::start(false) else {
        return;
    };
    let ports = (b.port, b.ws_port, b.tls_port);
    let p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    wait_for(&mut rx, is_connected, 5).await;

    drop(b);
    wait_for(&mut rx, is_disconnected, 5).await;

    let _b2 = support::start_on(false, ports).unwrap();
    // The subscription is re-sent on reconnect, so messages flow again.
    ready(&conn, &mut rx, "zapp/ready/again").await;
}

#[tokio::test]
async fn subscribes_even_when_request_queue_is_full_at_connack() {
    // No broker yet: the first attempt fails and the client backs off for 1 s.
    let ports = support::free_ports();
    let p = profile("127.0.0.1", ports.0, Protocol::Mqtt);
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    wait_for(&mut rx, is_disconnected, 5).await;

    // Queue more publishes than the request channel holds, so it is still
    // full when the ConnAck arrives.
    for i in 0..150 {
        let c = conn.clone();
        tokio::spawn(async move {
            let _ = c.publish(&format!("q/{i}"), vec![], 0, false).await;
        });
    }
    tokio::task::yield_now().await;

    let Some(_b) = support::start_on(false, ports) else {
        return;
    };
    // A retained message reaches us whenever our SUBSCRIBE is processed,
    // however late, so this proves the subscribe was not dropped.
    let (other, mut rx_other) = Connection::connect(&p, None).unwrap();
    ready(&other, &mut rx_other, "zapp/ready/other").await;
    other
        .publish("zapp/kept", b"x".to_vec(), 1, true)
        .await
        .unwrap();
    wait_for(&mut rx_other, |e| is_message(e, "zapp/kept"), 5).await;

    wait_for(&mut rx, is_connected, 5).await;
    wait_for(&mut rx, |e| is_message(e, "zapp/kept"), 10).await;
}
