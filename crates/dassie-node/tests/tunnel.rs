//! QUIC over a WebSocket, between endpoints on loopback
//! (spec/areas/peer-protocol.md#quic-over-websocket): a periphery node whose
//! UDP does not reach its uplink connects over a tunnel, which its uplink
//! sends and relays on as on any connection; it keeps UDP where UDP gets
//! through; a dial too short for the tunnel is not judged; the uplink's
//! `NO_ROOM` reaches it; the tunnel and its connection end together; a
//! node that does not serve tunnels is a failed dial; and a tunnel goes
//! through an HTTP proxy where one is set, unless `NO_PROXY` lists the host.
//! A tunnel holds the place of the connection it carries: one among those
//! being set up, until its connection proves a key within the time to
//! connect, having carried a first packet sooner, and then its
//! connection's, in its class. So a stranger's tunnels count against
//! anyone else's places, and downlinks behind one address are bounded as
//! over UDP. The HTTPS listener's side of the bounds is in `limits.rs`.
//!
//! The uplink's URL names a port whose UDP socket drops everything, as a
//! filter would, while its QUIC endpoint listens elsewhere, and a TLS
//! listener with a self-signed certificate on the URL's port hands the
//! WebSockets at `/quic` to it, as a node's HTTPS listener does, refusing
//! a request with an origin as that listener does: so every dial here shows
//! that a node's client sends none.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // tungstenite's handshake callback answers a refusal with a response.
    clippy::result_large_err
)]

use std::collections::BTreeSet;
use std::future::Future;
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_identity::{IdentityKey, NodeId};
use dassie_kit::ClientKey;
use dassie_node::outbound::Resolver;
use dassie_node::peer_server::Answer;
use dassie_node::quic::places::{Classes, Peerings};
use dassie_node::quic::proxy::{NoProxy, Proxy};
use dassie_node::quic::relay::{Purpose, ServedStream};
use dassie_node::quic::{
    Body, CONNECT_TIMEOUT, Handler, IDLE_TIMEOUT, Quic, TUNNEL_HEAD_START, tunnel,
};
use dassie_node_core::connections::ConnectionLimits;
use dassie_peer::wire::{Endpoint, Kind};
use dassie_peer::{Failure, Request as PeerRequest, RequestId};
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_util::sync::CancellationToken;

/// Answers peer requests with their body, and echoes a relayed stream for
/// payments; room for `anyone` connections, being set up and of anyone
/// else.
struct Echo {
    anyone: usize,
    /// Each client address's share of those, counted on loopback too; or
    /// none, counted in the total alone.
    per_client: Option<u32>,
    /// The peerings, which give downlinks places of their own.
    peerings: Option<Peerings>,
}

impl Echo {
    /// Room for `anyone` connections, in the total alone.
    const fn anyone(anyone: usize) -> Self {
        Self {
            anyone,
            per_client: None,
            peerings: None,
        }
    }
}

impl Handler for Echo {
    fn client(&self, address: IpAddr) -> Option<ClientKey> {
        self.per_client.map(|_| ClientKey::from_ip(address))
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: self.anyone,
            max_per_client: self.per_client.unwrap_or(16),
        }
    }

    fn peerings(&self) -> Option<Peerings> {
        self.peerings.clone()
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        _from: dassie_identity::NodeIdentity,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async move {
            Answer {
                status: 200,
                body: body.await.unwrap_or_default(),
            }
        })
    }

    fn serves(&self, purpose: Purpose) -> bool {
        purpose == Purpose::Payments
    }

    fn serve_relayed<'a>(
        &'a self,
        _purpose: Purpose,
        _client: NodeId,
        _relay: SocketAddr,
        stream: ServedStream,
        _shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let (mut reader, mut writer) = tokio::io::split(stream);
            let _ = tokio::io::copy(&mut reader, &mut writer).await;
            let _ = writer.shutdown().await;
        })
    }
}

