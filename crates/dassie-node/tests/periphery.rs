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
use dassie_node::rpc::proto::{self, ErrorCode, request::Method, response};
use support::btp_client::{BtpClient, pay_stream};
use support::pair::{CA, bootstrap, key, peered, prepare, settle, until};
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
    // The first may still be on its way once both sides count the peering.
    let quic = a.quic.as_ref().unwrap();
    until(&clock, async || {
        quic.stats().sent.load(std::sync::atomic::Ordering::Relaxed) > 0
    })
    .await;
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
    // By its books, its packets with its uplink came to what it was paid,
    // less what it paid back.
    assert_eq!(carried(&p, &key_a).await, -i128::from(PAYMENT - BACK));
    a.stop().await;
    p.stop().await;
}

// spec/areas/network.md#pinned-uplinks: a periphery daemon with
// `DASSIE_UPLINKS` asks only the node it names, and looks for no other
// uplink, though it knows the bootstrap node and would ask it too.
#[tokio::test(flavor = "multi_thread")]
async fn a_pinned_periphery_node_asks_only_its_uplink() {
    let (key_a, key_b, key_p) = (key(43), key(44), key(45));
    let (bound_a, bound_b, bound_p) = (Bound::new(), Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let pinned = key_b.node_id().to_string();
    let mut env_p = env.to_vec();
    env_p.push(("DASSIE_UPLINKS", pinned.as_str()));
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 3, Policies::DEV, &env).await;
    let b = bound_b.start(clock.clone(), 4, Policies::DEV, &env).await;
    let p = bound_p.start(clock.clone(), 5, Policies::DEV, &env_p).await;
    prepare(&a, &key_a).await;
    prepare(&b, &key_b).await;
    prepare_periphery(&p, &key_p).await;
    until(&clock, async || {
        peered(&a, &key_b).await && peered(&b, &key_p).await && peered(&p, &key_b).await
    })
    .await;
    // Time to ask for a second uplink, as an unpinned node would.
    settle(&clock).await;
    settle(&clock).await;
    let at_p = peers(&p).await;
    assert_eq!(at_p.len(), 1, "{at_p:?}");
    assert_eq!(at_p[0].node, key_b.node_id().to_string());
    assert!(
        !peers(&a)
            .await
            .iter()
            .any(|peer| peer.node == key_p.node_id().to_string())
    );
    a.stop().await;
    b.stop().await;
    p.stop().await;
}

// spec/areas/payments.md#receiving: a periphery node with two uplinks
// hands out credentials beneath the one its owner names, and a payment to
// each arrives through that uplink. One it has not is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_periphery_node_is_paid_beneath_the_uplink_it_names() {
    let (key_a, key_b, key_p) = (key(46), key(47), key(48));
    let (bound_a, bound_b, bound_p) = (Bound::new(), Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let pinned = format!("{},{}", key_a.node_id(), key_b.node_id());
    let mut env_p = env.to_vec();
    env_p.push(("DASSIE_UPLINKS", pinned.as_str()));
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 6, Policies::DEV, &env).await;
    let b = bound_b.start(clock.clone(), 7, Policies::DEV, &env).await;
    let p = bound_p.start(clock.clone(), 8, Policies::DEV, &env_p).await;
    prepare(&a, &key_a).await;
    prepare(&b, &key_b).await;
    prepare_periphery(&p, &key_p).await;
    // Both uplinks confirmed, which is when the node is paid beneath them.
    let confirmed = [key_a.node_id().to_string(), key_b.node_id().to_string()];
    until(&clock, async || {
        let at_p = peers(&p).await;
        confirmed
            .iter()
            .all(|id| at_p.iter().any(|peer| &peer.node == id && peer.advertised))
            && peered(&a, &key_p).await
            && peered(&b, &key_p).await
    })
    .await;
    for (uplink, key_uplink) in [(&a, &key_a), (&b, &key_b)] {
        let payment = create(&p, Some(key_uplink.node_id().to_string()))
            .await
            .unwrap();
        let child = format!("test.das.{}.{}.", key_uplink.node_id(), key_p.node_id());
        assert!(
            payment.destination_account.starts_with(&child),
            "{}",
            payment.destination_account
        );
        // The uplink's client pays it there, through that uplink and not
        // the other.
        let before = [carried(&p, &key_a).await, carried(&p, &key_b).await];
        let mut paying = client(uplink).await;
        pay_stream(
            &clock,
            &mut paying,
            &payment.destination_account,
            &payment.shared_secret,
            PAYMENT,
        )
        .await;
        let after = [carried(&p, &key_a).await, carried(&p, &key_b).await];
        let moved = if key_uplink.node_id() == key_a.node_id() {
            [-i128::from(PAYMENT), 0]
        } else {
            [0, -i128::from(PAYMENT)]
        };
        assert_eq!([after[0] - before[0], after[1] - before[1]], moved);
    }
    // A node that is no uplink of its, and no node at all, are refused.
    let refused = create(&p, Some(key(49).node_id().to_string())).await;
    assert_eq!(refused.unwrap_err().code(), ErrorCode::FailedPrecondition);
    let refused = create(&p, Some("not a node".into())).await;
    assert_eq!(refused.unwrap_err().code(), ErrorCode::InvalidArgument);
    a.stop().await;
    b.stop().await;
    p.stop().await;
}

/// The owner's incoming payment on `node`, beneath `uplink` if named,
/// made over RPC.
async fn create(
    node: &TestNode,
    uplink: Option<String>,
) -> Result<proto::IncomingPayment, proto::Error> {
    let created = node
        .ipc()
        .await
        .call(Method::CreateIncomingPayment(
            proto::CreateIncomingPaymentRequest {
                ledger: "stub+usd".into(),
                uplink,
                ..proto::CreateIncomingPaymentRequest::default()
            },
        ))
        .await?;
    let response::Method::CreateIncomingPayment(proto::CreateIncomingPaymentResponse {
        payment: Some(payment),
    }) = created
    else {
        panic!("answered out of turn: {created:?}")
    };
    Ok(payment)
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

/// An incoming payment of the owner of `node`, made through its engine.
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
            uplink: None,
        })
        .await
        .unwrap()
}

/// What the packets between `node` and its peer `peer` moved net, by
/// `node`'s own books: what it owes the peer for them, negative when the
/// peer owes it. Settlements do not count: a senior extends its junior no
/// credit and asks it to pay in advance at once
/// (spec/decisions/earned-trust.md), at a time of its own, so what the
/// node owes in all can move while a payment runs.
async fn carried(node: &TestNode, peer: &NodeKey) -> i128 {
    let path = format!("stub+usd:assets/interledger/{}", peer.node_id());
    let mut owner = node.ipc().await;
    let mut after = None;
    let mut net = 0;
    // A test's journal fits in a few pages.
    for _ in 0..100 {
        let Ok(response::Method::ListTransfers(list)) = owner
            .call(Method::ListTransfers(proto::ListTransfersRequest {
                after_sequence: after,
                limit: 1000,
                account: None,
            }))
            .await
        else {
            panic!("no journal")
        };
        let Some(last) = list.transfers.last() else {
            return net;
        };
        after = Some(last.sequence);
        for transfer in &list.transfers {
            let packet = matches!(
                transfer.reference,
                Some(proto::transfer::Reference::Packet(_))
            );
            if !packet || transfer.state() != proto::TransferState::Posted {
                continue;
            }
            if transfer.credit_path == path {
                net += i128::from(transfer.amount);
            }
            if transfer.debit_path == path {
                net -= i128::from(transfer.amount);
            }
        }
    }
    panic!("the journal did not end");
}
