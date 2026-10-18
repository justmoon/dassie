//! Two daemons in one process, peering over real HTTPS (spec 7 §4.7): they
//! register, learn of each other, peer, pay several times the credit by
//! settling, keep their peering and balances across a restart, and end
//! the peering settled.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use dassie_identity::NodeKey;
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use support::btp_client::{BtpClient, pay_stream};
use support::pair::{CA, bootstrap, key, peered, prepare, settle, until};
use support::{Bound, START_MS, TestNode};

/// 5 USD: over four times the credit, so it only gets through if A
/// settles with B as it goes.
const PAYMENT: u64 = 5_000_000_000;

/// What each stub starts with: 100 USD.
const DEPOSIT: u64 = 100_000_000_000;

/// The credit a node extends to a peer: 1.1 USD.
const CREDIT: u64 = 1_100_000_000;

/// The net credit of `node`'s account for its peer `peer`.
async fn net(node: &TestNode, peer: &NodeKey) -> i128 {
    let path = format!("stub+usd:assets/interledger/{}", peer.node_id());
    let reply = node
        .ipc()
        .await
        .call(Method::ListAccounts(proto::ListAccountsRequest {}))
        .await;
    let Ok(response::Method::ListAccounts(list)) = reply else {
        panic!("{reply:?}")
    };
    let account = list.accounts.into_iter().find(|a| a.path == path).unwrap();
    i128::from(account.credits_posted) - i128::from(account.debits_posted)
}

/// What the network methods say of a pair: one peer, one route, the
/// other node known with its URL, and the registration with A.
async fn check_network(node: &TestNode, own: &NodeKey, other: &NodeKey) {
    let mut owner = node.ipc().await;
    let Ok(response::Method::ListPeers(peers)) = owner
        .call(Method::ListPeers(proto::ListPeersRequest {}))
        .await
    else {
        panic!("no peers")
    };
    assert_eq!(peers.peers.len(), 1);
    let peer = &peers.peers[0];
    assert_eq!(peer.node, other.node_id().to_string());
    assert_eq!((peer.scheme.as_str(), peer.version), ("stub", 1));
    assert_eq!(peer.credit_out, CREDIT);
    let Ok(response::Method::ListRoutes(routes)) = owner
        .call(Method::ListRoutes(proto::ListRoutesRequest {}))
        .await
    else {
        panic!("no routes")
    };
    assert_eq!(routes.routes.len(), 1);
    assert_eq!(
        routes.routes[0].prefix,
        format!("test.das.{}", other.node_id())
    );
    let Ok(response::Method::ListNodes(nodes)) = owner
        .call(Method::ListNodes(proto::ListNodesRequest {}))
        .await
    else {
        panic!("no nodes")
    };
    let them = nodes
        .nodes
        .iter()
        .find(|n| n.id == other.node_id().to_string())
        .unwrap();
    assert!(them.url.starts_with("https://localhost:") && them.peer && them.majority);
    let Ok(response::Method::GetRegistrationStatus(status)) = owner
        .call(Method::GetRegistrationStatus(
            proto::GetRegistrationStatusRequest {},
        ))
        .await
    else {
        panic!("no status")
    };
    assert_eq!(status.node, own.node_id().to_string());
    assert_eq!(status.bootstrap.len(), 1);
    assert!(status.bootstrap[0].listed);
}

/// The node's peerings.
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

/// The node's stub's balance on its "underlying ledger".
async fn stub_balance(node: &TestNode) -> u64 {
    let Ok(response::Method::ListLedgers(ledgers)) = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await
    else {
        panic!("no ledgers")
    };
    ledgers.ledgers[0].on_ledger_balance
}

/// Whether settling between A and B is quiet: nothing on its way either
/// way, and their books mirror each other.
async fn quiet(a: &TestNode, b: &TestNode, key_a: &NodeKey, key_b: &NodeKey) -> bool {
    for node in [a, b] {
        let Ok(response::Method::ListSettlementChannels(channels)) = node
            .ipc()
            .await
            .call(Method::ListSettlementChannels(
                proto::ListSettlementChannelsRequest {},
            ))
            .await
        else {
            return false;
        };
        if channels.channels.iter().any(|channel| channel.sending) {
            return false;
        }
    }
    net(a, key_b).await + net(b, key_a).await == 2 * i128::from(CREDIT)
}

