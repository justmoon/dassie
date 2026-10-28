//! The owner's payments over a settlement hub that the hub account alone
//! does not cover (spec/decisions/one-balance-per-asset.md): the funds
//! check counts what the owner book holds, the node deposits the shortfall
//! with the float, and the payment starts once the hub credits it. On one
//! engine core in the XRP hub, paying its own receiver on the hub's ledger.

use std::sync::Arc;

use dassie_hub::member::SCHEME_ID;
use dassie_ledger::LedgerId;
use dassie_node_core::config::Realm;
use dassie_settlement::policy;
use dassie_xrpl::UNITS_PER_DROP;

use super::super::payers::NewOutgoing;
use super::super::{Core, EngineError, NewIncoming};

use super::{
    Policies, Store, Validators, core_taking_downlinks, deposits, follow_xrpl, funded_ledger,
    funding_core, holding_core, wait_and_close,
};
use crate::store::{ForeignAmount, OutgoingState, PaymentId};

/// What the owner holds on the hub's ledger, in its units.
fn in_hub(core: &Core) -> u64 {
    let view = core
        .ledger_views()
        .into_iter()
        .find(|view| view.id == LedgerId::HubXrplTestnetXrp)
        .unwrap();
    u64::try_from(view.owner_balance).unwrap()
}

/// What a new payment in XRP may spend now, and why the owner book does
/// not count toward it, if it does not.
fn spendable(core: &Core) -> (u128, Option<&'static str>) {
    let views = core.ledger_views();
    let balance = core
        .balance_views(&views)
        .into_iter()
        .find(|balance| balance.name == "xrp")
        .unwrap();
    (balance.spendable, balance.spendable_held)
}

/// A payment of `debit` from the hub's ledger to the node's own receiver
/// there.
fn pay_self(core: &mut Core, debit: u64) -> Result<PaymentId, EngineError> {
    let incoming = core
        .create_incoming_payment(NewIncoming {
            ledger: LedgerId::HubXrplTestnetXrp,
            wallet_address: "https://node.example/pay/xrp".into(),
            client: None,
            incoming_amount: None,
            expires_at_ms: None,
            metadata: None,
            uplink: None,
        })
        .unwrap();
    let created = core.create_outgoing_payment(NewOutgoing {
        ledger: LedgerId::HubXrplTestnetXrp,
        wallet_address: "https://node.example/pay/xrp".into(),
        client: None,
        grant: None,
        quote_id: None,
        receiver: format!("https://node.example/incoming-payments/{}", incoming.id),
        destination: incoming.ilp_address.clone(),
        shared_secret: incoming.shared_secret,
        debit_amount: debit,
        receive_amount: ForeignAmount {
            value: 1,
            asset_code: "XRP".into(),
            asset_scale: 9,
        },
        metadata: None,
        recipient: None,
        fixed_debit: true,
    })?;
    Ok(created.row.id)
}

