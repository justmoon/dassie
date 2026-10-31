//! How a node shares what it can hold toward its peers over a hub among
//! them (spec/decisions/hold-allotment.md, spec/areas/settlement.md#credit):
//! while its hub account's funds cannot give every peer the default credit,
//! a request raises the hold toward a peer no higher than the peer's
//! allotment; while they can, a request funds what the rule says, as
//! before; and requests funded late go to core peers before downlinks. On
//! one engine core in the XRP hub, with a core peer and a downlink peered
//! over it, and on one over the stub with commitments.

use std::sync::Arc;

use dassie_hub::member::SCHEME_ID;
use dassie_identity::NodeId;
use dassie_settlement::policy;
use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::fake::FakeLedger;

use super::super::{Core, Event, Output, SettlementEvent};
use super::{
    NOW, Policies, SHARED_CONFIG, Validators, add_xrpl, core_with_bootstrap, deposits,
    downlink_request_on, follow_xrpl, funded_ledger, hub_books, hub_config, peer_as_downlink,
    scheme_id, send_link_state, step_with_list, wait_and_close,
};
use crate::store::Store;

/// The largest packet, in internal units.
const P: u64 = Policies::DEFAULT.connector.max_packet_amount;

/// The core peer's account in the hub, and the downlink's.
const CORE_ACCOUNT: u32 = 9;
const DOWNLINK_ACCOUNT: u32 = 10;

/// What a request has a payer over a hub with nothing held fund, as the
/// rule says: half the way from the default credit down to the trigger.
fn one_request() -> u64 {
    policy::requested_amount(
        u128::from(Policies::DEFAULT.peer_credit),
        policy::trigger(P),
        policy::minimum(P),
    )
}

/// Spends all the stub holds, behind the books, as a hub account whose
/// other holds took it.
fn drain(core: &mut Core) {
    use dassie_kit::sim::SeededRng;

    let stub = dassie_ledger::name!("stub");
    let elsewhere = dassie_ledger::Name::new("delsewhere").unwrap();
    let scheme = &mut core.schemes.get_mut(&stub).unwrap().scheme;
    let take = scheme.balance();
    let prepared = scheme
        .prepare_settlement(&mut SeededRng::new(9), None, elsewhere, take, &[])
        .unwrap();
    scheme.execute_settlement(None, prepared.id, &mut Vec::new());
    assert_eq!(scheme.balance(), 0);
}

/// The membership a peer over the hub of `hub` gives at peering: the hub,
/// its account `number` and its key there, of `seed`.
pub(super) fn membership(hub: &Validators, number: u32, seed: u8) -> Vec<u8> {
    let key = dassie_hub::member::member_key(
        &dassie_identity::TableKeygen::identity_key(&[seed; 32]),
        &hub.rules.id(),
    )
    .public_key();
    let mut membership = hub.rules.id().0.as_bytes().to_vec();
    membership.extend_from_slice(&number.to_be_bytes());
    membership.extend_from_slice(key.as_bytes());
    membership
}

/// `node` peers with `core` over the hub of `hub` as a downlink, at `at`,
/// as account `number`, its key there of `seed`.
pub(super) fn peer_over_hub(
    core: &mut Core,
    hub: &Validators,
    node: &Arc<dassie_identity::IdentityKey>,
    (number, seed): (u32, u8),
    at: u64,
) {
    core.begin_step(at);
    let body = downlink_request_on(node, at, SCHEME_ID, &membership(hub, number, seed));
    core.peer_inbound(dassie_peer::InboundId(at), node.identity(), &body, None);
    core.finish_step(&mut Vec::new());
    assert!(core.peer.is_downlink(&node.node_id()));
}

/// `peer`, a downlink with a URL, turns core from `at` on, once the core
/// the node computes names it: the dev core list does here
/// (spec/decisions/promotion-to-core.md). Returns when it did.
fn turn_core(core: &mut Core, peer: NodeId, at: u64) -> u64 {
    let own = *core.peer.own();
    core.peer.set_dev_core(vec![own, peer]);
    let mut at = at;
    while core.peer.is_downlink(&peer) {
        at += 1_000;
        assert!(at < NOW + 600_000, "it turns core");
        step_with_list(core, at, &[peer]);
    }
    at
}

