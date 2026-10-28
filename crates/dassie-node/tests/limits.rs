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

use dassie_identity::IdentityKey;
use dassie_node::clock::ManualClock;
use dassie_node::node::Policies;
use dassie_node::quic::tunnel;
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
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{self, Message};

fn key() -> IdentityKey {
    dassie_identity::TableKeygen::identity_key(&[7; 32])
}

/// The close code the socket gets next, skipping other messages. What
/// closes it is the IO layer's own, on real time (an idle socket's or a
/// lifetime's timer, the grace an ending session's calls get), so this
/// waits for it until a hang ([`HANG`]), which no loaded machine reaches.
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
    tokio::time::timeout(HANG, wait)
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
    websocket_eventually(&node).await;
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
    // The body's timeout is the IO layer's, on real time: this waits for
    // the answer it gives until a hang.
    let read = tokio::time::timeout(HANG, stream.read(&mut reply))
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

/// Opens `/rpc` without a session once it opens: the place of a socket
/// that closed comes back once the node has seen it go, which the IO layer
/// does in its own time, so this tries until a hang ([`HANG`]).
async fn websocket_eventually(node: &TestNode) -> Socket {
    eventually("a socket's place coming back", async || {
        websocket(node, None, &node.origin()).await.ok()
    })
    .await
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
    websocket_eventually(&node).await;
    node.stop().await;
}

/// Asks for the basic state every 100 ms until the socket closes, and
/// returns its close code. The lifetime that closes it is the IO layer's
/// own, on real time, so this waits for it until a hang ([`HANG`]); the
/// pause between calls only keeps the asking from being a flood.
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
    tokio::time::timeout(HANG, ask)
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

/// Checks that the node closes `stream` without a word, as it does a
/// connection past the client's share on arrival. It does so at once. A
/// connection it held instead would close too, but only at the TLS
/// handshake's timeout, so the wait ends at half of that: a close that
/// comes later is a held connection, and fails the test.
async fn is_refused(stream: &mut TcpStream, what: &str) {
    let mut buffer = [0; 16];
    let refused_within = dassie_node::http::TLS_HANDSHAKE_TIMEOUT / 2;
    let read = tokio::time::timeout(refused_within, stream.read(&mut buffer))
        .await
        .unwrap_or_else(|_| panic!("{what} was held"));
    assert!(matches!(read, Ok(0) | Err(_)), "{what}: {read:?}");
}

/// Whether the node still serves `stream`: it answers a request on it.
async fn is_served(stream: &mut TlsStream<TcpStream>, node: &TestNode) -> bool {
    let request = format!("HEAD / HTTP/1.1\r\nHost: {}\r\n\r\n", node.host());
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut reply = [0; 9];
    let read = tokio::time::timeout(HANG, stream.read_exact(&mut reply))
        .await
        .expect("no answer");
    read.is_ok() && &reply == b"HTTP/1.1 "
}

/// A TLS connection to `address`, if the node completes the handshake. A
/// connection past the client's share is closed on arrival, and fails its
/// handshake at once, and one the node takes completes it: the timeout is
/// a hang's guard ([`HANG`]), never what decides.
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
    tokio::time::timeout(HANG, connect)
        .await
        .expect("a TLS handshake hung")
        .ok()
}

/// A TLS connection the node takes: the place of a connection that just
/// closed comes back once the node has seen it go, which the IO layer does
/// in its own time, so this tries until a hang ([`HANG`]).
async fn tls_eventually(address: SocketAddr) -> TlsStream<TcpStream> {
    eventually("a place for an HTTPS connection", async || {
        try_tls(address).await
    })
    .await
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
    // Two raw connections that never start their handshake: nothing else
    // is open, so the node takes both, and it takes connections in the
    // order they came. So the third finds both places held, and is closed:
    // had the node not held one of the first two, it would hold the third.
    let first = TcpStream::connect(node.https).await.unwrap();
    let second = TcpStream::connect(node.https).await.unwrap();
    let mut third = TcpStream::connect(node.https).await.unwrap();
    is_refused(&mut third, "the third connection").await;
    drop(first);
    tls_eventually(node.https).await;
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
    // Each completes its handshake while those before it are open: past a
    // share of one, the node would have closed the second on arrival.
    let mut streams = Vec::new();
    for _ in 0..3 {
        let stream = try_tls(node.https).await;
        streams.push(stream.expect("a loopback connection was closed"));
    }
    drop(streams);
    node.stop().await;
}

/// How a request for a tunnel asks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Asks {
    /// As a node's client does.
    AsANode,
    /// Without the tunnel's subprotocol.
    WithoutSubprotocol,
    /// With an origin, as a browser does.
    AsABrowser,
}

