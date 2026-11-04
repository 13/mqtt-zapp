//! Integration tests against a real mosquitto (skipped when it is not installed).

mod support;

use mqtt_zapp_core::client::{ConnState, Connection, CoreEvent};
use mqtt_zapp_core::config::{MqttVersion, PasswordSource, Profile, Protocol, TlsConfig};
use mqtt_zapp_core::{Bytes, payload::Props};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

fn profile(host: &str, port: u16, protocol: Protocol) -> Profile {
    Profile {
        name: "test".into(),
        host: host.into(),
        port,
        protocol,
        mqtt_version: Default::default(),
        ws_path: "/".into(),
        client_id: None,
        username: None,
        password: PasswordSource::None,
        tls: TlsConfig::default(),
        subscriptions: vec!["zapp/#".into()],
        retained: true,
        keepalive_s: None,
        clean_session: None,
        session_expiry_s: None,
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
    conn.publish(marker, b"ready".to_vec(), 1, false, None)
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
        .publish("zapp/t", b"hi".to_vec(), 1, false, None)
        .await
        .unwrap();

    let CoreEvent::Message { topic, msg } =
        wait_for(&mut rx_b, |e| is_message(e, "zapp/t"), 5).await
    else {
        unreachable!()
    };
    assert_eq!(topic, "zapp/t");
    assert_eq!(&msg.payload[..], b"hi");
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
        .publish("zapp/r", b"kept".to_vec(), 1, true, None)
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
    assert_eq!(&msg.payload[..], b"kept");
}

#[tokio::test]
async fn auth_ok_and_bad_password_reports_failed() {
    let Some(b) = support::start(true) else {
        return;
    };
    let mut p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    p.username = Some("zapp".into());

    let (_good, mut rx_good) = Connection::connect(&p, Some("zapp-pw".into())).unwrap();
    wait_for(&mut rx_good, is_connected, 5).await;

    let (_bad, mut rx_bad) = Connection::connect(&p, Some("wrong".into())).unwrap();
    let CoreEvent::State(ConnState::Failed { reason }) = wait_for(
        &mut rx_bad,
        |e| matches!(e, CoreEvent::State(ConnState::Failed { .. })),
        5,
    )
    .await
    else {
        unreachable!()
    };
    assert!(!reason.is_empty());
}

#[tokio::test]
async fn auth_refused_stops_retrying() {
    if std::process::Command::new("mosquitto_passwd")
        .arg("-h")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("SKIP: mosquitto_passwd not on PATH");
        return;
    }
    let Some(b) = support::start(true) else {
        return;
    };
    let p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    let mut p = p;
    p.username = Some("zapp".into());
    let (_conn, mut rx) = Connection::connect(&p, Some("wrong".into())).unwrap();
    let mut connecting = 0;
    let mut failed = 0;
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(ev) = rx.recv().await {
            match ev {
                CoreEvent::State(ConnState::Connecting) => connecting += 1,
                CoreEvent::State(ConnState::Failed { reason }) => {
                    assert!(!reason.is_empty());
                    failed += 1;
                }
                CoreEvent::State(ConnState::Connected) => panic!("must not connect"),
                CoreEvent::State(ConnState::Disconnected { .. }) => {
                    panic!("a refusal is final, not a retry")
                }
                _ => {}
            }
        }
    })
    .await;
    assert_eq!(failed, 1);
    assert_eq!(connecting, 1, "no second connection attempt");
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
    conn.publish("zapp/ws", b"over ws".to_vec(), 0, false, None)
        .await
        .unwrap();
    let CoreEvent::Message { msg, .. } = wait_for(&mut rx, |e| is_message(e, "zapp/ws"), 5).await
    else {
        unreachable!()
    };
    assert_eq!(&msg.payload[..], b"over ws");
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
async fn clear_while_broker_down_is_not_replayed_after_reconnect() {
    let ports = support::free_ports();
    let data = tempfile::tempdir().unwrap();
    let Some(b) = support::start_persistent_on(ports, data.path()) else {
        return;
    };
    let mut p = profile("127.0.0.1", ports.0, Protocol::Mqtt);
    p.subscriptions = vec!["x/#".into()];
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    wait_for(&mut rx, is_connected, 5).await;
    conn.publish("x/keep", b"v".to_vec(), 1, true, None)
        .await
        .unwrap();
    wait_for(&mut rx, |e| is_message(e, "x/keep"), 5).await;
    // Let the broker save its retained messages before it is killed.
    tokio::time::sleep(Duration::from_millis(2500)).await;

    // Freeze the broker: the client still looks connected, so the clear is
    // accepted and sent, but never acknowledged. Then the broker dies.
    b.pause();
    let _ = conn.clear_retained(&["x/keep".into()]).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(b);
    wait_for(&mut rx, is_disconnected, 5).await;

    let _b2 = support::start_persistent_on(ports, data.path()).unwrap();
    wait_for(&mut rx, is_connected, 10).await;
    // Give a replay time to happen if it were going to.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let (other, mut rx_other) = Connection::connect(&p, None).unwrap();
    wait_for(&mut rx_other, |e| is_message(e, "x/keep"), 5).await;
    other.disconnect();
}

