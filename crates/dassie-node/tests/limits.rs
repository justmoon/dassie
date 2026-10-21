//! The limits that keep a node's resources bounded against clients that
//! misbehave (spec/decisions/tigerstyle.md), and the ends of sessions that
//! are not logouts. Each test guards a defect a review found.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::clock::ManualClock;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, client_message, request::Method, response};
use dassie_node::tls;
use dassie_node_core::session::SessionPolicy;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use rustls::pki_types::ServerName;
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::tungstenite::Message;

fn key() -> NodeKey {
    NodeKey::from_bytes(&[7; 32])
}

/// The close code the socket gets next, skipping other messages.
async fn close_code(socket: &mut Socket) -> Option<u16> {
    let wait = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| u16::from(frame.code)),
                Some(Ok(_)) => {}
                _ => return None,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect("the socket stayed open")
}

// WebSockets outlive the HTTP connection and its permit; they have their
// own cap.
#[tokio::test]
async fn websockets_are_capped() {
    let node = TestNode::start_with(Policies {
        max_websockets: 2,
        ..Policies::DEFAULT
    })
    .await;
    let first = websocket(&node, None, &node.origin()).await.unwrap();
    let _second = websocket(&node, None, &node.origin()).await.unwrap();
    let refused = websocket(&node, None, &node.origin()).await.unwrap_err();
    assert!(refused.contains("503"), "{refused}");
    drop(first);
    // The place comes back once the socket is gone.
    let mut again = Err(String::new());
    for _ in 0..50 {
        again = websocket(&node, None, &node.origin()).await;
        if again.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(again.is_ok(), "{again:?}");
    node.stop().await;
}

#[tokio::test]
async fn idle_sockets_close() {
    let node = TestNode::start_with(Policies {
        socket_idle_ms: 200,
        ..Policies::DEFAULT
    })
    .await;
    let mut socket = websocket(&node, None, &node.origin()).await.unwrap();
    assert_eq!(close_code(&mut socket).await, Some(1000));
    node.stop().await;
}

// A client that sends headers and then trickles the body must not hold its
// connection for ever.
#[tokio::test]
async fn slow_bodies_time_out() {
    let node = TestNode::start_with(Policies {
        body_timeout_ms: 200,
        ..Policies::DEFAULT
    })
    .await;
    let (mut stream, _) = connect_tls(node.https).await;
    let head = format!(
        "POST /api/setup HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nContent-Type: application/json\r\n\
         Content-Length: 100\r\n\r\n{{\"a\":",
        node.host(),
        node.origin()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut reply = vec![0; 64];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut reply))
        .await
        .unwrap()
        .unwrap();
    let reply = String::from_utf8_lossy(&reply[..read]);
    assert!(reply.starts_with("HTTP/1.1 408"), "{reply}");
    node.stop().await;
}

// Sessions evicted to make room, like expired ones, close their sockets
// with the session-ended code, as the contract says.
#[tokio::test]
async fn evicted_sessions_close_their_sockets() {
    let sessions = SessionPolicy {
        max_sessions: 1,
        ..SessionPolicy::DEFAULT
    };
    let node = TestNode::start_with(Policies {
        sessions,
        ..Policies::DEFAULT
    })
    .await;
    let cookie = set_up(&node, &key()).await;
    let mut socket = websocket(&node, Some(&cookie), &node.origin())
        .await
        .unwrap();
    let login = send(
        node.https,
        true,
        post_json(&node, "/api/login", &login_body(&key())),
    )
    .await;
    assert_eq!(login.status, hyper::StatusCode::OK);
    assert_eq!(
        close_code(&mut socket).await,
        Some(dassie_node::http::SESSION_ENDED)
    );
    node.stop().await;
}

// Ending your own session answers the call before the socket closes.
#[tokio::test]
async fn revoking_your_own_session_answers_first() {
    let node = TestNode::start().await;
    let cookie = set_up(&node, &key()).await;
    let mut owner = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    let revoke = proto::RevokeAllSessionsRequest {
        keep_current: false,
    };
    let reply = owner.call(Method::RevokeAllSessions(revoke)).await;
    let Ok(response::Method::RevokeAllSessions(revoked)) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(revoked.revoked, 1);
    let mut socket = owner.into_inner();
    assert_eq!(
        close_code(&mut socket).await,
        Some(dassie_node::http::SESSION_ENDED)
    );
    node.stop().await;
}

