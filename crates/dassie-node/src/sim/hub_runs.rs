//! The node simulation's further runs over a settlement hub
//! (spec/areas/testing.md#over-a-settlement-hub), beside `node-hub`
//! ([`hubbed`](super::hubbed)), whose helpers and checks they share:
//!
//! - **`node-hub-lies`:** members peered over a hub whose XRP Ledger
//!   servers lie to them, each kind of lie in turn, in rounds that give
//!   every member one, while they pay and their owners ask the faucet, one
//!   owner moves funds into the hub by hand and the other members' nodes
//!   move theirs by themselves; in each round, some of the hub's validators'
//!   servers lie too, each another lie or all the same one; then a
//!   validator of the XRP Ledger rotates its key, a quorum of them is
//!   withheld and the publisher's list lapses
//!   (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
//!   Each member's books hold to the fake ledger's truth and to the hub,
//!   the validators attest nothing false, the payments complete, and the
//!   hub keeps running.
//! - **`node-hub-currencies`:** a run across currencies over two hubs:
//!   node 0 in dollars in the DSUSD hub, node 2 in XRP in the XRP hub, nodes
//!   1 and 3 in both, node 1 charging. Clients pay receivers in the other
//!   currency, an outgoing payment crosses, and the converting nodes price
//!   by their inventory (spec/decisions/passive-rebalancing.md).

use std::collections::BTreeMap;

use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_xrpl::account::{LIST_RETRY_MS, STALL_MS};
use dassie_xrpl::fake::Lie;
use dassie_xrpl::{DROPS_PER_XRP, UNITS_PER_DROP};

use super::checks::{check_across, check_anchored, check_true, check_withdrawn, first_failure};
use super::hub_pairs::{check_hubs, join_hubs, status_in};
use super::hubbed::{PAID_WITHIN_MS, check_calm, form, over_hub, paid, pay, routed, settle_down};
use super::scripted::{READY_WITHIN_MS, ended};
use super::{HUB, LEDGER_CLOSE_MS, LEDGER_RECONNECT_MS, LONGEST_LIE_MS, Settling, Sim};
use crate::engine::core::Direction;
use crate::node::Policies;

/// How long a server of [`run_lies`] lies to a member: the longest a drawn
/// one does.
const LIE_MS: u64 = LONGEST_LIE_MS;

/// How long a quorum of the XRP Ledger's validators is withheld: longer
/// than an account waits before it connects anew.
const WITHHELD_MS: u64 = 4 * STALL_MS;

/// How long the list a publisher lets lapse lasts once the quorum withheld
/// is back: a stall, so that the members anchor again before it lapses.
/// They fetched it when they lost quorum, at once
/// (spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers), so
/// each holds it when it expires.
const LAPSES_AFTER_MS: u64 = STALL_MS;

/// What each member's owner moves into the XRP hub in [`run_across`] and
/// `node-hubs`, in XRP: as much as each node's owner starts with on the
/// hub's ledger in a run, [`Settling::deposit`], which the checks of its
/// assets count from.
pub const HUB_STAKE_XRP: u64 = 100;

const _: () = assert!(
    Settling::deposit(HUB) == HUB_STAKE_XRP as i128 * DROPS_PER_XRP as i128 * 1_000,
    "a stake is what the checks count each owner started with in the hub"
);

/// The most an owner moves into the hub at once in [`run_lies`], in XRP:
/// as much as a node keeps there at its ceiling.
const MOST_MOVED_XRP: u64 =
    dassie_settlement::policy::hub_ceiling(Policies::DEFAULT.connector.max_packet_amount)
        / (DROPS_PER_XRP * UNITS_PER_DROP);

/// What a seed of a run over a hub draws first: its generator, how many
/// validators the hub has (4 to 7, all honest), and how many nodes.
fn size(seed: u64, salt: u64) -> (SeededRng, usize, usize) {
    let mut rng = SeededRng::new(seed ^ salt);
    let validators = 4 + usize::try_from(rng.below(4)).unwrap();
    let nodes = 3 + usize::try_from(rng.below(2)).unwrap();
    (rng, validators, nodes)
}

/// The salt of [`run_lies`]'s generator: "hub-lies".
const LIES: u64 = 0x6875_622d_6c69_6573;

/// The salt of [`run_across`]'s generator: "hub-curr".
const ACROSS: u64 = 0x6875_622d_6375_7272;

/// What a seed of `node-hub-lies` draws, before the run starts.
#[must_use]
pub fn describe_lies(seed: u64) -> String {
    let (_, validators, members) = size(seed, LIES);
    format!("{members} members, {validators} validators")
}