/// Room enough for every test's connections.
const ROOMY: usize = 16;

struct Side {
    quic: Arc<Quic>,
    id: NodeId,
    /// Its QUIC endpoint's own address, over UDP.
    udp_url: String,
    _identity: watch::Sender<Option<Arc<IdentityKey>>>,
}

fn side_on(seed: u8, socket: UdpSocket, handler: Echo, shutdown: &CancellationToken) -> Side {
    side_behind(seed, socket, handler, None, shutdown)
}

/// A node whose tunnels go through `proxy`, if any.
fn side_behind(
    seed: u8,
    socket: UdpSocket,
    handler: Echo,
    proxy: Option<Proxy>,
    shutdown: &CancellationToken,
) -> Side {
    let key = Arc::new(IdentityKey::from_bytes(&[seed; 32]));
    let id = key.node_id();
    let (identity, receiver) = watch::channel(Some(key));
    let quic = Quic::new(socket, receiver, true)
        .unwrap()
        .with_tunnel_proxy(proxy);
    let quic = Arc::new(quic);
    let address = quic.local_addr().unwrap();
    tokio::spawn(Arc::clone(&quic).serve(Arc::new(handler), shutdown.clone()));
    Side {
        quic,
        id,
        udp_url: format!("https://{address}"),
        _identity: identity,
    }
}

/// A node without a URL, which dials over a tunnel where UDP fails.
fn periphery(seed: u8, shutdown: &CancellationToken) -> Side {
    let side = side_on(
        seed,
        UdpSocket::bind("127.0.0.1:0").unwrap(),
        Echo::anyone(ROOMY),
        shutdown,
    );
    side.quic.set_periphery(true);
    side
}

/// A node with a URL, which dials over UDP only.
fn core(seed: u8, shutdown: &CancellationToken) -> Side {
    side_on(
        seed,
        UdpSocket::bind("127.0.0.1:0").unwrap(),
        Echo::anyone(ROOMY),
        shutdown,
    )
}

/// A TCP listener and a UDP socket on the same port number.
fn tcp_and_udp() -> (TcpListener, UdpSocket) {
    for _ in 0..16 {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        if let Ok(udp) = UdpSocket::bind(tcp.local_addr().unwrap()) {
            return (tcp, udp);
        }
    }
    panic!("no port free on both TCP and UDP");
}

/// How an uplink's UDP stands at its URL's port.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Udp {
    /// Its QUIC endpoint is there.
    Open,
    /// A socket there drops everything; the endpoint is elsewhere.
    Blocked,
}

/// A core node, whose URL's port takes TCP for its tunnels.
struct Uplink {
    side: Side,
    /// Its URL, whose port takes its tunnels over TCP.
    url: String,
    /// TCP connections its tunnel listener took.
    accepted: Arc<AtomicUsize>,
    _blackhole: Option<UdpSocket>,
}

fn uplink(seed: u8, udp: Udp, anyone: usize, shutdown: &CancellationToken) -> Uplink {
    uplink_with(seed, udp, Echo::anyone(anyone), shutdown)
}

