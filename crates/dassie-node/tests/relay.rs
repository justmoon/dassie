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
use dassie_node::clock::ManualClock;
use dassie_node::http::peer::Answer;
use dassie_node::node::Policies;
use dassie_node::peer_client::PeerClient;
use dassie_node::quic::relay::{
    MAX_RELAYED_PER_DESTINATION, Purpose, RelayError, RelayedStream, ServedStream,
};
use dassie_node::quic::{Body, Handler, Quic};
use dassie_node::rpc::proto::{self, request::Method};
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use support::{Bound, START_MS, set_up};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Answers peer requests with 200 and nothing, and echoes what a relayed
/// stream for payments carries; refuses the owner's purpose.
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
    let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
    let id = key.node_id();
    let (identity, receiver) = watch::channel(Some(key));
    let quic = Arc::new(Quic::new(socket, receiver, true).unwrap());
    let port = quic.local_addr().unwrap().port();
    tokio::spawn(Arc::clone(&quic).serve(Arc::new(Echo), shutdown.clone()));
    Side {
        quic,
        id,
        url: format!("https://127.0.0.1:{port}"),
        identity,
    }
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
    tokio::spawn(Arc::clone(&destination.quic).keep_relay(
        relay.id,
        relay.url.clone(),
        client(),
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
    let (relay, destination, from) = (side(1, &shutdown), side(2, &shutdown), side(3, &shutdown));
    keep(&destination, &relay, &shutdown);
    let mut open = Vec::new();
    for _ in 0..MAX_RELAYED_PER_DESTINATION {
        open.push(reach(&from, &relay, destination.id, Purpose::Payments).await);
    }
    let result = from
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
        match from
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
    let result = from
        .quic
        .open_relayed(relay.relay(), destination.id, Purpose::Payments, &client())
        .await;
    assert_eq!(result.err(), Some(RelayError::UnknownDestination));
    destination.identity.send_replace(key);
    let mut stream = reach(&from, &relay, destination.id, Purpose::Payments).await;
    assert_eq!(echo(&mut stream, b"one").await, b"one");
    drop(stream);
    // The relay restarts on the same port: the node dials it again.
    relay_shutdown.cancel();
    while relay.quic.is_serving() {
        tokio::task::yield_now().await;
    }
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

/// A response over a relayed stream, read in full.
async fn get(
    sender: &mut hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    method: &str,
    path: &str,
    host: &str,
) -> (StatusCode, Bytes) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .body(Full::new(Bytes::new()))
        .unwrap();
    let response = sender.send_request(request).await.unwrap();
    let status = response.status();
    (
        status,
        response.into_body().collect().await.unwrap().to_bytes(),
    )
}

// D26: through a relay a node serves Open Payments and nothing else; never
// the wallet, `/api/*`, `/rpc`, `/peer` or BTP.
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
    let (status, body) = get(&mut sender, "GET", "/.well-known/pay", &host).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let address: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(address["id"], format!("{}/.well-known/pay", node.origin()));
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/index.html"),
        ("GET", "/rpc"),
        ("GET", "/stats"),
        ("POST", "/api/login"),
        ("POST", "/peer"),
        ("GET", "/btp"),
    ] {
        let (status, _) = get(&mut sender, method, path, &host).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
    }
    node.stop().await;
    shutdown.cancel();
}

fn relay_key(seed: u8) -> NodeKey {
    NodeKey::from_bytes(&[seed; 32])
}
