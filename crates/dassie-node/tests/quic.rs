//! The peer protocol over QUIC, between two endpoints on loopback: requests
//! and their answers, keys pinned by node ID, bounds on bodies, and one
//! connection per pair of nodes (spec/areas/peer-protocol.md#quic).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::net::{IpAddr, UdpSocket};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dassie_identity::{NodeId, NodeKey};
use dassie_node::http::peer::{Answer, BodyError};
use dassie_node::peer_client::PeerClient;
use dassie_node::quic::{Body, Handler, Quic, SendError, Transport, send_peer};
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use dassie_peer::wire::{Kind, MAX_MESSAGE_BYTES};
use dassie_peer::{Request, RequestId};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Answers each request with its own body, or a status for a body that
/// could not be read.
struct Echo;

impl Handler for Echo {
    fn client(&self, _address: IpAddr) -> Option<ClientKey> {
        None
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: 16,
            max_per_client: 16,
        }
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async move {
            match body.await {
                Ok(body) => Answer { status: 200, body },
                Err(BodyError::TooLarge) => Answer {
                    status: 413,
                    body: Vec::new(),
                },
                Err(BodyError::Broken) => Answer {
                    status: 400,
                    body: Vec::new(),
                },
            }
        })
    }
}

/// Answers only once `waiting` requests are inside it at once, as when
/// requests to one node run side by side.
struct Together(tokio::sync::Barrier);

impl Handler for Together {
    fn client(&self, address: IpAddr) -> Option<ClientKey> {
        Echo.client(address)
    }

    fn limits(&self) -> ConnectionLimits {
        Echo.limits()
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async move {
            let body = body.await.unwrap_or_default();
            self.0.wait().await;
            Answer { status: 200, body }
        })
    }
}

/// Says when a request reaches it, and answers it with its body only once
/// let go.
#[derive(Default)]
struct Held {
    arrived: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl Handler for Held {
    fn client(&self, address: IpAddr) -> Option<ClientKey> {
        Echo.client(address)
    }

    fn limits(&self) -> ConnectionLimits {
        Echo.limits()
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async move {
            let body = body.await.unwrap_or_default();
            self.arrived.notify_one();
            self.release.notified().await;
            Answer { status: 200, body }
        })
    }
}

struct Side {
    quic: Arc<Quic>,
    id: NodeId,
    url: String,
    // Kept alive: the endpoint presents the key it holds.
    _identity: watch::Sender<Option<Arc<NodeKey>>>,
}

fn side(seed: u8, shutdown: &CancellationToken) -> Side {
    side_with(seed, shutdown, Arc::new(Echo))
}

fn side_with(seed: u8, shutdown: &CancellationToken, handler: Arc<dyn Handler>) -> Side {
    let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
    let id = key.node_id();
    let (identity, receiver) = watch::channel(Some(key));
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let quic = Arc::new(Quic::new(socket, receiver, true).unwrap());
    let port = quic.local_addr().unwrap().port();
    tokio::spawn(Arc::clone(&quic).serve(handler, shutdown.clone()));
    Side {
        quic,
        id,
        url: format!("https://127.0.0.1:{port}"),
        _identity: identity,
    }
}

/// Time to connect, in the tests.
const CONNECT: std::time::Duration = std::time::Duration::from_secs(2);

fn client() -> PeerClient {
    // Roots of its own: the build sandbox has none of the system's. QUIC
    // trusts no roots at all; the client only resolves names here.
    let roots = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/pebble/root.pem"
    );
    PeerClient::new(Some(Path::new(roots)), true).unwrap()
}

fn request(to: &Side, body: Vec<u8>) -> Request {
    Request {
        id: RequestId(1),
        to: to.id,
        url: to.url.clone(),
        quic: true,
        kind: Kind::LinkStateRequest,
        body,
        timeout_ms: 5_000,
    }
}

#[tokio::test]
async fn a_request_gets_its_answer() {
    let shutdown = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &shutdown));
    let response = a
        .quic
        .send(&request(&b, b"hello".to_vec()), &client(), CONNECT)
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"hello");
    assert_eq!(a.quic.stats().sent.load(Ordering::Relaxed), 1);
    assert_eq!(b.quic.stats().answered.load(Ordering::Relaxed), 1);
    shutdown.cancel();
}

#[tokio::test]
async fn a_server_with_another_key_is_not_reached() {
    let shutdown = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &shutdown));
    let impostor = NodeKey::from_bytes(&[3; 32]).node_id();
    let mut wrong = request(&b, b"hello".to_vec());
    wrong.to = impostor;
    let result = a.quic.send(&wrong, &client(), CONNECT).await;
    assert_eq!(result, Err(SendError::Unreachable));
    assert_eq!(b.quic.stats().answered.load(Ordering::Relaxed), 0);
    // Asked over HTTPS for a while, without trying again.
    assert!(a.quic.is_unreachable(&impostor));
    assert!(!a.quic.is_unreachable(&b.id));
    shutdown.cancel();
}

