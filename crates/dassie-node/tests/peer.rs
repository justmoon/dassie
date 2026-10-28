//! Two daemons in one process, peering over real QUIC and HTTPS
//! (spec/areas/network.md#peering): they register, learn of each other, peer
//! over the stub with commitments, pay several times what one settlement
//! funds by funding the payer's hold as it goes, keep their peering, holds
//! and balances across a restart, and end the peering
//! (spec/areas/settlement.md#the-stub-scheme, spec/areas/hubs.md#in-the-node).
//! Two methods over two hubs, kept across restarts and wound down when the
//! owner removes a scheme, are the core tests' (`engine::core::tests::winding`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::sync::atomic::Ordering;

use dassie_identity::IdentityKey;
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use support::btp_client::{BtpClient, pay_stream};
use support::pair::{bootstrap, ca_file, key, peered, prepare, prepare_settling, settle, until};
use support::{Bound, START_MS, TestNode};

/// 5 USD: over four times the credit a payer over a hub funds its hold as
/// if it extended, when asked, so it only gets through if A funds its hold
/// toward B several times as it goes.
const PAYMENT: u64 = 5_000_000_000;
const _: () = assert!(PAYMENT > 4 * CREDIT);

/// The most packets each stub holds in flight per peer each way: the most
/// the stub with commitments takes, as a hub's member holds more than the
/// connector puts in flight.
const COMMITMENTS: usize = 64;
const _: () = assert!(COMMITMENTS >= Policies::DEV.connector.max_pending_per_endpoint);

/// What each stub starts with: 100 USD.
const DEPOSIT: u64 = 100_000_000_000;

/// The credit a payer over a hub funds its hold as if it extended, when
/// asked to settle (spec/areas/hubs.md#in-the-node): the node's default.
const CREDIT: u64 = Policies::DEV.peer_credit;

/// The net credit of `node`'s account `account` for its peer `peer`.
async fn net_of(node: &TestNode, account: &str, peer: &IdentityKey) -> i128 {
    let path = format!("stub+usd:{account}/{}", peer.node_id());
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

/// What `node` owes its peer `peer` net, by its own books: its position
/// less the credit it extends; negative when the peer owes it.
async fn owes(node: &TestNode, peer: &IdentityKey) -> i128 {
    net_of(node, "assets/interledger", peer).await + net_of(node, "contra/trust", peer).await
}

/// What the network methods say of a pair: one peer, one route, the
/// other node known with its URL, and the registration with A.
async fn check_network(node: &TestNode, own: &IdentityKey, other: &IdentityKey) {
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
    let schemes: Vec<&str> = peer.methods.iter().map(|m| m.scheme.as_str()).collect();
    assert_eq!(schemes, ["stub"]);
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

/// Whether `node` has what [`check_network`] checks, once the other's
/// link state reached it: a route to the other, which it knows as a peer
/// with a majority, and its registration with A listed.
async fn knows(node: &TestNode, other: &IdentityKey) -> bool {
    let mut owner = node.ipc().await;
    let Ok(response::Method::ListRoutes(routes)) = owner
        .call(Method::ListRoutes(proto::ListRoutesRequest {}))
        .await
    else {
        return false;
    };
    let prefix = format!("test.das.{}", other.node_id());
    let Ok(response::Method::ListNodes(nodes)) = owner
        .call(Method::ListNodes(proto::ListNodesRequest {}))
        .await
    else {
        return false;
    };
    let Ok(response::Method::GetRegistrationStatus(status)) = owner
        .call(Method::GetRegistrationStatus(
            proto::GetRegistrationStatusRequest {},
        ))
        .await
    else {
        return false;
    };
    let other = other.node_id().to_string();
    routes.routes.iter().any(|route| route.prefix == prefix)
        && nodes
            .nodes
            .iter()
            .any(|node| node.id == other && node.peer && node.majority)
        && status.bootstrap.iter().any(|bootstrap| bootstrap.listed)
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

/// The credit `node` extends its one peer, on its one method.
async fn credit_out(node: &TestNode) -> u64 {
    let peers = peers(node).await;
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].methods.len(), 1);
    peers[0].methods[0].credit_out
}

/// The settlements `node` sent its one peer: each takes the next sequence.
async fn settlements(node: &TestNode) -> u64 {
    let Ok(response::Method::ListSettlementChannels(channels)) = node
        .ipc()
        .await
        .call(Method::ListSettlementChannels(
            proto::ListSettlementChannelsRequest {},
        ))
        .await
    else {
        panic!("no channels")
    };
    assert_eq!(channels.channels.len(), 1);
    channels.channels[0].settled_out
}

/// What A and B hold toward each other, and their stubs' balances.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Holds {
    /// The credit A extends B: its hold toward B.
    to_b: u64,
    /// The credit B extends A: its hold toward A.
    to_a: u64,
    /// A's stub's balance.
    stub_a: u64,
    /// B's stub's balance.
    stub_b: u64,
}

