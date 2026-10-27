//! The node simulation's further runs over a settlement hub
//! (spec/areas/testing.md#over-a-settlement-hub), beside `node-hub`
//! ([`hubbed`](super::hubbed)), whose helpers and checks they share:
//!
//! - **`node-hub-lies`:** members peered over a hub whose XRP Ledger
//!   servers lie to them, each kind of lie in turn, in rounds that give
//!   every member one, while they pay, and their owners ask the faucet and
//!   move funds into the hub; then a validator of the XRP Ledger rotates
//!   its key, a quorum of them is withheld and the publisher's list lapses
//!   (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
//!   Each member's books hold to the fake ledger's truth and to the hub,
//!   the payments complete, and the hub keeps running. The hub's
//!   validators keep honest connections to the ledger.
//! - **`node-hub-currencies`:** a run across currencies over hubs: node 0
//!   in dollars over the stub with commitments, the packet path of a hub
//!   without one, node 2 in XRP in a hub, nodes 1 and 3 on both, node 1
//!   charging. Clients pay receivers in the other currency, an outgoing
//!   payment crosses, and the converting nodes price by their inventory
//!   (spec/decisions/passive-rebalancing.md).

use std::collections::BTreeMap;

use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_xrpl::account::{LIST_RETRY_MS, STALL_MS};
use dassie_xrpl::fake::Lie;
use dassie_xrpl::{DROPS_PER_XRP, UNITS_PER_DROP};

use super::checks::{
    check_across, check_anchored, check_hub_credit, check_hub_packets, check_true, check_withdrawn,
    first_failure,
};
use super::hubbed::{
    JOINED_WITHIN_MS, PAID_WITHIN_MS, check_calm, check_escrow, check_hub_books,
    check_hub_validators, check_in_step, check_losses, check_moved, form, join_hub, members,
    moving, over_hub, paid, pay, routed, settle_down,
};
use super::scripted::{READY_WITHIN_MS, ended};
use super::{HUB_STAKE_XRP, LEDGER_RECONNECT_MS, LONGEST_LIE_MS, Settling, Sim};
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
/// step with the ledger, nor while another move is on its way.
fn move_in(sim: &mut Sim, index: usize, drops: u64) {
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
        for &(liar, lie) in &round {
            // Its owner asks the faucet for more three times, and moves
            // funds into the hub, as the member's connection drops, and
            // again while the server lies: so the history the server gives
            // on connecting, and its stream, have something to leave out,
            // or to hint at in an old ledger, the member's own deposit to
            // the escrow among it.
            for _ in 0..3 {
                sim.fund(liar);
            }
            move_in(sim, liar, drops);
            // Only the liar's pairs may be withdrawn meanwhile: it sees
            // nothing of its ledger until its next connection anchors.
            sim.outages.push((
                Some(liar),
                sim.now,
                sim.now + LEDGER_RECONNECT_MS + LIE_MS + STALL_MS,
            ));
            sim.lie_to(liar, lie, LIE_MS);
        }
        sim.run_until(sim.now + STALL_MS / 2);
        for &(liar, _) in &round {
            sim.fund(liar);
            move_in(sim, liar, drops);
        }
        sim.run_until(sim.now + LIE_MS + STALL_MS / 2);
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
        "{count} nodes, 2 bootstrap, dollars over the stub with commitments and XRP in a hub of \
         {validators} validators"
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
    // The hub's members join it first, then everyone the network, from
    // nodes 0 and 1: node 0 and node 2 share no ledger, so what one pays
    // the other crosses node 1 or 3, which converts.
    join_hub(sim)?;
    stake(sim)?;
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
    settle_down(sim, None);
    Ok(())
}

/// Each member's owner moves its stake for the run into the hub,
/// [`HUB_STAKE_XRP`], and turns the automatic moves off, so that its node
/// keeps it there past the ceiling: an owner who converts into the hub, or
/// pays from it, keeps funds there as on a ledger it settles on, more than
/// the automatic moves keep for settling alone
/// (spec/areas/hubs.md#in-the-node).
fn stake(sim: &mut Sim) -> Result<(), Outcome> {
    let idle = |sim: &Sim| members(sim).all(|index| !moving(sim, index));
    if !sim.run_until_or(sim.now + JOINED_WITHIN_MS, idle) {
        return Err(Outcome::liveness(
            "staked",
            "a member's first moves were not done in time",
        ));
    }
    let units = HUB_STAKE_XRP * DROPS_PER_XRP * UNITS_PER_DROP;
    for index in members(sim).collect::<Vec<_>>() {
        let mut moved = Ok(());
        sim.step(index, |core| {
            moved = core
                .set_hub_automatic(None, false)
                .and_then(|_| core.hub_deposit(units))
                .map(|_| ());
        });
        moved.map_err(|error| {
            Outcome::liveness(
                "staked",
                format!("node {index}'s owner could not move its stake: {error:?}"),
            )
        })?;
    }
    sim.trace.push(format!(
        "{} the owners move {HUB_STAKE_XRP} XRP each into the hub",
        sim.now
    ));
    if !sim.run_until_or(sim.now + JOINED_WITHIN_MS, idle) {
        return Err(Outcome::liveness(
            "staked",
            "a member's stake did not reach the hub in time",
        ));
    }
    Ok(())
}

/// Everything [`run_across`] checks once the network is calm: what a run
/// across currencies checks, and on the hub what a run over one checks of
/// the hub and its members' books.
fn check_across_hubs(sim: &Sim) -> Result<(), Outcome> {
    first_failure([
        check_across(sim),
        check_hub_validators(sim).map_err(|problem| Outcome::safety("hub-agreement", problem)),
        check_hub_packets(sim).map_err(|problem| Outcome::safety("hub-packets", problem)),
        check_hub_credit(sim).map_err(|problem| Outcome::safety("hub-credit", problem)),
        check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
        check_hub_books(sim, None).map_err(|problem| Outcome::safety("hub-books", problem)),
        check_escrow(sim).map_err(|problem| Outcome::safety("hub-escrow", problem)),
        check_in_step(sim, None).map_err(|problem| Outcome::liveness("hub-in-step", problem)),
        check_moved(sim, None).map_err(|problem| Outcome::liveness("hub-moves", problem)),
    ])
}
