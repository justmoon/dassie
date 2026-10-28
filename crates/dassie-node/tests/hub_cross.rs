//! Payments that cross assets over settlement hubs, between daemons on
//! loopback over real QUIC, on a clock the test moves by hand
//! (spec/areas/hubs.md#in-the-node, spec/areas/connector.md#exchange-rates):
//! four validators run an XRP hub and a DSUSD hub on the fake XRP Ledger,
//! whose ledgers close on that clock too. A payment crosses from the XRP
//! hub to the DSUSD hub through a member of both, which converts; and one
//! from dollars on the stub with unlimited trust reaches XRP over the hub
//! through a node with both.

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
use support::TestNode;
use support::hubs::{
    Books, HubAsset, HubNet, Joining, STUB_LEDGER, accounts, incoming, ledger, list_ledgers, pay,
    peer_accounts, peer_ledgers, peers, until,
};
use support::rates::{RatesServer, coinbase};

const XRP: &[HubAsset] = &[HubAsset::Xrp];
const DSUSD: &[HubAsset] = &[HubAsset::Dollar];
const BOTH: &[HubAsset] = &[HubAsset::Xrp, HubAsset::Dollar];

/// The rate every node reads: two XRP to the dollar.
const XRP_PER_USD: &str = "2";

/// Has `node` read its rates from `rates`, and waits until it has a fresh
/// one for XRP.
async fn rated(node: &TestNode, rates: &RatesServer) {
    let set = node
        .ipc()
        .await
        .call(Method::SetRateSources(proto::SetRateSourcesRequest {
            sources: vec![proto::RateSource {
                kind: "coinbase".into(),
                url: rates.url(),
            }],
        }))
        .await;
    assert!(set.is_ok(), "{set:?}");
    until("a fresh rate for XRP", async || {
        list_ledgers(node)
            .await
            .rates
            .iter()
            .any(|rate| rate.asset_code == "XRP" && !rate.stale)
    })
    .await;
}

/// What the owner of a member moved on `ids`, after less before.
fn moved(before: &Books, after: &Books, ids: &[&str]) -> i64 {
    ids.iter()
        .map(|id| after.get(*id).map_or(0, |b| b.1) - before.get(*id).map_or(0, |b| b.1))
        .sum()
}

/// 1 XRP: ten of the largest packets, so the payer funds its holds toward
/// the members of both hubs as it pays.
const CROSSING: u64 = 1_000_000_000;