fn v5(mut p: Profile) -> Profile {
    p.mqtt_version = MqttVersion::V5;
    p
}

#[tokio::test]
async fn receives_published_message_v5() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p5 = v5(profile("127.0.0.1", b.port, Protocol::Mqtt));
    let (conn_b, mut rx_b) = Connection::connect(&p5, None).unwrap();
    ready(&conn_b, &mut rx_b, "zapp/ready/b5").await;

    // A v3.1.1 publisher reaches the v5 subscriber, and the other way round.
    let p4 = profile("127.0.0.1", b.port, Protocol::Mqtt);
    let (conn_a, mut rx_a) = Connection::connect(&p4, None).unwrap();
    ready(&conn_a, &mut rx_a, "zapp/ready/a4").await;
    conn_a
        .publish("zapp/t4", b"from v4".to_vec(), 1, false, None)
        .await
        .unwrap();
    let CoreEvent::Message { msg, .. } = wait_for(&mut rx_b, |e| is_message(e, "zapp/t4"), 5).await
    else {
        unreachable!()
    };
    assert_eq!(&msg.payload[..], b"from v4");
    conn_b
        .publish("zapp/t5", b"from v5".to_vec(), 2, false, None)
        .await
        .unwrap();
    let CoreEvent::Message { msg, .. } = wait_for(&mut rx_a, |e| is_message(e, "zapp/t5"), 5).await
    else {
        unreachable!()
    };
    assert_eq!(&msg.payload[..], b"from v5");
}

#[tokio::test]
async fn v5_properties_arrive() {
    use rumqttc::v5::{AsyncClient, MqttOptions, mqttbytes::QoS, mqttbytes::v5::PublishProperties};
    let Some(b) = support::start(false) else {
        return;
    };
    let p5 = v5(profile("127.0.0.1", b.port, Protocol::Mqtt));
    let (conn, mut rx) = Connection::connect(&p5, None).unwrap();
    ready(&conn, &mut rx, "zapp/ready/props").await;

    let opts = MqttOptions::new("props-pub", "127.0.0.1", b.port);
    let (client, mut ev) = AsyncClient::new(opts, 10);
    let props = PublishProperties {
        content_type: Some("application/json".into()),
        user_properties: vec![("src".into(), "test".into())],
        ..Default::default()
    };
    client
        .publish_with_properties("zapp/props", QoS::AtLeastOnce, false, "{}", props)
        .await
        .unwrap();
    // Drive the publisher's event loop until the broker has acked.
    let drive = tokio::spawn(async move { while ev.poll().await.is_ok() {} });
    let CoreEvent::Message { msg, .. } =
        wait_for(&mut rx, |e| is_message(e, "zapp/props"), 5).await
    else {
        unreachable!()
    };
    drive.abort();
    let props = msg.props.expect("properties should arrive");
    assert_eq!(props.content_type.as_deref(), Some("application/json"));
    assert_eq!(props.user, [("src".to_owned(), "test".to_owned())]);
}