#[tokio::test(flavor = "multi_thread")]
// `DEBUG` only turns on the log, to follow a failing run.
#[allow(clippy::disallowed_methods)]
async fn two_daemons_peer_and_keep_their_peering() {
    if let Ok(debug) = std::env::var("DEBUG") {
        dassie_node::log::init(dassie_node::log::LogFilter::new(
            dassie_node::log::LogLevel::Debug,
            &debug,
        ));
    }
    let (key_a, key_b) = (key(21), key(22));
    let (bound_a, bound_b) = (Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 1, Policies::DEV, &env).await;
    let b = bound_b.start(clock.clone(), 2, Policies::DEV, &env).await;
    prepare(&a, &key_a).await;
    prepare(&b, &key_b).await;
    until(&clock, async || {
        peered(&a, &key_b).await && peered(&b, &key_a).await
    })
    .await;
    // Both link states name the other, so each has a route to the other.
    settle(&clock).await;
    check_network(&a, &key_a, &key_b).await;
    check_network(&b, &key_b, &key_a).await;
    // Found in review: removing a scheme that peers use left their
    // peerings without a ledger, and the engine could not start again.
    let removed = a
        .ipc()
        .await
        .call(Method::RemoveSettlementScheme(
            proto::RemoveSettlementSchemeRequest { id: "stub".into() },
        ))
        .await;
    assert_eq!(
        removed.unwrap_err().code(),
        proto::ErrorCode::FailedPrecondition
    );
    // A client of A pays B's receiver, one hop away, beyond B's credit.
    let mut owner_a = a.ipc().await;
    let Ok(response::Method::CreateBtpToken(token)) = owner_a
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "wallet".into(),
            ledger: "stub+usd".into(),
            cap: None,
        }))
        .await
    else {
        panic!("no token")
    };
    let credentials = b.ipc().await.incoming_payment(None).await;
    let mut client = BtpClient::connect(&a, &token.secret).await.unwrap();
    pay_stream(
        &clock,
        &mut client,
        &credentials.destination_account,
        &credentials.shared_secret,
        PAYMENT,
    )
    .await;
    // A settled as it paid; once quiet, the books mirror each other, and
    // what A settled left its stub for B's.
    until(&clock, async || quiet(&a, &b, &key_a, &key_b).await).await;
    let at_a = net(&a, &key_b).await;
    let settled = u64::try_from(i128::from(PAYMENT) - (at_a - i128::from(CREDIT))).unwrap();
    assert!(settled >= PAYMENT - CREDIT, "settled {settled}");
    assert_eq!(stub_balance(&a).await, DEPOSIT - settled);
    assert_eq!(stub_balance(&b).await, DEPOSIT + settled);
    // Restarted, B still has its peering and its accounts: nothing to redo.
    let at_b = net(&b, &key_a).await;
    let (dir, clock_b) = b.stop().await;
    assert_eq!(clock_b.now_ms(), clock.now_ms());
    let b = Bound::bind_in(dir)
        .start(clock.clone(), 3, Policies::DEV, &env)
        .await;
    assert!(peered(&b, &key_a).await);
    assert_eq!(net(&b, &key_a).await, at_b);
    assert_eq!(stub_balance(&b).await, DEPOSIT + settled);
    // A's owner ends the peering: it ends on both sides, and A settles
    // everything it still owes.
    let ended = a
        .ipc()
        .await
        .call(Method::TerminatePeering(proto::TerminatePeeringRequest {
            node: key_b.node_id().to_string(),
        }))
        .await;
    assert!(ended.is_ok(), "{ended:?}");
    until(&clock, async || {
        peers(&a).await.is_empty()
            && peers(&b).await.is_empty()
            && quiet(&a, &b, &key_a, &key_b).await
            && net(&a, &key_b).await == i128::from(CREDIT)
    })
    .await;
    assert_eq!(stub_balance(&a).await + stub_balance(&b).await, 2 * DEPOSIT);
    // And they do not peer again straight away.
    settle(&clock).await;
    settle(&clock).await;
    assert!(peers(&a).await.is_empty() && peers(&b).await.is_empty());
    // A ban is kept, and lifted.
    let mut owner_a = a.ipc().await;
    let node_b = key_b.node_id().to_string();
    let banned = owner_a
        .call(Method::BanNode(proto::BanNodeRequest {
            node: node_b.clone(),
        }))
        .await;
    assert!(banned.is_ok(), "{banned:?}");
    let Ok(response::Method::ListBans(bans)) = owner_a
        .call(Method::ListBans(proto::ListBansRequest {}))
        .await
    else {
        panic!("no bans")
    };
    assert_eq!(bans.nodes, std::slice::from_ref(&node_b));
    let unbanned = owner_a
        .call(Method::UnbanNode(proto::UnbanNodeRequest { node: node_b }))
        .await;
    assert!(unbanned.is_ok(), "{unbanned:?}");
    a.stop().await;
    b.stop().await;
}

// D7: the owner's fee and spread are read, changed within range, and
// kept.
#[tokio::test(flavor = "multi_thread")]
async fn the_peer_settings_are_the_owners() {
    let node = TestNode::start().await;
    prepare(&node, &key(1)).await;
    let mut owner = node.ipc().await;
    let read = |response| match response {
        Ok(response::Method::GetPeerSettings(reply)) => reply.settings.unwrap().connector_fee_ppm,
        Ok(response::Method::SetPeerSettings(reply)) => reply.settings.unwrap().connector_fee_ppm,
        other => panic!("{other:?}"),
    };
    let get = || Method::GetPeerSettings(proto::GetPeerSettingsRequest {});
    assert_eq!(read(owner.call(get()).await), 0);
    let set = |connector_fee_ppm, fx_spread_ppm| {
        Method::SetPeerSettings(proto::SetPeerSettingsRequest {
            settings: Some(proto::PeerSettings {
                connector_fee_ppm,
                fx_spread_ppm,
            }),
        })
    };
    assert_eq!(read(owner.call(set(1_000, 2_000)).await), 1_000);
    for (fee, spread) in [(100_001, 0), (0, 100_001)] {
        assert!(
            owner.call(set(fee, spread)).await.is_err(),
            "{fee} {spread}"
        );
    }
    let Ok(response::Method::GetPeerSettings(reply)) = owner.call(get()).await else {
        panic!("no settings");
    };
    let settings = reply.settings.unwrap();
    assert_eq!(
        (settings.connector_fee_ppm, settings.fx_spread_ppm),
        (1_000, 2_000)
    );
    node.stop().await;
}
