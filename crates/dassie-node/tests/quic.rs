//! The peer protocol over QUIC, between endpoints on loopback: requests and
//! their answers, keys pinned by node ID, bounds on bodies, one connection
//! per pair of nodes, places for connections by who opened them, and backing
//! off from a node that cannot be reached (spec/areas/peer-protocol.md#quic).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::future::Future;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use dassie_identity::{IdentityKey, NodeId};
use dassie_kit::ClientKey;
use dassie_node::outbound::Resolver;
use dassie_node::peer_server::{Answer, BodyError};
use dassie_node::quic::places::{Classes, Counts, Peerings};
use dassie_node::quic::{
    BACKOFF_MIN, Body, CONNECT_TIMEOUT, Handler, IDLE_TIMEOUT, Quic, send_peer,
};
use dassie_node_core::connections::ConnectionLimits;
use dassie_peer::wire::{Endpoint, Kind, MAX_MESSAGE_BYTES};
use dassie_peer::{Failure, Request, RequestId};
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
        _from: dassie_identity::NodeIdentity,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(echo(body))
    }
}

async fn echo(body: Body<'_>) -> Answer {
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
}

/// Echoes, with the peerings a test sets: room for one core peer and one
/// downlink, and for [`ANYONE`] connections from anyone else.
struct Classed(watch::Receiver<Arc<Classes>>);

/// The places for anyone else's connections, and for those being set up.
const ANYONE: usize = 2;

impl Handler for Classed {
    fn client(&self, _address: IpAddr) -> Option<ClientKey> {
        None
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: ANYONE,
            max_per_client: 16,
        }
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        _from: dassie_identity::NodeIdentity,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(echo(body))
    }

    fn peerings(&self) -> Option<Peerings> {
        Some(Peerings {
            classes: self.0.clone(),
            max_core_peers: 1,
            max_downlinks: 1,
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
        _from: dassie_identity::NodeIdentity,
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
        _from: dassie_identity::NodeIdentity,
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
    endpoint: Endpoint,
    // Kept alive: the endpoint presents the key it holds.
    _identity: watch::Sender<Option<Arc<IdentityKey>>>,
}

fn side(seed: u8, shutdown: &CancellationToken) -> Side {
    side_with(seed, shutdown, Arc::new(Echo))
}

fn side_with(seed: u8, shutdown: &CancellationToken, handler: Arc<dyn Handler>) -> Side {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let key = Arc::new(IdentityKey::from_bytes(&[seed; 32]));
    let id = key.node_id();
    let (identity, receiver) = watch::channel(Some(key));
    let quic = Arc::new(Quic::new(socket, receiver, true).unwrap());
    let port = quic.local_addr().unwrap().port();
    tokio::spawn(Arc::clone(&quic).serve(handler, shutdown.clone()));
    Side {
        quic,
        id,
        endpoint: Endpoint::parse(&format!("127.0.0.1:{port}")).unwrap(),
        _identity: identity,
    }
}

/// QUIC trusts no roots: each connection pins its node's key, and the
/// resolver only resolves names, private ones included.
fn client() -> Resolver {
    Resolver::new(true)
}

fn request(to: &Side, body: Vec<u8>) -> Request {
    Request {
        id: RequestId(1),
        to: to.id,
        endpoint: Some(to.endpoint.clone()),
        kind: Kind::LinkStateRequest,
        body,
        timeout_ms: 5_000,
        fresh: false,
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
    let impostor = IdentityKey::from_bytes(&[3; 32]).node_id();
    let mut wrong = request(&b, b"hello".to_vec());
    wrong.to = impostor;
    let result = a.quic.send(&wrong, &client()).await;
    assert_eq!(result, Err(Failure::NotDelivered));
    assert_eq!(b.quic.stats().answered.load(Ordering::Relaxed), 0);
    // Not dialled again for a while.
    assert!(a.quic.backing_off(&impostor));
    assert!(!a.quic.backing_off(&b.id));
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
    assert_eq!(result, Err(Failure::NotDelivered));
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

// A request goes over QUIC, the only transport; without a QUIC endpoint
// nothing is sent.
#[tokio::test]
async fn a_peer_request_goes_over_quic() {
    let shutdown = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &shutdown));
    let client = client();
    let response = send_peer(
        Some(Arc::clone(&a.quic)),
        &client,
        request(&b, b"hello".to_vec()),
    )
    .await
    .unwrap();
    assert_eq!(response.body, b"hello");
    let result = send_peer(None, &client, request(&b, b"hello".to_vec())).await;
    assert_eq!(result, Err(Failure::NotDelivered));
    shutdown.cancel();
}

// A node whose UDP drops everything is not delivered to, within the
// request's time, and is not dialled again for a while: the next request
// fails at once.
#[tokio::test]
async fn a_node_out_of_reach_is_not_dialled_again_for_a_while() {
    let shutdown = CancellationToken::new();
    let a = side(1, &shutdown);
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    let target = IdentityKey::from_bytes(&[9; 32]).node_id();
    let request = Request {
        id: RequestId(7),
        to: target,
        endpoint: Endpoint::parse(&format!("127.0.0.1:{port}")),
        kind: Kind::LinkStateRequest,
        body: b"probe".to_vec(),
        timeout_ms: 2_000,
        fresh: false,
    };
    let client = client();
    let started = tokio::time::Instant::now();
    let result = a.quic.send(&request, &client).await;
    assert_eq!(result, Err(Failure::NotDelivered));
    // Within its time, which is shorter than the time to connect: past
    // that, the dial would not have given up with the request.
    assert!(started.elapsed() < CONNECT_TIMEOUT);
    assert!(a.quic.backing_off(&target));
    // Dialled again, it would take the request's time once more.
    let again = tokio::time::Instant::now();
    assert_eq!(
        a.quic.send(&request, &client).await,
        Err(Failure::NotDelivered)
    );
    assert!(again.elapsed() < std::time::Duration::from_millis(request.timeout_ms));
    shutdown.cancel();
}

// A node backed off from is reached again as soon as it connects to us: a
// peer that restarted and dials us is not left waiting out the backoff.
#[tokio::test]
async fn a_node_that_connects_ends_the_backoff() {
    let shutdown = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &shutdown));
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    // b, at an address where it cannot be reached.
    let mut elsewhere = request(&b, b"lost".to_vec());
    elsewhere.endpoint = Endpoint::parse(&format!("127.0.0.1:{port}"));
    elsewhere.timeout_ms = 1_000;
    let client = client();
    assert_eq!(
        a.quic.send(&elsewhere, &client).await,
        Err(Failure::NotDelivered)
    );
    assert!(a.quic.backing_off(&b.id));
    b.quic
        .send(&request(&a, b"back".to_vec()), &client)
        .await
        .unwrap();
    until(|| !a.quic.backing_off(&b.id)).await;
    // And asked on the connection b opened.
    elsewhere.timeout_ms = 5_000;
    let response = a.quic.send(&elsewhere, &client).await.unwrap();
    assert_eq!(response.body, b"lost");
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
    let sending = tokio::spawn(async move { quic.send(&request, &client()).await });
    // Shut down while the request is in the handler, and let it answer only
    // once b takes no more connections: it is draining.
    held.arrived.notified().await;
    shutdown_b.cancel();
    while b.quic.is_serving() {
        tokio::task::yield_now().await;
    }
    held.release.notify_one();
    let response = sending.await.unwrap().unwrap();
    assert_eq!((response.status, response.body), (200, b"late".to_vec()));
    shutdown_a.cancel();
}

