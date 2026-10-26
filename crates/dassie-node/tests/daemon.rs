//! The daemon end to end, in-process: setup, login, logout, sessions, the
//! RPC over both transports, the HTTP surface, certificates and restarts.
//! Each test that guards a fix of the original names the gap.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::os::unix::net::UnixDatagram;
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeKey;
use dassie_kit::DAY_MS;
use dassie_node::clock::ManualClock;
use dassie_node::daemon::DaemonError;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use futures_util::StreamExt;
use http_body_util::Full;
use hyper::{Request, StatusCode};
use support::*;
use tokio_tungstenite::tungstenite::Message;

fn key() -> NodeKey {
    NodeKey::from_bytes(&[7; 32])
}

#[tokio::test]
async fn setup_then_login_then_logout() {
    let node = TestNode::start().await;
    let mut anonymous = RpcClient::new(websocket(&node, None, &node.origin()).await.unwrap());
    assert_eq!(anonymous.state().await, proto::NodeState::Uninitialized);

    let cookie = set_up(&node, &key()).await;
    assert_eq!(anonymous.state().await, proto::NodeState::Anonymous);
    let mut owner = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    assert_eq!(owner.state().await, proto::NodeState::Authenticated);

    // Setup is gone once done.
    let token = "00".repeat(32);
    let again = send(
        node.https,
        true,
        post_json(&node, "/api/setup", &setup_body(&token, &key())),
    )
    .await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);

    let wrong = send(
        node.https,
        true,
        post_json(
            &node,
            "/api/login",
            &login_body(&NodeKey::from_bytes(&[8; 32])),
        ),
    )
    .await;
    assert_eq!(
        (wrong.status, wrong.text().as_str()),
        (
            StatusCode::UNAUTHORIZED,
            "Invalid login authorization signature"
        )
    );
    let login = send(
        node.https,
        true,
        post_json(&node, "/api/login", &login_body(&key())),
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);
    assert_ne!(login.cookie(), cookie);

    let logout = Request::post("/api/logout")
        .header("host", node.host())
        .header("origin", node.origin())
        .header("cookie", &cookie)
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = send(node.https, true, logout).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.header("set-cookie").unwrap().contains("Max-Age=0"));
    node.stop().await;
}

// RT-11: the original's cookie lasted ten years and its sessions forever.
#[tokio::test]
async fn cookies_carry_the_session_lifetime() {
    let node = TestNode::start().await;
    let token = node.setup_token().await;
    let reply = send(
        node.https,
        true,
        post_json(&node, "/api/setup", &setup_body(&token, &key())),
    )
    .await;
    let cookie = reply.header("set-cookie").unwrap();
    assert!(cookie.starts_with("__Host-Dassie-Session="));
    assert!(
        cookie.ends_with("; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=31536000"),
        "{cookie}"
    );
    node.stop().await;
}

