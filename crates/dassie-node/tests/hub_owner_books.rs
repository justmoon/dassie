//! Members of settlement hubs and their owner books on the XRP Ledger, as
//! daemons on loopback over real QUIC, on a clock the test moves by hand
//! (spec/areas/hubs.md#in-the-node): the fake XRP Ledger closes its
//! ledgers on that clock, and four validators run the hubs. A member moves
//! XRP into a hub from its owner book as it pays, and both keep their books
//! across a restart; and on the fake ledger's mainnet, in the live realm,
//! members of an XRP hub and an RLUSD hub pay each other over both.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

mod support;

use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_xrpl::network::{MAINNET, TESTNET};
use support::TestNode;
use support::hubs::{
    Books, HubAsset, HubNet, Joining, account_of, hub_status, ledger, pay, peer_accounts,
    peer_ledgers,
};

/// The largest packet, in internal units: 0.1 XRP.
const MAX_PACKET: u64 = 100_000_000;

/// What a payment takes beyond what the payer's account in the hub can
/// spend above its floor: ten of the largest packets, more than the
/// trigger and the payee's redeem threshold together, so that what the
/// payer funds of its hold takes the account below its floor even with
/// what it still owes unfunded.
const BEYOND_FLOOR: u64 = 10 * MAX_PACKET;

const XRP: &[HubAsset] = &[HubAsset::Xrp];

/// The transfers `node` wrote or changed after the sequence `after`, each
/// once, as it last was; and the last sequence it read.
async fn transfers_since(
    node: &TestNode,
    after: Option<u64>,
) -> (Vec<proto::Transfer>, Option<u64>) {
    let mut owner = node.ipc().await;
    let mut by_id = std::collections::BTreeMap::new();
    let mut last = after;
    loop {
        let Ok(response::Method::ListTransfers(page)) = owner
            .call(Method::ListTransfers(proto::ListTransfersRequest {
                after_sequence: last,
                limit: 1_000,
                account: None,
            }))
            .await
        else {
            panic!("no journal")
        };
        let Some(newest) = page.transfers.last() else {
            return (by_id.into_values().collect(), last);
        };
        last = Some(newest.sequence);
        for transfer in page.transfers {
            by_id.insert(transfer.id, transfer);
        }
    }
}

/// The fees `node` booked on its ledger `id` after the sequence `after`:
/// the XRP Ledger's for its transactions, or the hub's for its operations.
async fn fees_since(node: &TestNode, id: &str, after: Option<u64>) -> i64 {
    let (transfers, _) = transfers_since(node, after).await;
    let fees: u64 = transfers
        .iter()
        .filter(|t| matches!(t.reference, Some(proto::transfer::Reference::Fee(_))))
        .filter(|t| t.state() == proto::TransferState::Posted)
        .filter(|t| t.debit_path.starts_with(&format!("{id}:")))
        .map(|t| t.amount)
        .sum();
    i64::try_from(fees).unwrap()
}

/// What a member's books hold, and its account's counters in the hub of
/// `scheme`: what deposits credited it, what withdrawals took, and what
/// came back.
async fn kept(net: &HubNet, member: &support::hubs::Member, scheme: &str) -> (Books, [u64; 3]) {
    let status = hub_status(&member.node, scheme).await;
    (
        net.books(member).await,
        [status.deposited, status.withdrawn, status.returned],
    )
}