/// A server whose peerings the test sets, as `classes`.
fn classed(shutdown: &CancellationToken) -> (Side, watch::Sender<Arc<Classes>>) {
    let (classes, receiver) = watch::channel(Arc::new(Classes::default()));
    let server = side_with(1, shutdown, Arc::new(Classed(receiver)));
    (server, classes)
}

fn peerings(core: &[&Side], downlinks: &[&Side]) -> Arc<Classes> {
    Arc::new(Classes {
        core: core.iter().map(|side| side.id).collect(),
        downlinks: downlinks.iter().map(|side| side.id).collect(),
    })
}

fn counts(core: usize, downlinks: usize, others: usize) -> Counts {
    Counts {
        setting_up: 0,
        core,
        downlinks,
        others,
    }
}

/// Waits until `done`, which the endpoint reaches on its own, on real time
/// as QUIC's own timers run: in a moment, or once a backoff of its is over.
/// The limit only tells a hang: twice the idle timeout, the longest of the
/// timers any wait here depends on, which no machine is slow enough to
/// reach.
async fn until(mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + 2 * IDLE_TIMEOUT;
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "not done in time");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Sends `from` a request to `to`, and says whether it was answered.
async fn answered(from: &Side, to: &Side) -> bool {
    let request = request(to, b"hello".to_vec());
    from.quic.send(&request, &client()).await.is_ok()
}

// Anyone else's places are full, yet a core peer and a downlink each find
// room among their own.
#[tokio::test]
async fn core_peers_and_downlinks_have_room_of_their_own() {
    let shutdown = CancellationToken::new();
    let (server, classes) = classed(&shutdown);
    let (core, downlink) = (side(2, &shutdown), side(3, &shutdown));
    classes.send_replace(peerings(&[&core], &[&downlink]));
    for seed in 4..4 + ANYONE {
        let stranger = side(u8::try_from(seed).unwrap(), &shutdown);
        assert!(answered(&stranger, &server).await);
    }
    let late = side(9, &shutdown);
    assert!(!answered(&late, &server).await);
    assert_eq!(server.quic.places(), Some(counts(0, 0, ANYONE)));
    assert!(answered(&core, &server).await);
    assert!(answered(&downlink, &server).await);
    assert_eq!(server.quic.places(), Some(counts(1, 1, ANYONE)));
    shutdown.cancel();
}

