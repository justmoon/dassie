//! The checks the node simulation judges a run over a settlement hub by
//! (spec/areas/testing.md#over-a-settlement-hub), shared by the scripted
//! runs over a hub (`node-hub` and the runs beside it) and the `node`
//! simulation's runs over a hub ([`swarm`](super::swarm)): each member by its
//! own books (spec/decisions/own-books.md), in the hub and on the XRP Ledger
//! account it funds its hub account from, the hub's validators, its escrow,
//! and the payout once it stalls.

use std::collections::BTreeMap;

use dassie_hub::escrow::PayoutTransaction;
use dassie_kit::sim::Outcome;
use dassie_xrpl::api;
use dassie_xrpl::codec::Amount;
use dassie_xrpl::fake::FAUCET_DROPS;

use super::checks::{
    check_anchored, check_delivered, check_funded, check_held, check_true, first_failure,
    outgoing_flows,
};
use super::hub::owed;
use super::{HUB, Sim, XRP, key, received, xrpl_account};

/// Each member's books over a hub of XRP, on their own, once the network
/// is calm and no member lies: what a run on one ledger checks of them
/// ([`checks::check_books`](super::checks::check_books)), the hub's way.
/// Its books hold what its schemes hold, in the hub as its honest
/// validators have it; its owner's funds on the XRP Ledger and in the hub
/// together moved by what its receivers took; what it booked as lost on a
/// peer is no more than the holds between them; its books hold to the fake
/// ledger's truth; its account is in step with the hub, no move of its
/// funds is on its way, and no honest peer is at its ask point. And the
/// validators broke nothing, and the escrow holds what the hub owes.
pub(super) fn check_books(sim: &Sim) -> Result<(), Outcome> {
    first_failure([
        check_hub_validators(sim).map_err(|problem| Outcome::safety("hub-agreement", problem)),
        check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
        check_hub_books(sim, None).map_err(|problem| {
            Outcome::safety("hub-books", format!("{problem}; {}", hub_standing(sim)))
        }),
        check_owners(sim, &[]).map_err(|problem| Outcome::safety("hub-owner-books", problem)),
        check_escrow(sim).map_err(|problem| Outcome::safety("hub-escrow", problem)),
        check_true(sim).map_err(|problem| Outcome::safety("proven", problem)),
        check_funded(sim).map_err(|problem| Outcome::liveness("funded", problem)),
        check_anchored(sim).map_err(|problem| Outcome::liveness("anchored", problem)),
        check_in_step(sim, None).map_err(|problem| Outcome::liveness("hub-in-step", problem)),
        check_moved(sim, None).map_err(|problem| Outcome::liveness("hub-moves", problem)),
        check_settled_with_honest(sim, None)
            .map_err(|problem| Outcome::liveness("settled", problem)),
        check_delivered(sim).map_err(|problem| Outcome::liveness("delivered", problem)),
    ])
}

/// What a run over a hub still waits for before its books may be judged,
/// if anything: a member's account that has not read the hub up to a
/// recent block, or a move of its funds into or out of the hub on its way.
pub(super) fn waiting(sim: &Sim) -> Option<String> {
    check_in_step(sim, None)
        .err()
        .or_else(|| check_moved(sim, None).err())
}

/// How the hub and its members stand, for a failure's message: each
/// validator's last final block, view, and whether a quorum checked the
/// escrow, and each member's account and how it stands.
pub(super) fn hub_standing(sim: &Sim) -> String {
    let hub = sim.hub.as_deref().unwrap();
    let validators: Vec<String> = hub
        .validators
        .iter()
        .map(|validator| match &validator.driver {
            Some(driver) => {
                let ledger = driver.validator().ledger();
                format!(
                    "{:?} at {} view {} checked {}",
                    validator.role,
                    ledger.height(),
                    driver.validator().engine().view(),
                    ledger.escrow().checked
                )
            }
            None => format!("{:?} down", validator.role),
        })
        .collect();
    let members: Vec<String> = (0..sim.nodes.len())
        .map(|index| {
            sim.nodes[index]
                .core
                .as_ref()
                .and_then(crate::engine::Core::hub_status)
                .map_or_else(
                    || "no hub account".into(),
                    |status| {
                        let key =
                            dassie_hub::member::member_key(&key(index), &hub.id()).public_key();
                        let known = hub.ledger().and_then(|ledger| ledger.account_of(&key));
                        format!(
                            "{:?} (the hub's {known:?}) {:?} reading {:?} moving {:?} on the XRP \
                             Ledger {:?}",
                            status.account,
                            status.status,
                            sim.nodes[index]
                                .core
                                .as_ref()
                                .and_then(crate::engine::Core::hub_reading),
                            status.moving.map(|moving| (moving.direction, moving.drops)),
                            sim.ledger.balance(xrpl_account(index))
                        )
                    },
                )
        })
        .collect();
    format!("validators {validators:?}, members {members:?}")
}

