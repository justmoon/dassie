//! The moves between the XRP hub and its owner book while the hub
//! account's own operations race them (spec/areas/hubs.md#in-the-node): a
//! withdrawal is sized from what the account can still spend once the
//! operations before it executed, never from an agreed state older than
//! they are; and a move that failed one way holds back only the next move
//! that way, never the deposit the floor calls for. On one engine core in
//! the XRP hub, with a downlink peered over it.

use std::sync::Arc;

use dassie_hub::client::Statement;
use dassie_hub::ledger::{Detail, Failure, Outcome};
use dassie_hub::member::SCHEME_ID;
use dassie_hub::operation::{AccountNumber, MemberOperation, Operation, SignedMemberOperation};
use dassie_hub::promise::HoldNumber;
use dassie_identity::NodeId;
use dassie_settlement::policy;
use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::fake::FakeLedger;

use super::super::hub_funding::{Direction, FIRST_BACKOFF_MS};
use super::super::{Core, Output};
use super::{
    NOW, Policies, SHARED_CONFIG, Validators, add_xrpl, deposits, downlink_request_on, follow_xrpl,
    funded_ledger, hub_config, scheme_id, wait_and_close, withdrawals,
};

/// The downlink's account in the hub.
const PEER_ACCOUNT: u32 = 9;

/// The seed of the downlink's key in the hub.
const PEER_SEED: u8 = 12;

/// The floor and the ceiling of a hub with one peering over it, in drops,
/// and their midpoint.
fn limits() -> (u64, u64, u64) {
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let floor = policy::hub_floor(p, Policies::DEFAULT.peer_credit, 1) / UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / UNITS_PER_DROP;
    (floor, ceiling, floor.midpoint(ceiling))
}

/// The seed the tests run on; any other gives the same outcome.
const SEED: u64 = 9;

/// A core of `seed` in the XRP hub whose account, number 5, the opening
/// deposit credited, holding `balance` drops as last agreed, with a
/// downlink peered over the hub as account [`PEER_ACCOUNT`]: the core, the
/// hub, the downlink and what the opening deposit paid.
fn hub_with_peer(
    ledger: &mut FakeLedger,
    balance: u64,
    seed: u64,
) -> (Core, Validators, NodeId, u64) {
    let mut core = super::core(seed);
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(SCHEME_ID, hub_config(600, 300)).unwrap();
    follow_xrpl(&mut core, ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    wait_and_close(&mut core, ledger, 1_000);
    let opening = deposits(ledger, hub.escrow())[0].amount;
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, ledger, 1_000);

    let downlink = Arc::new(dassie_identity::TableKeygen::identity_key(&[11; 32]));
    let mut membership = hub.rules.id().0.as_bytes().to_vec();
    membership.extend_from_slice(&PEER_ACCOUNT.to_be_bytes());
    membership.extend_from_slice(peer_key(&hub).as_bytes());
    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    let body = downlink_request_on(&downlink, at, SCHEME_ID, &membership);
    core.peer_inbound(dassie_peer::InboundId(at), downlink.identity(), &body, None);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let peer = downlink.node_id();
    assert!(core.peer.peering_of(&peer).is_some(), "peered over the hub");
    assert_eq!(core.hub_status().unwrap().moving, None, "the opening done");
    (core, hub, peer, opening)
}

