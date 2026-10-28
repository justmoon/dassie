//! The node simulation's run in two hubs (`node-hubs`,
//! spec/areas/testing.md#over-a-settlement-hub,
//! spec/decisions/settlement-methods.md): 3 or 4 nodes, each a member of the
//! XRP hub and of the DSUSD hub, two hubs of 4 to 7 honest validators each
//! run in the simulation on the one fake XRP Ledger. Each node runs the
//! owner books of XRP and DSUSD on its XRP Ledger testnet account, peering
//! on neither, and funds each hub from the book of its asset; its link
//! state lists both hubs, and each pair peers over both, each hub a method
//! of the peering. Payments go in XRP and in dollars, and from one into the
//! other, across hops, each packet on the method of the ledger it came in
//! on. Then the owner of one node removes the scheme of one hub, the
//! seed's: its methods on it wind down, each asked to settle once and
//! dropped within its bound, while its peerings carry on over the other
//! hub. That hub then loses f + 1 validators, stalls and pays out: each
//! other member's scheme over it can no longer receive, so the member
//! offers it to no peer, while the other hub keeps carrying payments and
//! every peering is kept on it.
//!
//! Each node is judged by its own books (spec/decisions/own-books.md): in
//! each hub, what its account and its peers' holds toward it hold, as that
//! hub's honest validators have it; and what it booked as lost on a peer
//! over a hub is never more than the holds between them on that hub,
//! which bounds what the peer can cost summed over the methods of their
//! peering. The helpers here judge a run of any number of hubs, hub
//! by hub, and `node-hub-currencies` shares them.

use std::collections::{BTreeMap, BTreeSet};

use dassie_hub::consensus::MAX_VIEW_TIMEOUT_MS;
use dassie_hub::escrow::PayoutTransaction;
use dassie_hub::rules::Asset;
use dassie_identity::NodeId;
use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_ledger::LedgerId;
use dassie_xrpl::api;
use dassie_xrpl::codec::Amount;
use dassie_xrpl::{DROPS_PER_XRP, UNITS_PER_DROP};

use super::checks::{
    check_across, check_events, check_hub_credit, check_hub_packets, check_settled, first_failure,
};
use super::hub::{PREPARES_OVER_DSUSD, PREPARES_OVER_XRP, owed};
use super::hub_checks::{
    check_each, check_losses, hub_standing, idle_in, joined as joined_all, ledger_of, members_of,
    moving_in, places,
};
use super::hubbed::{paid, routed};
use super::scripted::{READY_WITHIN_MS, ended};
use super::{HUB, HUB_DSUSD, MINT_MICROS, START, Settling, Sim, key, xrpl_account};

/// How long a move into a hub may be on its way before it is late
/// (`CREDIT_WAIT_MS`, `engine/core/hub_funding.rs`).
const CREDIT_WAIT_MS: u64 = 600_000;

/// How long the members may take to join both hubs: the issuer's mint once
/// each member's DSUSD line exists, then a move into each hub, each within
/// its wait.
pub(super) const JOINED_WITHIN_MS: u64 = 2 * CREDIT_WAIT_MS;

/// How long the payments may take, at production timings.
const PAID_WITHIN_MS: u64 = 10 * 60_000;

/// How long the network is left calm before the books are checked.
const CALM_MS: u64 = 60_000;

/// How long the network may then take to settle down: a peer short of room
/// is asked again once a minute, and may have to move funds into the hub
/// before it can settle, which is late only after `CREDIT_WAIT_MS`.
const SETTLED_WITHIN_MS: u64 =
    crate::node::Policies::DEFAULT.settlement_request_every_ms + CREDIT_WAIT_MS;

/// How long a peering may take to gain the method of every hub both nodes
/// are in, once it is made: a request and its answer, retried a few times.
const METHODS_WITHIN_MS: u64 = 5 * 60_000;

/// What each member's owner moves into the DSUSD hub as its stake, in
/// millionths of a dollar: half of what the issuer mints it, so that its
/// DSUSD book keeps the other half.
pub(super) const HUB_STAKE_MICROS: u64 = MINT_MICROS / 2;

/// What each member's owner moves into the XRP hub as its stake, in XRP.
pub(super) const HUB_STAKE_XRP: u64 = super::hub_runs::HUB_STAKE_XRP;