/// An uplink whose endpoint answers with `handler`.
fn uplink_with(seed: u8, udp: Udp, handler: Echo, shutdown: &CancellationToken) -> Uplink {
    let (tcp, at_port) = tcp_and_udp();
    let (socket, blackhole) = match udp {
        Udp::Open => (at_port, None),
        Udp::Blocked => (UdpSocket::bind("127.0.0.1:0").unwrap(), Some(at_port)),
    };
    let side = side_on(seed, socket, handler, shutdown);
    let url = format!("https://{}", tcp.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    listen(tcp, Some(Arc::clone(&side.quic)), Arc::clone(&accepted));
    Uplink {
        side,
        url,
        accepted,
        _blackhole: blackhole,
    }
}

/// Serves TLS with a self-signed certificate on `tcp`, and hands each
/// WebSocket at `/quic` with the tunnel's subprotocol to `quic`; with no
/// `quic`, answers 404, as a node of an earlier release does. A request
/// with an origin gets 403, as from a node's listener.
fn listen(tcp: TcpListener, quic: Option<Arc<Quic>>, accepted: Arc<AtomicUsize>) {
    tcp.set_nonblocking(true).unwrap();
    let tcp = tokio::net::TcpListener::from_std(tcp).unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(self_signed());
    tokio::spawn(async move {
        while let Ok((stream, peer)) = tcp.accept().await {
            accepted.fetch_add(1, Ordering::SeqCst);
            let (acceptor, quic) = (acceptor.clone(), quic.clone());
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(stream).await else {
                    return;
                };
                let serves = quic.is_some();
                let handshake = move |request: &Request, mut response: Response| {
                    if !serves || request.uri().path() != tunnel::PATH {
                        let mut refused = ErrorResponse::new(None);
                        *refused.status_mut() = StatusCode::NOT_FOUND;
                        return Err(refused);
                    }
                    // As a node's listener refuses a browser's: so every
                    // dial here shows that a node's client sends none.
                    if request.headers().contains_key("origin") {
                        let mut refused = ErrorResponse::new(None);
                        *refused.status_mut() = StatusCode::FORBIDDEN;
                        return Err(refused);
                    }
                    response.headers_mut().insert(
                        "sec-websocket-protocol",
                        HeaderValue::from_static(tunnel::SUBPROTOCOL),
                    );
                    Ok(response)
                };
                let config = Some(tunnel::websocket_config());
                let Ok(ws) =
                    tokio_tungstenite::accept_hdr_async_with_config(tls, handshake, config).await
                else {
                    return;
                };
                if let Some(quic) = quic {
                    // Past the places among the connections being set up,
                    // the tunnel ends, as a node's listener answers 503.
                    if let Ok(slot) = quic.tunnel_slot(peer.ip()) {
                        quic.serve_tunnel(ws, peer, slot);
                    }
                }
            });
        }
    });
}

/// A TLS server's settings with a self-signed certificate for `localhost`,
/// which nobody's roots hold.
fn self_signed() -> Arc<rustls::ServerConfig> {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let key = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
    let mut config = rustls::ServerConfig::builder_with_provider(dassie_node::tls::provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(certified.cert.der().to_vec())],
            key.into(),
        )
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

/// QUIC trusts no roots: each connection pins its node's key, and the
/// resolver only resolves names, private ones included.
fn client() -> Resolver {
    Resolver::new(true)
}

fn request(to: NodeId, url: &str, timeout_ms: u64) -> PeerRequest {
    PeerRequest {
        id: RequestId(1),
        to,
        // Without a URL, as to a periphery node.
        endpoint: Endpoint::of_url(url),
        kind: Kind::LinkStateRequest,
        body: b"hello".to_vec(),
        timeout_ms,
    }
}

/// A request to `to` as to a periphery node: without a URL.
fn to_periphery(to: &Side) -> PeerRequest {
    request(to.id, "", 5_000)
}

/// Sends `from`'s node a peer request to `to` at `url`, over QUIC.
async fn send(from: &Side, to: NodeId, url: &str) -> Result<Vec<u8>, Failure> {
    let response = from.quic.send(&request(to, url, 5_000), &client()).await?;
    assert_eq!(response.status, 200);
    Ok(response.body)
}

/// How long a wait here may take before the test calls it a hang: twice
/// the idle timeout, the longest of the timers any wait here depends on. Tunnels and QUIC
/// connections run on real time, as QUIC's own timers do, so the tests
/// wait for them in real time; but what a test waits for comes in a moment
/// on any machine, and a slow one never reaches this.
const HANG: Duration = Duration::from_secs(2 * IDLE_TIMEOUT.as_secs());