// A payment beyond what the hub account holds, which the owner book covers,
// is taken while the node moves funds by itself; the node deposits the shortfall and the float at once, even
// past the ceiling; the payment waits, not sending, until the hub credits
// the deposit, then completes. One beyond both is refused.
#[test]
fn a_payment_beyond_the_hub_waits_for_its_deposit() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let floor = policy::hub_floor(p, Policies::DEFAULT.peer_credit, 0) / UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / UNITS_PER_DROP;
    let balance = floor.midpoint(ceiling);
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);

    let extra = 2 * ceiling * UNITS_PER_DROP;
    let beyond = in_hub(&core) + 2_000 * 1_000_000 * UNITS_PER_DROP;
    let refused = pay_self(&mut core, beyond);
    assert!(
        matches!(refused, Err(EngineError::Precondition(_))),
        "{refused:?}"
    );
    let debit = in_hub(&core) + extra;
    // With the node's own moves off, the hub alone pays: the owner sees
    // what may be spent now and why, and a refusal says so too.
    let (_, whole) = spendable(&core);
    assert_eq!(whole, None, "the book counts");
    core.set_hub_automatic(SCHEME_ID, false).unwrap();
    let (now, held) = spendable(&core);
    assert_eq!(now, u128::from(in_hub(&core)));
    assert!(held.is_some_and(|why| why.contains("moves into the hub are off")));
    let refused = pay_self(&mut core, debit);
    assert!(
        matches!(&refused, Err(EngineError::Precondition(why)) if why.contains("moves into the hub are off")),
        "{refused:?}"
    );
    core.set_hub_automatic(SCHEME_ID, true).unwrap();
    let id = pay_self(&mut core, debit).unwrap();
    wait_and_close(&mut core, &mut ledger, 1_000);
    assert!(!core.is_sending(id), "it waits for its funds");
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 2, "the shortfall: {made:?}");
    let promised = debit.div_ceil(UNITS_PER_DROP);
    assert_eq!(made[1].amount, floor.midpoint(ceiling) + promised - balance);
    assert!(made[1].amount > ceiling, "past the ceiling");

    // Once the hub booked the deposit, it counts there alone, not again
    // as on its way.
    hub.account(
        &mut core,
        balance + made[1].amount,
        opening + made[1].amount,
    );
    let spendable = core.owner_available(LedgerId::XrplTestnetXrp) / UNITS_PER_DROP;
    let fee = dassie_xrpl::account::MAX_FEE_DROPS;
    assert_eq!(
        core.hub_refill(LedgerId::HubXrplTestnetXrp),
        u128::from(spendable.saturating_sub(fee)) * u128::from(UNITS_PER_DROP),
        "a deposit the hub booked counts once"
    );
    for _ in 0..4 {
        wait_and_close(&mut core, &mut ledger, 1_000);
    }
    let row = core.store.outgoing_payment(id).unwrap().unwrap();
    assert_eq!(row.state, OutgoingState::Completed, "{row:?}");
    assert_eq!(row.sent_amount, debit);
}

// A hub that shut down takes nothing more: the owner book no longer counts
// toward a payment over it, and a payment made before draws no deposit
// into its escrow, which would credit nothing.
#[test]
fn a_shut_down_hub_draws_no_deposit_for_payments() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let floor = policy::hub_floor(p, Policies::DEFAULT.peer_credit, 0) / UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / UNITS_PER_DROP;
    let balance = floor.midpoint(ceiling);
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);

    let debit = in_hub(&core) + 2 * ceiling * UNITS_PER_DROP;
    let id = pay_self(&mut core, debit).unwrap();
    hub.shutting_down(&mut core, balance, opening);
    assert_eq!(core.hub_refill(LedgerId::HubXrplTestnetXrp), 0);
    let refused = pay_self(&mut core, debit);
    assert!(
        matches!(refused, Err(EngineError::Precondition(_))),
        "{refused:?}"
    );
    for _ in 0..3 {
        wait_and_close(&mut core, &mut ledger, 4_000);
    }
    assert_eq!(deposits(&mut ledger, escrow).len(), 1, "no deposit");
    assert!(!core.is_sending(id), "it waits out its deadline");
}

// A hub that stopped is judged stalled only after an hour, but a deposit
// made meanwhile is never credited: until it shows progress again, a hub
// that made none for a few minutes takes no deposit for the owner's
// payments, and the owner book counts nothing toward them. A higher block
// heard after the gap, as by a node that slept, may be where the hub
// stopped meanwhile, and counts for nothing; one higher still, heard
// shortly after, shows the hub goes on, and the payment goes.
#[test]
fn a_hub_with_no_progress_lately_draws_no_deposit_for_payments() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let floor = policy::hub_floor(p, Policies::DEFAULT.peer_credit, 0) / UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / UNITS_PER_DROP;
    let balance = floor.midpoint(ceiling);
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let debit = in_hub(&core) + 2 * ceiling * UNITS_PER_DROP;
    let id = pay_self(&mut core, debit).unwrap();

    // The hub goes quiet before the funding check sees the payment.
    wait_and_close(&mut core, &mut ledger, 6 * 60 * 1_000);
    assert_eq!(core.hub_refill(LedgerId::HubXrplTestnetXrp), 0);
    assert_eq!(deposits(&mut ledger, escrow).len(), 1, "no deposit");
    assert!(!core.is_sending(id));

    hub.height += 5;
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 1_000);
    assert_eq!(core.hub_refill(LedgerId::HubXrplTestnetXrp), 0);
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        1,
        "where it stopped, heard after the gap"
    );

    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 1_000);
    assert_eq!(deposits(&mut ledger, escrow).len(), 2, "the hub goes on");
}

