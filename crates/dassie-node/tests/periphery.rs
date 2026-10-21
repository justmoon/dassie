//! A periphery daemon and its uplink, over real QUIC
//! (spec/areas/network.md#periphery-nodes): a node with a QUIC endpoint and
//! no hostname peers with a core node as its downlink, without a probe, is
//! paid beneath the uplink's address, and pays back through it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_ledger::LedgerId;
use dassie_node::clock::ManualClock;
use dassie_node::engine::NewIncoming;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use support::btp_client::{BtpClient, pay_stream};
use support::pair::{CA, bootstrap, key, peered, prepare, until};
use support::{Bound, START_MS, TestNode, set_up};

/// What the uplink's client pays the downlink: within the credit the
/// downlink, as junior, extends its uplink, so nothing needs settling.
const PAYMENT: u64 = 500_000_000;

/// What the downlink pays back: it nets against what the uplink owes it.
const BACK: u64 = 200_000_000;

/// Sets a node up with the stub scheme and no hostname: a periphery node.
async fn prepare_periphery(node: &TestNode, key: &NodeKey) {
    set_up(node, key).await;
    let mut owner = node.ipc().await;
    for _ in 0..200 {
        let added = owner
            .call(Method::AddSettlementScheme(
                proto::AddSettlementSchemeRequest {
                    id: "stub".into(),
                    config: String::new(),
                },
            ))
            .await;
        if added.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the stub scheme did not start");
}

async fn peers(node: &TestNode) -> Vec<proto::NetworkPeer> {
    let Ok(response::Method::ListPeers(peers)) = node
        .ipc()
        .await
        .call(Method::ListPeers(proto::ListPeersRequest {}))
        .await
    else {
        panic!("no peers")
    };
    peers.peers
}

async fn routes(node: &TestNode) -> Vec<String> {
    let Ok(response::Method::ListRoutes(routes)) = node
        .ipc()
        .await
        .call(Method::ListRoutes(proto::ListRoutesRequest {}))
        .await
    else {
        panic!("no routes")
    };
    routes
        .routes
        .into_iter()
        .map(|route| route.prefix)
        .collect()
}

/// A BTP client of `node`'s owner, on the stub ledger.
async fn client(node: &TestNode) -> BtpClient {
    let Ok(response::Method::CreateBtpToken(token)) = node
        .ipc()
        .await
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "wallet".into(),
            ledger: "stub+usd".into(),
            cap: None,
        }))
        .await
    else {
        panic!("no token")
    };
    BtpClient::connect(node, &token.secret).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
// `DEBUG` only turns on the log, to follow a failing run.
#[allow(clippy::disallowed_methods)]
async fn a_periphery_node_is_paid_beneath_its_uplink() {
    if let Ok(debug) = std::env::var("DEBUG") {
        dassie_node::log::init(dassie_node::log::LogFilter::new(
            dassie_node::log::LogLevel::Debug,
            &debug,
        ));
    }
    let (key_a, key_p) = (key(41), key(42));
    let (bound_a, bound_p) = (Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 1, Policies::DEV, &env).await;
    let p = bound_p.start(clock.clone(), 2, Policies::DEV, &env).await;
    prepare(&a, &key_a).await;
    prepare_periphery(&p, &key_p).await;
    until(&clock, async || {
        peered(&a, &key_p).await && peered(&p, &key_a).await
    })
    .await;
    // The periphery node asked; its uplink lists it nowhere and has no
    // route to it, while its own route goes through the uplink.
    let at_a = peers(&a).await;
    assert_eq!(at_a.len(), 1);
    assert!(at_a[0].inbound && at_a[0].node == key_p.node_id().to_string());
    let at_p = peers(&p).await;
    assert!(at_p.len() == 1 && !at_p[0].inbound);
    assert!(routes(&a).await.is_empty());
    assert_eq!(
        routes(&p).await,
        vec![format!("test.das.{}", key_a.node_id())]
    );
    // Only QUIC carried them, the uplink's requests on the connection the
    // periphery node opened.
    let quic = a.quic.as_ref().unwrap();
    assert!(quic.stats().sent.load(std::sync::atomic::Ordering::Relaxed) > 0);
    holds_a_downlinks_place(&a, &key_p).await;
    let payment = incoming(&p).await;
    let child = format!("test.das.{}.{}.", key_a.node_id(), key_p.node_id());
    assert!(
        payment.ilp_address.starts_with(&child),
        "{}",
        payment.ilp_address
    );
    // The uplink's client pays it there, through the uplink.
    let mut paying = client(&a).await;
    pay_stream(
        &clock,
        &mut paying,
        &payment.ilp_address,
        &payment.shared_secret,
        PAYMENT,
    )
    .await;
    // And it pays the uplink's receiver back, through the same link.
    let credentials = a.ipc().await.incoming_payment(None).await;
    let mut back = client(&p).await;
    pay_stream(
        &clock,
        &mut back,
        &credentials.destination_account,
        &credentials.shared_secret,
        BACK,
    )
    .await;
    // By its books, its uplink owes it what it was paid, less what it paid
    // back.
    assert_eq!(owes(&p, &key_a).await, -i128::from(PAYMENT - BACK));
    a.stop().await;
    p.stop().await;
}

/// Waits until `downlink`'s connection holds a downlink's place at its
/// uplink, apart from anyone else's (spec/areas/peer-protocol.md#quic).
async fn holds_a_downlinks_place(uplink: &TestNode, downlink: &NodeKey) {
    let node = uplink.node.upgrade().unwrap();
    assert!(
        node.peerings()
            .borrow()
            .downlinks
            .contains(&downlink.node_id())
    );
    let quic = uplink.quic.as_ref().unwrap();
    let mut places = quic.places().unwrap();
    for _ in 0..500 {
        if places.downlinks == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        places = quic.places().unwrap();
    }
    assert_eq!((places.core, places.downlinks), (0, 1), "{places:?}");
}

/// An incoming payment of the owner of `node`, made through its engine:
/// the RPC makes one only at a hostname, which a periphery node lacks.
async fn incoming(node: &TestNode) -> dassie_node::store::IncomingRow {
    let engine = node.node.upgrade().unwrap().engine().unwrap();
    engine
        .create_incoming_payment(NewIncoming {
            ledger: LedgerId::StubUsd,
            wallet_address: "pay/owner".into(),
            client: None,
            incoming_amount: None,
            expires_at_ms: None,
            metadata: None,
        })
        .await
        .unwrap()
}

/// What `node` owes its peer `peer` net, by its own books: its position
/// less the credit it extends; negative when the peer owes it.
async fn owes(node: &TestNode, peer: &NodeKey) -> i128 {
    let Ok(response::Method::ListAccounts(list)) = node
        .ipc()
        .await
        .call(Method::ListAccounts(proto::ListAccountsRequest {}))
        .await
    else {
        panic!("no accounts")
    };
    let net = |account: &str| {
        let path = format!("stub+usd:{account}/{}", peer.node_id());
        let found = list.accounts.iter().find(|a| a.path == path).unwrap();
        i128::from(found.credits_posted) - i128::from(found.debits_posted)
    };
    net("assets/interledger") + net("contra/trust")
}