#[tokio::test]
async fn retained_flag_v5() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p5 = v5(profile("127.0.0.1", b.port, Protocol::Mqtt));
    let (conn_a, mut rx_a) = Connection::connect(&p5, None).unwrap();
    ready(&conn_a, &mut rx_a, "zapp/ready/r5").await;
    conn_a
        .publish("zapp/kept5", b"r".to_vec(), 1, true, None)
        .await
        .unwrap();
    wait_for(&mut rx_a, |e| is_message(e, "zapp/kept5"), 5).await;
    let (_c, mut rx) = Connection::connect(&p5, None).unwrap();
    let CoreEvent::Message { msg, .. } =
        wait_for(&mut rx, |e| is_message(e, "zapp/kept5"), 5).await
    else {
        unreachable!()
    };
    assert!(msg.retain);
}

#[tokio::test]
async fn v5_retained_false_skips_retained() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p5 = v5(profile("127.0.0.1", b.port, Protocol::Mqtt));
    let (conn_a, mut rx_a) = Connection::connect(&p5, None).unwrap();
    ready(&conn_a, &mut rx_a, "zapp/ready/nr").await;
    conn_a
        .publish("zapp/old", b"r".to_vec(), 1, true, None)
        .await
        .unwrap();
    wait_for(&mut rx_a, |e| is_message(e, "zapp/old"), 5).await;

    let mut quiet = p5.clone();
    quiet.retained = false;
    let (conn_b, mut rx_b) = Connection::connect(&quiet, None).unwrap();
    wait_for(&mut rx_b, is_connected, 5).await;
    // A live publish from the same client is handled after its SUBSCRIBE, so
    // when it arrives the retained message would already have been sent.
    conn_b
        .publish("zapp/live", b"l".to_vec(), 1, false, None)
        .await
        .unwrap();
    // The first message must be the live one: the retained one, if it were
    // sent, would come before it.
    let CoreEvent::Message { topic, .. } =
        wait_for(&mut rx_b, |e| matches!(e, CoreEvent::Message { .. }), 5).await
    else {
        unreachable!()
    };
    assert_eq!(topic, "zapp/live", "retained message was delivered");
}

/// Only MQTT 5 can keep the retain flag on a live (not on-subscribe)
/// delivery ("retain as published"), so this tells the versions apart.
#[tokio::test]
async fn live_retained_publish_keeps_retain_flag_only_on_v5() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p4 = profile("127.0.0.1", b.port, Protocol::Mqtt);
    let p5 = v5(p4.clone());
    let (c4, mut rx4) = Connection::connect(&p4, None).unwrap();
    ready(&c4, &mut rx4, "zapp/ready/l4").await;
    let (c5, mut rx5) = Connection::connect(&p5, None).unwrap();
    ready(&c5, &mut rx5, "zapp/ready/l5").await;
    c4.publish("zapp/live", b"r".to_vec(), 1, true, None)
        .await
        .unwrap();
    let retain_of = |ev| match ev {
        CoreEvent::Message { msg, .. } => msg.retain,
        _ => unreachable!(),
    };
    assert!(retain_of(
        wait_for(&mut rx5, |e| is_message(e, "zapp/live"), 5).await
    ));
    assert!(!retain_of(
        wait_for(&mut rx4, |e| is_message(e, "zapp/live"), 5).await
    ));
}

#[tokio::test]
async fn websocket_v5() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p = v5(profile("127.0.0.1", b.ws_port, Protocol::Ws));
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    ready(&conn, &mut rx, "zapp/ready/ws5").await;
}

#[tokio::test]
async fn reconnects_after_broker_restart_v5() {
    let Some(b) = support::start(false) else {
        return;
    };
    let ports = (b.port, b.ws_port, b.tls_port);
    let p = v5(profile("127.0.0.1", b.port, Protocol::Mqtt));
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    wait_for(&mut rx, is_connected, 5).await;
    drop(b);
    wait_for(&mut rx, is_disconnected, 5).await;
    let _b2 = support::start_on(false, ports).unwrap();
    ready(&conn, &mut rx, "zapp/ready/again5").await;
}