/// A run over a hub whose members' XRP Ledger servers lie.
#[must_use]
pub fn run_lies(seed: u64) -> Run {
    let (mut rng, validators, members) = size(seed, LIES);
    let mut sim = Sim::with(seed, members, 0, BTreeMap::new(), Settling::Hub(validators));
    let result = script_lies(&mut sim, &mut rng).and_then(|()| check_lies(&sim));
    ended(sim, result)
}

/// The height of the hub's last final block, at the honest validator
/// furthest on.
fn hub_height(sim: &Sim) -> u64 {
    let hub = sim.hub.as_deref().unwrap();
    hub.ledger().map_or(0, dassie_hub::ledger::Ledger::height)
}

/// Node `index`'s owner moves `drops` from its XRP Ledger account into the
/// hub, if its node takes the move now: not while its account is not in
/// step with the ledger, nor while another move is on its way. Whether it
/// did.
fn move_in(sim: &mut Sim, index: usize, drops: u64) -> bool {
    let mut moved = false;
    sim.step(index, |core| {
        moved = core.hub_deposit(drops * UNITS_PER_DROP).is_ok();
    });
    if moved {
        sim.count("owner moves into the hub");
        sim.trace.push(format!(
            "{} the owner of {index} moves {drops} drops into the hub",
            sim.now
        ));
    }
    moved
}

/// Whether node `index`'s connection to the ledger is to a lying server:
/// from when its honest one drops until the lie ends.
fn lied_to(sim: &Sim, index: usize) -> bool {
    sim.lying.get(&index).is_some_and(|connection| {
        (sim.links.get(&index)).is_some_and(|link| link.connection == *connection)
    })
}

/// Whether node `index` has a move of its own on its way, not one its
/// owner asked for.
fn moving_by_itself(sim: &Sim, index: usize) -> bool {
    sim.core(index)
        .hub_status()
        .and_then(|status| status.moving)
        .is_some_and(|moving| !moving.owner)
}

/// Some of the hub's validators' servers lie for a round of [`run_lies`]
/// (spec/areas/hubs.md#what-validators-attest): one to all of them, drawn,
/// in one round in two each telling another of the round's lies, in the
/// other all the first. Returns the validators lied to.
fn lie_to_validators(sim: &mut Sim, rng: &mut SeededRng, round: &[(usize, Lie)]) -> Vec<usize> {
    let count = sim.hub.as_deref().unwrap().validators.len();
    let lying = 1 + usize::try_from(rng.below(count as u64)).unwrap();
    let first = usize::try_from(rng.below(count as u64)).unwrap();
    let each_another = rng.below(2) == 0;
    let validators: Vec<usize> = (0..lying).map(|turn| (first + turn) % count).collect();
    for (turn, &validator) in validators.iter().enumerate() {
        let lie = if each_another {
            round[turn % round.len()].1
        } else {
            round[0].1
        };
        sim.lie_to_validator(validator, Some(lie));
        sim.count("validator servers lying in a round");
    }
    if !each_another && lying > 1 {
        sim.count("validator servers telling one lie");
    }
    validators
}

/// What the hub's validators attested holds on the fake ledger, while
/// their servers lie: the escrow holds what the hub owes and the sponsor
/// left, but for the withdrawals it paid that a quorum has not attested
/// yet, and no more but for the members' deposits on their way, so that a
/// withdrawal attested as paid that the escrow never paid fails as well;
/// and the hub wrote off no reset the ledger never made. A lying server
/// makes a watcher attest nothing, never something false
/// (spec/areas/hubs.md#what-validators-attest).
fn attested_truly(sim: &Sim) -> Result<(), Outcome> {
    let hub = sim.hub.as_deref().unwrap();
    let Some(ledger) = hub.ledger() else {
        return Ok(());
    };
    let withdrawing: u64 = ledger
        .withdrawals()
        .map(|(_, withdrawal)| withdrawal.amount + withdrawal.ledger_fee)
        .sum();
    let depositing: u64 = (0..sim.nodes.len())
        .filter_map(|index| status_in(sim, index, hub.spec.place)?.moving)
        .filter(|moving| moving.direction == Direction::Deposit)
        .map(|moving| moving.drops)
        .sum();
    let held = sim.ledger.balance(hub.escrow()).unwrap_or(0);
    let owed = hub.escrow_funding + ledger.supply();
    if held + withdrawing < owed || held > owed + depositing {
        return Err(Outcome::safety(
            "attested",
            format!(
                "the hub owes {owed} with {withdrawing} queued out and {depositing} on its way \
                 in, while its escrow holds {held}"
            ),
        ));
    }
    if let Some(at) = ledger.written_off_at() {
        return Err(Outcome::safety(
            "attested",
            format!("the hub wrote off a reset at {at:?} that the ledger never made"),
        ));
    }
    Ok(())
}