// RT-6: in the original, logging out left open sockets authenticated.
#[tokio::test]
async fn logout_closes_the_sessions_sockets() {
    let node = TestNode::start().await;
    let cookie = set_up(&node, &key()).await;
    let mut socket = websocket(&node, Some(&cookie), &node.origin())
        .await
        .unwrap();
    let logout = Request::post("/api/logout")
        .header("host", node.host())
        .header("origin", node.origin())
        .header("cookie", &cookie)
        .body(Full::new(Bytes::new()))
        .unwrap();
    send(node.https, true, logout).await;
    let closed = loop {
        match socket.next().await {
            Some(Ok(Message::Close(frame))) => break frame,
            Some(Ok(_)) => {}
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(
        u16::from(closed.unwrap().code),
        dassie_node::http::SESSION_ENDED
    );
    let mut again = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    assert_eq!(again.state().await, proto::NodeState::Anonymous);
    node.stop().await;
}

// RT-11: sessions end after 30 days unused.
#[tokio::test]
async fn idle_sessions_expire() {
    let node = TestNode::start().await;
    let cookie = set_up(&node, &key()).await;
    node.clock.advance(29 * DAY_MS);
    let mut socket = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    assert_eq!(socket.state().await, proto::NodeState::Authenticated);
    node.clock.advance(30 * DAY_MS);
    // A socket opened with the expired cookie is anonymous...
    let mut fresh = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    assert_eq!(fresh.state().await, proto::NodeState::Anonymous);
    let denied = fresh
        .call(Method::GetConfig(proto::GetConfigRequest {}))
        .await
        .unwrap_err();
    assert_eq!(denied.code(), proto::ErrorCode::Unauthenticated);
    // ...and the one that had the session was closed when it ended.
    let mut socket = socket.into_inner();
    let closed = loop {
        match socket.next().await {
            Some(Ok(Message::Close(frame))) => break frame.map(|frame| u16::from(frame.code)),
            Some(Ok(_)) => {}
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(closed, Some(dassie_node::http::SESSION_ENDED));
    node.stop().await;
}

// RT-5: the original answered an unauthorized call with a *success* that
// carried a serialized failure.
#[tokio::test]
async fn protected_calls_fail_with_a_code() {
    let node = TestNode::start().await;
    let mut anonymous = RpcClient::new(websocket(&node, None, &node.origin()).await.unwrap());
    let error = anonymous
        .call(Method::GetConfig(proto::GetConfigRequest {}))
        .await
        .unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::Unauthenticated);
    let mut owner = node.ipc().await;
    let error = owner
        .call(Method::SetHostname(proto::SetHostnameRequest {
            hostname: "not a name".into(),
        }))
        .await;
    assert_eq!(error.unwrap_err().code(), proto::ErrorCode::InvalidArgument);
    let live = proto::SetRealmRequest {
        realm: proto::Realm::Live.into(),
    };
    assert_eq!(
        owner.call(Method::SetRealm(live)).await.unwrap_err().code(),
        proto::ErrorCode::FailedPrecondition
    );
    let unknown = proto::ClientMessage {
        call_id: 9,
        kind: Some(proto::client_message::Kind::Request(proto::Request {
            method: None,
        })),
    };
    owner.send_raw(unknown).await;
    let reply = owner.receive().await.unwrap();
    let Some(proto::server_message::Kind::Error(error)) = reply.kind else {
        panic!("{reply:?}")
    };
    assert_eq!(error.code(), proto::ErrorCode::Unimplemented);
    node.stop().await;
}

// RT-6: the original accepted requests from any origin.
#[tokio::test]
async fn foreign_origins_and_bad_bodies_are_refused() {
    let node = TestNode::start().await;
    let token = node.setup_token().await;
    let body = setup_body(&token, &key());
    let mut request = post_json(&node, "/api/setup", &body);
    request
        .headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    assert_eq!(
        send(node.https, true, request).await.status,
        StatusCode::FORBIDDEN
    );
    let mut request = post_json(&node, "/api/setup", &body);
    request
        .headers_mut()
        .insert("host", "evil.example".parse().unwrap());
    request
        .headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    assert_eq!(
        send(node.https, true, request).await.status,
        StatusCode::FORBIDDEN,
        "DNS rebinding"
    );
    let mut request = post_json(&node, "/api/setup", &body);
    request
        .headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    assert_eq!(
        send(node.https, true, request).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    let huge = format!(r#"{{"x":"{}"}}"#, "a".repeat(20_000));
    assert_eq!(
        send(node.https, true, post_json(&node, "/api/setup", &huge))
            .await
            .status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        send(node.https, true, post_json(&node, "/api/setup", "{"))
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    assert!(
        websocket(&node, None, "https://evil.example")
            .await
            .unwrap_err()
            .contains("403")
    );
    node.stop().await;
}

// RT-10: the original allowed unlimited attempts.
#[tokio::test]
async fn login_attempts_are_limited() {
    let node = TestNode::start().await;
    set_up(&node, &key()).await;
    let wrong = login_body(&NodeKey::from_bytes(&[9; 32]));
    let mut statuses = vec![];
    for _ in 0..5 {
        statuses.push(
            send(node.https, true, post_json(&node, "/api/login", &wrong))
                .await
                .status,
        );
    }
    // Setup took one attempt of the five.
    assert_eq!(statuses[..4], [StatusCode::UNAUTHORIZED; 4]);
    let limited = send(node.https, true, post_json(&node, "/api/login", &wrong)).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.header("retry-after"), Some("12"));
    node.clock.advance(12_000);
    let right = send(
        node.https,
        true,
        post_json(&node, "/api/login", &login_body(&key())),
    )
    .await;
    assert_eq!(right.status, StatusCode::OK);
    node.stop().await;
}

// RT-1: the original redirected to https://0.0.0.0/<path>, without the query.
#[tokio::test]
async fn http_redirects_and_serves_challenges() {
    let node = TestNode::start().await;
    let request = Request::get("/wallet?x=1")
        .header("host", "node.example")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = send(node.http, false, request).await;
    assert_eq!(reply.status, StatusCode::MOVED_PERMANENTLY);
    let expected = format!("https://node.example:{}/wallet?x=1", node.https.port());
    assert_eq!(reply.header("location"), Some(expected.as_str()));
    let request = Request::get("/.well-known/acme-challenge/unknown")
        .body(Full::new(Bytes::new()))
        .unwrap();
    assert_eq!(
        send(node.http, false, request).await.status,
        StatusCode::NOT_FOUND
    );
    node.stop().await;
}

// RT-15 and RT-16: HEAD, headers, and unmatched upgrades.
#[tokio::test]
async fn https_surface() {
    let node = TestNode::start().await;
    let head = Request::head("/")
        .header("host", node.host())
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = send(node.https, true, head).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.body.is_empty());
    assert!(
        reply
            .header("content-security-policy")
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        reply.header("strict-transport-security"),
        None,
        "no HSTS with a self-signed certificate"
    );
    let upgrade = Request::get("/other")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .body(Full::new(Bytes::new()))
        .unwrap();
    assert_eq!(
        send(node.https, true, upgrade).await.status,
        StatusCode::NOT_FOUND
    );
    let put = Request::put("/").body(Full::new(Bytes::new())).unwrap();
    assert_eq!(
        send(node.https, true, put).await.status,
        StatusCode::METHOD_NOT_ALLOWED
    );
    node.stop().await;
}

fn names(certificate: &rustls::pki_types::CertificateDer<'_>) -> Vec<String> {
    let (_, parsed) = x509_parser::parse_x509_certificate(certificate.as_ref()).unwrap();
    let extension = parsed.subject_alternative_name().unwrap().unwrap();
    extension
        .value
        .general_names
        .iter()
        .map(ToString::to_string)
        .collect()
}

// RT-3 and spec/areas/http.md#certificates: the certificate follows the
// hostname at once, without restarting the listener.
#[tokio::test]
async fn the_certificate_follows_the_hostname() {
    let node = TestNode::start().await;
    let (_, before) = connect_tls(node.https).await;
    assert!(
        !names(&before)
            .iter()
            .any(|name| name.contains("node.example"))
    );
    let mut owner = node.ipc().await;
    let reply = owner
        .call(Method::SetHostname(proto::SetHostnameRequest {
            hostname: "Node.Example".into(),
        }))
        .await;
    let Ok(response::Method::SetHostname(set)) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(set.config.unwrap().hostname, "node.example");
    let (_, after) = connect_tls(node.https).await;
    assert!(
        names(&after)
            .iter()
            .any(|name| name.contains("node.example")),
        "{:?}",
        names(&after)
    );
    let url = owner
        .call(Method::GetSetupUrl(proto::GetSetupUrlRequest {}))
        .await;
    let Ok(response::Method::GetSetupUrl(setup)) = url else {
        panic!()
    };
    assert!(setup.url.starts_with(&format!(
        "https://node.example:{}/setup/",
        node.https.port()
    )));
    node.stop().await;
}

// Found in review: a DNS name whose URL peers refuse, past 256 bytes, made
// the node a periphery node with its downlinks still there. It is refused,
// as is any name whose URL would not fit with every port; the longest that
// fits is taken.
#[tokio::test]
async fn a_hostname_too_long_for_the_url_is_refused() {
    let node = TestNode::start().await;
    let mut owner = node.ipc().await;
    let name = |length: usize| {
        let mut labels = vec!["a".repeat(63); length / 64];
        labels.push("b".repeat(length % 64));
        labels.join(".")
    };
    let set = |hostname: String| Method::SetHostname(proto::SetHostnameRequest { hostname });
    // `https://` and `:65535` leave 242 bytes of the 256.
    let refused = owner.call(set(name(243))).await;
    assert_eq!(
        refused.unwrap_err().code(),
        proto::ErrorCode::InvalidArgument
    );
    let taken = owner.call(set(name(242))).await;
    assert!(taken.is_ok(), "{taken:?}");
    node.stop().await;
}

// RT-12: the original installed whatever it was sent.
#[tokio::test]
async fn installed_certificates_are_checked() {
    let node = TestNode::start().await;
    let names = vec!["manual.example".to_string()];
    let first = dassie_node::tls::self_signed(&names, START_MS).unwrap();
    let second = dassie_node::tls::self_signed(&names, START_MS).unwrap();
    let mut owner = node.ipc().await;
    let mismatched = proto::SetCertificateRequest {
        certificate_chain_pem: first.chain_pem.clone(),
        private_key_pem: second.key_pem.clone(),
    };
    let error = owner
        .call(Method::SetCertificate(mismatched))
        .await
        .unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::InvalidArgument);
    let good = proto::SetCertificateRequest {
        certificate_chain_pem: first.chain_pem,
        private_key_pem: first.key_pem,
    };
    let Ok(response::Method::SetCertificate(installed)) =
        owner.call(Method::SetCertificate(good)).await
    else {
        panic!()
    };
    let installed = installed.certificate.unwrap();
    assert_eq!(
        (installed.source(), installed.names),
        (proto::CertificateSource::Manual, names)
    );
    let head = Request::head("/").body(Full::new(Bytes::new())).unwrap();
    assert!(
        send(node.https, true, head)
            .await
            .header("strict-transport-security")
            .is_some()
    );
    node.stop().await;
}

// RT-14 and spec/areas/node.md#persistence: a restart keeps the
// certificate, the identity and the sessions.
#[tokio::test]
async fn a_restart_keeps_everything() {
    let node = TestNode::start().await;
    let cookie = set_up(&node, &key()).await;
    let (_, before) = connect_tls(node.https).await;
    let (dir, clock) = node.stop().await;
    let node = TestNode::start_in(dir, clock, 2).await;
    let (_, after) = connect_tls(node.https).await;
    assert_eq!(before, after);
    let mut owner = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    assert_eq!(owner.state().await, proto::NodeState::Authenticated);
    let error = node
        .ipc()
        .await
        .call(Method::GetSetupUrl(proto::GetSetupUrlRequest {}))
        .await
        .unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::FailedPrecondition);
    node.stop().await;
}

// The original drew the setup token at every start, so a restart before
// setup (a nightly update, say) voided the URL the owner had been given.
#[tokio::test]
async fn the_setup_url_survives_a_restart() {
    let node = TestNode::start().await;
    let token = node.setup_token().await;
    let (dir, clock) = node.stop().await;
    let node = TestNode::start_in(dir, clock, 2).await;
    assert_eq!(node.setup_token().await, token);
    let reply = send(
        node.https,
        true,
        post_json(&node, "/api/setup", &setup_body(&token, &key())),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    // Once used, it is gone for good: a restart draws no new one.
    let (dir, clock) = node.stop().await;
    let node = TestNode::start_in(dir, clock, 3).await;
    let error = node
        .ipc()
        .await
        .call(Method::GetSetupUrl(proto::GetSetupUrlRequest {}))
        .await
        .unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::FailedPrecondition);
    node.stop().await;
}

#[tokio::test]
async fn sessions_can_be_listed_and_revoked() {
    let node = TestNode::start().await;
    let first = set_up(&node, &key()).await;
    let second = send(
        node.https,
        true,
        post_json(&node, "/api/login", &login_body(&key())),
    )
    .await
    .cookie();
    let mut owner = RpcClient::new(
        websocket(&node, Some(&first), &node.origin())
            .await
            .unwrap(),
    );
    let Ok(response::Method::ListSessions(list)) = owner
        .call(Method::ListSessions(proto::ListSessionsRequest {}))
        .await
    else {
        panic!()
    };
    assert_eq!(list.sessions.len(), 2);
    assert_eq!(
        list.sessions
            .iter()
            .filter(|session| session.current)
            .count(),
        1
    );
    let revoke = proto::RevokeAllSessionsRequest { keep_current: true };
    let Ok(response::Method::RevokeAllSessions(revoked)) =
        owner.call(Method::RevokeAllSessions(revoke)).await
    else {
        panic!()
    };
    assert_eq!(revoked.revoked, 1);
    let mut other = RpcClient::new(
        websocket(&node, Some(&second), &node.origin())
            .await
            .unwrap(),
    );
    assert_eq!(other.state().await, proto::NodeState::Anonymous);
    assert_eq!(owner.state().await, proto::NodeState::Authenticated);
    node.stop().await;
}

// RT-7: the original had no shutdown handling; sockets got no close frame.
#[tokio::test]
async fn shutdown_closes_sockets_with_going_away() {
    let node = TestNode::start().await;
    let mut socket = websocket(&node, None, &node.origin()).await.unwrap();
    let ipc_path = node.ipc.clone();
    let waiter = tokio::spawn(async move {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| u16::from(frame.code)),
                Some(Ok(_)) => {}
                _ => return None,
            }
        }
    });
    node.stop().await;
    assert_eq!(waiter.await.unwrap(), Some(1001));
    assert!(!ipc_path.exists());
}