/// Reads [`Holds`], one value at a time.
async fn holds(a: &TestNode, b: &TestNode) -> Holds {
    Holds {
        to_b: credit_out(a).await,
        to_a: credit_out(b).await,
        stub_a: stub_balance(a).await,
        stub_b: stub_balance(b).await,
    }
}

/// Waits until settling between A and B is quiet and stays so across two
/// reads of their [`Holds`] a clock step apart that agree, and returns that
/// read: [`quiet`] sees only the settlements already on their way, so a
/// request to settle that landed in the last step shows up as a change
/// between the reads, not as a read torn halfway through a settlement.
async fn steady(
    clock: &ManualClock,
    a: &TestNode,
    b: &TestNode,
    key_a: &IdentityKey,
    key_b: &IdentityKey,
) -> Holds {
    let mut last = None;
    until(clock, async || {
        if !quiet(a, b, key_a, key_b).await {
            last = None;
            return false;
        }
        let read = holds(a, b).await;
        let agrees = last == Some(read) && quiet(a, b, key_a, key_b).await;
        last = Some(read);
        agrees
    })
    .await;
    last.unwrap()
}

/// Checks that the holds are what settling funded: over a hub, the credit
/// each extends the other is its own hold toward it, nothing unsecured
/// (spec/areas/hubs.md#in-the-node). What moved from A's stub to B's net is
/// what A settled less what B settled; what A settled funds its hold, which
/// paid B `paid` and holds what is left, and what B settled holds B's.
fn funded(holds: Holds, paid: u64) {
    let Holds {
        to_b,
        to_a,
        stub_a,
        stub_b,
    } = holds;
    let moved = i128::from(DEPOSIT) - i128::from(stub_a);
    assert_eq!(
        i128::from(to_b) + i128::from(paid) - i128::from(to_a),
        moved,
        "A holds {to_b} toward B, B {to_a} toward A"
    );
    assert_eq!(stub_a + stub_b, 2 * DEPOSIT);
}

/// Whether settling between A and B is quiet: nothing on its way either
/// way, and their books mirror each other.
async fn quiet(a: &TestNode, b: &TestNode, key_a: &IdentityKey, key_b: &IdentityKey) -> bool {
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
    owes(a, key_b).await + owes(b, key_a).await == 0
}

#[tokio::test(flavor = "multi_thread")]
// `DASSIE_LOG` only turns on the log, to follow a failing run.
#[allow(clippy::disallowed_methods)]
async fn two_daemons_peer_and_keep_their_peering() {
    if let Ok(text) = std::env::var("DASSIE_LOG") {
        dassie_node::log::init(dassie_node::log::LogFilter::parse(&text).unwrap());
    }
    let (key_a, key_b) = (key(21), key(22));
    let (bound_a, bound_b) = (Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_EXTRA_CA_FILE", ca_file()),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 1, Policies::DEV, &env).await;
    let b = bound_b.start(clock.clone(), 2, Policies::DEV, &env).await;
    prepare_settling(&a, &key_a, COMMITMENTS).await;
    prepare_settling(&b, &key_b, COMMITMENTS).await;
    until(&clock, async || {
        peered(&a, &key_b).await && peered(&b, &key_a).await
    })
    .await;
    // Both link states name the other, so each has a route to the other.
    until(&clock, async || {
        knows(&a, &key_b).await && knows(&b, &key_a).await
    })
    .await;
    check_network(&a, &key_a, &key_b).await;
    check_network(&b, &key_b, &key_a).await;
    // Both take QUIC, so they peered over it: each asked the other and
    // answered it there.
    for node in [&a, &b] {
        let quic = node.quic.as_ref().unwrap();
        assert!(quic.stats().sent.load(Ordering::Relaxed) > 0);
        assert!(quic.stats().answered.load(Ordering::Relaxed) > 0);
    }
    // A client of A pays B's receiver, one hop away, beyond what one
    // settlement funds.
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
    // A funded its hold toward B as it paid; once quiet, the books mirror
    // each other, and what moved from A's stub to B's paid B and holds
    // what is left.
    let after = steady(&clock, &a, &b, &key_a, &key_b).await;
    funded(after, PAYMENT);
    let moved = DEPOSIT - after.stub_a;
    // It did so many times as it went: asked to settle, A funds its hold
    // as if it extended the default credit, half the way to its
    // trigger (spec/areas/hubs.md#in-the-node), and B lets it owe little
    // beyond that hold, so no one settlement moves the credit: what moved
    // took at least one settlement per credit, several as A paid over four.
    let settled = settlements(&a).await;
    assert!(
        settled.saturating_mul(CREDIT) >= moved,
        "{settled} settlements moved {moved}"
    );
    // By A's books it owes B what it paid less what moved net.
    let left = i128::from(PAYMENT) - i128::from(moved);
    assert_eq!(owes(&a, &key_b).await, left);
    // Restarted, on its own ports as a node does, B still has its peering
    // and its accounts: nothing to redo.
    let ports_b = b.ports();
    let at_b = owes(&b, &key_a).await;
    let hold_b = credit_out(&b).await;
    let (dir, clock_b) = b.stop().await;
    assert_eq!(clock_b.now_ms(), clock.now_ms());
    let b = Bound::bind_again(dir, ports_b)
        .await
        .start(clock.clone(), 3, Policies::DEV, &env)
        .await;
    assert!(peered(&b, &key_a).await);
    assert_eq!(owes(&b, &key_a).await, at_b);
    assert_eq!(stub_balance(&b).await, DEPOSIT + moved);
    // Its stub's state keeps its hold toward A, which its credit follows.
    assert_eq!(credit_out(&b).await, hold_b);
    // A's owner ends the peering: A collects first, asking B to settle once
    // more, then tells it, and it ends on both sides
    // (spec/decisions/ending-a-peering.md). A pays nothing: what it owes B
    // can only have grown, by what B settled.
    let owed = owes(&a, &key_b).await;
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
    })
    .await;
    assert!(owes(&a, &key_b).await >= owed);
    assert_eq!(stub_balance(&a).await + stub_balance(&b).await, 2 * DEPOSIT);
    // And they do not peer again straight away: not in ten seconds of
    // their time, a check that nothing happens, which a slow machine only
    // weakens.
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

