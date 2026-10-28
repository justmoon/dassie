//! The rate feed of a core node, over a real UDP socket
//! (spec/areas/rate-feed.md#the-node-side): a subscriber is challenged,
//! proves its address and gets feeds signed with the node key that name
//! the settlement methods the node offers, and not its owner book; it
//! renews, and lapses when it stops; a renewal from
//! another address with its cookie is challenged; a full table refuses
//! newcomers until an entry lapses. The node names the feed's port in its
//! discovery document and in its link state.

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
use std::sync::atomic::Ordering;
use std::time::Duration;

use dassie_feed::{
    FEED_HEARTBEAT_MS, FEED_MIN_INTERVAL_MS, FeedReceiver, Limits, RENEW_INTERVAL_MS, Received,
    Rejected, SUBSCRIPTION_TTL_MS,
};
use dassie_identity::IdentityKey;
use dassie_node::clock::{Clock as _, ManualClock};
use dassie_node::fake_ledger::{Options, Server};
use dassie_node::node::{Node, Policies};
use dassie_node::rpc::proto::{self, request::Method};
use dassie_node::store::StoredCertificate;
use dassie_node_core::certificate::CertificateSource;
use support::pair::{CA, CERT, KEY, key, prepare_on_xrpl, until};
use support::rates::{RatesServer, coinbase};
use support::{Bound, START_MS, STUB_UNLIMITED, TestNode, send};
use tokio::net::UdpSocket;

const USD: &str = "stub+usd";
const XRP: &str = "xrpl-testnet-xrp+xrp";
/// The stub's scheme: the one method the test's node offers its peers. Its
/// XRP is in its owner book, on which no peer pays it.
const STUB: &str = "stub";

/// How long a datagram that is coming may take: the node sends it as it
/// reads a datagram or its clock moves, on no clock of its own, so only a
/// hang ends the wait ([`support::HANG`]).
const ARRIVES: Duration = support::HANG;
/// How long to watch for a datagram that should not come: a check that
/// nothing happens, which a slow machine can only make weaker. What the
/// node sent before it read the datagram a test waits on is in the socket
/// already, so a drain after that wait misses none of it.
const QUIET: Duration = Duration::from_millis(400);