/// The members join each hub, each from the owner book of its asset,
/// within the wait of a move.
pub(super) fn join_hubs(sim: &mut Sim) -> Result<(), Outcome> {
    if sim.run_until_or(START + JOINED_WITHIN_MS, joined_all) {
        sim.count("members joined both hubs");
        return Ok(());
    }
    Err(Outcome::liveness(
        "joined",
        format!(
            "the members did not all join their hubs in time: {}",
            hub_standing(sim)
        ),
    ))
}

/// Each member's owner moves its stake for the run into each of its hubs,
/// [`HUB_STAKE_XRP`] into the XRP hub and [`HUB_STAKE_MICROS`] into the
/// DSUSD hub, and turns that hub's automatic moves off, so that its node
/// keeps it there past the ceiling: an owner who converts into a hub, or
/// pays from it, keeps funds there as on a ledger it settles on, more than
/// the automatic moves keep for settling alone
/// (spec/areas/hubs.md#in-the-node).
pub(super) fn stake(sim: &mut Sim) -> Result<(), Outcome> {
    let idle = |sim: &Sim| {
        places(sim)
            .into_iter()
            .all(|place| members_of(sim, place).all(|index| !moving_in(sim, index, place)))
    };
    if !sim.run_until_or(sim.now + JOINED_WITHIN_MS, idle) {
        return Err(Outcome::liveness(
            "staked",
            format!(
                "a member's first moves were not done in time: {}",
                hub_standing(sim)
            ),
        ));
    }
    // An owner whose node cannot take the move now, its ledger busy or its
    // account not yet read again, tries again a ledger later, until the
    // wait of a move is over.
    let deadline = sim.now + JOINED_WITHIN_MS;
    let mut staking: Vec<(usize, usize)> = places(sim)
        .into_iter()
        .flat_map(|place| {
            members_of(sim, place)
                .map(move |index| (place, index))
                .collect::<Vec<_>>()
        })
        .collect();
    while let Some(&(place, index)) = staking.first() {
        let hub = sim.hub_at(place);
        let id = Some(*hub.id().0.as_bytes());
        let stake = match hub.spec.asset {
            Asset::Xrp => HUB_STAKE_XRP * DROPS_PER_XRP,
            Asset::Token { .. } => HUB_STAKE_MICROS,
        };
        let mut moved = Ok(());
        sim.step(index, |core| {
            moved = core
                .set_hub_automatic(id, false)
                .and_then(|_| core.hub_deposit_to(id, stake * UNITS_PER_DROP))
                .map(|_| ());
        });
        match moved {
            Ok(()) => {
                staking.remove(0);
            }
            Err(_) if sim.now < deadline => {
                sim.run_until(sim.now + super::LEDGER_CLOSE_MS);
            }
            Err(error) => {
                return Err(Outcome::liveness(
                    "staked",
                    format!("node {index}'s owner could not move its stake: {error:?}"),
                ));
            }
        }
    }
    sim.trace
        .push(format!("{} the owners move their stakes", sim.now));
    if !sim.run_until_or(sim.now + JOINED_WITHIN_MS, idle) {
        return Err(Outcome::liveness(
            "staked",
            format!(
                "a member's stake did not reach its hub in time: {}",
                hub_standing(sim)
            ),
        ));
    }
    Ok(())
}

/// The network settles down: each member's account in each hub reads what
/// touched it, its moves are done, and a peer short of room on a method
/// settles once asked; the books are checked once all of that is done.
/// Only the hubs at `places` are waited for: one that stalled reads nothing
/// new.
pub(super) fn settle_down_in(sim: &mut Sim, places: &[usize]) {
    sim.run_until(sim.now + CALM_MS);
    let places = places.to_vec();
    let settled = move |sim: &Sim| idle_in(sim, &places, None);
    let first = settled.clone();
    sim.run_until_or(sim.now + SETTLED_WITHIN_MS, move |sim| {
        first(sim) && check_settled(sim).is_ok()
    });
    // The last settlement may have executed a block before the accounts
    // read it: a while of quiet, then the accounts read the hubs again.
    sim.run_until(sim.now + CALM_MS);
    sim.run_until_or(sim.now + CALM_MS, settled);
}