// spec/areas/hubs.md#in-the-node, spec/areas/xrpl.md#owner-books: the
// faucet funds each member's owner book of XRP, which follows the fake
// ledger at an address that starts with `r`, and peers with nobody. A
// client of one member pays the other's receiver more than the payer's
// account in the hub can spend above its floor, so the payer moves XRP
// into the hub as it pays: its owner book pays exactly what moved and the
// XRP Ledger's fees for it, and its hub book gets what moved, less the
// payment and the hub's fees. Restarted, both members' owner books, hub
// books and accounts' counters in the hub are as they were, each book
// holding what its account holds, so nothing was booked twice.
#[tokio::test(start_paused = true)]
async fn members_settle_from_their_owner_books_across_a_restart() {
    let net = HubNet::start(&TESTNET, XRP).await;
    let hub = net.hub(HubAsset::Xrp).clone();
    let owner_ledger = HubAsset::Xrp.owner_ledger(&TESTNET);
    let mut members = net
        .members(&[Joining::of(86, XRP), Joining::of(87, XRP)], &[])
        .await;
    for member in &members {
        let view = ledger(&member.node, &owner_ledger).await.unwrap();
        assert!(view.faucet, "the testnet has a faucet");
        assert!(view.deposit_address.starts_with('r'));
        assert_eq!(
            view.deposit_address,
            account_of(&member.key, &TESTNET).to_address()
        );
    }
    net.peered(&[&members[0], &members[1]]).await;
    net.settled(&[&members[0], &members[1]]).await;

    let (payer, payee) = (&members[0], &members[1]);
    let status = hub_status(&payer.node, hub.scheme).await;
    assert!(status.automatic, "the payer moves funds by itself");
    let payment = status.balance.saturating_sub(status.floor) + BEYOND_FLOOR;
    let (before, counters) = kept(&net, payer, hub.scheme).await;
    let (_, journaled) = transfers_since(&payer.node, None).await;
    pay(
        &net.clock,
        (&payer.node, &payee.node),
        &hub.ledger,
        payment,
        &hub.ledger,
    )
    .await;
    net.claimed(&[payer, payee]).await;
    let (after, moved_counters) = kept(&net, payer, hub.scheme).await;
    let moved = i64::try_from(moved_counters[0] - counters[0]).unwrap();
    assert!(moved > 0, "the payer moved nothing into the hub");
    assert_eq!(moved_counters[1..], counters[1..], "withdrawn or returned");
    // The owner book paid what moved and the XRP Ledger's fees for moving
    // it; the hub book got what moved, less what was paid and the hub's
    // fees for the operations that paid it.
    let owner_fees = fees_since(&payer.node, &owner_ledger, journaled).await;
    assert!(owner_fees > 0, "the moves paid no ledger fee");
    assert_eq!(
        before[&owner_ledger].1 - after[&owner_ledger].1,
        moved + owner_fees,
        "what the owner book paid"
    );
    let hub_fees = fees_since(&payer.node, &hub.ledger, journaled).await;
    assert_eq!(
        after[&hub.ledger].1 - before[&hub.ledger].1,
        moved - i64::try_from(payment).unwrap() - hub_fees,
        "what the hub book got"
    );

    // Restarted, both read the ledger and the hub again, and book nothing
    // twice.
    let mut books = Vec::new();
    for member in &members {
        books.push(kept(&net, member, hub.scheme).await);
    }
    let owed = peer_accounts(&[&members[0], &members[1]], &hub.ledger).await;
    let members = net.restart_all(members.drain(..)).await;
    net.settled(&[&members[0], &members[1]]).await;
    for (member, kept_before) in members.iter().zip(books) {
        assert_eq!(
            kept(&net, member, hub.scheme).await,
            kept_before,
            "the books"
        );
    }
    assert_eq!(
        peer_accounts(&[&members[0], &members[1]], &hub.ledger).await,
        owed
    );
    assert!(
        peer_accounts(&[&members[0], &members[1]], &owner_ledger)
            .await
            .is_empty(),
        "the owner books peer with nobody"
    );
    net.stop(members).await;
}

/// 2 XRP: twenty of the largest packets, so the payer funds its hold
/// toward the payee over the hub as it pays.
const XRP_PAYMENT: u64 = 2_000_000_000;
/// 1 RLUSD, at the ledger's scale of 9.
const RLUSD_PAYMENT: u64 = 1_000_000_000;