/// Waits for `done`, a hang aside.
async fn until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + HANG;
    while !done() {
        assert!(Instant::now() < deadline, "not within {HANG:?}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// A periphery node whose UDP does not reach its uplink connects over a
// tunnel. The uplink sends on that connection, as on any its downlink
// opened, and relays to the node over it.
#[tokio::test]
async fn a_periphery_node_reaches_its_uplink_over_a_tunnel_where_udp_is_blocked() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let downlink = periphery(2, &shutdown);
    let started = Instant::now();
    let body = send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    assert_eq!(body, b"hello");
    // UDP had its head start first.
    assert!(started.elapsed() >= TUNNEL_HEAD_START);
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 1);
    assert_eq!(downlink.quic.open_tunnels(), 1);
    until(|| uplink.side.quic.open_tunnels() == 1).await;
    // The uplink reaches it on the connection it opened.
    let response = uplink.side.quic.send_held(&to_periphery(&downlink)).await;
    assert_eq!(response.unwrap().body, b"hello");
    // A third node reaches it through the uplink, which relays over the
    // tunnel.
    let from = core(3, &shutdown);
    let udp = Endpoint::of_url(&uplink.side.udp_url).unwrap();
    let relay = (uplink.side.id, &udp);
    let mut stream = from
        .quic
        .open_relayed(relay, downlink.id, Purpose::Payments, &client())
        .await
        .unwrap();
    stream.write_all(b"through the tunnel").await.unwrap();
    stream.flush().await.unwrap();
    let mut back = vec![0; b"through the tunnel".len()];
    stream.read_exact(&mut back).await.unwrap();
    assert_eq!(back, b"through the tunnel");
    // Further requests use the connection kept.
    send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 1);
    shutdown.cancel();
}

// Where UDP gets through, its handshake wins well within the head start,
// and no tunnel is dialled. A handshake on loopback takes a few
// milliseconds, beside the production head start of a second.
#[tokio::test]
async fn where_udp_gets_through_the_race_keeps_it() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Open, ROOMY, &shutdown);
    let downlink = periphery(2, &shutdown);
    send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    let response = uplink.side.quic.send_held(&to_periphery(&downlink)).await;
    assert!(response.is_ok(), "{response:?}");
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 0);
    assert_eq!(downlink.quic.open_tunnels(), 0);
    assert_eq!(uplink.side.quic.open_tunnels(), 0);
    shutdown.cancel();
}

// A node with a URL never dials a tunnel: where its UDP is blocked, it
// cannot reach the node.
#[tokio::test]
async fn a_core_node_never_dials_a_tunnel() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let other = core(2, &shutdown);
    let request = request(uplink.side.id, &uplink.url, 2_000);
    let response = other.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 0);
    shutdown.cancel();
}

// A periphery node's dial whose time runs out within UDP's head start never
// tried the tunnel, so it says nothing of the node: it does not back off,
// and the next request with time enough gets through.
#[tokio::test]
async fn a_dial_too_short_for_the_tunnel_does_not_back_off() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let downlink = periphery(2, &shutdown);
    let request = request(uplink.side.id, &uplink.url, 800);
    let response = downlink.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 0);
    assert!(!downlink.quic.backing_off(&uplink.side.id));
    send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 1);
    shutdown.cancel();
}

// An uplink with no room closes the tunnelled connection with `NO_ROOM`,
// which reaches the periphery node before the tunnel ends: it backs off,
// as from a node refusing over UDP.
#[tokio::test]
async fn no_room_reaches_a_periphery_node_over_its_tunnel() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, 1, &shutdown);
    // Another node holds the only place, over UDP.
    let other = core(3, &shutdown);
    send(&other, uplink.side.id, &uplink.side.udp_url)
        .await
        .unwrap();
    let downlink = periphery(2, &shutdown);
    let request = request(uplink.side.id, &uplink.url, 5_000);
    let started = Instant::now();
    let response = downlink.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 1);
    // Refused once its handshake was done, not a dial that failed, which
    // would have waited out the time to connect, UDP's being dropped.
    assert!(started.elapsed() < CONNECT_TIMEOUT);
    // Only `NO_ROOM` backs off: a connection lost with its tunnel would
    // have been dialled again.
    assert!(downlink.quic.backing_off(&uplink.side.id));
    // Its tunnel ended with the connection.
    until(|| downlink.quic.open_tunnels() == 0 && uplink.side.quic.open_tunnels() == 0).await;
    shutdown.cancel();
}