/// What a run over hubs checks of the hubs at `places` and of each member's
/// books in each, once the network is calm, as every run over hubs does
/// ([`check_each`]): the validators, each member's books and its owner's,
/// its account in step and its moves done, each escrow; and over all of
/// them, no packet held past the connector, no credit apart from the holds,
/// and no loss on a peer over a hub beyond the holds between them on that
/// hub.
pub(super) fn check_hubs(sim: &Sim, places: &[usize]) -> Result<(), Outcome> {
    first_failure(
        [
            check_hub_packets(sim).map_err(|problem| Outcome::safety("hub-packets", problem)),
            check_hub_credit(sim).map_err(|problem| Outcome::safety("hub-credit", problem)),
            check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
        ]
        .into_iter()
        .chain(check_each(sim, places, None, &[]))
        .collect::<Vec<_>>(),
    )
}

/// The salt of the run's generator: "hubpairs".
const SALT: u64 = 0x6875_6270_6169_7273;

/// What a seed draws.
struct Setup {
    validators: usize,
    members: usize,
}

fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ SALT);
    let validators = 4 + usize::try_from(rng.below(4)).unwrap();
    let members = 3 + usize::try_from(rng.below(2)).unwrap();
    (
        rng,
        Setup {
            validators,
            members,
        },
    )
}

/// What a seed of `node-hubs` draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    format!(
        "{} members of an XRP hub and a DSUSD hub of {} validators each; an owner removes \
         one hub's scheme, and that hub fails",
        setup.members, setup.validators
    )
}

/// A run of nodes in two hubs.
#[must_use]
pub fn run(seed: u64) -> Run {
    let (mut rng, setup) = setup(seed);
    let mut sim = Sim::with(
        seed,
        setup.members,
        0,
        BTreeMap::new(),
        Settling::Hubs(setup.validators),
    );
    let result = script(&mut sim, &mut rng);
    ended(sim, result)
}

/// The place of the run's hub of `ledger`.
fn place_of(sim: &Sim, ledger: LedgerId) -> usize {
    sim.hubs()
        .find(|hub| ledger_of(hub) == ledger)
        .map(|hub| hub.spec.place)
        .unwrap()
}