#[tokio::test]
async fn auth_bad_password_v5_reports_failed() {
    let Some(b) = support::start(true) else {
        return;
    };
    let mut p = v5(profile("127.0.0.1", b.port, Protocol::Mqtt));
    p.username = Some("zapp".into());
    let (_good, mut rx_good) = Connection::connect(&p, Some("zapp-pw".into())).unwrap();
    wait_for(&mut rx_good, is_connected, 5).await;
    let (_c, mut rx) = Connection::connect(&p, Some("wrong".into())).unwrap();
    wait_for(
        &mut rx,
        |e| matches!(e, CoreEvent::State(ConnState::Failed { .. })),
        5,
    )
    .await;
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
            let _ = c.publish(&format!("q/{i}"), vec![], 0, false, None).await;
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
        .publish("zapp/kept", b"x".to_vec(), 1, true, None)
        .await
        .unwrap();
    wait_for(&mut rx_other, |e| is_message(e, "zapp/kept"), 5).await;

    wait_for(&mut rx, is_connected, 5).await;
    wait_for(&mut rx, |e| is_message(e, "zapp/kept"), 10).await;
}

#[tokio::test]
async fn clear_retained_removes_each_topic_on_the_broker() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    ready(&conn, &mut rx, "zapp/ready/c").await;
    for t in ["zapp/c/1", "zapp/c/2", "zapp/c/keep"] {
        conn.publish(t, b"v".to_vec(), 1, true, None).await.unwrap();
        wait_for(&mut rx, |e| is_message(e, t), 5).await;
    }
    let topics = vec!["zapp/c/1".to_string(), "zapp/c/2".to_string()];
    assert_eq!(conn.clear_retained(&topics).await.unwrap(), 2);
    // The echoes are empty payloads.
    for t in &topics {
        let CoreEvent::Message { msg, .. } = wait_for(&mut rx, |e| is_message(e, t), 5).await
        else {
            unreachable!()
        };
        assert!(msg.payload.is_empty());
    }
    // A fresh subscriber only gets the kept one, then our marker.
    let (conn_b, mut rx_b) = Connection::connect(&p, None).unwrap();
    wait_for(&mut rx_b, is_connected, 5).await;
    conn_b
        .publish("zapp/ready/d", b"x".to_vec(), 1, false, None)
        .await
        .unwrap();
    let mut seen = vec![];
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(ev) = rx_b.recv().await {
            if let CoreEvent::Message { topic, .. } = ev {
                if topic == "zapp/ready/d" {
                    break;
                }
                seen.push(topic);
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(seen, ["zapp/c/keep"]);
}

/// Drain `rx` until it has been quiet for 500 ms.
async fn drain_idle(rx: &mut Receiver<CoreEvent>) -> Vec<CoreEvent> {
    let mut out = Vec::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        out.push(ev);
    }
    out
}