fn script_lies(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    form(sim)?;
    pay(sim, rng);
    let height = hub_height(sim);
    // The kinds of lie in an order the seed shuffles: one needs a ledger
    // so old its validations are stale, which the first rounds may not
    // have yet, so between them seeds tell every kind.
    let mut order = Lie::ALL;
    for at in (1..order.len()).rev() {
        let other = usize::try_from(rng.below(at as u64 + 1)).unwrap();
        order.swap(at, other);
    }
    // One member's owner moves funds into the hub by hand. An owner's move
    // pauses its node's own moves for longer than the run lasts
    // (`OWNER_PAUSE_MS`), so the other members' nodes keep moving theirs by
    // themselves, on what their lying servers tell them.
    let mover = usize::try_from(rng.below(count as u64)).unwrap();
    sim.trace.push(format!(
        "{} the owner of {mover} moves funds by hand",
        sim.now
    ));
    // A round of lies: each member in turn, from one drawn, gets the next
    // kind, until every kind was told.
    let mut lies = order.into_iter();
    while lies.len() > 0 {
        let first = usize::try_from(rng.below(count as u64)).unwrap();
        let round: Vec<(usize, Lie)> = (0..count)
            .map(|turn| (first + turn) % count)
            .zip(lies.by_ref())
            .collect();
        let drops = (1 + rng.below(MOST_MOVED_XRP)) * DROPS_PER_XRP;
        let lying = lie_to_validators(sim, rng, &round);
        for &(liar, lie) in &round {
            // Its owner asks the faucet for more three times as the
            // member's connection drops, and the mover's owner moves funds
            // into the hub: so the history the server gives on connecting,
            // and its stream, have something to leave out, or to hint at in
            // an old ledger, the member's own deposit to the escrow among
            // it.
            for _ in 0..3 {
                sim.fund(liar);
            }
            if liar == mover {
                move_in(sim, liar, drops);
            }
            // Only the liar's pairs may be withdrawn meanwhile: it sees
            // nothing of its ledger until its next connection anchors.
            sim.outages.push((
                Some(liar),
                sim.now,
                sim.now + LEDGER_RECONNECT_MS + LIE_MS + STALL_MS,
            ));
            sim.lie_to(liar, lie, LIE_MS);
        }
        // The members pay each other while their servers lie, so that
        // their holds in the hub, and so what their nodes move, change
        // under the lies.
        pay(sim, rng);
        sim.run_until(sim.now + STALL_MS / 2);
        for &(liar, _) in &round {
            sim.fund(liar);
        }
        // While the servers lie, a ledger at a time: the mover's owner
        // moves funds into the hub again once its node takes the move, and
        // the others' nodes start moves of their own.
        let end = sim.now + LIE_MS + STALL_MS / 2;
        let mut owner_moved = false;
        let mut was_moving: Vec<bool> = (0..count)
            .map(|index| moving_by_itself(sim, index))
            .collect();
        while sim.now < end {
            for &(liar, _) in &round {
                if !lied_to(sim, liar) {
                    continue;
                }
                if liar == mover {
                    if !owner_moved && move_in(sim, liar, drops) {
                        owner_moved = true;
                        sim.count("owner moves while lied to");
                    }
                } else if moving_by_itself(sim, liar) && !was_moving[liar] {
                    sim.count("own moves while lied to");
                }
            }
            for (index, was) in was_moving.iter_mut().enumerate() {
                *was = moving_by_itself(sim, index);
            }
            sim.run_until(end.min(sim.now + LEDGER_CLOSE_MS));
            attested_truly(sim)?;
        }
        for validator in lying {
            sim.lie_to_validator(validator, None);
        }
    }
    sim.end_lies();
    // A validator's new key: the members learn it from the server's
    // `manifest` method before any list carries it.
    sim.ledger.rotate_validator(0);
    sim.count("validator keys rotated");
    sim.trace
        .push(format!("{} validator 0 rotates its key", sim.now));
    sim.run_until(sim.now + LIE_MS);
    // A list that is to lapse, and then a quorum withheld: no ledger is
    // validated, every member waits, and having lost quorum, fetches the
    // lists at once, so that each holds the one that lapses when it does.
    let seconds = |ms: u64| u32::try_from(dassie_xrpl::unl::ledger_seconds(ms)).unwrap();
    let lapses = sim.now + WITHHELD_MS + LAPSES_AFTER_MS;
    sim.ledger.publish_validator_list(seconds(lapses), None);
    sim.trace.push(format!("{} the list is to lapse", sim.now));
    let quorum_short = [1, 2];
    for validator in quorum_short {
        sim.ledger.withhold_validator(validator, true);
    }
    sim.trace
        .push(format!("{} validators {quorum_short:?} withheld", sim.now));
    sim.count("quorums withheld");
    let withheld = sim.now;
    sim.run_until(sim.now + WITHHELD_MS);
    for validator in quorum_short {
        sim.ledger.withhold_validator(validator, false);
    }
    sim.outages.push((None, withheld, sim.now + STALL_MS));
    // The list lapses, and the members wait until the publisher publishes
    // another.
    sim.run_until(lapses + LIST_RETRY_MS);
    sim.count("validator lists lapsed");
    sim.ledger
        .publish_validator_list(seconds(sim.now + 30 * 24 * 3_600_000), None);
    sim.trace
        .push(format!("{} a new list is published", sim.now));
    sim.outages
        .push((None, lapses, sim.now + LIST_RETRY_MS + STALL_MS));
    // The payments left finish, and the network settles down.
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    settle_down(sim, None);
    // The hub kept running throughout: its validators watch the escrow on
    // connections of their own, which never lied.
    if hub_height(sim) <= height {
        return Err(Outcome::liveness(
            "hub-running",
            format!("the hub finalized nothing past height {height}"),
        ));
    }
    Ok(())
}