// spec/decisions/mainnet-schemes.md, spec/areas/hubs.md#in-the-node: in
// the live realm, by the dev switch, validators run an XRP hub and an
// RLUSD hub on the fake ledger's mainnet. A node of the live realm is
// offered mainnet's books and none of the testnet's schemes. Two members
// join both with their rules, their ledgers of them of the live realm, and
// peer over both, a method each. One pays the other in XRP over the XRP
// hub, and the other pays back in RLUSD over the RLUSD hub: each payment
// keeps its ledger, under `g.`, so what arrived is what was sent, and each
// owner's books show it, in step with what its accounts hold.
#[tokio::test(start_paused = true)]
async fn live_realm_members_pay_over_mainnet_hubs_of_xrp_and_rlusd() {
    const BOTH: &[HubAsset] = &[HubAsset::Xrp, HubAsset::Dollar];
    let net = HubNet::start(&MAINNET, BOTH).await;
    let (xrp, rlusd) = (
        net.hub(HubAsset::Xrp).clone(),
        net.hub(HubAsset::Dollar).clone(),
    );
    assert_eq!(
        (xrp.ledger.as_str(), rlusd.ledger.as_str()),
        ("hub-xrpl-xrp+xrp", "hub-xrpl-rlusd+usd")
    );
    // A validator of the live realm is offered mainnet's books, and beside
    // them at most a mainnet hub's scheme: none of the testnet's.
    let Ok(response::Method::ListSettlementSchemes(list)) = net.validators[0]
        .ipc()
        .await
        .call(Method::ListSettlementSchemes(
            proto::ListSettlementSchemesRequest {},
        ))
        .await
    else {
        panic!("no schemes");
    };
    let offered: Vec<&str> = list.schemes.iter().map(|s| s.id.as_str()).collect();
    for book in ["xrpl-xrp", "xrpl-rlusd"] {
        assert!(offered.contains(&book), "{offered:?}");
    }
    for id in &offered {
        assert!(
            ["xrpl-xrp", "xrpl-rlusd", "hub-xrpl-xrp", "hub-xrpl-rlusd"].contains(id),
            "{id} offered in the live realm"
        );
    }

    let members = net
        .members(&[Joining::of(88, BOTH), Joining::of(89, BOTH)], &[])
        .await;
    let (first, second) = (&members[0], &members[1]);
    net.peered(&[first, second]).await;
    for member in [first, second] {
        for id in [&xrp.ledger, &rlusd.ledger] {
            let view = ledger(&member.node, id).await.unwrap();
            assert_eq!(view.realm(), proto::Realm::Live, "{id}");
        }
    }
    let both = {
        let mut both = vec![xrp.ledger.clone(), rlusd.ledger.clone()];
        both.sort();
        both
    };
    assert_eq!(peer_ledgers(&first.node, &second.key).await, both);
    assert_eq!(peer_ledgers(&second.node, &first.key).await, both);

    let before = net.settled(&[first, second]).await;
    for (from, to, id, amount) in [
        (first, second, &xrp.ledger, XRP_PAYMENT),
        (second, first, &rlusd.ledger, RLUSD_PAYMENT),
    ] {
        let incoming = pay(&net.clock, (&from.node, &to.node), id, amount, id).await;
        assert!(
            incoming.destination_account.starts_with("g."),
            "{}",
            incoming.destination_account
        );
        assert_eq!(&incoming.ledger, id);
        assert_eq!(incoming.received_amount, amount, "what arrived on {id}");
    }
    let after = net.settled(&[first, second]).await;
    // What each owner has of an asset: on its hub and in the owner book of
    // it taken together, as the node moves funds between them by itself.
    let moved = |index: usize, asset: HubAsset, hub: &str| {
        let owner = asset.owner_ledger(&MAINNET);
        [hub, owner.as_str()]
            .iter()
            .map(|id| after[index][*id].1 - before[index][*id].1)
            .sum::<i64>()
    };
    let (xrp_paid, xrp_got) = (
        -moved(0, HubAsset::Xrp, &xrp.ledger),
        moved(1, HubAsset::Xrp, &xrp.ledger),
    );
    let (usd_paid, usd_got) = (
        -moved(1, HubAsset::Dollar, &rlusd.ledger),
        moved(0, HubAsset::Dollar, &rlusd.ledger),
    );
    let (xrp_payment, usd_payment) = (
        i64::try_from(XRP_PAYMENT).unwrap(),
        i64::try_from(RLUSD_PAYMENT).unwrap(),
    );
    assert!(xrp_paid >= xrp_payment, "paid {xrp_paid} XRP");
    assert!(xrp_got > 0 && xrp_got <= xrp_payment, "got {xrp_got} XRP");
    assert!(usd_paid >= usd_payment, "paid {usd_paid} RLUSD");
    assert!(usd_got > 0 && usd_got <= usd_payment, "got {usd_got} RLUSD");
    net.stop(members).await;
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs,
// spec/areas/hubs.md#in-the-node: an owner book that waited for nothing a
// while anchors on demand, its anchor stale. The owner's first deposit
// into the hub out of it goes through on the first ask all the same: the
// node waits for the book's account to read its ledger anew, a round,
// before it signs.
#[tokio::test(start_paused = true)]
async fn an_idle_books_first_deposit_into_its_hub_goes_through() {
    let net = HubNet::start(&TESTNET, XRP).await;
    let hub = net.hub(HubAsset::Xrp).clone();
    let members = net.members(&[Joining::of(90, XRP)], &[]).await;
    let member = &members[0];
    support::hubs::until("no move on its way", async || {
        hub_status(&member.node, hub.scheme).await.moving.is_none()
    })
    .await;
    // Longer than a move keeps the book's account reading every ledger,
    // and than its anchor stays fresh after.
    support::hubs::pass(200_000).await;
    let mut owner = member.node.ipc().await;
    let deposited = owner
        .call(Method::HubDeposit(proto::HubDepositRequest {
            amount: 2_000_000 * dassie_xrpl::UNITS_PER_DROP,
            scheme: hub.scheme.into(),
        }))
        .await;
    let Ok(response::Method::HubDeposit(deposited)) = deposited else {
        panic!("refused: {deposited:?}");
    };
    let moving = deposited.status.unwrap().moving.unwrap();
    assert!(moving.by_owner);
    net.stop(members).await;
}
