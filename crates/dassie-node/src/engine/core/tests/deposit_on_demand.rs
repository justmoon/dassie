//! The owner's payments over a settlement hub that the hub account alone
//! does not cover (spec/decisions/one-balance-per-asset.md): the funds
//! check counts what the owner book holds, the node deposits the shortfall
//! with the float, and the payment starts once the hub credits it. On one
//! engine core in the XRP hub, paying its own receiver on the hub's ledger.

use dassie_hub::member::SCHEME_ID;
use dassie_ledger::LedgerId;
use dassie_settlement::policy;
use dassie_xrpl::UNITS_PER_DROP;

use super::super::payers::NewOutgoing;
use super::super::{Core, EngineError, NewIncoming};
use super::{Policies, Validators, deposits, funded_ledger, funding_core, wait_and_close};
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
    // With the node's own moves off, the hub alone pays.
    core.set_hub_automatic(SCHEME_ID, false).unwrap();
    let refused = pay_self(&mut core, debit);
    assert!(
        matches!(refused, Err(EngineError::Precondition(_))),
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

    hub.account(
        &mut core,
        balance + made[1].amount,
        opening + made[1].amount,
    );
    for _ in 0..4 {
        wait_and_close(&mut core, &mut ledger, 1_000);
    }
    let row = core.store.outgoing_payment(id).unwrap().unwrap();
    assert_eq!(row.state, OutgoingState::Completed, "{row:?}");
    assert_eq!(row.sent_amount, debit);
}
