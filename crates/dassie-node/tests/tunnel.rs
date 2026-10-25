//! QUIC over a WebSocket, between endpoints on loopback
//! (spec/areas/peer-protocol.md#quic-over-websocket): a periphery node whose
//! UDP does not reach its uplink connects over a tunnel, which its uplink
//! sends and relays on as on any connection; it keeps UDP where UDP gets
//! through; the uplink's `NO_ROOM` reaches it; the tunnel and its connection
//! end together; and a node that does not serve tunnels is a failed dial.
//!
//! The uplink's URL names a port whose UDP socket drops everything, as a
//! filter would, while its QUIC endpoint listens elsewhere, and a TLS
//! listener with a self-signed certificate on the URL's port hands the
//! WebSockets at `/quic` to it, as a node's HTTPS listener does.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // tungstenite's handshake callback answers a refusal with a response.
    clippy::result_large_err
)]

use std::future::Future;
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_identity::{NodeId, NodeKey};
use dassie_node::peer_client::PeerClient;
use dassie_node::peer_server::Answer;
use dassie_node::quic::relay::{Purpose, ServedStream};
use dassie_node::quic::{Body, Handler, Quic, tunnel};
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use dassie_peer::wire::Kind;
use dassie_peer::{Failure, Request as PeerRequest, RequestId};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_util::sync::CancellationToken;

/// Answers peer requests with their body, and echoes a relayed stream for
/// payments; room for `anyone` connections.
struct Echo {
    anyone: usize,
}

impl Handler for Echo {
    fn client(&self, _address: IpAddr) -> Option<ClientKey> {
        None
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: self.anyone,
            max_per_client: 16,
        }
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
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
    _identity: watch::Sender<Option<Arc<NodeKey>>>,
}

fn side_on(seed: u8, socket: UdpSocket, anyone: usize, shutdown: &CancellationToken) -> Side {
    let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
    let id = key.node_id();
    let (identity, receiver) = watch::channel(Some(key));
    let quic = Arc::new(Quic::new(socket, receiver, true).unwrap());
    let address = quic.local_addr().unwrap();
    tokio::spawn(Arc::clone(&quic).serve(Arc::new(Echo { anyone }), shutdown.clone()));
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
        ROOMY,
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
        ROOMY,
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
    let (tcp, at_port) = tcp_and_udp();
    let (socket, blackhole) = match udp {
        Udp::Open => (at_port, None),
        Udp::Blocked => (UdpSocket::bind("127.0.0.1:0").unwrap(), Some(at_port)),
    };
    let side = side_on(seed, socket, anyone, shutdown);
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
/// `quic`, answers 404, as a node of an earlier release does.
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
                    let slot = quic.tunnel_slot(None).unwrap();
                    quic.serve_tunnel(ws, peer, slot);
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

fn client() -> PeerClient {
    // QUIC trusts no roots; the client only resolves names here.
    let roots = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/pebble/root.pem"
    );
    PeerClient::new(Some(Path::new(roots)), true).unwrap()
}

fn request(to: NodeId, url: &str, timeout_ms: u64) -> PeerRequest {
    PeerRequest {
        id: RequestId(1),
        to,
        url: url.to_owned(),
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

/// Waits up to `within` for `done`.
async fn until(within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "not within {within:?}");
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
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(uplink.accepted.load(Ordering::SeqCst), 1);
    assert_eq!(downlink.quic.open_tunnels(), 1);
    until(Duration::from_secs(1), || {
        uplink.side.quic.open_tunnels() == 1
    })
    .await;
    // The uplink reaches it on the connection it opened.
    let response = uplink.side.quic.send_held(&to_periphery(&downlink)).await;
    assert_eq!(response.unwrap().body, b"hello");
    // A third node reaches it through the uplink, which relays over the
    // tunnel.
    let from = core(3, &shutdown);
    let relay = (uplink.side.id, uplink.side.udp_url.as_str());
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
// and no tunnel is dialled.
#[tokio::test]
async fn where_udp_gets_through_the_race_keeps_it() {
    let shutdown = CancellationToken::new();
    let uplink = uplink(1, Udp::Open, ROOMY, &shutdown);
    let downlink = periphery(2, &shutdown);
    let started = Instant::now();
    send(&downlink, uplink.side.id, &uplink.url).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
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
    assert!(started.elapsed() < Duration::from_secs(4));
    // Only `NO_ROOM` backs off: a connection lost with its tunnel would
    // have been dialled again.
    assert!(downlink.quic.backing_off(&uplink.side.id));
    // Its tunnel ended with the connection.
    until(Duration::from_secs(2), || {
        downlink.quic.open_tunnels() == 0 && uplink.side.quic.open_tunnels() == 0
    })
    .await;
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
    until(Duration::from_secs(1), || {
        uplink.side.quic.places().unwrap().others == 1
    })
    .await;
    assert_eq!(downlink.quic.open_tunnels(), 1);
    let broken = Instant::now();
    for carried in carrying.lock().unwrap().drain(..) {
        carried.abort();
    }
    until(Duration::from_secs(2), || {
        uplink.side.quic.places().unwrap().others == 0
            && uplink.side.quic.open_tunnels() == 0
            && downlink.quic.open_tunnels() == 0
    })
    .await;
    assert!(broken.elapsed() < Duration::from_secs(2));
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
