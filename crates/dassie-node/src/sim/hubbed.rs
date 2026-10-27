//! The node simulation's runs over a settlement hub (`node-hub`,
//! spec/areas/testing.md#the-simulations): members peered over a hub of 4 to
//! 7 validators run in the simulation, its escrow on the fake XRP Ledger,
//! pay each other across hops, while up to f validators are faulty, a
//! validator restarts, and a member may lie. Then the hub loses f + 1
//! validators and stalls, and its escrow pays out the final balances once a
//! quorum signed one payout set, over time; or, one run in ten, f + 1
//! validators collude and fork the chain, which must leave evidence.
//!
//! Each node is judged by its own books (spec/decisions/own-books.md): its
//! books hold what its schemes hold, on the XRP Ledger and in the hub, its
//! owner's funds moved by what its receivers took, and what it booked as
//! lost on a peer over the hub is never more than the holds between them
//! (spec/designs/settlement-hubs.md#13-how-it-fits-the-node).

use std::collections::BTreeMap;

use dassie_hub::consensus::MAX_VIEW_TIMEOUT_MS;
use dassie_hub::escrow::PayoutTransaction;
use dassie_hub::sim::Role;
use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_xrpl::api;
use dassie_xrpl::codec::Amount;
use dassie_xrpl::fake::FAUCET_DROPS;

use super::checks::{check_events, check_held, check_incoming, first_failure};
use super::hub::{MemberLie, owed, terms};
use super::scripted::{READY_WITHIN_MS, ended};
use super::{HUB, START, Settling, Sim, XRP, key, received, xrpl_account};

/// How long the members may take to join the hub: as long as a move into
/// it may take before it is late (`CREDIT_WAIT_MS`,
/// `engine/core/hub_funding.rs`).
const JOINED_WITHIN_MS: u64 = 600_000;

/// How long the payments may take, after the validator's restart, at
/// production timings.
const PAID_WITHIN_MS: u64 = 10 * 60_000;

/// How long the network is left calm before the books are checked: no
/// payment, no move, nothing on its way.
const CALM_MS: u64 = 60_000;

/// How long the network may then take to settle down: a peer short of
/// room is asked again once a minute, and may have to move funds into
/// the hub before it can settle, which is late only after
/// `CREDIT_WAIT_MS` (`engine/core/hub_funding.rs`).
const SETTLED_WITHIN_MS: u64 =
    crate::node::Policies::DEFAULT.settlement_request_every_ms + JOINED_WITHIN_MS;

/// How a run ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// The hub loses f + 1 validators and stalls; the escrow pays out.
    Stall,
    /// f + 1 validators collude and fork the chain.
    Collusion,
}

/// What a seed draws.
struct Setup {
    validators: usize,
    members: usize,
    roles: Vec<Role>,
    liar: Option<MemberLie>,
    /// An honest validator that crashes while the payments are under way.
    restart: Option<usize>,
    ending: Ending,
}

fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ 0x6e6f_6465_2d68_7562);
    let validators = 4 + usize::try_from(rng.below(4)).unwrap();
    let faults = (validators - 1) / 3;
    let members = 3 + usize::try_from(rng.below(2)).unwrap();
    let ending = if rng.below(10) == 0 {
        Ending::Collusion
    } else {
        Ending::Stall
    };
    let kinds = [
        Role::Silent,
        Role::Withhold,
        Role::Invalid,
        Role::Equivocate,
        Role::Partial,
    ];
    let drawn = usize::try_from(rng.below(faults as u64 + 1)).unwrap();
    let byzantine: Vec<Role> = (0..drawn)
        .map(|_| kinds[usize::try_from(rng.below(kinds.len() as u64)).unwrap()])
        .collect();
    let roles = dassie_hub::sim::roles(validators, &byzantine);
    let margin = terms(validators).timers.margin;
    let liar = match rng.below(4) {
        0 => None,
        1 => Some(MemberLie::NoClaims),
        2 => Some(MemberLie::ClaimsLate(rng.below(margin))),
        _ => Some(MemberLie::ClaimsTwice),
    };
    let honest = validators - drawn;
    let restart = (rng.below(2) == 0).then(|| usize::try_from(rng.below(honest as u64)).unwrap());
    let setup = Setup {
        validators,
        members,
        roles,
        liar,
        restart,
        ending,
    };
    (rng, setup)
}