// spec/areas/hubs.md#in-the-node, spec/decisions/passive-rebalancing.md:
// one member is in the XRP hub alone, one in the DSUSD hub alone, and two
// in both. The two in both peer over both hubs, with a peer account on
// each hub's ledger; the others over the one hub they share with each; the
// two in one hub alone share none and do not peer. A client of the member
// in the XRP hub alone pays a receiver in DSUSD of the member in the DSUSD
// hub alone through a member of both, which converts: what arrived is what
// was sent at the rate, moved by the member's inventory curve by at most
// its largest step and less the connectors' fees and spread. Once settling
// is over, each node's books hold what its accounts hold; the payer's
// owner paid in XRP at least what was sent, the payee's got dollars, at
// most what arrived, and the owners of the members of both took XRP and
// paid dollars.
#[tokio::test(start_paused = true)]
async fn a_payment_crosses_from_one_hub_to_another_through_a_member_of_both() {
    let net = HubNet::start(&TESTNET, BOTH).await;
    let (xrp, dsusd) = (
        net.hub(HubAsset::Xrp).clone(),
        net.hub(HubAsset::Dollar).clone(),
    );
    let rates = RatesServer::start(coinbase(&[("XRP", XRP_PER_USD)])).await;
    let members = net
        .members(
            &[
                Joining::of(90, XRP),
                Joining::of(91, BOTH),
                Joining::of(92, BOTH),
                Joining::of(93, DSUSD),
            ],
            &[],
        )
        .await;
    for member in &members[1..3] {
        rated(&member.node, &rates).await;
    }
    let all: Vec<&support::hubs::Member> = members.iter().collect();
    net.peered(&all).await;
    let (payer, first, second, payee) = (&members[0], &members[1], &members[2], &members[3]);
    let mut both = vec![xrp.ledger.clone(), dsusd.ledger.clone()];
    both.sort();
    assert_eq!(peer_ledgers(&first.node, &second.key).await, both);
    assert_eq!(peer_ledgers(&second.node, &first.key).await, both);
    for member in [first, second] {
        assert_eq!(
            peer_ledgers(&payer.node, &member.key).await,
            std::slice::from_ref(&xrp.ledger)
        );
        assert_eq!(
            peer_ledgers(&payee.node, &member.key).await,
            std::slice::from_ref(&dsusd.ledger)
        );
    }
    assert!(
        peer_ledgers(&payer.node, &payee.key).await.is_empty(),
        "no hub in common"
    );

    let before = net.settled(&all).await;
    let arrived = pay(
        &net.clock,
        (&payer.node, &payee.node),
        &xrp.ledger,
        CROSSING,
        &dsusd.ledger,
    )
    .await;
    assert_eq!(arrived.ledger, dsusd.ledger);
    // XRP at the rate, in dollars: a member of both converts it at a rate
    // its inventory moves from spot, either way, by at most the curve's
    // largest step, 10%, and the connectors keep their fees and spread.
    let dollars = CROSSING / 2;
    let received = arrived.received_amount;
    assert!(
        received > dollars * 85 / 100 && received < dollars * 110 / 100,
        "{received} arrived of {dollars} sent"
    );
    let after = net.settled(&all).await;
    let xrp_books = [xrp.ledger.as_str(), "xrpl-testnet-xrp+xrp"];
    let dollar_books = [dsusd.ledger.as_str(), "xrpl-testnet-dsusd+usd"];
    let spent = -moved(&before[0], &after[0], &xrp_books);
    assert!(
        spent >= i64::try_from(CROSSING).unwrap(),
        "the payer's owner spent {spent}"
    );
    let got = moved(&before[3], &after[3], &dollar_books);
    assert!(
        got > 0 && got <= i64::try_from(received).unwrap(),
        "the payee's owner got {got}"
    );
    let took_xrp: i64 = (1..3)
        .map(|i| moved(&before[i], &after[i], &xrp_books))
        .sum();
    let paid_dollars: i64 = (1..3)
        .map(|i| -moved(&before[i], &after[i], &dollar_books))
        .sum();
    assert!(took_xrp > 0, "the members of both took {took_xrp} in XRP");
    assert!(paid_dollars > 0, "the members of both paid {paid_dollars}");
    net.stop(members).await;
}

/// 2 XRP: twenty of the largest packets, so the converting node funds its
/// hold toward the payee over the hub as it pays.
const RECEIVE: u64 = 2_000_000_000;
/// How far from what was asked the payee's incoming payment may land and
/// be paid in full: a ten-thousandth of an XRP
/// (spec/decisions/paid-within-a-tolerance.md).
const TOLERANCE: u64 = 100_000;
/// The largest packet, in internal units: 0.1 XRP.
const MAX_PACKET: u64 = 100_000_000;
/// The converting node's fee, 1%, and spread, 0.5%; its curve off, so it
/// converts at spot less them.
const FEE_PPM: u32 = 10_000;
const SPREAD_PPM: u32 = 5_000;

/// The balance of `node`'s account at `path`, credits less debits.
async fn balance(node: &TestNode, path: &str) -> i128 {
    accounts(node)
        .await
        .iter()
        .find(|account| account.path == path)
        .map_or(0, |a| {
            i128::from(a.credits_posted) - i128::from(a.debits_posted)
        })
}