/// A TCP relay on a port whose UDP drops everything, to `to`; the tasks
/// that carry its connections, which the test may end.
fn proxy(to: SocketAddr) -> (String, UdpSocket, Arc<Mutex<Vec<JoinHandle<()>>>>) {
    let (tcp, blackhole) = tcp_and_udp();
    let url = format!("https://{}", tcp.local_addr().unwrap());
    tcp.set_nonblocking(true).unwrap();
    let tcp = tokio::net::TcpListener::from_std(tcp).unwrap();
    let carrying = Arc::new(Mutex::new(Vec::new()));
    let tasks = Arc::clone(&carrying);
    tokio::spawn(async move {
        while let Ok((mut inbound, _)) = tcp.accept().await {
            let carried = tokio::spawn(async move {
                let mut outbound = tokio::net::TcpStream::connect(to).await.unwrap();
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            });
            tasks.lock().unwrap().push(carried);
        }
    });
    (url, blackhole, carrying)
}

// A tunnel whose TCP connection breaks closes its QUIC connection at once,
// on both sides, though keep-alives would keep it open for good, and the
// next dial makes a new tunnel.
#[tokio::test]
async fn a_tunnel_that_ends_closes_its_connection_at_once() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let tunnel_address = uplink
        .url
        .strip_prefix("https://")
        .unwrap()
        .parse()
        .unwrap();
    let (url, _blackhole, carrying) = proxy(tunnel_address);
    let downlink = periphery(2, &shutdown);
    send(&downlink, uplink.side.id, &url).await.unwrap();
    until(|| uplink.side.quic.places().unwrap().others == 1).await;
    assert_eq!(downlink.quic.open_tunnels(), 1);
    let broken = Instant::now();
    for carried in carrying.lock().unwrap().drain(..) {
        carried.abort();
    }
    until(|| {
        uplink.side.quic.places().unwrap().others == 0
            && uplink.side.quic.open_tunnels() == 0
            && downlink.quic.open_tunnels() == 0
    })
    .await;
    // Well before the idle timeout, which would close it otherwise: nothing
    // gets through a tunnel that broke, keep-alives included. QUIC counts
    // that timeout from the last packet a side received, which came just
    // before the tunnel broke, so the idle close would come a little short
    // of an idle timeout after `broken`; half of it rules that out with
    // room on both sides.
    assert!(broken.elapsed() < IDLE_TIMEOUT / 2);
    let response = uplink.side.quic.send_held(&to_periphery(&downlink)).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    // The next request dials again.
    send(&downlink, uplink.side.id, &url).await.unwrap();
    assert_eq!(downlink.quic.open_tunnels(), 1);
    shutdown.cancel();
}