/// Whether every member has its account in the hub, in step with it.
#[must_use]
pub fn joined(sim: &Sim) -> bool {
    members(sim).all(|index| {
        sim.nodes[index]
            .core
            .as_ref()
            .and_then(crate::engine::Core::hub_status)
            .is_some_and(|status| {
                status.account.is_some() && status.status == dassie_settlement::LedgerStatus::Synced
            })
    })
}

/// The nodes in the hub: in a run across currencies, those with its
/// ledger.
pub(super) fn members(sim: &Sim) -> impl Iterator<Item = usize> + '_ {
    (0..sim.nodes.len()).filter(|index| sim.settling.ledgers(*index).contains(&HUB))
}

/// Once the network is calm, no honest node's books show an honest peer
/// still at its ask point on any method: asked, the peer settled
/// (spec/areas/settlement.md#asking-a-peer-to-settle). A liar's settling is
/// its own.
pub(super) fn check_settled_with_honest(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    for index in (0..sim.nodes.len()).filter(|index| Some(*index) != liar) {
        for peer in sim.core(index).network().peers {
            if liar.is_some_and(|liar| key(liar).node_id() == peer.id) {
                continue;
            }
            if peer.termination.is_some() {
                continue;
            }
            for (method, owed) in peer.peering.methods.iter().zip(&peer.owed_by_method) {
                let Some(owed) = *owed else {
                    continue;
                };
                let credit = method.credit_out;
                let position = u128::try_from(owed + i128::from(credit)).unwrap_or(0);
                if dassie_settlement::policy::ask(position, super::TRIGGER) {
                    return Err(format!(
                        "node {index}'s room for {} on {} is {position} of {credit}: it never \
                         settled",
                        peer.id, method.scheme,
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The validators broke nothing: their invariants held after every step,
/// no two honest ones finalized different states at one height, and no
/// evidence named an honest one.
pub(super) fn check_hub_validators(sim: &Sim) -> Result<(), String> {
    match sim.hub.as_deref().unwrap().breaches.as_slice() {
        [] => Ok(()),
        breaches => Err(breaches.join("; ")),
    }
}

/// What each node booked as lost on a peer over the hub is no more than
/// the holds between them: its own toward the peer, which a payee that
/// claims what it should not takes from, and the peer's toward it, from
/// which it takes what a payer that sends no claims owes, paying a fee per
/// claim (spec/designs/settlement-hubs.md#13-how-it-fits-the-node). By its
/// own books: its losses as it booked them, its holds as its scheme read
/// them.
pub(super) fn check_losses(sim: &Sim) -> Result<(), String> {
    // Per method: each hub the two share holds its own holds, in its own
    // asset.
    for (method, lost) in &sim.hub_losses {
        let (toward, from) = sim.hub_most_held.get(method).copied().unwrap_or((0, 0));
        if *lost > toward.saturating_add(from) {
            let (index, peer, scheme) = method;
            return Err(format!(
                "node {index} lost {lost} on {peer} over {scheme}, more than its hold toward \
                 it ({toward}) and its hold toward it ({from}) held"
            ));
        }
    }
    Ok(())
}

/// Each node's books hold what its schemes hold: on the XRP Ledger its
/// account's balance less the reserve, in the hub what its scheme says,
/// which is its account's balance and what its peers' holds toward it hold,
/// as the hub's honest validators have it once the network is calm and
/// it read the operations of its own they applied.
///
/// Once the network is calm and an honest node read up to the hub's newest
/// block, it has read every operation of its own the hub applied; only a
/// liar's claims, held back to just before their commitments expire, may
/// land after, and its books are checked once it read them.
pub(super) fn check_hub_books(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    for index in members(sim) {
        check_held(sim, index, XRP)?;
        let booked = check_held(sim, index, HUB)?;
        // One still reading the hub is behind it: [`check_in_step`] says.
        if !in_step(sim, index) {
            continue;
        }
        let member = dassie_hub::member::member_key(&key(index), &hub.id()).public_key();
        let number = ledger
            .account_of(&member)
            .ok_or_else(|| format!("node {index} has no account in the hub"))?;
        let account = ledger.account(number).unwrap();
        // An operation of its own the hub applied after the state it read
        // is booked once it reads it.
        // An honest node may be a block or so behind the newest, as it reads
        // the hub every so often; once it read up to it, it read all.
        let read = sim.core(index).hub_sequence();
        if read != Some(account.sequence) {
            let cursor = sim
                .core(index)
                .hub_reading()
                .map_or(0, |(cursor, _, _)| cursor);
            if Some(index) == liar || cursor < ledger.height() {
                continue;
            }
            return Err(format!(
                "node {index} read its hub account up to sequence {read:?}, where the hub \
                 applied its operations up to {}",
                account.sequence
            ));
        }
        let toward: u64 = ledger
            .holds()
            .filter(|(_, hold)| hold.payee == number)
            .map(|(_, hold)| hold.amount)
            .sum();
        let held = i128::from(dassie_hub::member::units(account.balance + toward));
        if booked != held {
            return Err(format!(
                "node {index} books {booked} in the hub, where its account holds {} and its \
                 peers' holds toward it {toward}",
                account.balance
            ));
        }
    }
    Ok(())
}

/// Whether node `index` has a move of its funds into or out of the hub on
/// its way.
pub(super) fn moving(sim: &Sim, index: usize) -> bool {
    sim.core(index)
        .hub_status()
        .is_some_and(|status| status.moving.is_some())
}

/// Whether node `index`'s hub account is in step with the hub.
pub(super) fn in_step(sim: &Sim, index: usize) -> bool {
    sim.core(index)
        .hub_status()
        .is_some_and(|status| status.status == dassie_settlement::LedgerStatus::Synced)
}

/// Once the network is calm, every honest member's hub account is in step
/// with the hub: it read what touched it up to a recent final block. A
/// liar's is its own affair.
pub(super) fn check_in_step(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    let behind = members(sim)
        .filter(|index| Some(*index) != liar)
        .find(|index| !in_step(sim, *index));
    match behind {
        Some(index) => Err(format!(
            "node {index}'s hub account is not in step with the hub: {}",
            hub_standing(sim)
        )),
        None => Ok(()),
    }
}

/// Once the network is calm, no honest node has funds on their way into
/// or out of the hub: each move is done within its wait
/// (`CREDIT_WAIT_MS`). A liar's moves are its own affair: one that makes
/// operations of its own account behind its node's back holds up its
/// node's.
pub(super) fn check_moved(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    let moves = members(sim)
        .filter(|index| Some(*index) != liar)
        .find(|index| moving(sim, *index));
    match moves {
        Some(index) => Err(format!(
            "node {index} still moves funds: {:?}",
            sim.core(index)
                .hub_status()
                .and_then(|status| status.moving)
        )),
        None => Ok(()),
    }
}

/// Each owner's funds, on the XRP Ledger and in the hub together, moved by
/// what its receivers took: what the faucet gave, less the XRP Ledger's
/// reserve and the hub's account reserve, which neither the owner nor
/// anyone else can spend. And each payment's token, every node's, the
/// liar's among them, paid what it sent, as
/// [`checks::check_owner_books`](super::checks::check_owner_books) has it:
/// a token's books follow its packets, not what the node read of the hub.
/// So do its outgoing payments: what each received counts as its
/// receiver's, and the payer's account paid what it sent.
/// The account reserve of a node `paid_in` by a peer came out of that
/// peer's `pay`, not out of its owner's funds
/// (spec/designs/settlement-hubs.md#5-joining).
pub(super) fn check_owners(sim: &Sim, paid_in: &[usize]) -> Result<(), String> {
    let account_reserve = i128::from(dassie_hub::member::units(
        sim.hub.as_deref().unwrap().rules.terms.fees.account_reserve,
    ));
    let reserve = |index: usize| {
        if paid_in.contains(&index) {
            0
        } else {
            account_reserve
        }
    };
    // One behind the hub, or moving funds, has not booked all of it yet:
    // the checks of being in step and of moves say so.
    let done = |index: &usize| in_step(sim, *index) && !moving(sim, *index);
    for index in (0..sim.nodes.len()).filter(done) {
        let owner = |ledger| {
            sim.balances_on(index, ledger)
                .get("equity/owner")
                .copied()
                .unwrap_or(0)
        };
        let pool = owner(XRP) + owner(HUB);
        let fundings = i128::from(sim.fundings.get(&index).copied().unwrap_or(0));
        let deposit = (fundings * i128::from(FAUCET_DROPS) - 1_000_000) * 1_000 - reserve(index);
        let received: i128 = sim
            .payments
            .iter()
            .chain(&sim.abandoned)
            .filter(|payment| payment.to == index)
            .map(|payment| i128::from(received(sim, payment)))
            .sum::<i128>()
            + outgoing_flows(sim)
                .filter(|flow| flow.to == index)
                .map(|flow| i128::from(flow.received))
                .sum::<i128>();
        if pool - deposit != received {
            return Err(format!(
                "node {index}'s owner gained {} for {received} received; its books: {:?} and {:?}",
                pool - deposit,
                sim.balances_on(index, XRP),
                sim.balances_on(index, HUB)
            ));
        }
        let paid: i128 = outgoing_flows(sim)
            .filter(|flow| flow.from == index)
            .map(|flow| i128::from(flow.sent))
            .sum();
        let payer = -sim
            .balances_on(index, HUB)
            .get("equity/owner/local/pay")
            .copied()
            .unwrap_or(0);
        if payer != paid {
            return Err(format!(
                "node {index}'s payer paid {payer} for {paid} its outgoing payments sent"
            ));
        }
    }
    for payment in &sim.payments {
        let index = payment.from;
        let account = format!("equity/owner/btp/{}", payment.token.0);
        let token = |ledger| {
            sim.balances_on(index, ledger)
                .get(&account)
                .copied()
                .unwrap_or(0)
        };
        let paid = -(token(XRP) + token(HUB));
        if paid != i128::from(super::sent(payment)) {
            return Err(format!(
                "node {index}'s token {} paid {paid} for {} sent",
                payment.token.0,
                super::sent(payment)
            ));
        }
    }
    Ok(())
}

/// The escrow holds what the hub owes and what the sponsor left in it, but
/// for what is on its way between them; and the fake ledger made or lost no
/// XRP.
///
/// A deposit on its way, a liar's among them, is in the escrow and not yet
/// in the hub: the escrow holds more than the hub owes, by no more than the
/// deposits the nodes are making. A withdrawal the hub queued is part of
/// what it owes until a quorum attests that the escrow paid it: once the
/// escrow paid it and until then, the escrow holds less than the hub owes,
/// by no more than the queued withdrawals and their ledger fees, whatever
/// the nodes think of their moves.
pub(super) fn check_escrow(sim: &Sim) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    let depositing: u64 = (0..sim.nodes.len())
        .filter_map(|index| sim.core(index).hub_status()?.moving)
        .filter(|moving| moving.direction == crate::engine::core::Direction::Deposit)
        .map(|moving| moving.drops)
        .sum();
    let withdrawing: u64 = ledger
        .withdrawals()
        .map(|(_, withdrawal)| withdrawal.amount + withdrawal.ledger_fee)
        .sum();
    let escrow = sim.ledger.balance(hub.escrow()).unwrap_or(0);
    let owed = hub.escrow_funding + ledger.supply();
    if escrow + withdrawing < owed || escrow > owed + depositing {
        return Err(format!(
            "the escrow holds {escrow}, while the hub owes {} and the sponsor left {}, with \
             {depositing} on its way in and {withdrawing} queued out",
            ledger.supply(),
            hub.escrow_funding
        ));
    }
    if sim.ledger.total() != dassie_xrpl::MAX_DROPS {
        return Err("the fake ledger made or lost XRP".into());
    }
    Ok(())
}

/// The escrow paid out exactly the final balances of the block the set is
/// of: each account with somewhere to go and enough to pay out received
/// its balance, reserves and the holds it paid into, less the fee, as the
/// design says, worked out apart from the set; every transaction of the set
/// applied; and the escrow is left with what it held less what the set
/// paid and its fees.
pub(super) fn check_payout(sim: &mut Sim) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let payout = hub.payout.as_ref().unwrap();
    let ledger = &payout.ledger;
    let fees = ledger.terms().fees;
    let (_, before, first) = payout.submitted.unwrap();
    let (after, last) = payout.escrow_after.unwrap();
    let escrow = hub.escrow();
    let hashes = payout.hashes.clone();
    let transactions = payout.set.transactions.clone();
    let mut expected: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    for (number, account) in ledger.accounts() {
        let Some(disbursal) = account.disbursal else {
            continue;
        };
        let total = owed(ledger, number);
        if total >= fees.ledger && total - fees.ledger >= fees.least_payout {
            expected.insert(
                dassie_xrpl::AccountId(disbursal.account),
                total - fees.ledger,
            );
        }
    }
    let mut paid_out = 0u64;
    let mut delivered: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    for (hash, transaction) in hashes.iter().zip(&transactions) {
        let record = transaction_record(sim, hash)
            .ok_or_else(|| format!("{transaction:?} was never applied"))?;
        if record.result != "tesSUCCESS" {
            return Err(format!("{transaction:?} ended {}", record.result));
        }
        // A withdrawal the hub had not seen settle when it stalled is in
        // the set, though a validator submitted it before: it applies
        // once, and moved the escrow before the payout began.
        let before_payout = record.ledger_index < first;
        if !before_payout {
            paid_out += record.fee;
        }
        if let PayoutTransaction::Payout(payment) | PayoutTransaction::Withdrawal(payment) =
            transaction
        {
            let Amount::Xrp(amount) = payment.amount else {
                return Err("a payout of something else than XRP".into());
            };
            if record.delivered != Some(amount) {
                return Err(format!(
                    "a payout of {amount} delivered {:?}",
                    record.delivered
                ));
            }
            if !before_payout {
                paid_out += amount;
            }
            if matches!(transaction, PayoutTransaction::Payout(_)) {
                *delivered.entry(payment.destination).or_default() += amount;
            }
        }
    }
    if delivered != expected {
        return Err(format!(
            "the escrow paid out {delivered:?}, where the final balances are {expected:?}"
        ));
    }
    for index in 0..sim.nodes.len() {
        if !delivered.contains_key(&xrpl_account(index)) {
            return Err(format!("node {index} was paid nothing out"));
        }
    }
    // Deposits a member made meanwhile, which the stalled hub never
    // credits, are the only other money the escrow may have moved.
    let mut deposited = 0u64;
    for record in escrow_transactions(sim, escrow, first, last)? {
        if hashes.contains(&record.hash) {
            continue;
        }
        if record.account == escrow {
            return Err(format!("the escrow paid {record:?} outside the payout set"));
        }
        if record.result == "tesSUCCESS" && record.destination == Some(escrow) {
            deposited += record.delivered.unwrap_or(0);
        }
    }
    if after + paid_out != before + deposited {
        return Err(format!(
            "the escrow went from {before} to {after}, having paid out {paid_out} and taken \
             {deposited} in deposits"
        ));
    }
    sim.count("payouts checked");
    Ok(())
}

/// The escrow's transactions in ledgers `first` to `last`.
fn escrow_transactions(
    sim: &mut Sim,
    escrow: dassie_xrpl::AccountId,
    first: u32,
    last: u32,
) -> Result<Vec<api::TxRecord>, String> {
    let connection = sim.hub.as_deref().unwrap().submitter();
    let mut records = Vec::new();
    let mut marker = None;
    loop {
        let request = api::account_tx(8, escrow, first, last, marker.as_ref());
        let reply = sim.ledger.request(connection, &request);
        let api::Message::Response { outcome, .. } = api::parse(&reply) else {
            return Err("the fake ledger did not answer".into());
        };
        let page = outcome
            .ok()
            .and_then(|result| api::account_tx_result(&result))
            .ok_or("the escrow's transactions do not read")?;
        records.extend(page.transactions);
        match page.marker {
            Some(next) => marker = Some(next),
            None => return Ok(records),
        }
    }
}

/// The fake ledger's record of the transaction `hash`, if it applied.
fn transaction_record(sim: &mut Sim, hash: &[u8; 32]) -> Option<api::TxRecord> {
    let connection = sim.hub.as_deref().unwrap().submitter();
    let reply = sim.ledger.request(connection, &api::tx_anywhere(9, hash));
    let api::Message::Response { outcome, .. } = api::parse(&reply) else {
        return None;
    };
    api::tx_result(&outcome.ok()?)
}
