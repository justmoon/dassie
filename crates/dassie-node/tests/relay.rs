//! Relayed streams over QUIC, on loopback: a client reaches a node through
//! a relay the node keeps a connection to, with TLS end to end, within the
//! relay's bounds; and a node serves only Open Payments on a relayed
//! stream (spec/areas/peer-protocol.md#relayed-streams).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::future::Future;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::{NodeId, NodeKey};
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::http::peer::Answer;
use dassie_node::node::Policies;
use dassie_node::peer_client::PeerClient;
use dassie_node::quic::relay::{
    KeepWhile, MAX_RELAYED_PER_CLIENT_TO_DESTINATION, MAX_RELAYED_PER_DESTINATION, Purpose,
    RelayError, RelayLink, RelayedStream, ServedStream,
};
use dassie_node::quic::{Body, Handler, Quic};
use dassie_node::rpc::proto::{self, request::Method};
use dassie_node_core::config::{Hostname, NodeConfig};
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use dassie_payments::httpsig;
use dassie_peer::wire::Kind;
use dassie_peer::{Request as PeerRequest, RequestId};
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use support::{Bound, START_MS, set_up};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Answers peer requests with 200 and nothing, and echoes what a relayed
/// stream for payments carries; refuses the owner's purpose. Counts each
/// address as a client, loopback too, so that the shares per client apply.
struct Echo;