/// Everything [`run_lies`] checks once the network is calm: what a run
/// over a hub checks, with no member lying, and that each member's books
/// held to the fake ledger's truth, every member anchored, and no honest
/// peer was withdrawn for room but where its ledger failed it.
fn check_lies(sim: &Sim) -> Result<(), Outcome> {
    first_failure([
        check_calm(sim, None),
        check_true(sim).map_err(|problem| Outcome::safety("proven", problem)),
        check_anchored(sim).map_err(|problem| Outcome::liveness("anchored", problem)),
        check_withdrawn(sim).map_err(|problem| Outcome::liveness("withdrawn", problem)),
    ])
}

/// What a seed of `node-hub-currencies` draws, before the run starts.
#[must_use]
pub fn describe_across(seed: u64) -> String {
    let (_, validators, count) = size(seed, ACROSS);
    format!(
        "{count} nodes, 2 bootstrap, dollars in a DSUSD hub and XRP in an XRP hub of \
         {validators} validators each"
    )
}

/// A run across currencies over hubs.
#[must_use]
pub fn run_across(seed: u64) -> Run {
    let (mut rng, validators, count) = size(seed, ACROSS);
    let mut sim = Sim::with(
        seed,
        count,
        0,
        BTreeMap::new(),
        Settling::HubAcross(validators),
    );
    let result = script_across(&mut sim, &mut rng).and_then(|()| check_across_hubs(&sim));
    ended(sim, result)
}

fn script_across(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    // The hubs' members join them first, then everyone the network, from
    // nodes 0 and 1: node 0 and node 2 share no hub, so what one pays the
    // other crosses node 1 or 3, which converts.
    join_hubs(sim)?;
    super::hub_pairs::stake(sim)?;
    sim.join_network(2);
    if !sim.run_until_or(sim.now + READY_WITHIN_MS, routed) {
        return Err(Outcome::liveness(
            "ready",
            "the network was not ready in time",
        ));
    }
    over_hub(sim)?;
    // Dollars to XRP, and back; each up to twice the credit, so the hops
    // settle on both ledgers.
    let amount = |rng: &mut SeededRng| 100_000_000 + rng.below(2_200_000_000);
    let there = amount(rng);
    sim.pay(0, 2, there, None, None);
    let back = amount(rng);
    sim.pay(2, 0, back, None, None);
    // Into a node's own receiver in the other currency: it converts, and
    // charges nothing.
    let own = amount(rng);
    sim.pay(0, 1, own, None, None);
    let debit = 50_000_000 + rng.below(1_000_000_000);
    sim.pay_outgoing_across(2, 0, debit);
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    let all: Vec<usize> = sim.hubs().map(|hub| hub.spec.place).collect();
    super::hub_pairs::settle_down_in(sim, &all);
    Ok(())
}

/// Everything [`run_across`] checks once the network is calm: what a run
/// across currencies checks, that no honest peer was withdrawn for room,
/// and in each hub what a run over one checks of the hub and its members'
/// books.
fn check_across_hubs(sim: &Sim) -> Result<(), Outcome> {
    let all: Vec<usize> = sim.hubs().map(|hub| hub.spec.place).collect();
    first_failure([
        check_across(sim),
        check_withdrawn(sim).map_err(|problem| Outcome::liveness("withdrawn", problem)),
        check_hubs(sim, &all),
    ])
}
