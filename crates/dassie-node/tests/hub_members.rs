//! Members of a settlement hub, as daemons on loopback over real QUIC, on
//! a clock the test moves by hand (spec/areas/hubs.md#in-the-node): four
//! validators run an XRP hub on the fake XRP Ledger, whose ledgers close on
//! that clock too, and members join it by a deposit from their owner books
//! of XRP, peer over it alone, pay each other over STREAM and settle over
//! it, their books in step with what their accounts hold across a restart.
//! A node added later joins as they did.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_xrpl::network::TESTNET;
use support::hubs::{HubAsset, HubNet, Joining, peer_accounts, peers};

/// 2 XRP: twenty of the largest packets, so the payer funds its hold
/// toward the payee over the hub as it pays.
const PAYMENT: u64 = 2_000_000_000;

const XRP: &[HubAsset] = &[HubAsset::Xrp];

/// What a member's owner has of XRP: on the hub and in the owner book.
fn owner_funds(books: &support::hubs::Books) -> i64 {
    books.values().map(|(_, owner)| owner).sum()
}

// spec/areas/hubs.md#in-the-node, spec/areas/payments.md#wallet-addresses:
// two members join the hub by a deposit from their owner books, which peer
// with nobody, and peer over the hub alone. The payee's one wallet address
// for XRP books on the hub's ledger and names no hub, and its deposits go
// to the owner book, never to the escrow. A client of the payer pays it
// over STREAM, more than the payer's first hold toward it: what arrived is
// what was sent; settling over the hub is over with each member's books
// holding what its accounts hold, its peers' holds toward it included; the
// payer's owner paid at least what was sent, and the payee's got at most
// that. Restarted, both members read the ledger and the hub again and book
// nothing twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn members_pay_over_a_hub_and_keep_their_books_across_a_restart() {
    let net = HubNet::start(&TESTNET, XRP).await;
    let hub = net.hub(HubAsset::Xrp).clone();
    let owner_ledger = HubAsset::Xrp.owner_ledger(&TESTNET);
    let mut members = net
        .members(&[Joining::of(81, XRP), Joining::of(82, XRP)], &[])
        .await;
    net.peered(&[&members[0], &members[1]]).await;

    let (payer, payee) = (&members[0], &members[1]);
    let Ok(response::Method::ListWalletAddresses(list)) = payee
        .node
        .ipc()
        .await
        .call(Method::ListWalletAddresses(
            proto::ListWalletAddressesRequest {},
        ))
        .await
    else {
        panic!("no wallet addresses");
    };
    let xrp = list
        .wallet_addresses
        .iter()
        .find(|address| address.name == "xrp")
        .expect("no XRP wallet address");
    assert_eq!(xrp.ledger, hub.ledger);
    assert_eq!(xrp.deposit_ledger, owner_ledger);
    assert!(
        !list
            .wallet_addresses
            .iter()
            .any(|address| address.url.contains("/pay/hub-")),
        "an address names the hub"
    );
    let hub_view = support::hubs::ledger(&payee.node, &hub.ledger)
        .await
        .unwrap();
    assert_eq!(
        hub_view.deposit_address, "",
        "the escrow is no deposit address"
    );

    let before = net.settled(&[payer, payee]).await;
    let incoming = support::hubs::pay(
        &net.clock,
        (&payer.node, &payee.node),
        &hub.ledger,
        PAYMENT,
        &xrp.ledger,
    )
    .await;
    assert_eq!(incoming.ledger, hub.ledger);
    assert_eq!(incoming.received_amount, PAYMENT, "what arrived");
    let after = net.settled(&[payer, payee]).await;
    let spent = owner_funds(&before[0]) - owner_funds(&after[0]);
    let got = owner_funds(&after[1]) - owner_funds(&before[1]);
    let payment = i64::try_from(PAYMENT).unwrap();
    assert!(spent >= payment, "the payer's owner spent {spent}");
    assert!(got > 0 && got <= payment, "the payee's owner got {got}");

    // Restarted, both read their accounts again and book nothing twice.
    let owed = peer_accounts(&[payer, payee], &hub.ledger).await;
    assert!(!owed.is_empty());
    let members = net.restart_all(members.drain(..)).await;
    let both = [&members[0], &members[1]];
    assert_eq!(net.settled(&both).await, after, "the books");
    assert_eq!(
        peer_accounts(&both, &hub.ledger).await,
        owed,
        "peer accounts"
    );
    net.stop(members).await;
}

// spec/areas/hubs.md#in-the-node: a node added once two members peer over
// the hub joins it as they did: its scheme takes the hub's rules, it funds
// its account from its owner book, and peers over the hub alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_node_added_later_joins_and_peers_over_the_hub_alone() {
    let net = HubNet::start(&TESTNET, XRP).await;
    let hub = net.hub(HubAsset::Xrp).clone();
    let members = net
        .members(&[Joining::of(83, XRP), Joining::of(84, XRP)], &[])
        .await;
    net.peered(&[&members[0], &members[1]]).await;

    let added = net
        .members(&[Joining::of(85, XRP)], &[&members[0], &members[1]])
        .await
        .pop()
        .unwrap();
    let state = added
        .node
        .ipc()
        .await
        .call(Method::GetBasicState(proto::GetBasicStateRequest {}))
        .await;
    let Ok(response::Method::GetBasicState(state)) = state else {
        panic!("{state:?}");
    };
    assert!(
        state
            .active_settlement_schemes
            .iter()
            .any(|s| s == hub.scheme),
        "{:?}",
        state.active_settlement_schemes
    );
    net.peered(&[&members[0], &members[1], &added]).await;
    for peer in peers(&added.node).await {
        for method in &peer.methods {
            assert_eq!(method.scheme, hub.scheme);
        }
    }
    net.stop(members.into_iter().chain([added])).await;
}
