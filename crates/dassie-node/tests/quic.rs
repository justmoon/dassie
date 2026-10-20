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
use dassie_node::quic::{Body, Handler, Quic, SendError};
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
        .send(&request(&b, b"hello".to_vec()), &client())
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
    let result = a.quic.send(&wrong, &client()).await;
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
    let response = a.quic.send(&request(&b, large), &client()).await.unwrap();
    assert_eq!(response.status, 413);
    let fits = vec![7; MAX_MESSAGE_BYTES];
    let response = a
        .quic
        .send(&request(&b, fits.clone()), &client())
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
        .send(&request(&b, b"one".to_vec()), &client)
        .await
        .unwrap();
    // The answer comes back on the connection a opened, which b keeps.
    let response = b
        .quic
        .send(&request(&a, b"two".to_vec()), &client)
        .await
        .unwrap();
    assert_eq!(response.body, b"two");
    a.quic
        .send(&request(&b, b"three".to_vec()), &client)
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
    let result = a.send(&request(&b, b"hello".to_vec()), &client()).await;
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
        async move { quic.send(&request, &client).await }
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