/// Opens a tunnel for QUIC at `/quic`, asking as `asks` has it; the status
/// of a refusal. The place of the connection upgraded before may take a
/// moment to come back.
async fn tunnel(node: &TestNode, asks: Asks) -> Result<Socket, u16> {
    let stream = tls_eventually(node.https).await;
    let mut request = format!("wss://{}{}", node.host(), tunnel::PATH)
        .into_client_request()
        .unwrap();
    let headers = request.headers_mut();
    if asks != Asks::WithoutSubprotocol {
        let value = tunnel::SUBPROTOCOL.parse().unwrap();
        headers.insert("sec-websocket-protocol", value);
    }
    if asks == Asks::AsABrowser {
        headers.insert("origin", "https://page.example".parse().unwrap());
    }
    let config = Some(tunnel::websocket_config());
    match tokio_tungstenite::client_async_with_config(request, stream, config).await {
        Ok((socket, _)) => Ok(socket),
        Err(tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
        Err(error) => panic!("{error}"),
    }
}

/// Bounds for tunnels a test holds open to count their places: as long as
/// a hang ([`HANG`]), so that a tunnel holds its place until the test ends
/// it. They are real time, and that a tunnel without a connection ends by
/// them is `tests/tunnel.rs`'s to check.
const HELD: tunnel::TunnelBounds = tunnel::TunnelBounds {
    first_packet: HANG,
    connect: HANG,
};

// spec/decisions/quic-over-websocket.md: a tunnel leaves the HTTPS
// listener's places once upgraded, and holds one among the QUIC connections
// being set up, by its client address, as a connection over UDP from there
// would: past the client's share of those, a tunnel is refused with 503,
// though HTTPS has room, and the client keeps its share of HTTPS beside
// it. A request without the tunnel's subprotocol is refused; and a tunnel
// that ends, here as its client closes it, gives its place back.
#[tokio::test]
async fn tunnels_hold_places_among_the_quic_connections_being_set_up() {
    let node = TestNode::start_with(Policies {
        http_connections_per_client: 1,
        tunnel_bounds: HELD,
        ..Policies::DEFAULT
    })
    .await;
    let quic = node.quic.clone().unwrap();
    let mut socket = tunnel(&node, Asks::AsANode).await.unwrap();
    assert_eq!(quic.open_tunnels(), 1);
    assert_eq!(quic.places().unwrap().setting_up, 1);
    // Past the client's share of connections being set up, though HTTPS
    // has room.
    assert_eq!(tunnel(&node, Asks::AsANode).await.err(), Some(503));
    // The client holds its share of HTTPS connections beside its tunnel,
    // and no more: the node took the first, so it has the second closed.
    let mut first = tls_eventually(node.https).await;
    let mut second = TcpStream::connect(node.https).await.unwrap();
    is_refused(&mut second, "a second connection").await;
    assert!(is_served(&mut first, &node).await);
    drop(first);
    assert_eq!(
        tunnel(&node, Asks::WithoutSubprotocol).await.err(),
        Some(400)
    );
    assert_eq!(quic.places().unwrap().setting_up, 1);
    // The client closes its tunnel; the node closes its side, and the
    // tunnel leaves, giving its place back.
    socket.close(None).await.unwrap();
    let ending = async {
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
        }
    };
    tokio::time::timeout(HANG, ending)
        .await
        .expect("the node did not close the tunnel");
    until_none_open(&quic).await;
    assert_eq!(quic.places().unwrap().setting_up, 0);
    drop(socket);
    tunnel(&node, Asks::AsANode).await.unwrap();
    node.stop().await;
}

// spec/decisions/quic-over-websocket.md: a browser sends an origin with
// every WebSocket handshake, and a node never does. A request for a tunnel
// that carries one is refused with 403, before it takes a place among the
// QUIC connections being set up, so that no page can have its visitors
// hold them.
#[tokio::test]
async fn a_browser_gets_no_tunnel() {
    let node = TestNode::start_with(Policies {
        tunnel_bounds: HELD,
        ..Policies::DEFAULT
    })
    .await;
    let quic = node.quic.clone().unwrap();
    assert_eq!(tunnel(&node, Asks::AsABrowser).await.err(), Some(403));
    assert_eq!(quic.open_tunnels(), 0);
    assert_eq!(quic.places().unwrap().setting_up, 0);
    let _socket = tunnel(&node, Asks::AsANode).await.unwrap();
    assert_eq!(quic.places().unwrap().setting_up, 1);
    node.stop().await;
}

/// Waits for the node's tunnels to be gone: they leave once they closed,
/// which takes the IO layer's own time, so until a hang ([`HANG`]).
async fn until_none_open(quic: &dassie_node::quic::Quic) {
    eventually("the tunnels to be gone", async || {
        (quic.open_tunnels() == 0).then_some(())
    })
    .await;
}