fn script(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    join_hubs(sim)?;
    stake(sim)?;
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
    let both = [Settling::scheme(HUB_DSUSD).0, Settling::scheme(HUB).0];
    if !sim.run_until_or(sim.now + METHODS_WITHIN_MS, |sim| {
        check_methods(sim, &both).is_ok()
    }) {
        return Err(Outcome::liveness(
            "methods",
            check_methods(sim, &both).unwrap_err(),
        ));
    }
    let peerings = peerings(sim);
    sim.count("pairs peered over both hubs");
    // A packet takes the method of the ledger it came in on, where the hop
    // has it (spec/areas/connector.md#settlement-method-slots): so payments
    // start on either ledger, each crossing its own hub, and one is paid
    // from the other method's ledger into the first's.
    let first = first_ledger(sim)?;
    let other = if first == HUB { HUB_DSUSD } else { HUB };
    let start = other;
    // In each currency across the hop between nodes 0 and 2, and from one
    // into the other between two nodes drawn.
    let amount = |rng: &mut SeededRng| 100_000_000 + rng.below(2_200_000_000);
    let there = amount(rng);
    sim.pay_in(0, (first, first), 2, there, None, None);
    let back = amount(rng);
    sim.pay_in(2, (start, other), 0, back, None, None);
    let count = sim.nodes.len();
    let from = usize::try_from(rng.below(count as u64)).unwrap();
    let to = (from + 1 + usize::try_from(rng.below(count as u64 - 1)).unwrap()) % count;
    let across = amount(rng);
    sim.pay_in(from, (start, first), to, across, None, None);
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    let all = places(sim);
    settle_down_in(sim, &all);
    first_failure([
        check_across(sim),
        check_hubs(sim, &all),
        check_carried(sim).map_err(|problem| Outcome::liveness("methods-carried", problem)),
    ])?;
    // Which hub fails is the seed's: each method carries packets. Before it
    // does, the owner of a node the seed draws removes that hub's scheme:
    // the node's methods on it wind down, each collected once within its
    // bound, while its peerings carry on over the other hub.
    let (failed, working_ledger) = if rng.below(2) == 0 {
        (other, first)
    } else {
        (first, other)
    };
    let failing = place_of(sim, failed);
    let working = place_of(sim, working_ledger);
    let remover = usize::try_from(rng.below(sim.nodes.len() as u64)).unwrap();
    remove_hub(sim, remover, failed, &peerings)?;
    // The hub then fails: its method is unavailable from then on, so the
    // payments after go over the other.
    let height = sim
        .hub_at(working)
        .ledger()
        .map_or(0, |ledger| ledger.height());
    let paid_before = sim.payments.len();
    stall(sim, rng, failing)?;
    let there = amount(rng);
    let back = amount(rng);
    let on = (working_ledger, working_ledger);
    if !sim.pay_in(0, on, 2, there, None, None) || !sim.pay_in(2, on, 0, back, None, None) {
        return Err(Outcome::liveness(
            "paid-after",
            format!("a payment after the {failed} hub failed could not start"),
        ));
    }
    assert_eq!(sim.payments.len(), paid_before + 2);
    sim.run_until_or(sim.now + PAID_WITHIN_MS, |sim| {
        sim.payments[paid_before..]
            .iter()
            .all(|payment| payment.done)
    });
    settle_down_in(sim, &[working]);
    // Each finished, having sent something, and arrived with what was sent
    // less what the hop between charged: a method that stopped carrying
    // packets when the other hub failed fails here.
    let after = sim.payments[paid_before..].iter().map(|payment| {
        let (sent, got) = (super::sent(payment), super::received(sim, payment));
        let least = super::least_across(working_ledger, working_ledger, sent);
        if !payment.done || sent == 0 {
            Err(Outcome::liveness(
                "paid-after",
                format!(
                    "a payment from node {} to {} after the {failed} hub failed did not finish: \
                     sent {sent} of {}, received {got}",
                    payment.from, payment.to, payment.amount,
                ),
            ))
        } else if got < least {
            Err(Outcome::safety(
                "received",
                format!(
                    "node {} received {got} of node {}'s {sent}",
                    payment.to, payment.from,
                ),
            ))
        } else {
            Ok(())
        }
    });
    first_failure(
        [
            check_events(sim).map_err(|problem| Outcome::safety("events", problem)),
            check_hubs(sim, &[working]),
            check_kept(sim, &peerings, working_ledger)
                .map_err(|problem| Outcome::liveness("kept", problem)),
            check_failed(sim, failed, remover)
                .map_err(|problem| Outcome::liveness("hub-failed", problem)),
            (sim.hub_at(working)
                .ledger()
                .map_or(0, |ledger| ledger.height())
                > height)
                .then_some(())
                .ok_or_else(|| {
                    Outcome::liveness(
                        "hub-running",
                        format!("the {working_ledger} hub finalized nothing past height {height}"),
                    )
                }),
        ]
        .into_iter()
        .chain(after)
        .collect::<Vec<_>>(),
    )?;
    sim.count("a hub failed and the other carried on");
    Ok(())
}

/// The most a method being wound down takes: its two waits
/// (spec/decisions/settlement-methods.md), and the time a check of the
/// peerings and a message take.
const WIND_WITHIN_MS: u64 = 2 * crate::node::Policies::DEFAULT.peer.depeer_wait_ms
    + crate::node::Policies::DEFAULT.peer.peering_check_ms
    + super::LEDGER_CLOSE_MS;