#[tokio::test]
async fn slow_consumer_stays_connected() {
    const N: usize = 20_000;
    let Some(b) = support::start(false) else {
        return;
    };
    let mut slow = profile("127.0.0.1", b.port, Protocol::Mqtt);
    slow.client_id = Some("zapp-slow".into());
    slow.subscriptions = vec!["x/#".into()];
    let (slow_conn, mut rx) = Connection::connect(&slow, None).unwrap();
    ready(&slow_conn, &mut rx, "x/ready/slow").await;

    let mut fast = profile("127.0.0.1", b.port, Protocol::Mqtt);
    fast.client_id = Some("zapp-flood".into());
    let (flood, mut rx_flood) = Connection::connect(&fast, None).unwrap();
    wait_for(&mut rx_flood, is_connected, 5).await;
    // `rx` is not read while the flood is published and delivered.
    for i in 0..N {
        flood
            .publish(&format!("x/flood/{i}"), vec![0], 0, false, None)
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_secs(3)).await;

    let mut events = drain_idle(&mut rx).await;
    // The count still pending goes out with the next poll of any kind, not
    // only the next delivered message: publish to a topic the slow client
    // does not subscribe to, so only its own outgoing publish and PUBACK
    // wake the loop.
    slow_conn
        .publish("y/nudge", vec![1], 1, false, None)
        .await
        .unwrap();
    let count = |events: &[CoreEvent]| {
        let mut delivered = 0;
        let mut dropped = 0;
        for ev in events {
            match ev {
                CoreEvent::Message { topic, .. } if topic.starts_with("x/flood/") => delivered += 1,
                CoreEvent::Dropped(n) => dropped += *n as usize,
                _ => {}
            }
        }
        (delivered, dropped)
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while count(&events).0 + count(&events).1 < N {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(ev)) => events.push(ev),
            Ok(None) => panic!("event channel closed"),
            Err(_) => break,
        }
    }

    let (delivered, dropped) = count(&events);
    assert!(
        dropped > 0,
        "the flood should overflow the channel and the count arrive \
         ({delivered} delivered of {N})"
    );
    assert_eq!(
        delivered + dropped,
        N,
        "every message accounted for once, without a later message"
    );
    assert!(
        !events.iter().any(is_disconnected),
        "a slow consumer must not be disconnected"
    );
}

/// mosquitto 2 logs "Received DISCONNECT from <id>" and then
/// "Client <id> [addr] disconnected." for a clean close, and
/// "Client <id> closed its connection." for a dropped socket.
fn clean_disconnect(log: &str) -> bool {
    log.contains("Received DISCONNECT from zapp-close-test")
        && log
            .lines()
            .any(|l| l.contains("Client zapp-close-test") && l.ends_with("disconnected."))
}

#[tokio::test]
async fn close_waits_for_disconnect() {
    let Some(b) = support::start(false) else {
        return;
    };
    let mut p = profile("127.0.0.1", b.port, Protocol::Mqtt);
    p.client_id = Some("zapp-close-test".into());
    let (conn, mut rx) = Connection::connect(&p, None).unwrap();
    ready(&conn, &mut rx, "zapp/ready/close").await;

    conn.close(Duration::from_secs(2)).await;
    assert!(conn.is_finished(), "close returned before the task ended");

    // The broker logs the client's close after it has read our DISCONNECT.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let log = loop {
        let log = b.log();
        if clean_disconnect(&log) || std::time::Instant::now() > deadline {
            break log;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        clean_disconnect(&log),
        "no clean disconnect in broker log:\n{log}"
    );
    assert!(
        !log.contains("closed its connection"),
        "broker saw a dropped connection:\n{log}"
    );
}

#[tokio::test]
async fn v5_publish_sends_properties() {
    let Some(b) = support::start(false) else {
        return;
    };
    let p5 = v5(profile("127.0.0.1", b.port, Protocol::Mqtt));
    let (sub, mut rx) = Connection::connect(&p5, None).unwrap();
    ready(&sub, &mut rx, "zapp/ready/pp-sub").await;
    let (publ, mut rx_p) = Connection::connect(&p5, None).unwrap();
    ready(&publ, &mut rx_p, "zapp/ready/pp-pub").await;

    let props = Props {
        content_type: Some("text/plain".into()),
        response_topic: Some("zapp/reply".into()),
        correlation_data: Some(Bytes::from_static(b"cid")),
        user: vec![("k".into(), "v".into())],
        ..Default::default()
    };
    publ.publish("zapp/pp", b"x".to_vec(), 1, false, Some(&props))
        .await
        .unwrap();
    let CoreEvent::Message { msg, .. } = wait_for(&mut rx, |e| is_message(e, "zapp/pp"), 5).await
    else {
        unreachable!()
    };
    let got = msg.props.expect("properties should arrive");
    assert_eq!(got.content_type.as_deref(), Some("text/plain"));
    assert_eq!(got.response_topic.as_deref(), Some("zapp/reply"));
    assert_eq!(got.correlation_data.as_deref(), Some(&b"cid"[..]));
    assert_eq!(got.user, [("k".to_owned(), "v".to_owned())]);
}