// A node that becomes a downlink takes its connection out of anyone
// else's places, and loses it once its peering ends with them full.
#[tokio::test]
async fn a_connection_moves_with_its_nodes_peering() {
    let shutdown = CancellationToken::new();
    let (server, classes) = classed(&shutdown);
    let newcomer = side(2, &shutdown);
    assert!(answered(&newcomer, &server).await);
    assert_eq!(server.quic.places(), Some(counts(0, 0, 1)));
    classes.send_replace(peerings(&[], &[&newcomer]));
    until(|| server.quic.places() == Some(counts(0, 1, 0))).await;
    // The same connection carries its requests still.
    assert!(answered(&newcomer, &server).await);
    assert_eq!(newcomer.quic.open_connections(), 1);
    for seed in 3..3 + ANYONE {
        let stranger = side(u8::try_from(seed).unwrap(), &shutdown);
        assert!(answered(&stranger, &server).await);
    }
    assert_eq!(server.quic.places(), Some(counts(0, 1, ANYONE)));
    classes.send_replace(peerings(&[], &[]));
    until(|| newcomer.quic.open_connections() == 0).await;
    assert_eq!(server.quic.places(), Some(counts(0, 0, ANYONE)));
    shutdown.cancel();
}

// A downlink cannot hold more than its share by opening more connections:
// the newest takes the place of its oldest, and nobody else's.
#[tokio::test]
async fn a_node_past_its_share_replaces_its_oldest_connection() {
    let shutdown = CancellationToken::new();
    let (server, classes) = classed(&shutdown);
    // One node, three endpoints: three connections under one key.
    let endpoints = [side(3, &shutdown), side(3, &shutdown), side(3, &shutdown)];
    classes.send_replace(peerings(&[], &[&endpoints[0]]));
    let [first, second, third] = &endpoints;
    assert!(answered(first, &server).await);
    assert!(answered(second, &server).await);
    assert_eq!(server.quic.places(), Some(counts(0, 2, 0)));
    assert!(answered(third, &server).await);
    until(|| first.quic.open_connections() == 0).await;
    assert_eq!(second.quic.open_connections(), 1);
    assert_eq!(server.quic.places(), Some(counts(0, 2, 0)));
    shutdown.cancel();
}

// A stranger the server has no room for is told so as its connection
// closes, before anything it sent was read: nothing was delivered, as when
// a connection is refused outright, and it backs off from the server.
#[tokio::test]
async fn a_stranger_without_room_backs_off() {
    let shutdown = CancellationToken::new();
    let (server, classes) = classed(&shutdown);
    for seed in 4..4 + ANYONE {
        let stranger = side(u8::try_from(seed).unwrap(), &shutdown);
        assert!(answered(&stranger, &server).await);
    }
    let late = side(9, &shutdown);
    let hello = request(&server, b"hello".to_vec());
    let result = late.quic.send(&hello, &client()).await;
    assert_eq!(result, Err(Failure::NotDelivered));
    assert!(late.quic.backing_off(&server.id));
    assert_eq!(server.quic.places(), Some(counts(0, 0, ANYONE)));
    // Refused again once the first wait is over, it waits twice as long:
    // the handshake the server let through ends no backoff. Refused after
    // it asked, it still waits half way between the first wait and the
    // second, however long the refusal took to come.
    until(|| !late.quic.backing_off(&server.id)).await;
    let asked = tokio::time::Instant::now();
    let result = late.quic.send(&hello, &client()).await;
    assert_eq!(result, Err(Failure::NotDelivered));
    tokio::time::sleep_until(asked + BACKOFF_MIN * 3 / 2).await;
    assert!(late.quic.backing_off(&server.id), "the wait did not double");
    drop(classes);
    shutdown.cancel();
}

/// The network between a node and a peer of it that crashes and comes
/// back: one address, behind which the peer runs, and which peer runs there
/// now. It carries packets between the node and that peer only, so nothing
/// a peer that crashed sends reaches the node any more.
struct Route {
    endpoint: Endpoint,
    peer: Arc<std::sync::Mutex<SocketAddr>>,
}

