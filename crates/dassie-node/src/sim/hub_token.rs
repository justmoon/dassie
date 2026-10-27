//! The node simulation's run over a DSUSD settlement hub (`node-hub-dsusd`,
//! spec/areas/testing.md#over-a-settlement-hub, spec/decisions/token-hubs.md):
//! members of a hub of 4 to 7 validators run in the simulation, whose escrow
//! holds DSUSD on the fake XRP Ledger, each with the owner books of XRP and
//! DSUSD on its XRP Ledger testnet account, peering on neither. The fake
//! ledger's issuer mints each member DSUSD; each opens its account in the
//! hub by a deposit from its DSUSD book, and the members pay each other
//! across hops over the hub.
//!
//! The escrow's XRP is brought below the least the hub's crate fixes, as
//! the fees of a long life would bring it, and a member's withdrawal first
//! tops it up from its XRP book, and goes only once that applied. Then
//! someone sends the escrow XRP, as anyone may: it is gas, and credits no
//! account. It also lifts the escrow above the reserve of its signer list
//! and trust line, which the least gas does not cover and the payout set's
//! change of signer list needs (spec/roadmap.md#token-hubs). Then the hub
//! loses f + 1 validators and stalls, and its escrow pays out the final
//! balances in DSUSD.
//!
//! Each node is judged by its own books (spec/decisions/own-books.md),
//! throughout: they hold what its schemes hold, its XRP and DSUSD books
//! exactly its account's XRP less the reserve and its DSUSD line, and in
//! the hub what its account and its peers' holds toward it hold; its
//! owner's DSUSD moved by what its receivers took. The escrow holds in
//! DSUSD what the hub owes, but for what is on its way between them.

use std::collections::BTreeMap;

use dassie_hub::consensus::MAX_VIEW_TIMEOUT_MS;
use dassie_hub::escrow::PayoutTransaction;
use dassie_hub::member::{GAS_CHECK_LASTS_MS, LEAST_ESCROW_GAS_DROPS};
use dassie_hub::rules::Asset;
use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::api;
use dassie_xrpl::codec::Amount;

use super::checks::{check_events, check_funded, check_held, check_incoming, first_failure};
use super::hub::owed;
use super::scripted::{READY_WITHIN_MS, ended};
use super::{DSUSD, HUB_DSUSD, START, Settling, Sim, XRP, key, received, xrpl_account};
use crate::engine::EngineError;
use crate::engine::core::Direction;

/// How long a move may be on its way before it is late
/// (`CREDIT_WAIT_MS`, `engine/core/hub_funding.rs`).
const CREDIT_WAIT_MS: u64 = 600_000;

/// What the run's hub holds: DSUSD, of its issuer.
pub const ASSET: Asset = Asset::Token {
    currency: dassie_xrpl::dsusd::CURRENCY,
    issuer: dassie_xrpl::dsusd::ISSUER,
};

/// How long the members may take to join the hub: as long as a move into
/// it may take before it is late (`CREDIT_WAIT_MS`,
/// `engine/core/hub_funding.rs`), and the issuer's mint first, once each
/// member's DSUSD line exists.
const JOINED_WITHIN_MS: u64 = 2 * CREDIT_WAIT_MS;

/// How long the payments may take, at production timings.
const PAID_WITHIN_MS: u64 = 10 * 60_000;

/// How long the network is left calm before the books are checked.
const CALM_MS: u64 = 60_000;

/// How long the network may then take to settle down: a peer short of room
/// is asked again once a minute, and may have to move funds into the hub
/// before it can settle, which is late only after `CREDIT_WAIT_MS`.
const SETTLED_WITHIN_MS: u64 =
    crate::node::Policies::DEFAULT.settlement_request_every_ms + CREDIT_WAIT_MS;

/// How long an owner's withdrawal may wait for the escrow's gas: a read at
/// the XRP account's anchor, a top-up, and the ledger that applies it,
/// each a few seconds; a top-up that waits for another payment of the XRP
/// book to apply first; and the check's first backoff once if one failed.
const GAS_WITHIN_MS: u64 = 5 * 60_000;

/// What someone sends the escrow, in drops: gas, never a deposit.
const GIFT_DROPS: u64 = 5_000_000;

