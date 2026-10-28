//! The UDP endpoint for a core node's snapshot, over a real UDP socket
//! (spec/areas/rate-feed.md#the-node-side): a padded request gets the
//! node's snapshot, signed with its node key, which quotes the settlement
//! methods the node offers and not its owner book, within a budget on
//! answers; anything else gets nothing, and the owner sees the counts. The
//! node names the port in its discovery document and its link state while
//! it answers; before it has a URL, or while its owner offers no swaps, it
//! names none and answers nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::disallowed_types,
    clippy::panic
)]

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use dassie_feed::{AnswerLimits, REQUEST_BYTES, SnapshotReader, encode_request, method_tag};
use dassie_identity::IdentityKey;
use dassie_node::clock::{Clock as _, ManualClock};
use dassie_node::fake_ledger::{Options, Server};
use dassie_node::node::{Node, Policies};
use dassie_node::rpc::proto::{self, request::Method};
use dassie_node::store::StoredCertificate;
use dassie_node_core::certificate::CertificateSource;
use support::pair::{ca_file, cert_file, key, key_file, prepare_on_xrpl, until};
use support::rates::{RatesServer, coinbase};
use support::{Bound, START_MS, STUB_UNLIMITED, TestNode, send};
use tokio::net::UdpSocket;

const USD: &str = "stub+usd";
const XRP: &str = "xrpl-testnet-xrp+xrp";
/// The stub's scheme: the one method the test's node offers its peers. Its
/// XRP is in its owner book, on which no peer pays it.
const STUB: &str = "stub";

/// How long an answer that is coming may take: the node answers as it
/// reads a request, on no clock of its own, so only a hang ends the wait
/// ([`support::HANG`]).
const ARRIVES: Duration = support::HANG;
/// How long to watch for an answer that should not come: a check that
/// nothing happens, which a slow machine can only make weaker.
const QUIET: Duration = Duration::from_millis(400);

async fn fake_ledger() -> Server {
    Server::start(Options {
        network: &dassie_xrpl::network::TESTNET,
        listen: "127.0.0.1:0".parse().unwrap(),
        faucet: Some((
            "127.0.0.1:0".parse().unwrap(),
            StoredCertificate {
                source: CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(cert_file()).unwrap(),
                key_pem: std::fs::read_to_string(key_file()).unwrap(),
            },
        )),
        close_every: Duration::from_millis(100),
    })
    .await
    .unwrap()
}

/// The node behind `node`, while it runs.
fn node_of(node: &TestNode) -> std::sync::Arc<Node> {
    node.node.upgrade().unwrap()
}

/// The node's rate between `from` and `to` as its price table has it.
async fn table_rate(node: &TestNode, from: &str, to: &str) -> Option<u64> {
    let engine = node_of(node).engine()?;
    let table = engine.price_table().await.ok()?;
    let (from, to) = (
        dassie_ledger::LedgerId::parse(from)?,
        dassie_ledger::LedgerId::parse(to)?,
    );
    table.price(from, to).map(|price| price.rate_e15)
}

/// Someone who asks, on a socket of their own.
struct Asker {
    socket: UdpSocket,
    endpoint: SocketAddr,
}

impl Asker {
    async fn new(endpoint: SocketAddr) -> Self {
        Self {
            socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            endpoint,
        }
    }

    /// Sends `datagram` to the endpoint.
    async fn send(&self, datagram: &[u8]) {
        self.socket.send_to(datagram, self.endpoint).await.unwrap();
    }

    /// Sends a request.
    async fn ask(&self) {
        self.send(&encode_request()).await;
    }

    /// The next datagram within `wait`, from the endpoint.
    async fn next(&self, wait: Duration) -> Option<Vec<u8>> {
        let mut buffer = [0u8; 1500];
        let received = tokio::time::timeout(wait, self.socket.recv_from(&mut buffer));
        let (length, from) = received.await.ok()?.unwrap();
        assert_eq!(from, self.endpoint);
        Some(buffer[..length].to_vec())
    }

    /// Whether nothing comes for a while.
    async fn quiet(&self) -> bool {
        self.next(QUIET).await.is_none()
    }
}