impl Route {
    /// The route from `node` to `peer`, until it leads elsewhere.
    async fn new(node: &Side, peer: &Side, shutdown: &CancellationToken) -> Self {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint::parse(&socket.local_addr().unwrap().to_string()).unwrap();
        let node = node.quic.local_addr().unwrap();
        let peer = Arc::new(std::sync::Mutex::new(peer.quic.local_addr().unwrap()));
        let to = Arc::clone(&peer);
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut packet = vec![0; 65_536];
            loop {
                let (length, from) = tokio::select! {
                    () = shutdown.cancelled() => return,
                    received = socket.recv_from(&mut packet) => received.unwrap(),
                };
                let peer = *to.lock().unwrap();
                let destination = match from {
                    from if from == node => peer,
                    from if from == peer => node,
                    _ => continue,
                };
                let received = packet.get(..length).unwrap();
                // A packet that cannot be sent is lost, as on any network.
                let _ = socket.send_to(received, destination).await;
            }
        });
        Self { endpoint, peer }
    }

    /// From now on, the route leads to `peer`.
    fn lead_to(&self, peer: &Side) {
        *self.peer.lock().unwrap() = peer.quic.local_addr().unwrap();
    }
}

// A peer that crashed, and so closed nothing, ends our connection to it as
// soon as it is back: its connection IDs and the key for its stateless
// resets derive from its node key, so the node back knows our packets for
// a connection it no longer has, and tells us so. We connect again at once,
// rather than wait out the idle timeout on the dead connection.
#[tokio::test]
async fn a_peer_back_from_a_crash_ends_our_old_connection_at_once() {
    let shutdown = CancellationToken::new();
    let crashed = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &crashed));
    let route = Route::new(&a, &b, &shutdown).await;
    let mut hello = request(&b, b"hello".to_vec());
    hello.endpoint = Some(route.endpoint.clone());
    let client = client();
    let response = a.quic.send(&hello, &client).await.unwrap();
    assert_eq!(response.body, b"hello");
    // b crashes: what it sends never reaches a again, the close of its
    // connections included. It comes back with its key, at its address.
    let back = side(2, &shutdown);
    route.lead_to(&back);
    crashed.cancel();
    drop(b);
    // a asks on the connection it kept, and hears at once that it is gone,
    // within the request's time, which it would otherwise wait out.
    let started = tokio::time::Instant::now();
    let lost = a.quic.send(&hello, &client).await;
    assert!(
        started.elapsed() < std::time::Duration::from_millis(hello.timeout_ms),
        "the old connection was not reset: {lost:?} after {:?}",
        started.elapsed(),
    );
    let response = a.quic.send(&hello, &client).await.unwrap();
    assert_eq!(response.body, b"hello");
    assert_eq!(back.quic.stats().answered.load(Ordering::Relaxed), 1);
    shutdown.cancel();
}

// spec/decisions/promotion-to-core.md: a fresh request is dialled at its
// endpoint on a connection of its own, never the one the node opened to us.
// b holds the connection a opened: a request to a at an endpoint nothing
// serves still reaches a over it, but a fresh one is not delivered; a fresh
// one at a's own endpoint is answered, on a connection b neither keeps nor
// takes the place of the one a opened.
#[tokio::test]
async fn a_fresh_request_dials_its_endpoint() {
    let shutdown = CancellationToken::new();
    let (a, b) = (side(1, &shutdown), side(2, &shutdown));
    let client = client();
    a.quic
        .send(&request(&b, b"open".to_vec()), &client)
        .await
        .unwrap();
    let nowhere = Endpoint::parse("127.0.0.1:9").unwrap();
    let held = Request {
        endpoint: Some(nowhere.clone()),
        ..request(&a, b"held".to_vec())
    };
    let response = send_peer(Some(Arc::clone(&b.quic)), &client, held)
        .await
        .unwrap();
    assert_eq!(response.body, b"held", "over the connection a opened");
    let fresh = Request {
        endpoint: Some(nowhere),
        fresh: true,
        timeout_ms: 200,
        ..request(&a, b"fresh".to_vec())
    };
    let result = send_peer(Some(Arc::clone(&b.quic)), &client, fresh).await;
    assert!(result.is_err(), "nothing serves there: {result:?}");
    let fresh = Request {
        fresh: true,
        ..request(&a, b"at a".to_vec())
    };
    let response = send_peer(Some(Arc::clone(&b.quic)), &client, fresh)
        .await
        .unwrap();
    assert_eq!(response.body, b"at a");
    // The connection kept for a is still the one a opened, not the fresh
    // one: a request at an endpoint nothing serves reaches a over it.
    let held = Request {
        endpoint: Some(Endpoint::parse("127.0.0.1:9").unwrap()),
        ..request(&a, b"still".to_vec())
    };
    let response = send_peer(Some(Arc::clone(&b.quic)), &client, held)
        .await
        .unwrap();
    assert_eq!(response.body, b"still");
    shutdown.cancel();
}