/// The owner of node `index` removes the scheme of the hub of `ledger`
/// (spec/areas/network.md#settlement-methods): it is not refused for the
/// methods on it, which wind down, each asked to settle and dropped
/// within its bound, on both sides (that it is asked only once, the core's
/// tests show). Then the scheme is being removed with
/// no method left on it, and every peering of the node in `peerings` is
/// kept, on its method on the other hub.
fn remove_hub(
    sim: &mut Sim,
    index: usize,
    ledger: LedgerId,
    peerings: &BTreeSet<(usize, NodeId)>,
) -> Result<(), Outcome> {
    let scheme = Settling::scheme(ledger).0;
    let id = dassie_peer::wire::SchemeId::parse(scheme).unwrap();
    // The owner first revokes the tokens its clients paid from that ledger
    // with, each payment done: a token in use refuses the removal.
    let tokens: BTreeSet<u64> = sim
        .payments
        .iter()
        .filter(|payment| payment.from == index && payment.ledger == ledger)
        .map(|payment| payment.token.0)
        .collect();
    let mut removed = Ok(());
    sim.step(index, |core| {
        for token in tokens {
            core.revoke_token(token).unwrap();
        }
        removed = core.remove_scheme(scheme);
    });
    removed.map_err(|error| {
        Outcome::liveness(
            "removed",
            format!("node {index}'s owner could not remove {scheme}: {error:?}"),
        )
    })?;
    sim.trace
        .push(format!("{} node {index}'s owner removes {scheme}", sim.now));
    // Each method's winding as last seen before it was dropped.
    let last: std::cell::RefCell<BTreeMap<NodeId, dassie_peer::Winding>> =
        std::cell::RefCell::default();
    let gone = |sim: &Sim| {
        let mut left = false;
        for peer in sim.core(index).network().peers {
            if let Some(method) = peer.peering.method(id) {
                left = true;
                if let Some(winding) = method.winding {
                    last.borrow_mut().insert(peer.id, winding);
                }
            }
        }
        !left
    };
    if !sim.run_until_or(sim.now + WIND_WITHIN_MS, gone) {
        return Err(Outcome::liveness(
            "wound-down",
            format!("node {index}'s methods on {scheme} were not dropped within their bound"),
        ));
    }
    let peers: Vec<NodeId> = peerings
        .iter()
        .filter(|(of, _)| *of == index)
        .map(|(_, peer)| *peer)
        .collect();
    if let Some(peer) = peers
        .iter()
        .find(|peer| last.borrow().get(peer).is_none_or(|w| w.asked_ms.is_none()))
    {
        return Err(Outcome::liveness(
            "wound-down",
            format!("node {index} dropped its method with {peer} without asking it to settle"),
        ));
    }
    // The peer drops it at once once told.
    let told = |sim: &Sim| {
        (0..sim.nodes.len()).all(|other| {
            sim.core(other)
                .network()
                .peers
                .iter()
                .filter(|peer| peer.id == key(index).node_id())
                .all(|peer| peer.peering.method(id).is_none())
        })
    };
    if !sim.run_until_or(sim.now + WIND_WITHIN_MS, told) {
        return Err(Outcome::liveness(
            "wound-down",
            format!("a peer of node {index} kept its method on {scheme}"),
        ));
    }
    sim.count("hub schemes removed");
    let kept = peerings
        .iter()
        .copied()
        .filter(|(of, peer)| *of == index || *peer == key(index).node_id())
        .collect();
    let other = if ledger == HUB { HUB_DSUSD } else { HUB };
    first_failure([
        check_removing(sim, index, ledger)
            .map_err(|problem| Outcome::liveness("removing", problem)),
        check_kept(sim, &kept, other).map_err(|problem| Outcome::liveness("kept", problem)),
    ])
}

/// Node `index` is removing the scheme over the hub of `ledger`, with no
/// method left on it, and offers it to no peer. It goes once nothing can
/// come over it any more: the wait for what peers may still settle on it
/// takes the longest backoff between a settlement's tries, longer than the
/// run (spec/decisions/settlement-methods.md); the core's tests show it go.
fn check_removing(sim: &Sim, index: usize, ledger: LedgerId) -> Result<(), String> {
    let scheme = Settling::scheme(ledger).0;
    let id = dassie_peer::wire::SchemeId::parse(scheme).unwrap();
    let core = sim.core(index);
    let removing = core
        .ledger_views()
        .into_iter()
        .find(|view| view.scheme == scheme)
        .map_or(Some(0), |view| view.removing);
    if removing != Some(0) {
        return Err(format!(
            "node {index}'s owner removed {scheme}, which is {removing:?} methods from going"
        ));
    }
    if offers(sim, index, id) {
        return Err(format!("node {index} still offers {scheme} once removed"));
    }
    Ok(())
}