// A node that answers `/quic` with 404, as one of an earlier release does,
// is a failed dial: the request is not delivered, and the node is backed
// off from.
#[tokio::test]
async fn a_node_that_does_not_serve_tunnels_is_a_failed_dial() {
    let shutdown = CancellationToken::new();
    let earlier = core(1, &shutdown);
    let (tcp, _blackhole) = tcp_and_udp();
    let url = format!("https://{}", tcp.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    listen(tcp, None, Arc::clone(&accepted));
    let downlink = periphery(2, &shutdown);
    let request = request(earlier.id, &url, 2_000);
    let response = downlink.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert!(downlink.quic.backing_off(&earlier.id));
    assert_eq!(downlink.quic.open_tunnels(), 0);
    shutdown.cancel();
}

/// A tunnel into `quic` from `peer`, upgraded already, as an HTTPS listener
/// hands it over once it took its place among the connections being set
/// up; the client's end of its WebSocket.
async fn raw_tunnel(quic: &Quic, peer: SocketAddr) -> WebSocketStream<DuplexStream> {
    let (theirs, ours) = tokio::io::duplex(64 * 1024);
    let config = Some(tunnel::websocket_config());
    let served = WebSocketStream::from_raw_socket(theirs, Role::Server, config).await;
    let ws = WebSocketStream::from_raw_socket(ours, Role::Client, config).await;
    until(|| quic.is_serving()).await;
    quic.serve_tunnel(served, peer, quic.tunnel_slot(peer.ip()).unwrap());
    ws
}

// A tunnel holds a place among the connections being set up from its
// upgrade, and ends, giving it back, unless its connection proved a key
// within the time to connect: here none ever arrives on it, though it
// carries a packet, which the endpoint drops.
#[tokio::test]
async fn a_tunnel_without_a_connection_ends_after_the_time_to_connect() {
    let shutdown = CancellationToken::new();
    let uplink = core(1, &shutdown);
    let quic = &uplink.quic;
    let started = Instant::now();
    let mut ws = raw_tunnel(quic, "127.0.0.1:40000".parse().unwrap()).await;
    assert_eq!(quic.open_tunnels(), 1);
    assert_eq!(quic.places().unwrap().setting_up, 1);
    ws.send(Message::binary(vec![0; 64])).await.unwrap();
    let ended = async { while let Some(Ok(_)) = ws.next().await {} };
    // Nothing else would end it.
    tokio::time::timeout(HANG, ended)
        .await
        .expect("a tunnel without a connection stayed open");
    assert!(started.elapsed() >= CONNECT_TIMEOUT - Duration::from_millis(500));
    until(|| quic.open_tunnels() == 0).await;
    assert_eq!(quic.places().unwrap().setting_up, 0);
    shutdown.cancel();
}

// A tunnel that carries no packet at all ends, giving its place back,
// after a round trip's bound, well within the time to connect: a node's
// client sends its first packet as soon as its upgrade completes.
#[tokio::test]
async fn a_tunnel_that_carries_no_packet_ends_sooner() {
    let shutdown = CancellationToken::new();
    let uplink = core(1, &shutdown);
    let quic = &uplink.quic;
    let started = Instant::now();
    let mut ws = raw_tunnel(quic, "127.0.0.1:40000".parse().unwrap()).await;
    assert_eq!(quic.places().unwrap().setting_up, 1);
    let ended = async { while let Some(Ok(_)) = ws.next().await {} };
    // Half way between the two timers: the tunnel's own deadline starts a
    // moment after this one, so the time to connect itself would leave the
    // two to race.
    let between = (tunnel::FIRST_PACKET_TIMEOUT + CONNECT_TIMEOUT) / 2;
    tokio::time::timeout(between, ended)
        .await
        .expect("a tunnel without a packet stayed open as long as one without a connection");
    let elapsed = started.elapsed();
    assert!(elapsed >= tunnel::FIRST_PACKET_TIMEOUT - Duration::from_millis(100));
    until(|| quic.open_tunnels() == 0).await;
    assert_eq!(quic.places().unwrap().setting_up, 0);
    shutdown.cancel();
}

// A stranger's tunnel holds anyone else's place once its connection proved
// a key, and that place alone: a stranger over UDP then finds no room.
#[tokio::test]
async fn a_strangers_tunnel_counts_against_anyone_elses_places() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, 1, &shutdown);
    let downlink = periphery(2, &shutdown);
    send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    assert_eq!(downlink.quic.open_tunnels(), 1);
    let places = uplink.side.quic.places().unwrap();
    assert_eq!((places.setting_up, places.others), (0, 1));
    let other = core(3, &shutdown);
    let request = request(uplink.side.id, &uplink.side.udp_url, 5_000);
    let response = other.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    // Refused for room, not unreached.
    assert!(other.quic.backing_off(&uplink.side.id));
    assert_eq!(uplink.side.quic.places().unwrap().others, 1);
    shutdown.cancel();
}