#[tokio::test]
async fn a_body_past_the_limit_is_refused() {
    let shutdown = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &shutdown));
    let large = vec![0; MAX_MESSAGE_BYTES + 1];
    let response = a
        .quic
        .send(&request(&b, large), &client(), CONNECT)
        .await
        .unwrap();
    assert_eq!(response.status, 413);
    let fits = vec![7; MAX_MESSAGE_BYTES];
    let response = a
        .quic
        .send(&request(&b, fits.clone()), &client(), CONNECT)
        .await
        .unwrap();
    assert_eq!((response.status, response.body), (200, fits));
    shutdown.cancel();
}

#[tokio::test]
async fn a_pair_of_nodes_shares_one_connection() {
    let shutdown = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &shutdown));
    let client = client();
    a.quic
        .send(&request(&b, b"one".to_vec()), &client, CONNECT)
        .await
        .unwrap();
    // The answer comes back on the connection a opened, which b keeps.
    let response = b
        .quic
        .send(&request(&a, b"two".to_vec()), &client, CONNECT)
        .await
        .unwrap();
    assert_eq!(response.body, b"two");
    a.quic
        .send(&request(&b, b"three".to_vec()), &client, CONNECT)
        .await
        .unwrap();
    assert_eq!(a.quic.open_connections(), 1);
    assert_eq!(b.quic.open_connections(), 1);
    shutdown.cancel();
}

#[tokio::test]
async fn a_node_without_its_key_reaches_nobody() {
    let shutdown = CancellationToken::new();
    let b = side(2, &shutdown);
    let (_identity, receiver) = watch::channel(None);
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let a = Arc::new(Quic::new(socket, receiver, true).unwrap());
    let result = a
        .send(&request(&b, b"hello".to_vec()), &client(), CONNECT)
        .await;
    assert_eq!(result, Err(SendError::Unreachable));
    shutdown.cancel();
}

#[tokio::test]
async fn requests_to_one_node_run_side_by_side() {
    const AT_ONCE: usize = 8;
    let shutdown = CancellationToken::new();
    let a = side(1, &shutdown);
    let b = side_with(
        2,
        &shutdown,
        Arc::new(Together(tokio::sync::Barrier::new(AT_ONCE))),
    );
    let client = client();
    let sends = (0..AT_ONCE).map(|index| {
        let (quic, request) = (Arc::clone(&a.quic), request(&b, vec![7; index + 1]));
        let client = client.clone();
        async move { quic.send(&request, &client, CONNECT).await }
    });
    let answers = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        futures_util::future::join_all(sends),
    )
    .await
    .expect("one request waited for another");
    for (index, answer) in answers.into_iter().enumerate() {
        assert_eq!(answer.unwrap().body, vec![7; index + 1]);
    }
    assert_eq!(a.quic.open_connections(), 1);
    shutdown.cancel();
}

/// A node whose UDP drops everything, and whose HTTPS takes connections but
/// answers nothing: a UDP socket that is never read, and a TCP listener on
/// the same port number.
async fn silent() -> (UdpSocket, tokio::net::TcpListener, u16) {
    for _ in 0..16 {
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = udp.local_addr().unwrap().port();
        if let Ok(tcp) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            return (udp, tcp, port);
        }
    }
    panic!("no port free on both UDP and TCP");
}

// A request as short as a probe still reaches HTTPS when QUIC cannot be
// reached: the connection attempt takes only part of its time.
#[tokio::test]
async fn a_short_request_falls_back_to_https_in_time() {
    let shutdown = CancellationToken::new();
    let a = side(1, &shutdown);
    let (_udp, tcp, port) = silent().await;
    let target = NodeKey::from_bytes(&[9; 32]).node_id();
    let request = Request {
        id: RequestId(7),
        to: target,
        url: format!("https://127.0.0.1:{port}"),
        quic: true,
        kind: Kind::LinkStateRequest,
        body: b"probe".to_vec(),
        timeout_ms: 2_000,
    };
    let client = client();
    let started = tokio::time::Instant::now();
    let (accepted, (result, transport)) = tokio::join!(
        tcp.accept(),
        send_peer(Some(Arc::clone(&a.quic)), &client, request)
    );
    assert!(accepted.is_ok(), "HTTPS was not tried");
    assert_eq!(transport, Transport::Https);
    assert!(result.is_err());
    // Within its time, give or take a loaded machine's slack.
    assert!(started.elapsed() < std::time::Duration::from_millis(4_000));
    // And asked over HTTPS from now on, without waiting for QUIC again.
    assert!(a.quic.is_unreachable(&target));
    shutdown.cancel();
}

// At shutdown the node stops taking connections, but answers the requests
// it has, before it closes.
#[tokio::test]
async fn a_request_in_progress_is_answered_at_shutdown() {
    let (shutdown_a, shutdown_b) = (CancellationToken::new(), CancellationToken::new());
    let a = side(1, &shutdown_a);
    let held = Arc::new(Held::default());
    let b = side_with(2, &shutdown_b, Arc::clone(&held) as Arc<dyn Handler>);
    let (quic, request) = (Arc::clone(&a.quic), request(&b, b"late".to_vec()));
    let sending = tokio::spawn(async move { quic.send(&request, &client(), CONNECT).await });
    // Shut down while the request is in the handler, then let it answer.
    held.arrived.notified().await;
    shutdown_b.cancel();
    held.release.notify_one();
    let response = sending.await.unwrap().unwrap();
    assert_eq!((response.status, response.body), (200, b"late".to_vec()));
    shutdown_a.cancel();
}