/// Opens `/rpc` until it opens, as a place may take a moment to come back.
async fn websocket_eventually(node: &TestNode, cookie: Option<&str>) -> Result<Socket, String> {
    let mut opened = Err(String::new());
    for _ in 0..50 {
        opened = websocket(node, cookie, &node.origin()).await;
        if opened.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    opened
}

// Anyone can open a socket without a session, browser or not; one client
// gets only a few, and a session still gets in.
#[tokio::test]
async fn anonymous_sockets_are_capped_per_client() {
    let node = TestNode::start_with(Policies {
        anonymous_websockets_per_client: 2,
        ..Policies::DEFAULT
    })
    .await;
    let _first = websocket(&node, None, &node.origin()).await.unwrap();
    let _second = websocket(&node, None, &node.origin()).await.unwrap();
    let refused = websocket(&node, None, &node.origin()).await.unwrap_err();
    assert!(refused.contains("503"), "{refused}");
    let cookie = set_up(&node, &key()).await;
    let mut owner = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    assert_eq!(owner.state().await, proto::NodeState::Authenticated);
    node.stop().await;
}

// Sockets without a session, from however many clients, cannot take the
// places of those with one.
#[tokio::test]
async fn anonymous_sockets_leave_room_for_sessions() {
    let node = TestNode::start_with(Policies {
        max_websockets: 3,
        max_anonymous_websockets: 2,
        anonymous_websockets_per_client: 8,
        ..Policies::DEFAULT
    })
    .await;
    let first = websocket(&node, None, &node.origin()).await.unwrap();
    let _second = websocket(&node, None, &node.origin()).await.unwrap();
    let refused = websocket(&node, None, &node.origin()).await.unwrap_err();
    assert!(refused.contains("503"), "{refused}");
    let cookie = set_up(&node, &key()).await;
    let _owner = websocket(&node, Some(&cookie), &node.origin())
        .await
        .unwrap();
    // All three places are taken now.
    let refused = websocket(&node, Some(&cookie), &node.origin())
        .await
        .unwrap_err();
    assert!(refused.contains("503"), "{refused}");
    drop(first);
    let again = websocket_eventually(&node, None).await;
    assert!(again.is_ok(), "{again:?}");
    node.stop().await;
}

/// Asks for the basic state every 100 ms until the socket closes, and
/// returns its close code.
async fn ask_until_closed(socket: &mut Socket) -> Option<u16> {
    let ask = async {
        for call_id in 1.. {
            let request = proto::ClientMessage {
                call_id,
                kind: Some(client_message::Kind::Request(proto::Request {
                    method: Some(Method::GetBasicState(proto::GetBasicStateRequest {})),
                })),
            };
            let bytes = request.encode_to_vec();
            if socket.send(Message::Binary(bytes.into())).await.is_err() {
                return None;
            }
            loop {
                match socket.next().await {
                    Some(Ok(Message::Binary(_))) => break,
                    Some(Ok(Message::Close(frame))) => {
                        return frame.map(|frame| u16::from(frame.code));
                    }
                    Some(Ok(_)) => {}
                    _ => return None,
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(10), ask)
        .await
        .expect("the socket stayed open")
}

// A socket without a session closes after its lifetime even while it keeps
// asking; one with a session does not.
#[tokio::test]
async fn anonymous_sockets_close_after_their_lifetime() {
    let node = TestNode::start_with(Policies {
        anonymous_socket_lifetime_ms: 300,
        socket_idle_ms: 60_000,
        ..Policies::DEFAULT
    })
    .await;
    let cookie = set_up(&node, &key()).await;
    let mut owner = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    let mut anonymous = websocket(&node, None, &node.origin()).await.unwrap();
    assert_eq!(ask_until_closed(&mut anonymous).await, Some(1000));
    assert_eq!(owner.state().await, proto::NodeState::Authenticated);
    node.stop().await;
}

/// Whether the node holds `stream` open: a short read neither ends nor
/// fails, as the node waits for a TLS handshake.
async fn is_held(stream: &mut TcpStream) -> bool {
    let mut buffer = [0; 16];
    tokio::time::timeout(Duration::from_millis(300), stream.read(&mut buffer))
        .await
        .is_err()
}

/// A raw connection to `address` that the node holds; the place of a
/// connection that just closed may take a moment to come back.
async fn held(address: SocketAddr) -> TcpStream {
    for _ in 0..50 {
        let mut stream = TcpStream::connect(address).await.unwrap();
        if is_held(&mut stream).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the node held no connection");
}

/// A TLS connection to `address`, if the node completes the handshake.
async fn try_tls(address: SocketAddr) -> Option<TlsStream<TcpStream>> {
    let config = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Recorder::default()))
        .with_no_client_auth();
    let stream = TcpStream::connect(address).await.ok()?;
    let name = ServerName::try_from("localhost").unwrap();
    let connect = TlsConnector::from(Arc::new(config)).connect(name, stream);
    tokio::time::timeout(Duration::from_secs(2), connect)
        .await
        .ok()?
        .ok()
}

// One address cannot take all of a listener's connections, by holding them
// in the TLS handshake or otherwise.
#[tokio::test]
async fn connections_are_capped_per_client() {
    let node = TestNode::start_with(Policies {
        http_connections_per_client: 2,
        ..Policies::DEFAULT
    })
    .await;
    let first = held(node.https).await;
    let second = held(node.https).await;
    let mut third = TcpStream::connect(node.https).await.unwrap();
    let mut buffer = [0; 16];
    let read = tokio::time::timeout(Duration::from_secs(5), third.read(&mut buffer))
        .await
        .expect("the third connection was held");
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    drop(first);
    let mut again = None;
    for _ in 0..50 {
        again = try_tls(node.https).await;
        if again.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(again.is_some(), "no place came back");
    // Handshakes in progress hold up shutdown until they time out.
    drop(second);
    node.stop().await;
}

// Dev environments run many nodes and tools on 127.0.0.1, with private
// peers allowed; loopback is not counted per client there.
#[tokio::test]
async fn loopback_is_not_counted_where_private_peers_are_allowed() {
    let policies = Policies {
        http_connections_per_client: 1,
        ..Policies::DEFAULT
    };
    let env = [("DASSIE_PEER_ALLOW_PRIVATE", "1")];
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 1, policies, &env)
        .await;
    let mut streams = Vec::new();
    for _ in 0..3 {
        streams.push(TcpStream::connect(node.https).await.unwrap());
    }
    for stream in &mut streams {
        assert!(is_held(stream).await, "a loopback connection was closed");
    }
    drop(streams);
    node.stop().await;
}