/// Every member of the hub of `failed` that still runs its scheme, all but
/// `remover`, can no longer receive over it once it stalled
/// (spec/decisions/settlement-methods.md): its own link state lists the
/// scheme no more, so it is offered to no peer, and its methods on it are
/// dropped once they could not receive for as long as a silent peer is
/// kept, which is longer than the run.
fn check_failed(sim: &Sim, failed: LedgerId, remover: usize) -> Result<(), String> {
    let scheme = dassie_peer::wire::SchemeId::parse(Settling::scheme(failed).0).unwrap();
    for index in members_of(sim, place_of(sim, failed)).filter(|index| *index != remover) {
        if offers(sim, index, scheme) {
            return Err(format!(
                "node {index} still offers {scheme} once its hub failed"
            ));
        }
    }
    Ok(())
}

/// Whether node `index`'s own link state lists `scheme`: whether it
/// offers peerings on it.
fn offers(sim: &Sim, index: usize, scheme: dassie_peer::wire::SchemeId) -> bool {
    let network = sim.core(index).network();
    network
        .nodes
        .iter()
        .find(|node| node.id == network.own)
        .is_some_and(|own| own.schemes.contains(&scheme))
}

/// Every peering has a method on each of `schemes`: every hub both nodes
/// are in.
fn check_methods(sim: &Sim, schemes: &[&str]) -> Result<(), String> {
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            let has: Vec<String> = (peer.peering.schemes())
                .map(|scheme| scheme.as_str().to_owned())
                .collect();
            if let Some(missing) = schemes
                .iter()
                .find(|scheme| !has.iter().any(|held| held == *scheme))
            {
                return Err(format!(
                    "node {index}'s peering with {} has no method on {missing}: {has:?}",
                    peer.id
                ));
            }
        }
    }
    Ok(())
}

/// Each node's peers.
fn peerings(sim: &Sim) -> BTreeSet<(usize, NodeId)> {
    (0..sim.nodes.len())
        .flat_map(|index| {
            sim.core(index)
                .network()
                .peers
                .into_iter()
                .map(move |peer| (index, peer.id))
        })
        .collect()
}

/// The ledger of every peering's first method, the one it was made on.
fn first_ledger(sim: &Sim) -> Result<LedgerId, Outcome> {
    let firsts: BTreeSet<String> = (0..sim.nodes.len())
        .flat_map(|index| sim.core(index).network().peers)
        .map(|peer| peer.peering.first().scheme.as_str().to_owned())
        .collect();
    match firsts.iter().collect::<Vec<_>>().as_slice() {
        [first] if first.as_str() == Settling::scheme(HUB).0 => Ok(HUB),
        [first] if first.as_str() == Settling::scheme(HUB_DSUSD).0 => Ok(HUB_DSUSD),
        _ => Err(Outcome::liveness(
            "first-method",
            format!("the peerings were made on {firsts:?}, not all on one hub"),
        )),
    }
}

/// Every peering of `before` is kept, with its method on the hub of `on`,
/// which carries on: a hub that failed, or whose scheme an owner removed,
/// takes only its own methods with it.
fn check_kept(sim: &Sim, before: &BTreeSet<(usize, NodeId)>, on: LedgerId) -> Result<(), String> {
    let now = peerings(sim);
    if let Some((index, peer)) = before.difference(&now).next() {
        return Err(format!(
            "node {index} no longer peers with {peer} once a hub went"
        ));
    }
    check_methods(sim, &[Settling::scheme(on).0])
}

/// Each method carried packets: packets that keep their ledger take the
/// method on it, so payments in XRP cross the XRP hub and payments in
/// dollars the DSUSD hub.
fn check_carried(sim: &Sim) -> Result<(), String> {
    let over = |what| sim.counts.get(what).copied().unwrap_or(0);
    let (xrp, dsusd) = (over(PREPARES_OVER_XRP), over(PREPARES_OVER_DSUSD));
    if xrp == 0 || dsusd == 0 {
        return Err(format!(
            "packets crossed the XRP hub {xrp} times and the DSUSD hub {dsusd} times"
        ));
    }
    Ok(())
}