/// The owner's withdrawal once the escrow's XRP fell, as the ledger stood
/// when the node took it.
struct Asked {
    /// The escrow's last transaction's ledger once the node took the
    /// withdrawal: the escrow pays it in a later one.
    asked_after: u32,
}

/// What a seed draws.
struct Setup {
    validators: usize,
    members: usize,
    /// The member that withdraws once the escrow's XRP is low.
    withdrawer: usize,
}

fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ 0x6875_622d_6473_7573);
    let validators = 4 + usize::try_from(rng.below(4)).unwrap();
    let members = 3 + usize::try_from(rng.below(2)).unwrap();
    let withdrawer = usize::try_from(rng.below(members as u64)).unwrap();
    (
        rng,
        Setup {
            validators,
            members,
            withdrawer,
        },
    )
}

/// What a seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    format!(
        "{} members, {} validators, node {} withdraws once the escrow's XRP is low",
        setup.members, setup.validators, setup.withdrawer
    )
}

/// A run over a DSUSD hub.
#[must_use]
pub fn run(seed: u64) -> Run {
    let (mut rng, setup) = setup(seed);
    let mut sim = Sim::with(
        seed,
        setup.members,
        0,
        BTreeMap::new(),
        Settling::HubDsusd(setup.validators),
    );
    let result = script(&mut sim, &mut rng, &setup);
    ended(sim, result)
}

/// Whether every member has its account in the hub, in step with it.
#[must_use]
pub fn joined(sim: &Sim) -> bool {
    (0..sim.nodes.len()).all(|index| {
        sim.nodes[index]
            .core
            .as_ref()
            .and_then(crate::engine::Core::hub_status)
            .is_some_and(|status| {
                status.account.is_some() && status.status == dassie_settlement::LedgerStatus::Synced
            })
    })
}

/// Whether every node has a peer and a route to every other node.
fn routed(sim: &Sim) -> bool {
    let count = sim.nodes.len();
    (0..count).all(|index| {
        let network = sim.core(index).network();
        !network.peers.is_empty()
            && (0..count).filter(|other| *other != index).all(|other| {
                let id = key(other).node_id();
                network
                    .nodes
                    .iter()
                    .any(|node| node.id == id && !node.first_hops.is_empty())
            })
    })
}

fn script(sim: &mut Sim, rng: &mut SeededRng, setup: &Setup) -> Result<(), Outcome> {
    if !sim.run_until_or(START + JOINED_WITHIN_MS, joined) {
        return Err(Outcome::liveness(
            "joined",
            format!(
                "the members did not all join the DSUSD hub in time: {}",
                standing(sim)
            ),
        ));
    }
    sim.count("members joined the DSUSD hub");
    // Nodes 0 and 2 never peer, so what one pays the other crosses a hop.
    sim.step(0, |core| core.ban_node(key(2).node_id()).unwrap());
    sim.step(2, |core| core.ban_node(key(0).node_id()).unwrap());
    sim.join_network(1);
    if !sim.run_until_or(sim.now + READY_WITHIN_MS, routed) {
        return Err(Outcome::liveness(
            "ready",
            "the network was not ready in time",
        ));
    }
    over_hub(sim)?;
    pay(sim, rng);
    sim.run_until_or(sim.now + PAID_WITHIN_MS, |sim| {
        sim.payments.iter().all(|payment| payment.done)
    });
    let asked = withdraw_low(sim, setup)?;
    // The least gas a member tops the escrow up to is below the reserve
    // of the escrow's signer list and trust line, which the payout set's
    // change of signer list needs: someone sends the escrow more, as
    // anyone may (spec/roadmap.md#token-hubs).
    gift(sim, setup);
    settle(sim);
    check_topped_up(sim, setup.withdrawer, asked)?;
    check_calm(sim)?;
    stall(sim, rng)
}