/// What a seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    let byzantine: Vec<&Role> = setup
        .roles
        .iter()
        .filter(|role| !role.is_honest())
        .collect();
    format!(
        "{} members, {} validators, faulty {byzantine:?}, liar {:?}, restart {:?}, ending {:?}",
        setup.members, setup.validators, setup.liar, setup.restart, setup.ending
    )
}

/// A run over a hub.
#[must_use]
pub fn run(seed: u64) -> Run {
    let (mut rng, setup) = setup(seed);
    let mut sim = Sim::with(
        seed,
        setup.members,
        0,
        BTreeMap::new(),
        Settling::Hub(setup.validators),
    );
    let result = script(&mut sim, &mut rng, &setup);
    ended(sim, result)
}

/// How the hub and its members stand, for a failure's message: each
/// validator's last final block, view, and whether a quorum checked the
/// escrow, and each member's account and how it stands.
fn hub_standing(sim: &Sim) -> String {
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

/// Whether the network is ready for payments: every node has a peer and a
/// route to every other node.
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

/// Whether every payment has finished.
fn paid(sim: &Sim) -> bool {
    sim.payments.iter().all(|payment| payment.done)
}

fn script(sim: &mut Sim, rng: &mut SeededRng, setup: &Setup) -> Result<(), Outcome> {
    for (index, role) in setup.roles.iter().enumerate() {
        sim.hub.as_deref_mut().unwrap().validators[index].role = *role;
    }
    sim.trace
        .push(format!("{} the validators are {:?}", sim.now, setup.roles));
    if !sim.run_until_or(START + JOINED_WITHIN_MS, joined) {
        return Err(Outcome::liveness(
            "joined",
            format!(
                "the members did not all join the hub in time: {}",
                hub_standing(sim)
            ),
        ));
    }
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
    if let Some(lie) = setup.liar {
        sim.set_liar(1, lie);
    }
    pay(sim, rng);
    if let Some(validator) = setup.restart {
        sim.run_until(sim.now + rng.below(5_000));
        sim.crash_validator(validator);
        sim.run_until(sim.now + 5_000 + rng.below(55_000));
        sim.start_validator(validator);
    }
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    sim.run_until(sim.now + CALM_MS);
    // Each account reads what touched it a few blocks after the hub
    // finalized it, a node moves funds into or out of the hub as its
    // floor and ceiling call for, and a peer short of room settles once
    // asked: the books are checked once all of that is done.
    let liar = setup.liar.map(|_| 1);
    let settled = |sim: &Sim| {
        (0..sim.nodes.len())
            .filter(|index| Some(*index) != liar)
            .all(|index| !moving(sim, index))
            && (0..sim.nodes.len())
                .filter(|index| Some(*index) != liar)
                .all(|index| in_step(sim, index))
            && check_settled_with_honest(sim, liar).is_ok()
    };
    sim.run_until_or(sim.now + SETTLED_WITHIN_MS, settled);
    // The last settlement may have executed a block before the accounts
    // read it: a while of quiet, then the accounts read the hub again.
    sim.run_until(sim.now + CALM_MS);
    sim.run_until_or(sim.now + CALM_MS, settled);
    check_calm(sim, setup.liar.map(|lie| (1, lie)))?;
    match setup.ending {
        Ending::Stall => stall(sim, rng),
        Ending::Collusion => collude(sim),
    }
}

/// Every peering is over the hub: both peers are members, and take peers
/// there (spec/designs/settlement-hubs.md#11-choosing-a-hub).
fn over_hub(sim: &Sim) -> Result<(), Outcome> {
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            if peer.peering.scheme.as_str() != dassie_hub::member::SCHEME_ID {
                return Err(Outcome::liveness(
                    "over-hub",
                    format!(
                        "node {index} peers with {} over {:?}, not the hub",
                        peer.id, peer.peering.scheme
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Payments across the hop between nodes 0 and 2, both ways, and one
/// between two nodes drawn: each of up to some times what a hold holds,
/// so that the holds are funded again and again.
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

/// Everything checked once the network is calm, every check run, a safety
/// failure first.
///
/// A member that keeps its Fulfills or claims what its payers' claims
/// cover may stop the payments that cross it: of those, only what must
/// never happen is checked. A payer that sends no claims stops nothing,
/// since its payees claim on the hub (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
fn check_calm(sim: &Sim, liar: Option<(usize, MemberLie)>) -> Result<(), Outcome> {
    let stops = liar.is_some_and(|(_, lie)| lie != MemberLie::NoClaims);
    let incoming = sim
        .payments
        .iter()
        .map(|payment| check_incoming(sim, payment))
        .map(|result| match result {
            Err(Outcome::Liveness(_)) if stops => Ok(()),
            other => other,
        });
    let finished = if stops {
        Ok(())
    } else {
        super::checks::check_finished(sim).map_err(|problem| Outcome::liveness("finished", problem))
    };
    first_failure(
        [
            check_events(sim).map_err(|problem| Outcome::safety("events", problem)),
            check_hub_validators(sim).map_err(|problem| Outcome::safety("hub-agreement", problem)),
            super::checks::check_hub_packets(sim)
                .map_err(|problem| Outcome::safety("hub-packets", problem)),
            super::checks::check_hub_credit(sim)
                .map_err(|problem| Outcome::safety("hub-credit", problem)),
            check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
            check_hub_books(sim, liar.map(|(node, _)| node)).map_err(|problem| {
                Outcome::safety("hub-books", format!("{problem}; {}", hub_standing(sim)))
            }),
            check_owners(sim).map_err(|problem| Outcome::safety("hub-owner-books", problem)),
            check_escrow(sim).map_err(|problem| Outcome::safety("hub-escrow", problem)),
            check_in_step(sim, liar.map(|(node, _)| node))
                .map_err(|problem| Outcome::liveness("hub-in-step", problem)),
            check_moved(sim, liar.map(|(node, _)| node))
                .map_err(|problem| Outcome::liveness("hub-moves", problem)),
            finished,
            check_settled_with_honest(sim, liar.map(|(node, _)| node))
                .map_err(|problem| Outcome::liveness("settled", problem)),
        ]
        .into_iter()
        .chain(incoming)
        .collect::<Vec<_>>(),
    )
}

/// Once the network is calm, no honest node's books show an honest peer
/// still at its ask point: asked, the peer settled
/// (spec/areas/settlement.md#asking-a-peer-to-settle). A liar's settling is
/// its own.
fn check_settled_with_honest(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    for index in (0..sim.nodes.len()).filter(|index| Some(*index) != liar) {
        for peer in sim.core(index).network().peers {
            if liar.is_some_and(|liar| key(liar).node_id() == peer.id) {
                continue;
            }
            let Some(owed) = peer.owed.filter(|_| peer.termination.is_none()) else {
                continue;
            };
            let credit = peer.peering.credit_out;
            let position = u128::try_from(owed + i128::from(credit)).unwrap_or(0);
            if dassie_settlement::policy::ask(position, super::TRIGGER) {
                return Err(format!(
                    "node {index}'s room for {} is {position} of {credit}: it never settled",
                    peer.id
                ));
            }
        }
    }
    Ok(())
}

/// The validators broke nothing: their invariants held after every step,
/// no two honest ones finalized different states at one height, and no
/// evidence named an honest one.
fn check_hub_validators(sim: &Sim) -> Result<(), String> {
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
fn check_losses(sim: &Sim) -> Result<(), String> {
    for ((index, peer), lost) in &sim.hub_losses {
        let (toward, from) = sim
            .hub_most_held
            .get(&(*index, peer.clone()))
            .copied()
            .unwrap_or((0, 0));
        if *lost > toward.saturating_add(from) {
            return Err(format!(
                "node {index} lost {lost} on {peer}, more than its hold toward it ({toward}) \
                 and its hold toward it ({from}) held"
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
fn check_hub_books(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    for index in 0..sim.nodes.len() {
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

/// Once the network is calm, every honest member's hub account is in step
/// with the hub: it read what touched it up to a recent final block. A
/// liar's is its own affair.
fn check_in_step(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    let behind = (0..sim.nodes.len())
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
fn check_moved(sim: &Sim, liar: Option<usize>) -> Result<(), String> {
    let moves = (0..sim.nodes.len())
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
fn check_owners(sim: &Sim) -> Result<(), String> {
    let reserve = i128::from(dassie_hub::member::units(
        sim.hub.as_deref().unwrap().rules.terms.fees.account_reserve,
    ));
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
        let deposit = (fundings * i128::from(FAUCET_DROPS) - 1_000_000) * 1_000 - reserve;
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
                sim.balances_on(index, XRP),
                sim.balances_on(index, HUB)
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

/// The hub loses f + 1 validators at once, its faulty ones among them, and
/// stalls: each validator still up declares it after the rules' stall, and
/// signs the payout set of its last final block. The honest ones lost come
/// back one by one, a while apart, and sign too, so the signatures of one
/// set are gathered over time; once a quorum signed one, it is submitted,
/// and the escrow pays out exactly the final balances.
fn stall(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let hub = sim.hub.as_deref().unwrap();
    let count = hub.validators.len();
    let lost_count = hub.faults() + 1;
    let stall_ms = hub.rules.terms.timers.stall;
    let faulty: Vec<usize> = (0..count)
        .filter(|index| !hub.validators[*index].honest())
        .collect();
    let mut honest: Vec<usize> = hub.honest().collect();
    let mut lost = faulty.clone();
    while lost.len() < lost_count {
        let at = usize::try_from(rng.below(honest.len() as u64)).unwrap();
        lost.push(honest.remove(at));
    }
    sim.trace
        .push(format!("{} the hub loses validators {lost:?}", sim.now));
    for &index in &lost {
        sim.crash_validator(index);
    }
    // Every validator still up declares the stall within the rules' stall
    // of its last final block, and a view more.
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
    // The honest validators lost come back one by one, a while apart, and
    // each declares the stall in its turn.
    let returning: Vec<usize> = lost
        .iter()
        .copied()
        .filter(|index| !faulty.contains(index))
        .collect();
    for index in returning {
        sim.run_until(sim.now + rng.below(stall_ms));
        sim.start_validator(index);
        let gathered = |sim: &Sim| sim.hub.as_deref().unwrap().payout.is_some();
        if sim.run_until_or(sim.now + wait, gathered) {
            break;
        }
    }
    let Some(payout) = sim.hub.as_deref().unwrap().payout.as_ref() else {
        let hub = sim.hub.as_deref().unwrap();
        let heights: Vec<(usize, u64, bool)> = (0..hub.validators.len())
            .map(|index| {
                let validator = &hub.validators[index];
                (index, validator.height(&hub.id()), validator.stalled())
            })
            .collect();
        return Err(Outcome::liveness(
            "payout",
            format!("no quorum signed one payout set: (validator, height, stalled) {heights:?}"),
        ));
    };
    let height = payout.set.height;
    sim.trace.push(format!(
        "{} the payout set of height {height} is gathered",
        sim.now
    ));
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
    check_payout(sim).map_err(|problem| Outcome::safety("payout", problem))
}

/// The escrow paid out exactly the final balances of the block the set is
/// of: each account with somewhere to go and enough to pay out received
/// its balance, reserves and the holds it paid into, less the fee, as the
/// design says, worked out apart from the set; every transaction of the set
/// applied; and the escrow is left with what it held less what the set
/// paid and its fees.
fn check_payout(sim: &mut Sim) -> Result<(), String> {
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

/// f + 1 validators, the faulty ones among them, collude: the first of
/// them to lead proposes two blocks, each to half of the honest validators,
/// and all of them vote for both. Should honest validators finalize
/// different blocks, evidence must name the colluders, and only them.
fn collude(sim: &mut Sim) -> Result<(), Outcome> {
    let hub = sim.hub.as_deref_mut().unwrap();
    let count = hub.validators.len();
    let colluders = hub.faults() + 1;
    for index in count - colluders..count {
        hub.validators[index].role = Role::Collude;
    }
    sim.trace.push(format!(
        "{} validators {:?} collude",
        sim.now,
        (count - colluders..count).collect::<Vec<_>>()
    ));
    let forked = |sim: &Sim| {
        let hub = sim.hub.as_deref().unwrap();
        hub.forked
            && !hub.breaches.is_empty()
            && hub
                .evidence
                .iter()
                .any(|(index, _)| hub.validators[*index].honest())
    };
    sim.run_until_or(sim.now + 5 * 60_000, forked);
    let hub = sim.hub.as_deref().unwrap();
    let blamed = hub
        .breaches
        .iter()
        .find(|breach| breach.contains("evidence against honest"));
    if let Some(breach) = blamed {
        return Err(Outcome::safety("evidence", breach.clone()));
    }
    let fork = hub
        .breaches
        .iter()
        .find(|breach| breach.contains("finalized different states"));
    let evidence = hub
        .evidence
        .iter()
        .any(|(index, _)| hub.validators[*index].honest());
    match fork {
        Some(fork) if !evidence => Err(Outcome::safety(
            "evidence",
            format!("a fork without evidence: {fork}"),
        )),
        Some(_) => {
            sim.count("forks with evidence");
            Ok(())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A withdrawal the escrow paid and the hub has not settled yet: the
    // hub still owes it until a quorum attests the payment, so the escrow
    // holds less than the hub owes, by the withdrawal and its fee, and the
    // check takes that, whatever the node thinks of its move.
    #[test]
    fn a_withdrawal_paid_and_not_yet_settled_passes_the_escrow_check() {
        let mut sim = Sim::with(3, 2, 1, BTreeMap::new(), Settling::Hub(4));
        sim.loss_per_mille = 0;
        assert!(
            sim.run_until_or(START + 600_000, joined),
            "the members did not join the hub"
        );
        sim.run_until(sim.now + 30_000);
        check_escrow(&sim).unwrap();
        let hub = sim.hub.as_deref().unwrap();
        let before = sim.ledger.balance(hub.escrow()).unwrap_or(0);
        let least = hub.rules.terms.fees.least_payout;
        sim.step(0, |core| {
            core.hub_withdraw(least * dassie_xrpl::UNITS_PER_DROP)
                .unwrap();
        });
        let paid_unsettled = |sim: &Sim| {
            let hub = sim.hub.as_deref().unwrap();
            let paid = sim.ledger.balance(hub.escrow()).unwrap_or(0) < before;
            let queued = hub
                .ledger()
                .is_some_and(|ledger| ledger.withdrawals().next().is_some());
            paid && queued
        };
        let deadline = sim.now + 120_000;
        while !paid_unsettled(&sim) {
            assert!(sim.now < deadline, "the escrow never paid the withdrawal");
            sim.run_until(sim.now + 5);
        }
        check_escrow(&sim).unwrap();
    }
}