/// A core node with dollars on the stub and XRP in its owner book and a
/// rate between them, whose endpoint answers two requests at once: the
/// owner holds both, but only the stub is a method.
async fn core_node(server: &Server, rates: &RatesServer, owner: &IdentityKey) -> TestNode {
    let bound = Bound::new();
    let env = [
        ("DASSIE_EXTRA_CA_FILE", ca_file()),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let policies = Policies {
        feed: AnswerLimits {
            per_second: 1,
            burst: 2,
        },
        ..Policies::DEV
    };
    let clock = ManualClock::new(START_MS);
    server.close_on(std::sync::Arc::new(clock.clone()));
    let node = bound.start(clock.clone(), 61, policies, &env).await;
    let config = support::pair::xrpl_config(server);
    prepare_on_xrpl(&node, owner, "xrpl-testnet-xrp", &config).await;
    let mut ipc = node.ipc().await;
    ipc.call(Method::AddSettlementScheme(
        proto::AddSettlementSchemeRequest {
            id: "stub".into(),
            config: STUB_UNLIMITED.into(),
        },
    ))
    .await
    .unwrap();
    ipc.call(Method::StubDeposit(proto::StubDepositRequest {
        amount: 10_000_000_000,
    }))
    .await
    .unwrap();
    ipc.call(Method::SetRateSources(proto::SetRateSourcesRequest {
        sources: vec![proto::RateSource {
            kind: "coinbase".into(),
            url: rates.url(),
        }],
    }))
    .await
    .unwrap();
    until(&clock, async || {
        table_rate(&node, USD, XRP).await.is_some() && table_rate(&node, XRP, USD).await.is_some()
    })
    .await;
    node
}

/// The endpoint in the node's status, as its owner reads it.
async fn feed_status(node: &TestNode) -> Option<proto::RateFeed> {
    match node
        .ipc()
        .await
        .call(Method::GetStatus(proto::GetStatusRequest {}))
        .await
    {
        Ok(proto::response::Method::GetStatus(status)) => status.feed,
        other => panic!("{other:?}"),
    }
}

/// The node's discovery document.
async fn discovery_of(node: &TestNode) -> serde_json::Value {
    let request = hyper::Request::get("/.well-known/dassie/node")
        .header("host", node.host())
        .body(http_body_util::Full::new(bytes::Bytes::new()))
        .unwrap();
    let reply = send(node.https, true, request).await;
    assert_eq!(reply.status, hyper::StatusCode::OK);
    serde_json::from_str(&reply.text()).unwrap()
}

/// Sets the owner's word on offering swaps, which the engine takes.
async fn offer_swaps(node: &TestNode, offer: bool) {
    let engine = node_of(node).engine().unwrap();
    let settings = engine.peer_settings().await.unwrap();
    engine
        .set_peer_settings(dassie_node::store::PeerSettings {
            offer_swaps: offer,
            ..settings
        })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_gets_the_nodes_snapshot_within_the_budget() {
    let server = fake_ledger().await;
    let rates = RatesServer::start(coinbase(&[("XRP", "2")])).await;
    let owner = key(61);
    let node = core_node(&server, &rates, &owner).await;
    let clock = node.clock.clone();

    // The snapshot quotes the stub, the one method the node offers, and not
    // the owner book, though the table prices it: no peer pays the node on
    // it (spec/areas/rate-feed.md#what-it-publishes).
    let stub = method_tag(STUB);
    until(&clock, async || {
        let snapshot = node_of(&node).current_snapshot();
        snapshot.is_some_and(|bytes| {
            let mut reader = SnapshotReader::new(owner.public_key());
            reader
                .receive(&bytes, clock.now_ms())
                .is_ok_and(|snapshot| snapshot.methods.iter().map(|m| m.tag).eq([stub]))
        })
    })
    .await;

    // A request gets it, signed with the node's key; so does a second.
    let asker = Asker::new(node.feed).await;
    let mut reader = SnapshotReader::new(owner.public_key());
    asker.ask().await;
    let bytes = asker.next(ARRIVES).await.expect("an answer");
    assert!(bytes.len() <= REQUEST_BYTES, "never more than was sent");
    let snapshot = reader.receive(&bytes, clock.now_ms()).unwrap();
    assert!(snapshot.method(stub).is_some());
    asker.ask().await;
    assert!(asker.next(ARRIVES).await.is_some());

    // The budget is spent: a third gets nothing until it fills, as the
    // node's clock moves; nor does a request cut short, or a snapshot.
    asker.ask().await;
    asker.send(&encode_request()[..64]).await;
    asker.send(&bytes).await;
    assert!(asker.quiet().await);
    support::eventually("the node read the datagrams", async || {
        let feed = feed_status(&node).await?;
        (feed.received == 5).then_some(())
    })
    .await;
    clock.advance(1_000);
    asker.ask().await;
    assert!(asker.next(ARRIVES).await.is_some());

    // The owner sees it in the node's status
    // (spec/areas/rate-feed.md#what-the-owner-sees).
    let feed = feed_status(&node).await.unwrap();
    assert!(feed.serving);
    assert_eq!(feed.address, node.feed.to_string());
    assert_eq!(feed.received, 6);
    assert_eq!((feed.answered, feed.over_budget, feed.ignored), (3, 1, 2));
    assert_eq!(feed.send_failed, 0);

    // An owner who offers no swaps has no snapshot: the node answers
    // nothing, and names no port.
    offer_swaps(&node, false).await;
    until(&clock, async || {
        !dassie_node::feed::serving(&node_of(&node))
    })
    .await;
    assert_eq!(discovery_of(&node).await.get("feedPort"), None);
    clock.advance(1_000);
    asker.ask().await;
    assert!(asker.quiet().await);
    offer_swaps(&node, true).await;
    until(&clock, async || dassie_node::feed::serving(&node_of(&node))).await;
    asker.ask().await;
    assert!(asker.next(ARRIVES).await.is_some());

    node.stop().await;
    server.stop();
}

// A core node names the port it answers on in its discovery document and
// in its link state; before it has a URL, it names none, and answers no
// request (spec/areas/rate-feed.md#finding-a-node).
#[tokio::test(flavor = "multi_thread")]
async fn a_core_node_names_its_port() {
    let owner = key(62);
    let bound = Bound::new();
    let port = bound.feed_address().port();
    let clock = ManualClock::new(START_MS);
    let node = bound.start(clock.clone(), 62, Policies::DEV, &[]).await;
    support::set_up(&node, &owner).await;
    node.engine_started().await;
    assert_eq!(discovery_of(&node).await.get("feedPort"), None);
    assert_eq!(node_of(&node).own_info().feed_port, None);
    let asker = Asker::new(node.feed).await;
    asker.ask().await;
    assert!(asker.quiet().await);
    // Its owner sees the socket, and that it does not answer.
    let feed = feed_status(&node).await.unwrap();
    assert!(!feed.serving);
    assert_eq!(feed.address, node.feed.to_string());
    assert_eq!((feed.received, feed.answered, feed.ignored), (1, 0, 1));

    // With a hostname, the node is a core node: it names the port, and
    // answers.
    let mut ipc = node.ipc().await;
    ipc.call(Method::SetHostname(proto::SetHostnameRequest {
        hostname: "localhost".into(),
    }))
    .await
    .unwrap();
    assert_eq!(node_of(&node).own_info().feed_port, Some(port));
    until(&clock, async || dassie_node::feed::serving(&node_of(&node))).await;
    assert_eq!(discovery_of(&node).await["feedPort"], u64::from(port));
    asker.ask().await;
    let bytes = asker.next(ARRIVES).await.expect("an answer");
    let mut reader = SnapshotReader::new(owner.public_key());
    reader.receive(&bytes, clock.now_ms()).unwrap();
    let feed = feed_status(&node).await.unwrap();
    assert!(feed.serving);
    assert_eq!(feed.answered, 1);
    node.stop().await;
}

// A node without the socket answers no request, and its status has no
// endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_without_the_socket_reports_no_endpoint() {
    let owner = key(63);
    let clock = ManualClock::new(START_MS);
    let node = Bound::new()
        .without_feed()
        .start(clock.clone(), 63, Policies::DEV, &[])
        .await;
    support::set_up(&node, &owner).await;
    let mut ipc = node.ipc().await;
    ipc.call(Method::SetHostname(proto::SetHostnameRequest {
        hostname: "localhost".into(),
    }))
    .await
    .unwrap();
    assert!(node_of(&node).feed().is_none());
    assert_eq!(feed_status(&node).await, None);
    assert_eq!(discovery_of(&node).await.get("feedPort"), None);
    node.stop().await;
}