async fn fake_ledger() -> Server {
    Server::start(Options {
        network: &dassie_xrpl::network::TESTNET,
        listen: "127.0.0.1:0".parse().unwrap(),
        faucet: Some((
            "127.0.0.1:0".parse().unwrap(),
            StoredCertificate {
                source: CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(CERT).unwrap(),
                key_pem: std::fs::read_to_string(KEY).unwrap(),
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

/// A subscriber on a socket of its own.
struct Subscriber {
    socket: UdpSocket,
    feed: SocketAddr,
    receiver: FeedReceiver,
}

impl Subscriber {
    async fn new(feed: SocketAddr, node: dassie_identity::NodeIdentity) -> Self {
        Self {
            socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            feed,
            receiver: FeedReceiver::new(node),
        }
    }

    /// Sends a Subscribe with the last cookie it was handed.
    async fn subscribe(&self) {
        let datagram = self.receiver.subscribe();
        self.socket.send_to(&datagram, self.feed).await.unwrap();
    }

    /// The next datagram from the feed within `wait` that it takes: a
    /// Challenge, or a feed's methods, how many rates it has, and its
    /// sequence. A feed that expired while it waited in the socket, as the
    /// test moved the clock on, is passed over.
    async fn next(&mut self, wait: Duration, now_ms: u64) -> Option<Taken> {
        let mut buffer = [0u8; 1500];
        loop {
            let received = tokio::time::timeout(wait, self.socket.recv_from(&mut buffer));
            let (length, from) = received.await.ok()?.unwrap();
            assert_eq!(from, self.feed);
            match self.receiver.receive(&buffer[..length], now_ms) {
                Ok(Received::Challenge) => return Some(Taken::Challenge),
                Ok(Received::Feed(feed)) => {
                    return Some(Taken::Feed {
                        sequence_ms: feed.sequence_ms,
                        methods: feed
                            .methods
                            .iter()
                            .map(|method| (method.id.to_owned(), method.max_packet_amount))
                            .collect(),
                        rates: feed.rates.len(),
                    });
                }
                Err(Rejected::Expired) => {}
                Err(rejected) => panic!("a datagram refused: {rejected:?}"),
            }
        }
    }

    /// Takes datagrams until a feed, and returns it.
    async fn feed(&mut self, now_ms: u64) -> Taken {
        loop {
            match self.next(ARRIVES, now_ms).await {
                Some(feed @ Taken::Feed { .. }) => return feed,
                Some(Taken::Challenge) => {}
                None => panic!("no feed came"),
            }
        }
    }

    /// Proves its address: a Subscribe gets a Challenge, which it sends
    /// back.
    async fn prove(&mut self, now_ms: u64) {
        self.subscribe().await;
        assert_eq!(self.next(ARRIVES, now_ms).await, Some(Taken::Challenge));
        self.subscribe().await;
    }

    /// Whether nothing comes for a while.
    async fn quiet(&mut self, now_ms: u64) -> bool {
        self.next(QUIET, now_ms).await.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Taken {
    Challenge,
    Feed {
        sequence_ms: u64,
        /// Each method's ID and largest packet.
        methods: Vec<(String, u64)>,
        rates: usize,
    },
}

/// A core node with dollars on the stub and XRP in its owner book and a
/// rate between them, whose feed takes two subscribers: the owner holds
/// both, so both pairs convert, but only the stub is a method.
async fn core_node(server: &Server, rates: &RatesServer, owner: &IdentityKey) -> TestNode {
    let bound = Bound::new();
    let env = [
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let policies = Policies {
        feed: Limits {
            max_subscribers: 2,
            ..Limits::DEFAULT
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

#[tokio::test(flavor = "multi_thread")]
async fn proven_subscribers_get_the_nodes_prices_until_they_lapse() {
    let server = fake_ledger().await;
    let rates = RatesServer::start(coinbase(&[("XRP", "2")])).await;
    let owner = key(61);
    let node = core_node(&server, &rates, &owner).await;
    let clock = node.clock.clone();
    let stats = || {
        let node = node_of(&node);
        let stats = node.feed().unwrap().stats();
        (
            stats.renewed.load(Ordering::Relaxed),
            stats.refused_full.load(Ordering::Relaxed),
        )
    };
    // Datagrams the node read: it reads them as they come, on no clock.
    let received = || {
        node_of(&node)
            .feed()
            .unwrap()
            .stats()
            .received
            .load(Ordering::Relaxed)
    };
    let read = async |count: u64| {
        support::eventually("the node read the datagrams", async || {
            (received() >= count).then_some(())
        })
        .await;
    };
    let id = owner.identity();
    let now = || clock.now_ms();

    // A proves its address and gets the latest feed at once, signed with
    // the node's key; it names the stub, the one method the node offers,
    // with the connector's largest packet, and not the owner book, though
    // the table prices it: no peer pays the node on it, so no rate is said
    // (spec/areas/rate-feed.md#what-it-publishes).
    let mut a = Subscriber::new(node.feed, id).await;
    a.prove(now()).await;
    let max_packet = node_of(&node).policies().connector.max_packet_amount;
    let expected = vec![(STUB.to_owned(), max_packet)];
    let mut said = a.feed(now()).await;
    for _ in 0..50 {
        if matches!(&said, Taken::Feed { methods, .. } if *methods == expected) {
            break;
        }
        clock.advance(FEED_MIN_INTERVAL_MS);
        if let Some(feed @ Taken::Feed { .. }) = a.next(QUIET, now()).await {
            said = feed;
        }
    }
    let Taken::Feed { methods, rates, .. } = said else {
        panic!("not a feed");
    };
    assert_eq!((methods, rates), (expected, 0));
    assert!(table_rate(&node, USD, XRP).await.is_some());

    // B takes the second place; C finds the table full, and gets nothing.
    let mut b = Subscriber::new(node.feed, id).await;
    b.prove(now()).await;
    b.feed(now()).await;
    let mut c = Subscriber::new(node.feed, id).await;
    let before = received();
    c.prove(now()).await;
    read(before + 2).await;
    assert!(c.quiet(now()).await);
    assert_eq!(stats().1, 1);

    // The owner sees it in the node's status: two subscribers, three
    // challenged, C refused for want of room, and the feeds that went out
    // (spec/areas/rate-feed.md#what-the-owner-sees).
    let feed = feed_status(&node).await.unwrap();
    assert!(feed.serving);
    assert_eq!(feed.address, node.feed.to_string());
    assert_eq!(feed.subscribers, 2);
    assert_eq!((feed.challenged, feed.subscribed), (3, 2));
    assert_eq!((feed.refused_full, feed.refused_client), (1, 0));
    assert_eq!((feed.renewed, feed.ignored), (0, 0));
    assert_eq!(feed.received, 6);
    assert!(feed.published >= 1, "{feed:?}");
    // Three challenges, and at least one feed to each of A and B.
    assert!(feed.sent >= 5, "{feed:?}");
    assert_eq!((feed.send_failed, feed.unsent), (0, 0));

    // C sends A's cookie from its own address: it is challenged, not
    // taken.
    let spoofed = a.receiver.subscribe();
    c.socket.send_to(&spoofed, node.feed).await.unwrap();
    assert_eq!(c.next(ARRIVES, now()).await, Some(Taken::Challenge));
    assert_eq!(stats().1, 1);

    // A renews; at the heartbeat A and B get a newer feed, C none.
    a.subscribe().await;
    until(&clock, async || stats().0 >= 1).await;
    clock.advance(FEED_HEARTBEAT_MS);
    for subscriber in [&mut a, &mut b] {
        let Taken::Feed { sequence_ms, .. } = subscriber.feed(now()).await else {
            panic!("not a feed");
        };
        assert!(sequence_ms >= START_MS + FEED_HEARTBEAT_MS);
    }
    assert!(c.quiet(now()).await);

    // A renews every minute, B does not: B lapses, and its place goes to
    // C, which asks again.
    let lapse = START_MS + FEED_HEARTBEAT_MS + SUBSCRIPTION_TTL_MS;
    while now() < lapse + RENEW_INTERVAL_MS {
        clock.advance(RENEW_INTERVAL_MS);
        let before = received();
        a.subscribe().await;
        // The node polls for the moved clock before it reads the renewal,
        // so it has sent the feeds due then.
        read(before + 1).await;
        // A refresh of its cookie, or a heartbeat: whatever comes.
        while a.next(QUIET, now()).await.is_some() {}
    }
    while b.next(QUIET, now()).await.is_some() {}
    clock.advance(FEED_HEARTBEAT_MS);
    a.feed(now()).await;
    assert!(b.quiet(now()).await);
    c.prove(now()).await;
    c.feed(now()).await;
    let feed = feed_status(&node).await.unwrap();
    assert_eq!(feed.subscribers, 2);
    assert!(feed.expired >= 1 && feed.renewed >= 1, "{feed:?}");

    node.stop().await;
    server.stop();
}

/// The rate feed in the node's status, as its owner reads it.
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

// A core node names its feed's port in its discovery document and in its
// link state;
// before it has a URL, it names none, and answers no subscriber
// (spec/areas/rate-feed.md#finding-a-feed).
#[tokio::test(flavor = "multi_thread")]
async fn a_core_node_names_its_feed_port() {
    let owner = key(62);
    let bound = Bound::new();
    let port = bound.feed_address().port();
    let clock = ManualClock::new(START_MS);
    let node = bound.start(clock.clone(), 62, Policies::DEV, &[]).await;
    support::set_up(&node, &owner).await;
    node.engine_started().await;
    assert_eq!(discovery_of(&node).await.get("feedPort"), None);
    assert_eq!(node_of(&node).own_info().feed_port, None);
    let mut subscriber = Subscriber::new(node.feed, owner.identity()).await;
    subscriber.subscribe().await;
    assert!(subscriber.quiet(clock.now_ms()).await);
    // Its owner sees the socket, and that the feed does not serve.
    let feed = feed_status(&node).await.unwrap();
    assert!(!feed.serving);
    assert_eq!(feed.address, node.feed.to_string());
    assert_eq!((feed.subscribers, feed.received, feed.published), (0, 0, 0));

    // With a hostname, the node is a core node: it names the port, and
    // the feed answers.
    let mut ipc = node.ipc().await;
    ipc.call(Method::SetHostname(proto::SetHostnameRequest {
        hostname: "localhost".into(),
    }))
    .await
    .unwrap();
    assert_eq!(node_of(&node).own_info().feed_port, Some(port));
    assert_eq!(discovery_of(&node).await["feedPort"], u64::from(port));
    subscriber.subscribe().await;
    assert_eq!(
        subscriber.next(ARRIVES, clock.now_ms()).await,
        Some(Taken::Challenge)
    );
    let feed = feed_status(&node).await.unwrap();
    assert!(feed.serving);
    // The Subscribe sent before waited in the socket, and is answered too,
    // though not necessarily before the answer that just arrived.
    until(&clock, async || {
        let feed = feed_status(&node).await.unwrap();
        (feed.received, feed.challenged, feed.sent) == (2, 2, 2)
    })
    .await;
    node.stop().await;
}

// A node without the feed's socket publishes no feed, and its status has
// none.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_without_the_socket_reports_no_feed() {
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
    node.stop().await;
}