impl Handler for Echo {
    fn client(&self, address: IpAddr) -> Option<ClientKey> {
        Some(ClientKey::from_ip(address))
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
        _body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async {
            Answer {
                status: 200,
                body: Vec::new(),
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

struct Side {
    quic: Arc<Quic>,
    id: NodeId,
    url: String,
    identity: watch::Sender<Option<Arc<NodeKey>>>,
}

impl Side {
    fn relay(&self) -> (NodeId, &str) {
        (self.id, &self.url)
    }
}

fn side(seed: u8, shutdown: &CancellationToken) -> Side {
    side_on(seed, UdpSocket::bind("127.0.0.1:0").unwrap(), shutdown)
}

fn side_on(seed: u8, socket: UdpSocket, shutdown: &CancellationToken) -> Side {
    side_with(seed, socket, None, shutdown)
}

/// A side on a loopback address of its own, `127.0.0.<last>`, so that the
/// relay counts it as another client.
fn side_at(seed: u8, last: u8, shutdown: &CancellationToken) -> Side {
    let socket = UdpSocket::bind((IpAddr::from([127, 0, 0, last]), 0)).unwrap();
    side_on(seed, socket, shutdown)
}

/// A side whose connections close after `idle` without traffic, if given.
fn side_with(
    seed: u8,
    socket: UdpSocket,
    idle: Option<Duration>,
    shutdown: &CancellationToken,
) -> Side {
    let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
    let id = key.node_id();
    let (identity, receiver) = watch::channel(Some(key));
    let mut quic = Quic::new(socket, receiver, true).unwrap();
    if let Some(idle) = idle {
        quic = quic.with_idle_timeout(idle).unwrap();
    }
    let quic = Arc::new(quic);
    let address = quic.local_addr().unwrap();
    tokio::spawn(Arc::clone(&quic).serve(Arc::new(Echo), shutdown.clone()));
    Side {
        quic,
        id,
        url: format!("https://{address}"),
        identity,
    }
}

/// Sends `from`'s node a peer request to `to`, over QUIC.
async fn send(from: &Side, to: &Side) {
    let request = PeerRequest {
        id: RequestId(1),
        to: to.id,
        url: to.url.clone(),
        quic: true,
        kind: Kind::LinkStateRequest,
        body: b"hello".to_vec(),
        timeout_ms: 5_000,
    };
    let response = from
        .quic
        .send(&request, &client(), Duration::from_secs(5))
        .await;
    assert!(response.is_ok(), "{response:?}");
}

fn client() -> PeerClient {
    // QUIC trusts no roots; the client only resolves names here.
    let roots = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/pebble/root.pem"
    );
    PeerClient::new(Some(Path::new(roots)), true).unwrap()
}

/// `destination` keeps a connection to `relay`, as with `DASSIE_RELAY`.
fn keep(destination: &Side, relay: &Side, shutdown: &CancellationToken) {
    keep_while(destination, relay, KeepWhile::Always, shutdown);
}

/// `destination` keeps a connection to `relay` while `keep` holds.
fn keep_while(destination: &Side, relay: &Side, keep: KeepWhile, shutdown: &CancellationToken) {
    tokio::spawn(Arc::clone(&destination.quic).keep_relay(
        relay.id,
        relay.url.clone(),
        client(),
        keep,
        shutdown.clone(),
    ));
}

/// Opens a relayed stream from `from` to `to` through `relay`, waiting
/// while the relay does not hold `to`'s connection yet.
async fn reach(from: &Side, relay: &Side, to: NodeId, purpose: Purpose) -> RelayedStream {
    for _ in 0..500 {
        match from
            .quic
            .open_relayed(relay.relay(), to, purpose, &client())
            .await
        {
            Ok(stream) => return stream,
            Err(RelayError::UnknownDestination) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("{error}"),
        }
    }
    panic!("the relay never held the destination's connection");
}

async fn echo(stream: &mut RelayedStream, message: &[u8]) -> Vec<u8> {
    stream.write_all(message).await.unwrap();
    stream.flush().await.unwrap();
    let mut back = vec![0; message.len()];
    stream.read_exact(&mut back).await.unwrap();
    back
}

#[tokio::test]
async fn a_client_reaches_a_node_through_its_relay_and_the_relay_sees_only_ciphertext() {
    let shutdown = CancellationToken::new();
    let (relay, destination, from) = (side(1, &shutdown), side(2, &shutdown), side(3, &shutdown));
    let relayed = Arc::new(Mutex::new(Vec::new()));
    let tap = Arc::clone(&relayed);
    relay
        .quic
        .tap_relayed(move |chunk| tap.lock().unwrap().extend_from_slice(chunk));
    keep(&destination, &relay, &shutdown);
    let mut stream = reach(&from, &relay, destination.id, Purpose::Payments).await;
    let secret = b"a secret only the destination may read";
    assert_eq!(echo(&mut stream, secret).await, secret);
    stream.shutdown().await.unwrap();
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty());
    let relayed = relayed.lock().unwrap().clone();
    // The relay copied both ways, handshake and records, and never the
    // plaintext.
    assert!(relayed.len() > 2 * secret.len());
    assert!(!relayed.windows(secret.len()).any(|window| window == secret));
    shutdown.cancel();
}

// spec/decisions/default-relay.md: the default relay, `DASSIE_RELAY` unset,
// is kept only while the node has no hostname: not dialled while it has one,
// dialled once it has none, and closed once it has one again. Only then does
// the node report it.
#[tokio::test]
async fn the_default_relay_is_kept_only_while_the_node_has_no_hostname() {
    let shutdown = CancellationToken::new();
    let (relay, destination, from) = (side(1, &shutdown), side(2, &shutdown), side(3, &shutdown));
    let named = NodeConfig {
        hostname: Some(Hostname::parse("node.example").unwrap()),
        ..NodeConfig::default()
    };
    let (config, watched) = watch::channel(named.clone());
    // Chosen after the endpoint serves, so only for what it reports: the
    // test keeps the relay itself.
    destination.quic.choose_relay(
        relay.id,
        relay.url.clone(),
        client(),
        KeepWhile::NoHostname(watched.clone()),
    );
    keep_while(
        &destination,
        &relay,
        KeepWhile::NoHostname(watched),
        &shutdown,
    );
    assert_eq!(destination.quic.chosen_relay(), None);
    // Time to dial, were it to: the relay never gets a connection.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let unknown = from
        .quic
        .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
        .await;
    assert_eq!(unknown.err(), Some(RelayError::UnknownDestination));
    config.send_replace(NodeConfig::default());
    let mut stream = reach(&from, &relay, destination.id, Purpose::Payments).await;
    assert_eq!(echo(&mut stream, b"ping").await, b"ping");
    let reported = destination.quic.chosen_relay();
    let connected = RelayLink::Connected;
    assert_eq!(reported, Some((relay.id, relay.url.clone(), connected)));
    config.send_replace(named);
    assert_eq!(destination.quic.chosen_relay(), None);
    let mut closed = false;
    for _ in 0..250 {
        let result = from
            .quic
            .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
            .await;
        if result.err() == Some(RelayError::UnknownDestination) {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(closed, "the node kept the default relay with a hostname");
    shutdown.cancel();
}

#[tokio::test]
async fn a_relay_refuses_a_node_it_holds_no_connection_to() {
    let shutdown = CancellationToken::new();
    let (relay, destination, from) = (side(1, &shutdown), side(2, &shutdown), side(3, &shutdown));
    // The destination is up, but not connected to the relay: a relay never
    // dials.
    let result = from
        .quic
        .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
        .await;
    assert_eq!(result.err(), Some(RelayError::UnknownDestination));
    let nobody = NodeKey::from_bytes(&[9; 32]).node_id();
    let result = from
        .quic
        .open_relayed(relay.relay(), nobody, Purpose::Payments, &client())
        .await;
    assert_eq!(result.err(), Some(RelayError::UnknownDestination));
    shutdown.cancel();
}

#[tokio::test]
async fn a_purpose_the_node_does_not_serve_is_refused() {
    let shutdown = CancellationToken::new();
    let (relay, destination, from) = (side(1, &shutdown), side(2, &shutdown), side(3, &shutdown));
    keep(&destination, &relay, &shutdown);
    reach(&from, &relay, destination.id, Purpose::Payments).await;
    let result = from
        .quic
        .open_relayed(relay.relay(), destination.id, Purpose::Owner, &client())
        .await;
    assert_eq!(result.err(), Some(RelayError::Refused));
    shutdown.cancel();
}

#[tokio::test]
async fn a_relay_carries_a_bounded_number_of_streams_to_one_node() {
    let shutdown = CancellationToken::new();
    let (relay, destination) = (side(1, &shutdown), side(2, &shutdown));
    keep(&destination, &relay, &shutdown);
    let shares = MAX_RELAYED_PER_DESTINATION / MAX_RELAYED_PER_CLIENT_TO_DESTINATION;
    let clients: Vec<Side> = (0..=shares)
        .map(|index| {
            let index = u8::try_from(index).unwrap();
            side_at(10 + index, 10 + index, &shutdown)
        })
        .collect();
    let (late, clients) = clients.split_last().unwrap();
    let mut open = Vec::new();
    for (index, from) in clients.iter().enumerate() {
        for _ in 0..MAX_RELAYED_PER_CLIENT_TO_DESTINATION {
            open.push(reach(from, &relay, destination.id, Purpose::Payments).await);
        }
        if index == 0 {
            // One address has only its share of the node, so it cannot
            // shut other clients out.
            let result = from
                .quic
                .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
                .await;
            assert_eq!(result.err(), Some(RelayError::Busy));
        }
    }
    assert_eq!(open.len(), MAX_RELAYED_PER_DESTINATION);
    // The node has its share: another client waits too.
    let result = late
        .quic
        .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
        .await;
    assert_eq!(result.err(), Some(RelayError::Busy));
    // Each stream still works, and one that ends frees its place.
    for stream in &mut open {
        assert_eq!(echo(stream, b"ping").await, b"ping");
    }
    let mut ended = open.pop().unwrap();
    ended.shutdown().await.unwrap();
    let _ = ended.read_to_end(&mut Vec::new()).await;
    drop(ended);
    let mut freed = None;
    for _ in 0..200 {
        match late
            .quic
            .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
            .await
        {
            Ok(stream) => {
                freed = Some(stream);
                break;
            }
            Err(RelayError::Busy) => tokio::time::sleep(Duration::from_millis(20)).await,
            Err(error) => panic!("{error}"),
        }
    }
    assert_eq!(echo(&mut freed.unwrap(), b"pong").await, b"pong");
    shutdown.cancel();
}

/// Connections to the relay come and go beside the one the node keeps
/// open, both ways and whichever node ID is lower: the relay reaches the
/// node all along, over the kept one once the others are gone.
async fn a_relay_reaches_its_node_beside_other_connections(relay_is_lower: bool) {
    let shutdown = CancellationToken::new();
    let idle = Duration::from_secs(1);
    let make = |seed: u8| {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        (seed, side_with(seed, socket, Some(idle), &shutdown))
    };
    let (seeds, from) = if relay_is_lower {
        ((40, 41), 42)
    } else {
        ((50, 51), 52)
    };
    let (a, b) = (make(seeds.0), make(seeds.1));
    let ((relay_seed, relay), (node_seed, node)) = if (a.1.id < b.1.id) == relay_is_lower {
        (a, b)
    } else {
        (b, a)
    };
    let (_, from) = make(from);
    // The node reached its relay before it kept a connection to it: it
    // sends over that first one, which then goes idle.
    send(&node, &relay).await;
    keep(&node, &relay, &shutdown);
    let mut stream = reach(&from, &relay, node.id, Purpose::Payments).await;
    assert_eq!(echo(&mut stream, b"one").await, b"one");
    drop(stream);
    tokio::time::sleep(idle * 2).await;
    send(&node, &relay).await;
    // Each also takes another connection from the other's key, as when one
    // dials the other while it holds no open connection; they go idle too.
    let (_, node_twin) = make(node_seed);
    let (_, relay_twin) = make(relay_seed);
    send(&node_twin, &relay).await;
    send(&relay_twin, &node).await;
    tokio::time::sleep(idle * 2).await;
    // Only the kept connection is left, and the relay holds it.
    let reached = from
        .quic
        .open_relayed(relay.relay(), node.id, Purpose::Payments, &client())
        .await;
    let mut stream = reached.expect("the relay lost the node's kept connection");
    assert_eq!(echo(&mut stream, b"two").await, b"two");
    // The node holds it too, and sends over it rather than dialling again.
    send(&node, &relay).await;
    assert_eq!(node.quic.open_connections(), 1);
    shutdown.cancel();
}

#[tokio::test]
async fn a_relay_reaches_its_node_beside_other_connections_either_way() {
    tokio::join!(
        a_relay_reaches_its_node_beside_other_connections(true),
        a_relay_reaches_its_node_beside_other_connections(false),
    );
}

#[tokio::test]
async fn a_node_does_not_choose_itself_as_its_relay() {
    let shutdown = CancellationToken::new();
    let node = side(1, &shutdown);
    keep(&node, &node, &shutdown);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(node.quic.open_connections(), 0);
    // It will never connect: that is a fault from the start.
    assert_eq!(node.quic.relay_link(), RelayLink::Unreachable);
    shutdown.cancel();
}

// A relay the node cannot reach reads as connecting while the node dials it
// again quickly, and as unreachable once the wait before each dial has
// reached its cap: after over a minute of failed dials, and not before.
#[tokio::test(start_paused = true)]
async fn a_relay_out_of_reach_reads_as_unreachable_once_its_dials_slow_down() {
    let shutdown = CancellationToken::new();
    let node = side(1, &shutdown);
    let relay = NodeKey::from_bytes(&[2; 32]).node_id();
    // A URL with no port to dial: each dial fails at once.
    tokio::spawn(Arc::clone(&node.quic).keep_relay(
        relay,
        "https://127.0.0.1:none".into(),
        client(),
        KeepWhile::Always,
        shutdown.clone(),
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(node.quic.relay_link(), RelayLink::Connecting);
    tokio::time::sleep(Duration::from_secs(59)).await;
    assert_eq!(node.quic.relay_link(), RelayLink::Connecting);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(node.quic.relay_link(), RelayLink::Unreachable);
    shutdown.cancel();
}

#[tokio::test]
async fn a_relay_out_of_reach_is_still_asked_for_peer_requests() {
    let shutdown = CancellationToken::new();
    let from = side(1, &shutdown);
    let relay = NodeKey::from_bytes(&[2; 32]).node_id();
    // A URL with no port to dial: the dial fails at once.
    let result = from
        .quic
        .open_relayed(
            (relay, "https://127.0.0.1:none"),
            NodeKey::from_bytes(&[3; 32]).node_id(),
            Purpose::Payments,
            &client(),
        )
        .await;
    assert_eq!(result.err(), Some(RelayError::Unreachable));
    assert!(!from.quic.is_unreachable(&relay));
    shutdown.cancel();
}

#[tokio::test]
async fn a_client_must_reach_the_node_it_asked_for() {
    let shutdown = CancellationToken::new();
    let (relay, destination, from) = (side(1, &shutdown), side(2, &shutdown), side(3, &shutdown));
    keep(&destination, &relay, &shutdown);
    reach(&from, &relay, destination.id, Purpose::Payments).await;
    // A relay that joins the stream to another node than the one named
    // fails the client's TLS: here, the destination swaps its key.
    destination
        .identity
        .send_replace(Some(Arc::new(NodeKey::from_bytes(&[4; 32]))));
    let result = from
        .quic
        .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
        .await;
    assert_eq!(result.err(), Some(RelayError::Handshake));
    shutdown.cancel();
}

#[tokio::test]
async fn a_node_waits_for_its_key_and_reconnects_to_its_relay() {
    let relay_shutdown = CancellationToken::new();
    let shutdown = CancellationToken::new();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    let relay = side_on(1, socket, &relay_shutdown);
    let (destination, from) = (side(2, &shutdown), side(3, &shutdown));
    // No key yet: nothing to prove, so nothing dialled.
    let key = destination.identity.send_replace(None);
    keep(&destination, &relay, &shutdown);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(destination.quic.relay_link(), RelayLink::Connecting);
    let result = from
        .quic
        .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
        .await;
    assert_eq!(result.err(), Some(RelayError::UnknownDestination));
    destination.identity.send_replace(key);
    let mut stream = reach(&from, &relay, destination.id, Purpose::Payments).await;
    assert_eq!(echo(&mut stream, b"one").await, b"one");
    assert_eq!(destination.quic.relay_link(), RelayLink::Connected);
    drop(stream);
    // The relay restarts on the same port: the node dials it again.
    relay_shutdown.cancel();
    while relay.quic.is_serving() {
        tokio::task::yield_now().await;
    }
    // Meanwhile the node is connecting, which is no fault yet.
    for _ in 0..500 {
        if destination.quic.relay_link() != RelayLink::Connected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(destination.quic.relay_link(), RelayLink::Connecting);
    let url = relay.url.clone();
    drop(relay);
    let relay_shutdown = CancellationToken::new();
    let mut socket = None;
    for _ in 0..200 {
        if let Ok(bound) = UdpSocket::bind(address) {
            socket = Some(bound);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let relay = side_on(1, socket.expect("the port stayed taken"), &relay_shutdown);
    assert_eq!(relay.url, url);
    let mut stream = reach(&from, &relay, destination.id, Purpose::Payments).await;
    assert_eq!(echo(&mut stream, b"two").await, b"two");
    relay_shutdown.cancel();
    shutdown.cancel();
}

/// The nonce each request over a relayed stream sends, for the node's
/// signature on its answer.
const NONCE: &str = "relayed-nonce";

/// A response over a relayed stream, read in full: its status, its fields
/// and its body.
async fn get(
    sender: &mut hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    method: &str,
    path: &str,
    host: &str,
) -> (StatusCode, Vec<(String, String)>, Bytes) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header(httpsig::NONCE_FIELD, NONCE)
        .body(Full::new(Bytes::new()))
        .unwrap();
    // HTTP/1.1 carries one request at a time: the connection takes the next
    // once it has finished with the last answer.
    if let Err(error) = sender.ready().await {
        panic!("{method} {path}: the relayed stream is not ready: {error:?}");
    }
    let response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(error) => panic!("{method} {path} over the relayed stream: {error:?}"),
    };
    let status = response.status();
    let fields = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_owned()))
        .collect();
    (
        status,
        fields,
        response.into_body().collect().await.unwrap().to_bytes(),
    )
}

/// Checks the node `id`'s signature on an answer to a GET of `target` sent
/// with [`NONCE`], as a Dassie-aware client does.
fn signed_by(
    target: &str,
    (status, fields, body): (StatusCode, &[(String, String)], &[u8]),
    id: NodeId,
    now_s: u64,
) -> Result<NodeId, httpsig::NodeResponseError> {
    let nonce = [(httpsig::NONCE_FIELD, NONCE)];
    let request = httpsig::Request {
        method: "GET",
        target_uri: target,
        fields: &nonce,
    };
    let fields: Vec<(&str, &str)> = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let answer = httpsig::Response {
        status: status.as_u16(),
        fields: &fields,
    };
    httpsig::check_node_response(&request, &answer, body, Some(id), now_s)
}

// spec/decisions/core-and-periphery.md: through a relay a node serves Open
// Payments and nothing else; never the wallet, `/api/*`, `/rpc`, `/peer` or
// BTP.
#[tokio::test(flavor = "multi_thread")]
async fn a_relayed_payments_stream_serves_only_open_payments() {
    let shutdown = CancellationToken::new();
    let (relay, from) = (side(1, &shutdown), side(3, &shutdown));
    let key = NodeKey::from_bytes(&[31; 32]);
    let entry = support::pair::bootstrap(&relay_key(1), 0)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .replace("https://localhost:0", &relay.url);
    let env = [
        ("DASSIE_PEER_CA_FILE", support::op::CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_RELAY", entry.as_str()),
    ];
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 3, Policies::DEV, &env)
        .await;
    set_up(&node, &key).await;
    let mut owner = node.ipc().await;
    owner
        .call(Method::SetHostname(proto::SetHostnameRequest {
            hostname: "localhost".into(),
        }))
        .await
        .unwrap();
    // A ledger, for a wallet address: the stub, once the engine takes it.
    let mut added = false;
    for _ in 0..200 {
        let adding = owner.call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: "stub".into(),
                config: String::new(),
            },
        ));
        if adding.await.is_ok() {
            added = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(added, "the stub scheme did not start");
    let stream = reach(&from, &relay, key.node_id(), Purpose::Payments).await;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(connection);
    let host = node.host();
    let (status, fields, body) = get(&mut sender, "GET", "/.well-known/pay", &host).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let target = format!("{}/.well-known/pay", node.origin());
    let address: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(address["id"], target);
    // The answer is signed with the node key, as over HTTPS, so a client
    // can check it whoever passed it on.
    let now_s = node.clock.now_ms() / 1_000;
    let signer = signed_by(&target, (status, &fields, &body), key.node_id(), now_s);
    assert_eq!(signer, Ok(key.node_id()));
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/index.html"),
        ("GET", "/rpc"),
        ("GET", "/stats"),
        ("POST", "/api/login"),
        ("POST", "/peer"),
        ("GET", "/btp"),
    ] {
        let (status, _, _) = get(&mut sender, method, path, &host).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    node.stop().await;
    shutdown.cancel();
}

fn relay_key(seed: u8) -> NodeKey {
    NodeKey::from_bytes(&[seed; 32])
}

/// A request to `to` as to a periphery node: without a URL.
fn to_periphery(to: &Side) -> PeerRequest {
    PeerRequest {
        id: RequestId(2),
        to: to.id,
        url: String::new(),
        quic: true,
        kind: Kind::LinkStateRequest,
        body: b"hello".to_vec(),
        timeout_ms: 5_000,
    }
}

// spec/areas/peer-protocol.md#quic: a periphery node's connection to its
// uplink is the only way the uplink reaches it, so the node keeps it alive
// past the idle timeout; the uplink sends on it with no URL, and relays to
// the node over it. A chosen relay that is the same node holds a second
// connection beside it, kept alive alike, one of them standing by.
#[tokio::test]
async fn an_uplink_reaches_its_downlink_and_relays_to_it() {
    let shutdown = CancellationToken::new();
    let idle = Duration::from_millis(600);
    let short = |seed| {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        side_with(seed, socket, Some(idle), &shutdown)
    };
    let (uplink, downlink, from) = (short(1), short(2), side(3, &shutdown));
    downlink.quic.set_keep_alive(true);
    send(&downlink, &uplink).await;
    tokio::time::sleep(idle * 3).await;
    let response = uplink.quic.send_held(&to_periphery(&downlink)).await;
    assert!(response.is_ok(), "kept alive: {response:?}");
    let mut stream = reach(&from, &uplink, downlink.id, Purpose::Payments).await;
    assert_eq!(
        echo(&mut stream, b"through the uplink").await,
        b"through the uplink"
    );
    drop(stream);
    keep(&downlink, &uplink, &shutdown);
    tokio::time::sleep(idle * 3).await;
    let response = uplink.quic.send_held(&to_periphery(&downlink)).await;
    assert!(response.is_ok(), "beside the chosen relay: {response:?}");
    // Without keep-alives, the connection closes once idle, and the node
    // without a URL is out of reach.
    let (core, other) = (short(4), short(5));
    send(&other, &core).await;
    tokio::time::sleep(idle * 3).await;
    let response = core.quic.send_held(&to_periphery(&other)).await;
    assert_eq!(response.err(), Some(dassie_peer::Failure::NotDelivered));
    shutdown.cancel();
}