/// What reached the notification socket so far.
fn notifications(socket: &UnixDatagram) -> Vec<String> {
    socket.set_nonblocking(true).unwrap();
    let mut buffer = [0; 64];
    let mut received = Vec::new();
    while let Ok(length) = socket.recv(&mut buffer) {
        received.push(String::from_utf8_lossy(&buffer[..length]).into_owned());
    }
    received
}

/// Waits for `expected` on the notification socket, skipping the others.
async fn until_notified(socket: &UnixDatagram, expected: &str) {
    for _ in 0..500 {
        if notifications(socket)
            .iter()
            .any(|message| message == expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no {expected}");
}

// spec/areas/node.md#readiness: a node waiting for its owner is ready at
// once, and a set-up one once its engine has committed its first step.
#[tokio::test]
async fn readiness_is_reported_before_and_after_setup() {
    let sockets = tempfile::tempdir().unwrap();
    let path = sockets.path().join("notify");
    let notify = UnixDatagram::bind(&path).unwrap();
    let node = Bound::new()
        .notifying(&path)
        .start(ManualClock::new(START_MS), 1, Policies::DEFAULT, &[])
        .await;
    until_notified(&notify, "READY=1").await;
    set_up(&node, &key()).await;
    let (dir, clock) = node.stop().await;
    let node = Bound::bind_in(dir)
        .notifying(&path)
        .start(clock, 2, Policies::DEFAULT, &[])
        .await;
    until_notified(&notify, "READY=1").await;
    // The engine answers.
    let ledgers = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await;
    assert!(ledgers.is_ok(), "{ledgers:?}");
    node.stop().await;
}

// A release whose engine fails at every start fails its start, so that
// `dassie update` rolls back. Before, the daemon reported ready at once and
// ran on without an engine until the next release.
#[tokio::test]
async fn an_engine_that_fails_at_every_start_ends_the_daemon_unready() {
    let node = TestNode::start().await;
    set_up(&node, &key()).await;
    let (dir, clock) = node.stop().await;
    // A scheme this version does not know: its engine cannot start on this
    // database.
    let database = dir
        .path()
        .join("state")
        .join(dassie_node::store::DATABASE_FILE);
    rusqlite::Connection::open(database)
        .unwrap()
        .execute(
            "INSERT INTO settlement_schemes (id, config, activated_at_ms) \
             VALUES ('nonesuch', '{}', 0)",
            [],
        )
        .unwrap();
    let sockets = tempfile::tempdir().unwrap();
    let path = sockets.path().join("notify");
    let notify = UnixDatagram::bind(&path).unwrap();
    let node = Bound::bind_in(dir)
        .notifying(&path)
        .start(clock, 2, Policies::DEFAULT, &[])
        .await;
    let (outcome, _dir) = node.ended().await;
    match outcome {
        Err(DaemonError::StartFailed { service, error }) => {
            assert_eq!(service, "engine");
            assert_eq!(error, "returned");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(notifications(&notify), ["STOPPING=1"]);
}

// spec/decisions/mainnet-schemes.md: `live` is refused where it is set and
// where the node starts, but by the dev switch, which the tests, built with
// debug assertions, honour. A node the owner set to it by the switch runs
// in it again with the switch, and without it does not start.
#[tokio::test]
async fn the_live_realm_runs_only_by_the_dev_switch() {
    const SWITCH: [(&str, &str); 1] = [("DASSIE_DEV_LIVE_REALM", "1")];
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 1, Policies::DEFAULT, &SWITCH)
        .await;
    set_up(&node, &key()).await;
    let live = proto::SetRealmRequest {
        realm: proto::Realm::Live.into(),
    };
    let reply = node.ipc().await.call(Method::SetRealm(live)).await;
    let Ok(response::Method::SetRealm(reply)) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(
        reply.config.map(|config| config.realm),
        Some(proto::Realm::Live.into())
    );
    let (dir, clock) = node.stop().await;
    let node = Bound::bind_in(dir)
        .start(clock, 2, Policies::DEFAULT, &SWITCH)
        .await;
    let ledgers = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await;
    assert!(ledgers.is_ok(), "{ledgers:?}");
    let (dir, clock) = node.stop().await;
    let node = Bound::bind_in(dir)
        .start(clock, 3, Policies::DEFAULT, &[])
        .await;
    let (outcome, _dir) = node.ended().await;
    match outcome {
        Err(DaemonError::StartFailed { service, .. }) => assert_eq!(service, "engine"),
        other => panic!("{other:?}"),
    }
}