// A restart forgets what each validator named, so a hub that stopped looks
// fresh to an account that only started to hear it: a payment left waiting
// across the restart draws no deposit while the validators name the block
// they named before it, and does once the account hears the hub go on.
#[test]
fn after_a_restart_a_quiet_hub_draws_no_deposit_for_payments() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let (mut core, mut hub) = holding_core(&store, &mut ledger);
    let escrow = hub.escrow();
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let floor = policy::hub_floor(p, Policies::DEFAULT.peer_credit, 0) / UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / UNITS_PER_DROP;
    let balance = floor.midpoint(ceiling);
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let debit = in_hub(&core) + 2 * ceiling * UNITS_PER_DROP;
    let id = pay_self(&mut core, debit).unwrap();
    wait_and_close(&mut core, &mut ledger, 6 * 60 * 1_000);
    assert_eq!(deposits(&mut ledger, escrow).len(), 1, "quiet: no deposit");
    core.flush();
    let now = core.now_ms() + 1_000;
    drop(core);

    let refused = dassie_node_core::config::LiveRealm::Refused;
    let mut core = core_taking_downlinks(Arc::clone(&store), Realm::Test, refused, now);
    follow_xrpl(&mut core, &mut ledger);
    assert!(core.store.outgoing_payment(id).unwrap().is_some());
    hub.height -= 1;
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(core.hub_refill(LedgerId::HubXrplTestnetXrp), 0);
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        1,
        "the block it named before"
    );

    // A rise the account saw itself: two higher blocks, as a hub that goes
    // on names within seconds.
    for _ in 0..2 {
        hub.account(&mut core, balance, opening);
        wait_and_close(&mut core, &mut ledger, 1_000);
    }
    assert_eq!(deposits(&mut ledger, escrow).len(), 2, "a higher one");
}

/// The owner's one balance of XRP, and the ledger its payments start on.
fn xrp(core: &Core) -> (i128, Option<LedgerId>, Vec<LedgerId>) {
    let views = core.ledger_views();
    let balance = core
        .balance_views(&views)
        .into_iter()
        .find(|balance| balance.name == "xrp")
        .unwrap();
    (balance.owner_balance, balance.pays_from, balance.ledgers)
}

// The owner sees one balance of XRP: the owner book and the hub together,
// paid from the hub; a deposit into the hub counts all the way, held on
// the book, then gone from it before the hub books it, then in the hub.
#[test]
fn one_balance_counts_the_book_the_hub_and_a_deposit_on_its_way() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, 100_000, opening);
    let (before, pays_from, ledgers) = xrp(&core);
    assert_eq!(pays_from, Some(LedgerId::HubXrplTestnetXrp));
    assert_eq!(
        ledgers,
        [LedgerId::XrplTestnetXrp, LedgerId::HubXrplTestnetXrp]
    );
    let views = core.ledger_views();
    let parts: i128 = views.iter().map(|view| view.owner_balance).sum();
    assert_eq!(before, parts, "nothing on its way");

    wait_and_close(&mut core, &mut ledger, 4_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 2, "below the floor: {made:?}");
    let fee = i128::from(dassie_xrpl::account::MAX_FEE_DROPS * UNITS_PER_DROP);
    let (during, ..) = xrp(&core);
    assert!(
        before - during <= fee,
        "the deposit counts while on its way: {before} {during}"
    );
    hub.account(
        &mut core,
        100_000 + made[1].amount,
        opening + made[1].amount,
    );
    let (after, ..) = xrp(&core);
    assert_eq!(after, during, "and once the hub booked it");
}