/// What `node` holds toward `peer` over the hub of `scheme`: the credit it
/// extends the peer there.
async fn hold_toward(node: &TestNode, peer: &dassie_identity::IdentityKey, scheme: &str) -> u64 {
    let id = peer.node_id().to_string();
    peers(node)
        .await
        .into_iter()
        .find(|p| p.node == id)
        .and_then(|p| p.methods.into_iter().find(|m| m.scheme == scheme))
        .unwrap()
        .credit_out
}

// spec/areas/connector.md#exchange-rates, spec/areas/hubs.md#in-the-node:
// d1 has dollars on the stub with unlimited trust, which never settles; d3
// has XRP in the hub; d2 has both, peering with d1 on the stub and with d3
// over the hub alone, and charges a fee and a spread, its rebalancing
// curve off. d1 quotes an incoming payment of d3's on the hub's ledger:
// the quote measures the path's rate, spot less d2's fee and spread and
// STREAM's slippage, from d1's dollars. d1 pays it; d2 converts and keeps
// its fee in dollars; d3 receives XRP over the hub, all of it within the
// tolerance. d2 settles with d3 over the hub, funding its hold toward d3
// as d3 asks: both sides' books of each other move alike, and the hold
// loses what d2 funded less what d3 redeemed, which is what arrived in
// whole drops but for the claims below d3's redeem threshold. d3's owner
// gets what arrived, less the fees of d3's own moves; the stub settles nothing, and d1 owes d2 what it
// sent. All three restarted, their books are as they were.
#[tokio::test(start_paused = true)]
async fn dollars_on_the_stub_arrive_as_xrp_over_a_hub() {
    let net = HubNet::start(&TESTNET, XRP).await;
    let hub = net.hub(HubAsset::Xrp).clone();
    let rates = RatesServer::start(coinbase(&[("XRP", XRP_PER_USD)])).await;
    let mut nodes = net
        .members(
            &[
                Joining::of(94, &[]).with_stub(),
                Joining::of(95, XRP).with_stub(),
                Joining::of(96, XRP),
            ],
            &[],
        )
        .await;
    rated(&nodes[1].node, &rates).await;
    net.peered(&[&nodes[1], &nodes[2]]).await;
    until("d1 and d2 peered on the stub", async || {
        peer_ledgers(&nodes[0].node, &nodes[1].key).await == [STUB_LEDGER]
            && peer_ledgers(&nodes[1].node, &nodes[0].key).await == [STUB_LEDGER]
    })
    .await;
    let (d1, d2, d3) = (&nodes[0], &nodes[1], &nodes[2]);
    // d1 and d3 share no scheme, so they never peer; d3 peers over the hub
    // alone.
    assert!(peer_ledgers(&d3.node, &d1.key).await.is_empty());
    for peer in peers(&d3.node).await {
        for method in &peer.methods {
            assert_eq!(method.scheme, hub.scheme);
        }
    }
    // SetPeerSettings sets every setting: the swap settings stay as they
    // are.
    let Ok(response::Method::GetPeerSettings(kept)) = d2
        .node
        .ipc()
        .await
        .call(Method::GetPeerSettings(proto::GetPeerSettingsRequest {}))
        .await
    else {
        panic!("no peer settings");
    };
    let kept = kept.settings.unwrap();
    let set = d2
        .node
        .ipc()
        .await
        .call(Method::SetPeerSettings(proto::SetPeerSettingsRequest {
            settings: Some(proto::PeerSettings {
                connector_fee_ppm: FEE_PPM,
                fx_spread_ppm: SPREAD_PPM,
                rebalance_ppm: 0,
                ..kept
            }),
        }))
        .await;
    assert!(set.is_ok(), "{set:?}");

    // d1 quotes 2 XRP to d3's incoming payment on the hub's ledger.
    let created = d3
        .node
        .ipc()
        .await
        .call(Method::CreateIncomingPayment(
            proto::CreateIncomingPaymentRequest {
                ledger: hub.ledger.clone(),
                incoming_amount: Some(RECEIVE),
                description: "across".into(),
                ..proto::CreateIncomingPaymentRequest::default()
            },
        ))
        .await;
    let Ok(response::Method::CreateIncomingPayment(created)) = created else {
        panic!("{created:?}");
    };
    let receiver = created.payment.unwrap();
    assert_eq!(receiver.ledger, hub.ledger);
    let before = net.settled(&[d1, d2, d3]).await;
    let mut quoter = d1.node.ipc().await;
    let request = Method::QuoteSend(proto::QuoteSendRequest {
        recipient: String::new(),
        credentials: Some(proto::StreamCredentials {
            destination_account: receiver.destination_account.clone(),
            shared_secret: receiver.shared_secret.clone(),
            asset_code: receiver.asset_code.clone(),
            asset_scale: receiver.asset_scale,
        }),
        ledger: String::new(),
        amount: Some(proto::quote_send_request::Amount::ReceiveAmount(RECEIVE)),
        note: String::new(),
    });
    // The probe's timers run on the nodes' clock, which moves meanwhile.
    let quoting = tokio::spawn(async move { quoter.call(request).await });
    until("d1 quotes", async || quoting.is_finished()).await;
    let Ok(response::Method::QuoteSend(quoted)) = quoting.await.unwrap() else {
        panic!("no quote");
    };
    let quote = quoted.quote.unwrap();
    assert_eq!(quote.ledger, STUB_LEDGER);
    let receive = quote.receive_amount.clone().unwrap();
    assert_eq!(
        (receive.asset_code.as_str(), receive.value),
        ("XRP", RECEIVE)
    );
    // 2 XRP are a dollar at spot; less d2's 1% and 0.5%, and STREAM's 1%:
    // a little over 1.025 dollars.
    assert!(
        (1_020_000_000..1_030_000_000).contains(&quote.debit_amount),
        "{}",
        quote.debit_amount
    );
    let usd_before = ledger(&d1.node, STUB_LEDGER).await.unwrap().owner_balance;
    let fees_path = format!("{STUB_LEDGER}:revenue/fees");
    let fees_before = balance(&d2.node, &fees_path).await;
    let hold_before = hold_toward(&d2.node, &d3.key, hub.scheme).await;
    let hub_before = peer_accounts(&[d2, d3], &hub.ledger).await;
    let Ok(response::Method::SendPayment(sent)) = d1
        .node
        .ipc()
        .await
        .call(Method::SendPayment(proto::SendPaymentRequest {
            quote_id: quote.id.clone(),
        }))
        .await
    else {
        panic!("not sent");
    };
    let id = sent.payment.unwrap().id;
    let outgoing = async || {
        let Ok(response::Method::ListOutgoingPayments(list)) = d1
            .node
            .ipc()
            .await
            .call(Method::ListOutgoingPayments(
                proto::ListOutgoingPaymentsRequest { limit: 0 },
            ))
            .await
        else {
            panic!("no payments");
        };
        list.payments.into_iter().find(|p| p.id == id).unwrap()
    };
    until("the payment ends", async || {
        outgoing().await.state != "sending"
    })
    .await;
    let sent = outgoing().await;
    assert_eq!(sent.state, "completed", "{}", sent.failure);
    until("d3 has it all", async || {
        incoming(&d3.node, &receiver.id)
            .await
            .completed_at_unix_ms
            .is_some()
    })
    .await;
    let arrived = incoming(&d3.node, &receiver.id).await;
    assert!(
        arrived.received_amount.abs_diff(RECEIVE) <= TOLERANCE,
        "{}",
        arrived.received_amount
    );
    assert_eq!(arrived.asset_code, "XRP");
    assert_eq!(arrived.ledger, hub.ledger);
    // d1 paid in dollars; d2 kept its fee in dollars: 1% of what passed,
    // less under a unit a packet, as each rounds down.
    let paid = i64::try_from(sent.sent_amount).unwrap();
    assert_eq!(
        ledger(&d1.node, STUB_LEDGER).await.unwrap().owner_balance,
        usd_before - paid
    );
    let fees = balance(&d2.node, &fees_path).await - fees_before;
    let due = i128::from(sent.sent_amount) * i128::from(FEE_PPM) / 1_000_000;
    assert!(fees <= due && fees > due - 1_000, "fees {fees} on {paid}");

    // d2 settles with d3 over the hub; the stub settles nothing.
    let after = net.claimed(&[d1, d2, d3]).await;
    // Over the hub: d2 funded its hold toward d3 as d3 asked, and d3 was
    // paid from the hold for what arrived. Each side's books of the other
    // moved by the packets less d2's fundings, and the two agree, since d2
    // counts what it funded as d3 does, from the hub
    // (spec/areas/settlement.md#settlement-hubs). The hold lost what d2
    // funded less what d3 redeemed or claimed: what arrived, in whole
    // drops, since a claim's total is the packets' rounded down to drops
    // and these are the first packets on it, but for the claims d3 has not
    // redeemed, which it lets add up to less than its threshold before it
    // pays the hub's fee to redeem them
    // (spec/designs/settlement-hubs.md#4-peering-over-a-hub). How far the
    // hold moves in all depends on how much of it d2 funded before the
    // payment (spec/decisions/settle-when-funds-arrive.md).
    let hub_after = peer_accounts(&[d2, d3], &hub.ledger).await;
    let change = |pair: (u8, u8)| {
        hub_after.get(&pair).copied().unwrap_or(0) - hub_before.get(&pair).copied().unwrap_or(0)
    };
    assert_eq!(change((95, 96)), -change((96, 95)), "the books agree");
    let received = i128::from(arrived.received_amount);
    let funded = change((96, 95)) + received;
    assert!(funded > 0, "d2 funded {funded}");
    let claimed = received - received % i128::from(dassie_xrpl::UNITS_PER_DROP);
    let hold_moved =
        i128::from(hold_toward(&d2.node, &d3.key, hub.scheme).await) - i128::from(hold_before);
    let redeemed = funded - hold_moved;
    let threshold = i128::from(dassie_settlement::policy::redeem_threshold(MAX_PACKET));
    assert!(
        redeemed <= claimed && claimed - redeemed < threshold,
        "d3 redeemed {redeemed} of {claimed} claimed, d2 funding {funded}"
    );
    let owner = HubAsset::Xrp.owner_ledger(&TESTNET);
    let got = moved(&before[2], &after[2], &[&hub.ledger, &owner]);
    assert!(
        got > 0 && got <= i64::try_from(arrived.received_amount).unwrap(),
        "d3's owner got {got}"
    );
    let start = dassie_settlement::STUB_INITIAL_BALANCE;
    for books in &after[..2] {
        assert_eq!(books[STUB_LEDGER].0, start, "the stub settled");
    }
    let stub_owed = peer_accounts(&[d1, d2], STUB_LEDGER).await;
    let d1_owes = stub_owed[&(94, 95)];
    assert_eq!(stub_owed[&(95, 94)], -d1_owes, "the stub's books");
    assert_eq!(
        d1_owes,
        i128::from(sent.sent_amount),
        "d1 owes d2 what it sent"
    );
    let hub_owed = peer_accounts(&[d2, d3], &hub.ledger).await;

    // Restarted, all three catch up and book nothing twice.
    let restarted = net.restart_all(nodes.drain(..)).await;
    let all: Vec<&support::hubs::Member> = restarted.iter().collect();
    assert_eq!(net.settled(&all).await, after, "the books after a restart");
    assert_eq!(peer_accounts(&all[..2], STUB_LEDGER).await, stub_owed);
    assert_eq!(peer_accounts(&all[1..], &hub.ledger).await, hub_owed);
    net.stop(restarted).await;
}