/// The downlink's key in the hub.
fn peer_key(hub: &Validators) -> dassie_identity::PublicKey {
    dassie_hub::member::member_key(
        &dassie_identity::TableKeygen::identity_key(&[PEER_SEED; 32]),
        &hub.rules.id(),
    )
    .public_key()
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

/// The downlink asks the core to settle at `at`: the core funds its hold
/// toward it, and the account sends the operation. The operation, and the
/// drops it funds.
fn asked_to_settle(core: &mut Core, peer: NodeId, at: u64) -> (SignedMemberOperation, u64) {
    core.begin_step(at);
    let settling = core.settlement_requested(peer, scheme_id(SCHEME_ID));
    assert!(settling > 0, "the request is funded");
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let sent = submitted(&outputs);
    assert_eq!(sent.len(), 1, "{sent:?}");
    (sent[0].clone(), settling / UNITS_PER_DROP)
}

/// The hub executes, in a new final block, `ours` with `outcome`, and
/// opens the downlink's hold of `toward_us` drops toward the node's
/// account, if any; the validators say no new state of the account in it,
/// as they say one only every few seconds.
fn executed_without_a_state(
    core: &mut Core,
    hub: &mut Validators,
    ours: &SignedMemberOperation,
    outcome: Outcome,
    toward_us: Option<u64>,
) {
    hub.height += 1;
    let fees = hub.rules.terms.fees;
    let timestamp = NOW + hub.height;
    let mut statements = vec![Statement::Executed {
        height: hub.height,
        timestamp,
        index: 0,
        operation: Operation::Member(ours.clone()),
        outcome,
    }];
    if let Some(amount) = toward_us {
        let key = dassie_hub::member::member_key(
            &dassie_identity::TableKeygen::identity_key(&[PEER_SEED; 32]),
            &hub.rules.id(),
        );
        let theirs = SignedMemberOperation::sign(
            &hub.rules.id(),
            AccountNumber(PEER_ACCOUNT),
            1,
            fees.operation,
            MemberOperation::HoldOpen {
                payee: AccountNumber(5),
                amount,
            },
            &key,
        );
        statements.push(Statement::Executed {
            height: hub.height,
            timestamp,
            index: 1,
            operation: Operation::Member(theirs),
            outcome: Outcome::Applied {
                fee: fees.operation,
                detail: Detail::Hold(HoldNumber(2)),
            },
        });
    }
    statements.push(Statement::Final {
        height: hub.height,
        timestamp,
    });
    hub.say(core, &statements);
}

/// Whether a failure holds the node's next withdrawal back at `now`.
fn withdrawals_back_off(core: &Core, now: u64) -> bool {
    let hub = dassie_ledger::name!(SCHEME_ID);
    core.funding[&hub]
        .backing_off(Direction::Withdraw, now)
        .is_some()
}

// The race the e2e flow ran into: the node's hold funding to its peer
// executes, and the agreed state of its account, which the validators say
// only every few seconds, still shows the balance from before. The peer's
// hold toward the node takes the hub past the ceiling, so a withdrawal is
// due. It is sized from what the account can spend once its operations
// executed, which leaves only the floor, so none is made: one sized from
// the older state the hub scheme would refuse, which held every move back
// for the backoff. Then the account's state shows the balance below the
// floor, as further fundings take it there, and the deposit goes at once.
#[test]
fn a_withdrawal_sized_before_a_funding_landed_holds_back_no_deposit() {
    withdrawal_sized_before_a_funding_landed(SEED);
}

/// [`a_withdrawal_sized_before_a_funding_landed_holds_back_no_deposit`]
/// on a core of `seed`.
fn withdrawal_sized_before_a_funding_landed(seed: u64) {
    let (floor, ceiling, middle) = limits();
    let (mut ledger, _) = funded_ledger();
    let balance = 1_600_000;
    let (mut core, mut hub, peer, opening) = hub_with_peer(&mut ledger, balance, seed);
    let fees = hub.rules.terms.fees;
    let at = core.now_ms() + 1_000;
    let (funding, drops) = asked_to_settle(&mut core, peer, at);
    let applied = Outcome::Applied {
        fee: fees.operation,
        detail: Detail::Hold(HoldNumber(1)),
    };
    // The peer's hold toward the node takes the hub just past the ceiling.
    let ours = balance + fees.account_reserve + fees.hold_reserve + drops;
    let toward_us = ceiling + 100_000 - ours;
    executed_without_a_state(&mut core, &mut hub, &funding, applied, Some(toward_us));
    let spendable = balance - drops - fees.hold_reserve - fees.operation;
    assert!(
        balance - floor - fees.operation - fees.ledger >= fees.least_payout,
        "the agreed state alone would call for a withdrawal"
    );
    assert!(
        spendable < floor + fees.operation + fees.ledger + fees.least_payout,
        "what the account can spend leaves none above the floor"
    );

    let outputs = wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(withdrawals(&outputs), [], "none beyond what it can spend");
    let now = core.now_ms();
    assert!(!withdrawals_back_off(&core, now), "nothing failed");
    assert_eq!(core.hub_status().unwrap().moving, None);

    // Further fundings take the balance below the floor: the deposit the
    // floor calls for goes at the next check, up to the midpoint.
    let low = 300_000;
    hub.account(&mut core, low, opening);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let made = deposits(&mut ledger, hub.escrow());
    assert_eq!(made.len(), 2, "the floor's deposit went: {made:?}");
    let exposure = low + fees.account_reserve + fees.hold_reserve + drops + toward_us;
    assert_eq!(made[1].amount, (middle - low).min(ceiling - exposure));
    assert!(core.now_ms() - at < FIRST_BACKOFF_MS, "within the backoff");
    core.check_invariants();
}

// A withdrawal the hub failed backs off the next withdrawal, doubling, as
// before, so a hub that keeps failing them is not asked again at every
// check; the deposit the floor then calls for goes at once all the same.
#[test]
fn a_failed_withdrawal_holds_back_only_withdrawals() {
    failed_withdrawal(SEED);
}

/// [`a_failed_withdrawal_holds_back_only_withdrawals`] on a core of `seed`.
fn failed_withdrawal(seed: u64) {
    let (floor, _, middle) = limits();
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub, _, opening) = hub_with_peer(&mut ledger, floor + 2_000_000, seed);
    let fees = hub.rules.terms.fees;

    // Past the ceiling: a withdrawal down to the midpoint.
    let high = 12_000_000;
    hub.account(&mut core, high, opening);
    let outputs = wait_and_close(&mut core, &mut ledger, 4_000);
    let sent = submitted(&outputs);
    let withdrawal = sent
        .iter()
        .find(|signed| matches!(signed.operation, MemberOperation::Withdraw { .. }))
        .unwrap()
        .clone();
    let exposure = high + fees.account_reserve;
    assert_eq!(withdrawals(&outputs).len(), 1);
    assert_eq!(withdrawals(&outputs)[0].0, exposure - middle);

    // The hub fails it.
    let failed = Outcome::Failed {
        fee: fees.operation,
        reason: Failure::Funds,
    };
    executed_without_a_state(&mut core, &mut hub, &withdrawal, failed, None);
    hub.account(&mut core, 200_000, opening);
    let failed_at = core.now_ms();
    let outputs = wait_and_close(&mut core, &mut ledger, 1_000);
    assert!(
        withdrawals_back_off(&core, core.now_ms()),
        "withdrawals wait"
    );
    assert_eq!(withdrawals(&outputs), []);
    let made = deposits(&mut ledger, hub.escrow());
    assert_eq!(made.len(), 2, "the floor's deposit went at once: {made:?}");
    assert!(core.now_ms() - failed_at < FIRST_BACKOFF_MS);
    assert_eq!(made[1].amount, middle - 200_000);

    // Credited, and past the ceiling again: the next withdrawal waits out
    // the backoff, then goes.
    hub.account(&mut core, high, opening + made[1].amount);
    let outputs = wait_and_close(&mut core, &mut ledger, 1_000);
    assert_eq!(core.hub_status().unwrap().moving, None, "the deposit done");
    assert_eq!(withdrawals(&outputs), [], "backing off");
    assert!(core.now_ms() - failed_at < FIRST_BACKOFF_MS);
    let wait = FIRST_BACKOFF_MS - (core.now_ms() - failed_at);
    let outputs = wait_and_close(&mut core, &mut ledger, wait);
    assert_eq!(withdrawals(&outputs).len(), 1, "after the backoff");
    core.check_invariants();
}