/// A core node in the XRP hub whose account, number 5, holds `balance`
/// drops as last agreed, all of it the owner's deposits, with a core peer
/// peered over the hub as account [`CORE_ACCOUNT`] and a downlink as account
/// [`DOWNLINK_ACCOUNT`]: the core, the hub, the core peer, the downlink and
/// what deposits credited the account. The core peer is a downlink first,
/// which turns core once its bootstrap node lists it.
fn hub_with_peers(
    ledger: &mut FakeLedger,
    balance: u64,
) -> (Core, Validators, NodeId, NodeId, u64) {
    let mut core = core_with_bootstrap(Arc::new(Store::open_in_memory().unwrap()));
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(SCHEME_ID, hub_config(600, 300)).unwrap();
    follow_xrpl(&mut core, ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    wait_and_close(&mut core, ledger, 1_000);
    let deposited = deposits(ledger, hub.escrow())[0].amount + balance;
    hub.account(&mut core, balance, deposited);

    let downlink = Arc::new(dassie_identity::TableKeygen::identity_key(&[11; 32]));
    let peer = Arc::new(dassie_identity::TableKeygen::identity_key(&[14; 32]));
    let at = core.now_ms() + 1_000;
    peer_over_hub(&mut core, &hub, &downlink, (DOWNLINK_ACCOUNT, 12), at);
    peer_over_hub(&mut core, &hub, &peer, (CORE_ACCOUNT, 13), at + 1);
    send_link_state(&mut core, &peer, at + 2, Some("https://c.example:4443"));
    let (downlink, peer) = (downlink.node_id(), peer.node_id());
    turn_core(&mut core, peer, at + 3);
    hub.account(&mut core, balance, deposited);
    assert!(core.peer.peering_of(&downlink).is_some(), "the downlink");
    assert!(core.peer.peering_of(&peer).is_some(), "the core peer");
    assert!(core.peer.is_downlink(&downlink));
    (core, hub, peer, downlink, deposited)
}

/// What a request of `node`'s over the hub may fund at most while the
/// hub's funds are short: what the account can spend beyond what the
/// other peers' reserves need.
fn room(core: &Core, node: NodeId) -> u64 {
    let scheme = scheme_id(SCHEME_ID);
    let allotments = core.hub_allotments(scheme).unwrap();
    let (held, _, _) = core
        .hub_books(dassie_ledger::name!(SCHEME_ID), node)
        .unwrap_or_default();
    let spendable = core.spendable_for(node, scheme);
    allotments.room(&node, held, spendable).unwrap()
}

/// What a peer's own share pays for its hold's opening: the hub's fee and
/// the reserve a new hold locks.
fn opening() -> u64 {
    let fees = Validators::new().rules.terms.fees;
    (fees.operation + fees.hold_reserve) * UNITS_PER_DROP
}

/// Whether `outputs` say a request from `peer` was capped by the
/// allotments.
fn allotted(outputs: &[Output], peer: NodeId) -> bool {
    outputs.iter().any(|output| {
        matches!(output,
            Output::Event(Event::Settlement(SettlementEvent::RequestAllotted { node, .. }))
                if *node == peer)
    })
}

/// The allotment the owner sees of the hold toward `peer`, once there is
/// one.
fn allotment_shown(core: &Core, peer: NodeId) -> Option<u64> {
    core.hub_status()?
        .paying
        .iter()
        .find(|hold| hold.peer == Some(peer))?
        .allotment
}

// With less in the hub than every peer's default credit, the core peer is
// allotted the least hold before the downlink, and its request funds the
// hold only with what the account can spend beyond the downlink's reserve,
// a largest packet: less than the rule says. The request is kept.
#[test]
fn a_scarce_pool_keeps_the_others_reserves() {
    let (mut ledger, _) = funded_ledger();
    // Seven packets: the least hold for the core peer, the rest for the
    // downlink.
    let pool = 7 * P;
    let (mut core, _, peer, downlink, _) = hub_with_peers(&mut ledger, pool / UNITS_PER_DROP);
    let scheme = scheme_id(SCHEME_ID);
    let least = policy::least_hold(P, Policies::DEFAULT.connector.max_in_flight());
    let allotments = core.hub_allotments(scheme).unwrap();
    assert!(!allotments.plentiful);
    assert_eq!(allotments.peers, [(peer, least), (downlink, pool - least)]);
    assert_eq!(allotments.reserved, [P, P]);
    let spendable = core.spendable_for(peer, scheme).unwrap();
    let room = room(&core, peer);
    assert_eq!(room, spendable - P);
    assert!(room < one_request(), "the downlink's reserve caps it");

    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    assert_eq!(core.settlement_requested(peer, scheme), room);
    assert!(core.settle.unanswered.contains_key(&(peer, scheme)), "kept");
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(allotted(&outputs, peer), "said so");
    for later in 1..5 {
        let at = core.now_ms() + later * 1_000;
        core.begin_step(at);
        core.finish_step(&mut outputs);
    }
    // Funded once: the downlink's packet stays spendable, of which its own
    // hold's opening is paid.
    let (credit, _, _) = core
        .hub_books(dassie_ledger::name!(SCHEME_ID), peer)
        .unwrap();
    assert_eq!(credit, room);
    let left = core.spendable_for(downlink, scheme).unwrap();
    assert!(left + opening() >= P, "the downlink's packet is left");
    core.check_invariants();
}

// spec/decisions/hold-allotment.md: an idle peer's share strands nothing.
// Twelve packets give the core peer seven and a half and the downlink its
// least hold; the core peer idle, the busy downlink's request is funded as
// the rule says, past its allotment, from what the core peer leaves unused
// above its packet.
#[test]
fn a_busy_peer_takes_past_its_allotment_what_idle_shares_leave() {
    let (mut ledger, _) = funded_ledger();
    let pool = 12 * P;
    let (mut core, _, peer, downlink, _) = hub_with_peers(&mut ledger, pool / UNITS_PER_DROP);
    let scheme = scheme_id(SCHEME_ID);
    let least = policy::least_hold(P, Policies::DEFAULT.connector.max_in_flight());
    let allotments = core.hub_allotments(scheme).unwrap();
    assert_eq!(allotments.peers, [(peer, pool - least), (downlink, least)]);
    assert!(one_request() > least, "the rule asks past the allotment");
    assert!(room(&core, downlink) >= one_request());

    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    assert_eq!(core.settlement_requested(downlink, scheme), one_request());
    assert!(!core.settle.unanswered.contains_key(&(downlink, scheme)));
    core.finish_step(&mut Vec::new());
    let (credit, _, _) = core
        .hub_books(dassie_ledger::name!(SCHEME_ID), downlink)
        .unwrap();
    assert_eq!(credit, one_request());
    let left = core.spendable_for(peer, scheme).unwrap();
    assert!(left + opening() >= P, "the core peer's packet is left");
    core.check_invariants();
}

// With twenty packets the pool falls short of both peers' default credit,
// yet the core peer, idle, leaves enough unused: the busy downlink's hold
// may rise to the default credit, past its allotment, while the core
// peer's packet stays set aside.
#[test]
fn a_busy_peer_may_rise_to_the_default_credit() {
    let (mut ledger, _) = funded_ledger();
    let pool = 20 * P;
    let (core, _, peer, downlink, _) = hub_with_peers(&mut ledger, pool / UNITS_PER_DROP);
    let allotments = core.hub_allotments(scheme_id(SCHEME_ID)).unwrap();
    assert!(!allotments.plentiful);
    let full = Policies::DEFAULT.peer_credit;
    assert!(allotments.of(&downlink).unwrap() < full);
    assert_eq!(room(&core, downlink), full);
    let spendable = core.spendable_for(downlink, scheme_id(SCHEME_ID)).unwrap();
    let reserved = allotments.reserved[allotments.rank(&peer)];
    assert!(spendable - full >= reserved);
}

// While the core peer's packet takes what the account can spend, the
// downlink's request funds nothing, books nothing, says the allotments
// capped it, and is kept until there is room; the owner sees the core
// peer's allotment beside the node's hold toward it.
#[test]
fn a_request_the_others_reserves_leave_nothing_waits() {
    let (mut ledger, _) = funded_ledger();
    let least = policy::least_hold(P, Policies::DEFAULT.connector.max_in_flight());
    let balance = 3 * P / UNITS_PER_DROP;
    let (mut core, mut hub, peer, downlink, deposited) = hub_with_peers(&mut ledger, balance);
    hub.opened_hold(&mut core, P / UNITS_PER_DROP / 10, balance, deposited);
    let scheme = scheme_id(SCHEME_ID);
    let share = core.hub_allotments(scheme).unwrap().of(&peer);
    assert!(share.is_some_and(|share| share < least));
    assert_eq!(allotment_shown(&core, peer), share);
    assert!(room(&core, downlink) < policy::minimum(P));

    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    let before = core.ledger.next_transfer_id();
    assert_eq!(core.settlement_requested(downlink, scheme), 0);
    assert_eq!(core.ledger.next_transfer_id(), before, "booked nothing");
    assert!(
        core.settle.unanswered.contains_key(&(downlink, scheme)),
        "kept"
    );
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(allotted(&outputs, downlink), "said so");
    core.check_invariants();
}

// With enough in the hub for every peer's default credit, nothing is
// capped: a request funds what the rule says, as without allotments, and
// the owner sees each allotment at the default credit.
#[test]
fn a_plentiful_pool_funds_what_the_rule_says() {
    let (mut ledger, _) = funded_ledger();
    // Every peer's default credit, and what opening its hold costs.
    let pool = 3 * Policies::DEFAULT.peer_credit;
    let (mut core, mut hub, peer, downlink, deposited) =
        hub_with_peers(&mut ledger, pool / UNITS_PER_DROP);
    hub.opened_hold(
        &mut core,
        P / UNITS_PER_DROP,
        pool / UNITS_PER_DROP,
        deposited,
    );
    let scheme = scheme_id(SCHEME_ID);
    let allotments = core.hub_allotments(scheme).unwrap();
    assert!(allotments.plentiful);
    assert_eq!(allotments.room(&peer, 0, Some(0)), None, "nothing capped");
    let full = Policies::DEFAULT.peer_credit;
    assert_eq!(allotment_shown(&core, peer), Some(full));

    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    assert_eq!(core.settlement_requested(downlink, scheme), one_request());
    assert!(!core.settle.unanswered.contains_key(&(downlink, scheme)));
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let (credit, _, _) = core
        .hub_books(dassie_ledger::name!(SCHEME_ID), downlink)
        .unwrap();
    assert_eq!(credit, one_request());
    core.check_invariants();
}

// Requests funded late go in the order of priority, core peers first,
// whatever their node IDs: with funds for one request and a half, the core
// peer's is funded in full, and the downlink's with what is left, though
// its node ID comes first.
#[test]
fn late_requests_go_to_core_peers_first() {
    let mut core = core_with_bootstrap(Arc::new(Store::open_in_memory().unwrap()));
    let stub = scheme_id("stub");
    let downlink = Arc::new(dassie_identity::TableKeygen::identity_key(&[11; 32]));
    let peer = Arc::new(dassie_identity::TableKeygen::identity_key(&[14; 32]));
    let at = NOW + 1_000;
    peer_as_downlink(&mut core, &downlink, at);
    peer_as_downlink(&mut core, &peer, at + 1);
    send_link_state(&mut core, &peer, at + 2, Some("https://c.example:4443"));
    let (downlink, peer) = (downlink.node_id(), peer.node_id());
    let at = turn_core(&mut core, peer, at + 3);
    assert!(downlink < peer, "the downlink first by node ID");
    drain(&mut core);
    core.begin_step(at + 1_000);
    assert_eq!(core.settlement_requested(downlink, stub), 0);
    assert_eq!(core.settlement_requested(peer, stub), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert_eq!(core.settle.unanswered.len(), 2);

    let one = one_request();
    core.begin_step(at + 2_000);
    core.stub_deposit(one + one / 2).unwrap();
    core.finish_step(&mut outputs);
    assert_eq!(
        hub_books(&mut core, peer),
        (one, one, 0),
        "the core peer first"
    );
    assert_eq!(hub_books(&mut core, downlink), (one / 2, one / 2, 0));
}
