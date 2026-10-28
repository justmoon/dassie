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

use dassie_hub::operation::{MemberOperation, Operation, SignedMemberOperation};

use super::super::{Core, Output};
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
fn membership(hub: &Validators, number: u32, seed: u8) -> Vec<u8> {
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
fn peer_over_hub(
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

/// A core node in the XRP hub whose account, number 5, holds `balance`
/// drops as last agreed, with a core peer peered over the hub as account
/// [`CORE_ACCOUNT`] and a downlink as account [`DOWNLINK_ACCOUNT`]: the
/// core, the hub, the core peer and the downlink. The core peer is a
/// downlink first, which turns core once its bootstrap node lists it.
fn hub_with_peers(ledger: &mut FakeLedger, balance: u64) -> (Core, Validators, NodeId, NodeId) {
    let mut core = core_with_bootstrap(Arc::new(Store::open_in_memory().unwrap()));
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(SCHEME_ID, hub_config(600, 300)).unwrap();
    follow_xrpl(&mut core, ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    wait_and_close(&mut core, ledger, 1_000);
    let opening = deposits(ledger, hub.escrow())[0].amount;
    hub.account(&mut core, balance, opening);

    let downlink = Arc::new(dassie_identity::TableKeygen::identity_key(&[11; 32]));
    let peer = Arc::new(dassie_identity::TableKeygen::identity_key(&[14; 32]));
    let mut at = core.now_ms() + 1_000;
    peer_over_hub(&mut core, &hub, &downlink, (DOWNLINK_ACCOUNT, 12), at);
    peer_over_hub(&mut core, &hub, &peer, (CORE_ACCOUNT, 13), at + 1);
    send_link_state(&mut core, &peer, at + 2, Some("https://c.example:4443"));
    let (downlink, peer) = (downlink.node_id(), peer.node_id());
    while core.peer.is_downlink(&peer) {
        at += Policies::DEFAULT.peer.list_poll_interval_ms;
        step_with_list(&mut core, at, &[peer]);
    }
    hub.account(&mut core, balance, opening);
    assert!(core.peer.peering_of(&downlink).is_some(), "the downlink");
    assert!(core.peer.peering_of(&peer).is_some(), "the core peer");
    assert!(core.peer.is_downlink(&downlink));
    (core, hub, peer, downlink)
}

/// The operations the node submitted to the validators, each once.
fn submitted(outputs: &[Output]) -> Vec<SignedMemberOperation> {
    use dassie_hub::client::Request;
    use dassie_hub::service::wire::Message;

    let mut seen = Vec::new();
    for output in outputs {
        let Output::HubSend { message, .. } = output else {
            continue;
        };
        if let Ok(Message::Request(Request::Submit(Operation::Member(signed)))) =
            Message::decode_exact(message)
            && !seen.contains(&signed)
        {
            seen.push(signed);
        }
    }
    seen
}

/// The hub applies `ours`, opening hold 1, in a new final block, after
/// which the node's account 5 holds `balance` and deposits credited it
/// `deposited`.
fn applied(
    core: &mut Core,
    hub: &mut Validators,
    ours: &SignedMemberOperation,
    (balance, deposited): (u64, u64),
) {
    use dassie_hub::client::{AccountView, Statement};
    use dassie_hub::ledger::{Detail, Outcome};
    use dassie_hub::operation::AccountNumber;
    use dassie_hub::promise::HoldNumber;

    hub.height += 1;
    let fees = hub.rules.terms.fees;
    let view = AccountView {
        key: hub.member,
        balance,
        reserve: fees.account_reserve,
        sequence: ours.sequence + 1,
        disbursal: true,
        closing: false,
        deposited,
        withdrawn: 0,
        returned: 0,
    };
    let timestamp = NOW + hub.height;
    let statements = [
        Statement::Executed {
            height: hub.height,
            timestamp,
            index: 0,
            operation: Operation::Member(ours.clone()),
            outcome: Outcome::Applied {
                fee: fees.operation,
                detail: Detail::Hold(HoldNumber(1)),
            },
        },
        Statement::Account {
            height: hub.height,
            key: hub.member,
            account: Some((AccountNumber(5), view)),
        },
        Statement::Final {
            height: hub.height,
            timestamp,
        },
    ];
    hub.say(core, &statements);
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
// allotted the least hold before the downlink gets anything, and a request
// of the core peer's funds the hold up to its allotment, less than the
// rule says, and nothing more while it holds that; the request is kept,
// to be funded once the hold has room under the allotment again.
#[test]
fn a_scarce_pool_funds_a_core_peer_up_to_its_allotment() {
    let (mut ledger, _) = funded_ledger();
    // Eight packets: the least hold for the core peer, the rest for the
    // downlink.
    let pool = 8 * P;
    let (mut core, _hub, peer, downlink) = hub_with_peers(&mut ledger, pool / UNITS_PER_DROP);
    let scheme = scheme_id(SCHEME_ID);
    let least = policy::least_hold(P, Policies::DEFAULT.connector.max_in_flight());
    let allotments = core.hub_allotments(scheme).unwrap();
    assert!(!allotments.plentiful);
    assert_eq!(allotments.peers, [(peer, least), (downlink, pool - least)]);
    assert!(least < one_request(), "the allotment caps the request");

    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    assert_eq!(core.settlement_requested(peer, scheme), least);
    assert!(core.settle.unanswered.contains_key(&(peer, scheme)), "kept");
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    for later in 1..5 {
        let at = core.now_ms() + later * 1_000;
        core.begin_step(at);
        core.finish_step(&mut outputs);
    }
    // Funded once, up to the allotment: what was on its way is all the
    // core peer gets until the hold falls back under it.
    let (credit, _, _) = core
        .hub_books(dassie_ledger::name!(SCHEME_ID), peer)
        .unwrap();
    assert_eq!(credit, least);
    core.check_invariants();
}

// A hold at its allotment takes no more: once the hub applied the core
// peer's hold, funded up to its allotment, the owner sees the allotment
// beside it, and the peer's next request funds nothing, booked nothing, and
// is kept until the hold has room under the allotment again.
#[test]
fn a_hold_at_its_allotment_takes_no_more() {
    let (mut ledger, _) = funded_ledger();
    let least = policy::least_hold(P, Policies::DEFAULT.connector.max_in_flight());
    let pool = 8 * P / UNITS_PER_DROP;
    let (mut core, mut hub, peer, _) = hub_with_peers(&mut ledger, pool);
    let opening = deposits(&mut ledger, hub.escrow())[0].amount;
    let scheme = scheme_id(SCHEME_ID);
    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    assert_eq!(core.settlement_requested(peer, scheme), least);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let funding = submitted(&outputs)
        .into_iter()
        .find(|signed| matches!(signed.operation, MemberOperation::HoldOpen { .. }))
        .unwrap();
    let fee = hub.rules.terms.fees.operation;
    let left = pool - least / UNITS_PER_DROP - fee;
    applied(&mut core, &mut hub, &funding, (left, opening));
    // Its notice goes, and the peer acknowledges it.
    let at = core.now_ms() + 1_000;
    let mut outputs = Vec::new();
    core.begin_step(at);
    core.finish_step(&mut outputs);
    let notice = outputs
        .into_iter()
        .find_map(|output| match output {
            Output::PeerRequest(request) if request.to == peer => Some(request.id),
            _ => None,
        })
        .unwrap();
    core.begin_step(at);
    let done = dassie_peer::Response {
        status: 200,
        body: Vec::new(),
    };
    core.peer_response(notice, Ok(done));
    core.finish_step(&mut Vec::new());
    let hub_scheme = dassie_ledger::name!(SCHEME_ID);
    let (credit, _, settling) = core.hub_books(hub_scheme, peer).unwrap();
    assert_eq!((credit, settling), (least, 0));
    assert_eq!(allotment_shown(&core, peer), Some(least));

    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    let before = core.ledger.next_transfer_id();
    assert_eq!(core.settlement_requested(peer, scheme), 0);
    assert_eq!(core.ledger.next_transfer_id(), before, "booked nothing");
    assert!(core.settle.unanswered.contains_key(&(peer, scheme)), "kept");
    core.finish_step(&mut Vec::new());
    assert_eq!(core.hub_books(hub_scheme, peer).unwrap().0, least);
    core.check_invariants();
}

// With enough in the hub for every peer's default credit, nothing is
// capped: a request funds what the rule says, as without allotments, and
// the owner sees each allotment at the default credit.
#[test]
fn a_plentiful_pool_funds_what_the_rule_says() {
    let (mut ledger, _) = funded_ledger();
    let pool = 2 * Policies::DEFAULT.peer_credit;
    let (mut core, mut hub, peer, downlink) = hub_with_peers(&mut ledger, pool / UNITS_PER_DROP);
    let opening = deposits(&mut ledger, hub.escrow())[0].amount;
    hub.opened_hold(
        &mut core,
        P / UNITS_PER_DROP,
        pool / UNITS_PER_DROP,
        opening,
    );
    let scheme = scheme_id(SCHEME_ID);
    let allotments = core.hub_allotments(scheme).unwrap();
    assert!(allotments.plentiful);
    assert_eq!(allotments.of(&peer), None);
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
    let mut at = NOW + 1_000;
    peer_as_downlink(&mut core, &downlink, at);
    peer_as_downlink(&mut core, &peer, at + 1);
    send_link_state(&mut core, &peer, at + 2, Some("https://c.example:4443"));
    let (downlink, peer) = (downlink.node_id(), peer.node_id());
    while core.peer.is_downlink(&peer) {
        at += Policies::DEFAULT.peer.list_poll_interval_ms;
        step_with_list(&mut core, at, &[peer]);
    }
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