/// The hub at `place` loses f + 1 validators at once and stalls: each
/// validator still up declares it within the stall and a view, and signs
/// the payout set of its last final block. The lost ones come back one by
/// one, a while apart, and sign too; once a quorum signed one set, it is
/// submitted, and the escrow pays out exactly the final balances.
fn stall(sim: &mut Sim, rng: &mut SeededRng, place: usize) -> Result<(), Outcome> {
    let hub = sim.hub_at(place);
    let lost_count = hub.faults() + 1;
    let stall_ms = hub.rules.terms.timers.stall;
    let mut honest: Vec<usize> = hub.honest().collect();
    let mut lost = Vec::new();
    while lost.len() < lost_count {
        let at = usize::try_from(rng.below(honest.len() as u64)).unwrap();
        lost.push(honest.remove(at));
    }
    let ids = |indexes: &[usize]| -> Vec<usize> {
        indexes
            .iter()
            .map(|index| super::hub::validator_id(place, *index))
            .collect()
    };
    let (lost, live) = (ids(&lost), ids(&honest));
    sim.trace.push(format!(
        "{} the hub at {place} loses validators {lost:?}",
        sim.now
    ));
    for &id in &lost {
        sim.crash_validator(id);
    }
    let declared = |sim: &Sim| {
        let hub = sim.hub_at(place);
        live.iter()
            .all(|id| hub.validators[id % super::hub::MAX_VALIDATORS].stalled())
    };
    let wait = stall_ms + MAX_VIEW_TIMEOUT_MS;
    if !sim.run_until_or(sim.now + wait, declared) {
        return Err(Outcome::liveness(
            "stalled",
            "a validator still up did not declare the stall in time",
        ));
    }
    sim.count("hubs stalled");
    let gathered = |sim: &Sim| sim.hub_at(place).payout.is_some();
    for &id in &lost {
        if gathered(sim) {
            break;
        }
        sim.run_until(sim.now + rng.below(stall_ms));
        sim.start_validator(id);
        sim.run_until_or(sim.now + wait, gathered);
    }
    if !gathered(sim) {
        return Err(Outcome::liveness(
            "payout",
            format!("no quorum signed one payout set: {}", hub_standing(sim)),
        ));
    }
    let applied = |sim: &Sim| {
        sim.hub_at(place)
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
    check_payout(sim, place).map_err(|problem| Outcome::safety("payout", problem))
}

/// The escrow of the hub at `place` paid out exactly the final balances of
/// the block its set is of, in the hub's asset: each account with
/// somewhere to go and enough to pay out received its balance, reserves and
/// the holds it paid into, less the ledger's fee where the hub charges one,
/// as the design says, worked out apart from the set; and every transaction
/// of the set applied.
fn check_payout(sim: &mut Sim, place: usize) -> Result<(), String> {
    let hub = sim.hub_at(place);
    let payout = hub.payout.as_ref().unwrap();
    let ledger = &payout.ledger;
    let terms = ledger.terms().clone();
    let fees = terms.fees;
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
    let transactions: Vec<([u8; 32], PayoutTransaction)> = payout
        .hashes
        .iter()
        .copied()
        .zip(payout.set.transactions.iter().cloned())
        .collect();
    let connection = hub.submitter();
    let mut delivered: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    for (hash, transaction) in &transactions {
        let reply = sim.ledger.request(connection, &api::tx_anywhere(9, hash));
        let record = match api::parse(&reply) {
            api::Message::Response { outcome, .. } => {
                outcome.ok().and_then(|result| api::tx_result(&result))
            }
            _ => None,
        }
        .ok_or_else(|| format!("{transaction:?} was never applied"))?;
        if record.result != "tesSUCCESS" {
            return Err(format!("{transaction:?} ended {}", record.result));
        }
        if let PayoutTransaction::Payout(payment) = transaction {
            let named = terms
                .hub_amount(&payment.amount)
                .ok_or("a payout of something else than the hub's asset")?;
            let got = match payment.amount {
                Amount::Xrp(_) => record.delivered,
                Amount::Token(_) => record
                    .delivered_token
                    .and_then(|token| terms.hub_amount(&Amount::Token(token))),
            };
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
    for index in members_of(sim, place) {
        if !delivered.contains_key(&xrpl_account(index)) {
            return Err(format!("node {index} was paid nothing out"));
        }
    }
    sim.count("payouts checked");
    Ok(())
}