/// Every peering is over the DSUSD hub: the owner books peer with nobody.
fn over_hub(sim: &Sim) -> Result<(), Outcome> {
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            for method in &peer.peering.methods {
                let scheme = method.scheme;
                if scheme.as_str() != dassie_hub::member::DSUSD_SCHEME_ID {
                    return Err(Outcome::liveness(
                        "over-hub",
                        format!(
                            "node {index} peers with {} over {scheme:?}, not the hub",
                            peer.id
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Payments across the hop between nodes 0 and 2, both ways, and one
/// between two nodes drawn: each of up to a few dollars, several times what
/// a hold holds, so that the holds are funded again and again.
fn pay(sim: &mut Sim, rng: &mut SeededRng) {
    let count = sim.nodes.len();
    let amount = |rng: &mut SeededRng| 100_000_000 + rng.below(3_300_000_000);
    let there = amount(rng);
    sim.pay(0, 2, there, None, None);
    let back = amount(rng);
    sim.pay(2, 0, back, None, None);
    let from = usize::try_from(rng.below(count as u64)).unwrap();
    let to = (from + 1 + usize::try_from(rng.below(count as u64 - 1)).unwrap()) % count;
    let other = amount(rng);
    sim.pay(from, to, other, None, None);
}

/// Someone sends the escrow XRP, tagged with a member's account as a
/// deposit would be: it is gas, and credits no account
/// (spec/decisions/token-hubs.md).
fn gift(sim: &mut Sim, setup: &Setup) {
    let hub = sim.hub.as_deref().unwrap();
    let escrow = hub.escrow();
    let member = dassie_hub::member::member_key(&key(setup.withdrawer), &hub.id()).public_key();
    let tag = hub
        .ledger()
        .and_then(|ledger| ledger.account_of(&member))
        .map(|number| number.0);
    sim.ledger.deposit_tagged(escrow, GIFT_DROPS, tag);
    sim.count("XRP sent to the escrow");
    sim.trace.push(format!(
        "{} someone sends the escrow {GIFT_DROPS} drops of XRP, tagged {tag:?}",
        sim.now
    ));
}

/// The escrow's XRP brought below the least gas, and node `withdrawer`'s
/// owner withdrawing: the node tops the escrow up from its XRP book before
/// it asks the hub, and the escrow pays the withdrawal only after the
/// top-up applied (spec/decisions/token-hubs.md).
///
/// Checks that the members' top-ups since it fell made up the escrow's
/// shortfall before the node took the withdrawal (the escrow's last
/// transaction before the fall bounds what is since: any in a later
/// ledger came after), and returns where the ledger stood when the node
/// took it.
fn withdraw_low(sim: &mut Sim, setup: &Setup) -> Result<Asked, Outcome> {
    let index = setup.withdrawer;
    let hub = sim.hub.as_deref().unwrap();
    let escrow = hub.escrow();
    let least = hub.rules.terms.fees.least_payout;
    let units = dassie_hub::member::UNITS_PER_DROP;
    let low = LEAST_ESCROW_GAS_DROPS / 2;
    let read = |problem| Outcome::safety("gas", problem);
    let fell_after = last_ledger(&escrow_transactions(sim, escrow, 1, u32::MAX).map_err(read)?);
    let held = sim.ledger.balance(escrow).unwrap_or(0);
    sim.ledger.take_unrecorded(escrow, held.saturating_sub(low));
    sim.trace
        .push(format!("{} the escrow's XRP falls to {low} drops", sim.now));
    // It fell over a long life, in truth: no check a node made before it
    // fell still holds when the owner asks.
    sim.run_until(sim.now + GAS_CHECK_LASTS_MS);
    // Enough in the hub for the least payout, whatever the node's own
    // moves keep there and its settlements take meanwhile: the owner
    // deposits some first.
    let deposited = |sim: &Sim| {
        sim.core(index)
            .hub_status()
            .is_some_and(|status| status.moving.is_none() && status.balance > 2 * least * units)
    };
    if !deposited(sim) {
        let mut asked = Ok(());
        sim.run_until_or(sim.now + GAS_WITHIN_MS, |sim| {
            sim.core(index)
                .hub_status()
                .is_some_and(|status| status.moving.is_none())
        });
        sim.step(index, |core| {
            asked = core.hub_deposit(5 * least * units).map(|_| ());
        });
        if let Err(error) = asked {
            return Err(Outcome::liveness(
                "gas",
                format!("node {index}'s owner could not deposit: {error}"),
            ));
        }
        if !sim.run_until_or(sim.now + GAS_WITHIN_MS, deposited) {
            return Err(Outcome::liveness(
                "gas",
                format!("node {index}'s deposit did not land: {}", standing(sim)),
            ));
        }
    }
    // The owner asks until the node has checked the gas, and topped it
    // up: each ask before is refused, to try again shortly, as is one while
    // the account reads the hub behind it.
    let deadline = sim.now + GAS_WITHIN_MS;
    let mut refused = 0u32;
    loop {
        let mut asked = Ok(());
        sim.step(index, |core| {
            asked = core.hub_withdraw(least * units).map(|_| ());
        });
        match asked {
            Ok(()) => break,
            Err(EngineError::Exhausted(_) | EngineError::Precondition(_)) if sim.now < deadline => {
                refused += 1;
                sim.run_until(sim.now + 2_000);
            }
            Err(error) => {
                return Err(Outcome::liveness(
                    "gas",
                    format!(
                        "node {index}'s withdrawal was refused {refused} times, then: {error}; {}",
                        standing(sim)
                    ),
                ));
            }
        }
    }
    // The node took the withdrawal only once the escrow had its gas back:
    // the members' top-ups since it fell, which applied before, make up
    // its shortfall. The withdrawer's own is only what others' left short.
    let records = escrow_transactions(sim, escrow, 1, u32::MAX).map_err(read)?;
    let asked_after = last_ledger(&records);
    let topped = topped_up(sim, &records, fell_after);
    let short = LEAST_ESCROW_GAS_DROPS - low;
    if topped < short {
        return Err(Outcome::safety(
            "gas",
            format!(
                "node {index} took its owner's withdrawal once the members had topped the \
                 escrow up by {topped} drops since it fell, {short} short"
            ),
        ));
    }
    let done = |sim: &Sim| {
        sim.core(index)
            .hub_status()
            .is_some_and(|status| status.moving.is_none())
    };
    if !sim.run_until_or(sim.now + GAS_WITHIN_MS, done) {
        return Err(Outcome::liveness(
            "gas",
            format!("node {index}'s withdrawal did not land: {}", standing(sim)),
        ));
    }
    Ok(Asked { asked_after })
}

/// The newest ledger of `records`, or 0 for none.
fn last_ledger(records: &[api::TxRecord]) -> u32 {
    records
        .iter()
        .map(|record| record.ledger_index)
        .max()
        .unwrap_or(0)
}

/// The XRP the members paid the escrow in `records` after ledger `after`,
/// in drops: their top-ups.
fn topped_up(sim: &Sim, records: &[api::TxRecord], after: u32) -> u64 {
    let escrow = sim.hub.as_deref().unwrap().escrow();
    let members: Vec<dassie_xrpl::AccountId> = (0..sim.nodes.len()).map(xrpl_account).collect();
    records
        .iter()
        .filter(|record| members.contains(&record.account) && record.succeeded())
        .filter(|record| record.destination == Some(escrow))
        .filter(|record| record.ledger_index > after)
        .filter_map(|record| record.delivered)
        .sum()
}

/// The escrow paid node `index` the withdrawal its owner asked for once
/// the escrow's XRP fell, after the node took it, which it took only once
/// the members' top-ups since the fall made up its shortfall
/// ([`withdraw_low`]): its first payment to the node after then, in DSUSD.
fn check_topped_up(sim: &mut Sim, index: usize, asked: Asked) -> Result<(), Outcome> {
    let escrow = sim.hub.as_deref().unwrap().escrow();
    let records = escrow_transactions(sim, escrow, 1, u32::MAX)
        .map_err(|problem| Outcome::safety("gas", problem))?;
    let to = xrpl_account(index);
    let Some(paid) = records
        .iter()
        .filter(|record| record.account == escrow && record.destination == Some(to))
        .filter(|record| record.succeeded() && record.ledger_index > asked.asked_after)
        .min_by_key(|record| (record.ledger_index, record.index))
    else {
        return Err(Outcome::safety(
            "gas",
            format!("the escrow paid node {index} no withdrawal after its owner asked"),
        ));
    };
    if paid.delivered_token.is_none() {
        return Err(Outcome::safety(
            "gas",
            format!("the escrow paid node {index}'s withdrawal in something else than DSUSD"),
        ));
    }
    sim.count("escrow gas topped up before a withdrawal");
    Ok(())
}

/// The network calms down: every move done, every account in step, every
/// peer that was asked settled.
fn settle(sim: &mut Sim) {
    sim.run_until(sim.now + CALM_MS);
    let settled =
        |sim: &Sim| (0..sim.nodes.len()).all(|index| !moving(sim, index) && in_step(sim, index));
    sim.run_until_or(sim.now + SETTLED_WITHIN_MS, settled);
    // The last settlement may have executed a block before the accounts
    // read it: a while of quiet, then the accounts read the hub again.
    sim.run_until(sim.now + CALM_MS);
    sim.run_until_or(sim.now + CALM_MS, settled);
}

/// Whether node `index` has a move of its funds into or out of the hub on
/// its way.
fn moving(sim: &Sim, index: usize) -> bool {
    sim.core(index)
        .hub_status()
        .is_some_and(|status| status.moving.is_some())
}

/// Whether node `index`'s hub account is in step with the hub.
fn in_step(sim: &Sim, index: usize) -> bool {
    sim.core(index)
        .hub_status()
        .is_some_and(|status| status.status == dassie_settlement::LedgerStatus::Synced)
}

/// How the hub's members stand, for a failure's message.
fn standing(sim: &Sim) -> String {
    let members: Vec<String> = (0..sim.nodes.len())
        .map(|index| {
            let status = sim.nodes[index]
                .core
                .as_ref()
                .and_then(crate::engine::Core::hub_status);
            format!(
                "node {index}: {:?}, DSUSD line {:?}, XRP {:?}",
                status.map(|status| (status.account, status.status, status.balance, status.moving)),
                sim.line(index),
                sim.ledger.balance(xrpl_account(index))
            )
        })
        .collect();
    let hub = sim.hub.as_deref().unwrap();
    format!(
        "{members:?}; the escrow holds {:?} drops and {:?} DSUSD",
        sim.ledger.balance(hub.escrow()),
        escrow_tokens(sim)
    )
}

/// What the escrow holds of DSUSD, in millionths.
fn escrow_tokens(sim: &Sim) -> u64 {
    let escrow = sim.hub.as_deref().unwrap().escrow();
    sim.ledger
        .token_balance(escrow, IssuedAsset::DSUSD)
        .and_then(|value| value.floor_units(6))
        .and_then(|micros| u64::try_from(micros).ok())
        .unwrap_or(0)
}

/// Everything checked once the network is calm, every check run, a safety
/// failure first.
fn check_calm(sim: &Sim) -> Result<(), Outcome> {
    let incoming = sim
        .payments
        .iter()
        .map(|payment| check_incoming(sim, payment));
    first_failure(
        [
            check_events(sim).map_err(|problem| Outcome::safety("events", problem)),
            check_validators(sim).map_err(|problem| Outcome::safety("hub-agreement", problem)),
            super::checks::check_hub_packets(sim)
                .map_err(|problem| Outcome::safety("hub-packets", problem)),
            super::checks::check_hub_credit(sim)
                .map_err(|problem| Outcome::safety("hub-credit", problem)),
            check_books(sim).map_err(|problem| {
                Outcome::safety("hub-books", format!("{problem}; {}", standing(sim)))
            }),
            check_owners(sim).map_err(|problem| Outcome::safety("hub-owner-books", problem)),
            check_escrow(sim).map_err(|problem| Outcome::safety("hub-escrow", problem)),
            check_funded(sim).map_err(|problem| Outcome::liveness("funded", problem)),
            (0..sim.nodes.len())
                .find(|index| !in_step(sim, *index) || moving(sim, *index))
                .map_or(Ok(()), |index| {
                    Err(Outcome::liveness(
                        "hub-in-step",
                        format!(
                            "node {index} is behind the hub or moving: {}",
                            standing(sim)
                        ),
                    ))
                }),
            super::checks::check_finished(sim)
                .map_err(|problem| Outcome::liveness("finished", problem)),
        ]
        .into_iter()
        .chain(incoming)
        .collect::<Vec<_>>(),
    )
}

/// The validators broke nothing: no two honest ones finalized different
/// states at one height, and their invariants held after every step.
fn check_validators(sim: &Sim) -> Result<(), String> {
    match sim.hub.as_deref().unwrap().breaches.as_slice() {
        [] => Ok(()),
        breaches => Err(breaches.join("; ")),
    }
}

/// Each node's books hold what its schemes hold: its XRP book its
/// account's XRP less the reserve, its DSUSD book its line, and in the hub
/// what its account and its peers' holds toward it hold, as the honest
/// validators have it, once it read the operations of its own they
/// applied.
fn check_books(sim: &Sim) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    for index in 0..sim.nodes.len() {
        check_held(sim, index, XRP)?;
        check_held(sim, index, DSUSD)?;
        let booked = check_held(sim, index, HUB_DSUSD)?;
        if !in_step(sim, index) {
            continue;
        }
        let member = dassie_hub::member::member_key(&key(index), &hub.id()).public_key();
        let number = ledger
            .account_of(&member)
            .ok_or_else(|| format!("node {index} has no account in the hub"))?;
        let account = ledger.account(number).unwrap();
        let read = sim.core(index).hub_sequence();
        if read != Some(account.sequence) {
            let cursor = sim
                .core(index)
                .hub_reading()
                .map_or(0, |(cursor, _, _)| cursor);
            if cursor < ledger.height() {
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

/// Each owner's DSUSD, in its book and in the hub together, moved by what
/// its receivers took: what the issuer minted, less the hub's account
/// reserve, which no one can spend. And each payment's token paid what it
/// sent.
fn check_owners(sim: &Sim) -> Result<(), String> {
    let reserve = i128::from(dassie_hub::member::units(
        sim.hub.as_deref().unwrap().rules.terms.fees.account_reserve,
    ));
    let done = |index: &usize| in_step(sim, *index) && !moving(sim, *index);
    for index in (0..sim.nodes.len()).filter(done) {
        let owner = |ledger| {
            sim.balances_on(index, ledger)
                .get("equity/owner")
                .copied()
                .unwrap_or(0)
        };
        let pool = owner(DSUSD) + owner(HUB_DSUSD);
        let (minted, clawed) = sim.issued.get(&index).copied().unwrap_or((0, 0));
        let deposit = minted - clawed - reserve;
        let received: i128 = sim
            .payments
            .iter()
            .chain(&sim.abandoned)
            .filter(|payment| payment.to == index)
            .map(|payment| i128::from(received(sim, payment)))
            .sum();
        if pool - deposit != received {
            return Err(format!(
                "node {index}'s owner gained {} for {received} received; its books: {:?} and {:?}",
                pool - deposit,
                sim.balances_on(index, DSUSD),
                sim.balances_on(index, HUB_DSUSD)
            ));
        }
    }
    for payment in &sim.payments {
        let index = payment.from;
        let account = format!("equity/owner/btp/{}", payment.token.0);
        let paid = -sim
            .balances_on(index, HUB_DSUSD)
            .get(&account)
            .copied()
            .unwrap_or(0);
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

/// The escrow holds in DSUSD what the hub owes, but for what is on its way
/// between them: a deposit made and not yet credited, or a withdrawal the
/// hub queued that the escrow paid and a quorum has not attested yet. The
/// XRP sent to it credits no account, so the hub owes no more than the
/// DSUSD it holds. And the fake ledger made or lost no XRP and no DSUSD.
fn check_escrow(sim: &Sim) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    let depositing: u64 = (0..sim.nodes.len())
        .filter_map(|index| sim.core(index).hub_status()?.moving)
        .filter(|moving| moving.direction == Direction::Deposit)
        .map(|moving| moving.drops)
        .sum();
    let withdrawing: u64 = ledger
        .withdrawals()
        .map(|(_, withdrawal)| withdrawal.amount + withdrawal.ledger_fee)
        .sum();
    let escrow = escrow_tokens(sim);
    let owed = ledger.supply();
    if escrow + withdrawing < owed || escrow > owed + depositing {
        return Err(format!(
            "the escrow holds {escrow} millionths of DSUSD, while the hub owes {owed}, with \
             {depositing} on its way in and {withdrawing} queued out"
        ));
    }
    if sim.ledger.total() != dassie_xrpl::MAX_DROPS {
        return Err("the fake ledger made or lost XRP".into());
    }
    if !sim.ledger.tokens_conserved() {
        return Err("the fake ledger made or lost DSUSD".into());
    }
    Ok(())
}

/// The hub loses f + 1 honest validators at once and stalls: each still up
/// declares it, and the lost ones come back one by one until a quorum
/// signed one payout set, which the escrow pays out in DSUSD, exactly the
/// final balances. Each member's books then hold its DSUSD line.
fn stall(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let hub = sim.hub.as_deref().unwrap();
    let lost_count = hub.faults() + 1;
    let stall_ms = hub.rules.terms.timers.stall;
    let mut honest: Vec<usize> = hub.honest().collect();
    let mut lost = Vec::new();
    while lost.len() < lost_count {
        let at = usize::try_from(rng.below(honest.len() as u64)).unwrap();
        lost.push(honest.remove(at));
    }
    sim.trace
        .push(format!("{} the hub loses validators {lost:?}", sim.now));
    for &index in &lost {
        sim.crash_validator(index);
    }
    let live = honest.clone();
    let declared = |sim: &Sim| {
        live.iter()
            .all(|index| sim.hub.as_deref().unwrap().validators[*index].stalled())
    };
    let wait = stall_ms + MAX_VIEW_TIMEOUT_MS;
    if !sim.run_until_or(sim.now + wait, declared) {
        return Err(Outcome::liveness(
            "stalled",
            "a validator still up did not declare the stall in time",
        ));
    }
    sim.count("hubs stalled");
    for index in lost {
        sim.run_until(sim.now + rng.below(stall_ms));
        sim.start_validator(index);
        let gathered = |sim: &Sim| sim.hub.as_deref().unwrap().payout.is_some();
        if sim.run_until_or(sim.now + wait, gathered) {
            break;
        }
    }
    if sim.hub.as_deref().unwrap().payout.is_none() {
        return Err(Outcome::liveness(
            "payout",
            "no quorum signed one payout set",
        ));
    }
    let applied = |sim: &Sim| {
        sim.hub
            .as_deref()
            .unwrap()
            .payout
            .as_ref()
            .is_some_and(|payout| payout.escrow_after.is_some())
    };
    if !sim.run_until_or(sim.now + 10 * super::LEDGER_CLOSE_MS, applied) {
        return Err(Outcome::liveness(
            "payout",
            "the payout set was not applied in time",
        ));
    }
    check_payout(sim).map_err(|problem| Outcome::safety("payout", problem))?;
    // Each member's DSUSD book reads what the payout paid it as its
    // owner's deposit, and holds its line.
    sim.run_until(sim.now + CALM_MS);
    for index in 0..sim.nodes.len() {
        check_held(sim, index, DSUSD).map_err(|problem| Outcome::safety("paid-out", problem))?;
        check_held(sim, index, XRP).map_err(|problem| Outcome::safety("paid-out", problem))?;
    }
    Ok(())
}

/// The escrow paid out exactly the final balances of the block the set is
/// of, in DSUSD: each account with somewhere to go and at least the least
/// payout received what the hub owed it, worked out apart from the set, and
/// every transaction of the set applied.
fn check_payout(sim: &mut Sim) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let payout = hub.payout.as_ref().unwrap();
    let ledger = &payout.ledger;
    let fees = ledger.terms().fees;
    let terms = ledger.terms().clone();
    let hashes = payout.hashes.clone();
    let transactions = payout.set.transactions.clone();
    let mut expected: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    for (number, account) in ledger.accounts() {
        let Some(disbursal) = account.disbursal else {
            continue;
        };
        let total = owed(ledger, number);
        if total >= fees.least_payout {
            expected.insert(dassie_xrpl::AccountId(disbursal.account), total);
        }
    }
    let mut delivered: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    for (hash, transaction) in hashes.iter().zip(&transactions) {
        let record = transaction_record(sim, hash)
            .ok_or_else(|| format!("{transaction:?} was never applied"))?;
        if record.result != "tesSUCCESS" {
            return Err(format!("{transaction:?} ended {}", record.result));
        }
        if let PayoutTransaction::Payout(payment) = transaction {
            let Amount::Token(_) = payment.amount else {
                return Err("a payout of something else than DSUSD".into());
            };
            let named = terms
                .hub_amount(&payment.amount)
                .ok_or("a payout of another token")?;
            let got = record
                .delivered_token
                .and_then(|token| terms.hub_amount(&Amount::Token(token)));
            if got != Some(named) {
                return Err(format!("a payout of {named} delivered {got:?}"));
            }
            *delivered.entry(payment.destination).or_default() += named;
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
    sim.count("DSUSD payouts checked");
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