// Downlinks behind one address, over tunnels, each hold a downlink's
// place, as over UDP: their address's share of the connections being set
// up and of anyone else's bounds the strangers there, not them, and no
// count of tunnels per address bounds either.
#[tokio::test]
async fn downlinks_behind_one_address_are_bounded_as_over_udp() {
    const DOWNLINKS: u8 = 4;
    let shutdown = CancellationToken::new();
    let downlinks: Vec<Side> = (0..DOWNLINKS)
        .map(|index| periphery(10 + index, &shutdown))
        .collect();
    let classes = Classes {
        core: BTreeSet::new(),
        downlinks: downlinks.iter().map(|side| side.id).collect(),
    };
    let (_peerings, receiver) = watch::channel(Arc::new(classes));
    let handler = Echo {
        anyone: 1,
        per_client: Some(1),
        peerings: Some(Peerings {
            classes: receiver,
            max_core_peers: 1,
            max_downlinks: usize::from(DOWNLINKS),
        }),
    };
    let uplink = uplink_with(1, Udp::Blocked, handler, &shutdown);
    for downlink in &downlinks {
        send(downlink, uplink.side.id, &uplink.url).await.unwrap();
    }
    let places = uplink.side.quic.places().unwrap();
    assert_eq!(places.downlinks, usize::from(DOWNLINKS));
    assert_eq!(uplink.side.quic.open_tunnels(), usize::from(DOWNLINKS));
    // A stranger from the same address takes its address's one place of
    // anyone else's, and the next finds none.
    let stranger = periphery(2, &shutdown);
    send(&stranger, uplink.side.id, &uplink.url).await.unwrap();
    assert_eq!(uplink.side.quic.places().unwrap().others, 1);
    let next = periphery(3, &shutdown);
    let request = request(uplink.side.id, &uplink.url, 5_000);
    let response = next.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    assert!(next.quic.backing_off(&uplink.side.id));
    // Every downlink is still reached on the connection it opened.
    for downlink in &downlinks {
        let response = uplink.side.quic.send_held(&to_periphery(downlink)).await;
        assert_eq!(response.unwrap().body, b"hello");
    }
    shutdown.cancel();
}

/// An HTTP proxy on loopback that reads a `CONNECT` and answers `status`;
/// on a 2xx, it carries the stream to the address asked for. Its address,
/// and the request heads it read.
fn http_proxy(status: u16) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = tcp.local_addr().unwrap();
    tcp.set_nonblocking(true).unwrap();
    let tcp = tokio::net::TcpListener::from_std(tcp).unwrap();
    let heads = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&heads);
    tokio::spawn(async move {
        while let Ok((mut inbound, _)) = tcp.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    assert!(head.len() < 8192);
                    head.push(inbound.read_u8().await.unwrap());
                }
                let head = String::from_utf8(head).unwrap();
                let target = head.split(' ').nth(1).unwrap().to_owned();
                seen.lock().unwrap().push(head);
                let answer = format!("HTTP/1.1 {status} Proxy says so\r\nVia: test\r\n\r\n");
                inbound.write_all(answer.as_bytes()).await.unwrap();
                if (200..300).contains(&status) {
                    let mut outbound = tokio::net::TcpStream::connect(target).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                }
            });
        }
    });
    (address, heads)
}