// spec/decisions/exchange-rates.md: the owner's fee and spread are read,
// changed within range, and kept.
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
    let set = |connector_fee_ppm, fx_spread_ppm, rebalance_ppm| {
        Method::SetPeerSettings(proto::SetPeerSettingsRequest {
            settings: Some(proto::PeerSettings {
                connector_fee_ppm,
                fx_spread_ppm,
                rebalance_ppm,
                offer_swaps: true,
                swapping: true,
                swap_targets: Vec::new(),
            }),
        })
    };
    let settings = || async {
        let Ok(response::Method::GetPeerSettings(reply)) = node.ipc().await.call(get()).await
        else {
            panic!("no settings");
        };
        let settings = reply.settings.unwrap();
        (
            settings.connector_fee_ppm,
            settings.fx_spread_ppm,
            settings.rebalance_ppm,
        )
    };
    // The curve is on from the start (spec/decisions/passive-rebalancing.md).
    assert_eq!(settings().await, (0, 0, 50_000));
    assert_eq!(read(owner.call(set(1_000, 2_000, 30_000)).await), 1_000);
    for (fee, spread, rebalance) in [(100_001, 0, 0), (0, 100_001, 0), (0, 0, 100_001)] {
        assert!(
            owner.call(set(fee, spread, rebalance)).await.is_err(),
            "{fee} {spread} {rebalance}"
        );
    }
    assert_eq!(settings().await, (1_000, 2_000, 30_000));
    // The swap settings, and a periphery node's targets, by asset
    // (spec/decisions/peer-swaps.md); a hub's ledger is no asset.
    let swaps = |swap_targets| {
        Method::SetPeerSettings(proto::SetPeerSettingsRequest {
            settings: Some(proto::PeerSettings {
                connector_fee_ppm: 1_000,
                fx_spread_ppm: 2_000,
                rebalance_ppm: 30_000,
                offer_swaps: false,
                swapping: true,
                swap_targets,
            }),
        })
    };
    let target = |ledger: &str, target_ppm| proto::SwapTarget {
        ledger: ledger.to_owned(),
        target_ppm,
    };
    let targets = vec![
        target("stub+usd", 400_000),
        target("xrpl-testnet-xrp+xrp", 600_000),
    ];
    let Ok(response::Method::SetPeerSettings(reply)) = owner.call(swaps(targets.clone())).await
    else {
        panic!("refused");
    };
    let kept = reply.settings.unwrap();
    assert!(!kept.offer_swaps && kept.swapping);
    assert_eq!(kept.swap_targets, targets);
    for refused in [
        vec![target("hub-xrpl-testnet-xrp+xrp", 1)],
        vec![target("stub+usd", 1), target("stub+usd", 2)],
        vec![target("stub+usd", 0)],
        vec![target("nothing", 1)],
    ] {
        assert!(owner.call(swaps(refused)).await.is_err());
    }
    let Ok(response::Method::GetSwapStatus(status)) = owner
        .call(Method::GetSwapStatus(proto::GetSwapStatusRequest {}))
        .await
    else {
        panic!("no swap status");
    };
    assert_eq!((status.sent, status.in_flight), (0, 0));
    // A swap needs a rate: one left out reads as 0, and is refused before
    // anything else is looked at.
    let free = |rate_numerator, rate_denominator| {
        Method::SwapWithPeer(proto::SwapWithPeerRequest {
            node: key(1).node_id().as_str().to_owned(),
            pay_on: "stub".to_owned(),
            back_on: "stub-two".to_owned(),
            amount: 1_000,
            rate_numerator,
            rate_denominator,
        })
    };
    for (numerator, denominator) in [(0, 1), (1, 0), (0, 0)] {
        let refused = owner.call(free(numerator, denominator)).await.unwrap_err();
        assert_eq!(refused.code(), proto::ErrorCode::InvalidArgument);
    }
    node.stop().await;
}