/// A periphery node whose tunnels go through the proxy at `address`, as
/// `user:pass`, unless `no_proxy` lists the host.
fn periphery_behind(
    seed: u8,
    address: SocketAddr,
    no_proxy: &str,
    shutdown: &CancellationToken,
) -> Side {
    let url = format!("http://user:pass@{address}");
    let (bypass, problem) = NoProxy::parse(no_proxy);
    assert_eq!(problem, None);
    let proxy = Proxy::parse(&url, bypass).unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let side = side_behind(seed, socket, Echo::anyone(ROOMY), Some(proxy), shutdown);
    side.quic.set_periphery(true);
    side
}

// Where nothing gets out but through an HTTP proxy, a periphery node's
// tunnel asks the proxy to `CONNECT` it to its uplink's host and port, with
// its credentials, and the tunnel works through it as without one.
#[tokio::test]
async fn a_tunnel_goes_through_an_http_proxy() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let (address, heads) = http_proxy(200);
    let downlink = periphery_behind(2, address, "", &shutdown);
    let body = send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    assert_eq!(body, b"hello");
    let authority = uplink.url.strip_prefix("https://").unwrap();
    let heads = heads.lock().unwrap().clone();
    assert_eq!(
        heads,
        vec![format!(
            "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\
             Proxy-Authorization: Basic dXNlcjpwYXNz\r\n\r\n"
        )]
    );
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 1);
    assert_eq!(downlink.quic.open_tunnels(), 1);
    // The uplink reaches it on the connection it opened, through the proxy.
    let response = uplink.side.quic.send_held(&to_periphery(&downlink)).await;
    assert_eq!(response.unwrap().body, b"hello");
    shutdown.cancel();
}

// A proxy that refuses to connect, here for want of credentials, is a
// failed dial: the tunnel never reaches the node.
#[tokio::test]
async fn a_proxy_that_refuses_is_a_failed_dial() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let (address, heads) = http_proxy(407);
    let downlink = periphery_behind(2, address, "", &shutdown);
    let request = request(uplink.side.id, &uplink.url, 2_000);
    let response = downlink.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    assert_eq!(heads.lock().unwrap().len(), 1);
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 0);
    assert!(downlink.quic.backing_off(&uplink.side.id));
    assert_eq!(downlink.quic.open_tunnels(), 0);
    shutdown.cancel();
}

// A host `NO_PROXY` lists is dialled directly, though a proxy is set.
#[tokio::test]
async fn a_host_no_proxy_lists_is_dialled_directly() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let (address, heads) = http_proxy(200);
    let downlink = periphery_behind(2, address, "localhost,127.0.0.0/8", &shutdown);
    send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    assert!(heads.lock().unwrap().is_empty());
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 1);
    shutdown.cancel();
}

// A proxy that takes the connection and never answers holds the dial no
// longer than the dial's time: it is a failed dial, and the tunnel never
// reaches the node.
#[tokio::test]
async fn a_proxy_that_never_answers_is_a_failed_dial_in_time() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Blocked, ROOMY, &shutdown);
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = tcp.local_addr().unwrap();
    let held = Arc::new(Mutex::new(Vec::new()));
    let holding = Arc::clone(&held);
    tokio::spawn(async move {
        while let Ok((inbound, _)) = tcp.accept().await {
            holding.lock().unwrap().push(inbound);
        }
    });
    let downlink = periphery_behind(2, address, "", &shutdown);
    let request = request(uplink.side.id, &uplink.url, 2_000);
    let started = Instant::now();
    let response = downlink.quic.send(&request, &client()).await;
    assert_eq!(response.err(), Some(Failure::NotDelivered));
    // Held at most the time to connect, here the request's shorter time:
    // were the proxy's silence to hold the dial, it would never end.
    assert!(started.elapsed() < CONNECT_TIMEOUT);
    assert_eq!(held.lock().unwrap().len(), 1);
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 0);
    assert_eq!(downlink.quic.open_tunnels(), 0);
    shutdown.cancel();
}
