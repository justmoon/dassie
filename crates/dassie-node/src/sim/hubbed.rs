//! The node simulation's runs over a settlement hub (`node-hub`,
//! spec/areas/testing.md#the-simulations): members peered over a hub of 4 to
//! 7 validators run in the simulation, its escrow on the fake XRP Ledger,
//! pay each other across hops, while up to f validators are faulty, a
//! validator restarts, and a member may lie. Then the hub loses f + 1
//! validators and stalls, and its escrow pays out the final balances once a
//! quorum signed one payout set, over time; or, one run in ten, f + 1
//! validators collude and fork the chain, which must leave evidence.
//!
//! Two runs join a hub by being paid (spec/designs/settlement-hubs.md#5-joining):
//! in [`paid_in`] a node with no account in the hub is paid into it by a
//! member it peers with, and in [`cycled`] fresh keys are paid less than
//! the hub's minimum for a new account and go away.
//!
//! Each node is judged by its own books (spec/decisions/own-books.md): its
//! books hold what its schemes hold, on the XRP Ledger and in the hub, its
//! owner's funds moved by what its receivers took, and what it booked as
//! lost on a peer over the hub is never more than the holds between them
//! (spec/designs/settlement-hubs.md#13-how-it-fits-the-node). Those checks
//! are [`hub_checks`](super::hub_checks), which the `node` simulation's
//! runs over a hub share.

use std::collections::BTreeMap;

use dassie_hub::consensus::MAX_VIEW_TIMEOUT_MS;
use dassie_hub::sim::Role;
use dassie_kit::sim::{Outcome, Run, SeededRng};

use super::checks::{check_events, check_incoming, first_failure};
use super::hub::{MemberLie, terms};
// Whether the members joined, still reachable here, where the tests found
// it.
pub use super::hub_checks::joined;
use super::hub_checks::{
    check_each, check_losses, check_payout, check_settled_with_honest, check_validators_in,
    hub_standing, idle_in, places,
};
use super::scripted::{READY_WITHIN_MS, ended};
use super::{START, Settling, Sim, key, received};
use crate::store::OutgoingState;

/// How long the members may take to join the hub: as long as a move into
/// it may take before it is late (`CREDIT_WAIT_MS`,
/// `engine/core/hub_funding.rs`).
pub(super) const JOINED_WITHIN_MS: u64 = 600_000;

/// How long the payments may take, after the validator's restart, at
/// production timings.
pub(super) const PAID_WITHIN_MS: u64 = 10 * 60_000;

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

/// Whether the network is ready for payments: every node has a peer and a
/// route to every other node.
pub(super) fn routed(sim: &Sim) -> bool {
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

/// Whether every payment has finished, and every outgoing payment ended
/// one way or the other.
pub(super) fn paid(sim: &Sim) -> bool {
    sim.payments.iter().all(|payment| payment.done)
        && sim.outgoing.iter().all(|&(from, _, _, outgoing)| {
            sim.nodes[from]
                .store
                .outgoing_payment(outgoing)
                .unwrap()
                .is_some_and(|row| row.state != OutgoingState::Sending)
        })
}

fn script(sim: &mut Sim, rng: &mut SeededRng, setup: &Setup) -> Result<(), Outcome> {
    for (index, role) in setup.roles.iter().enumerate() {
        sim.hub.as_deref_mut().unwrap().validators[index].role = *role;
    }
    sim.trace
        .push(format!("{} the validators are {:?}", sim.now, setup.roles));
    form(sim)?;
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
    settle_down(sim, setup.liar.map(|_| 1));
    check_calm(sim, setup.liar.map(|lie| (1, lie)))?;
    match setup.ending {
        Ending::Stall => stall(sim, rng),
        Ending::Collusion => collude(sim),
    }
}

/// The members join the hub, then the network, from node 0, so that they
/// peer over the hub. Nodes 0 and 2 never peer, so what one pays the other
/// crosses a hop.
pub(super) fn form(sim: &mut Sim) -> Result<(), Outcome> {
    join_hub(sim)?;
    sim.step(0, |core| core.ban_node(key(2).node_id()).unwrap());
    sim.step(2, |core| core.ban_node(key(0).node_id()).unwrap());
    sim.join_network(1);
    if !sim.run_until_or(sim.now + READY_WITHIN_MS, routed) {
        return Err(Outcome::liveness(
            "ready",
            "the network was not ready in time",
        ));
    }
    over_hub(sim)
}

/// The members join the hub by deposit, each within the wait of a move.
pub(super) fn join_hub(sim: &mut Sim) -> Result<(), Outcome> {
    if sim.run_until_or(START + JOINED_WITHIN_MS, joined) {
        return Ok(());
    }
    Err(Outcome::liveness(
        "joined",
        format!(
            "the members did not all join the hub in time: {}",
            hub_standing(sim)
        ),
    ))
}

/// Every peering is over the hub, or in a run across currencies over the
/// stub with commitments, a ledger both peers have: never over the XRP
/// Ledger account the members fund their hub accounts from, which is the
/// owner's alone (spec/designs/settlement-hubs.md#11-choosing-a-hub).
pub(super) fn over_hub(sim: &Sim) -> Result<(), Outcome> {
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            for method in &peer.peering.methods {
                let scheme = method.scheme;
                let ours = (sim.settling.ledgers(index).iter())
                    .any(|ledger| Settling::scheme(*ledger).0 == scheme.as_str());
                if !ours {
                    return Err(Outcome::liveness(
                        "over-hub",
                        format!(
                            "node {index} peers with {} over {scheme:?}, not the hub",
                            peer.id,
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// The network settles down: each member's account reads what touched it
/// a few blocks after the hub finalized it, a member moves funds into or
/// out of the hub as its floor and ceiling call for, and a peer short of
/// room settles once asked; the books are checked once all of that is
/// done. A liar's moves and reading are its own.
pub(super) fn settle_down(sim: &mut Sim, liar: Option<usize>) {
    sim.run_until(sim.now + CALM_MS);
    let all = places(sim);
    let settled =
        move |sim: &Sim| idle_in(sim, &all, liar) && check_settled_with_honest(sim, liar).is_ok();
    sim.run_until_or(sim.now + SETTLED_WITHIN_MS, &settled);
    // The last settlement may have executed a block before the accounts
    // read it: a while of quiet, then the accounts read the hub again.
    sim.run_until(sim.now + CALM_MS);
    sim.run_until_or(sim.now + CALM_MS, settled);
}

/// Payments across the hop between nodes 0 and 2, both ways, and one
/// between two nodes drawn: each of up to some times what a hold holds,
/// so that the holds are funded again and again.
pub(super) fn pay(sim: &mut Sim, rng: &mut SeededRng) {
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
pub(super) fn check_calm(sim: &Sim, liar: Option<(usize, MemberLie)>) -> Result<(), Outcome> {
    check_calm_with(sim, liar, &[])
}

/// [`check_calm`], with the nodes `paid_in` a peer paid into the hub, whose
/// owners paid no account reserve.
fn check_calm_with(
    sim: &Sim,
    liar: Option<(usize, MemberLie)>,
    paid_in: &[usize],
) -> Result<(), Outcome> {
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
    let liar = liar.map(|(node, _)| node);
    first_failure(
        [
            check_events(sim).map_err(|problem| Outcome::safety("events", problem)),
            super::checks::check_hub_packets(sim)
                .map_err(|problem| Outcome::safety("hub-packets", problem)),
            super::checks::check_hub_credit(sim)
                .map_err(|problem| Outcome::safety("hub-credit", problem)),
            check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
        ]
        .into_iter()
        .chain(check_each(sim, &places(sim), liar, paid_in))
        .chain([
            finished,
            check_settled_with_honest(sim, liar)
                .map_err(|problem| Outcome::liveness("settled", problem)),
        ])
        .chain(incoming)
        .collect::<Vec<_>>(),
    )
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

/// The node of the runs of joining by being paid that has no account in
/// the hub, and peers with node 1 alone.
pub const NEWCOMER: usize = 2;

/// The payer of [`NEWCOMER`]: a member that pays it in.
const PAYER: usize = 1;

/// The hub's minimum for a new account, in internal units.
fn minimum(sim: &Sim) -> u64 {
    let terms = &sim.hub.as_deref().unwrap().rules.terms;
    dassie_hub::member::units(terms.new_account_minimum())
}

/// Whether the hub has an account for node `index`'s key.
fn has_account(sim: &Sim, index: usize) -> bool {
    let hub = sim.hub.as_deref().unwrap();
    let key = dassie_hub::member::member_key(&key(index), &hub.id()).public_key();
    hub.ledger()
        .is_some_and(|ledger| ledger.account_of(&key).is_some())
}

/// Nodes 0 and 1 join a hub of 4 validators by deposit; the nodes from
/// [`NEWCOMER`] on turn their own moves off, so they never deposit, and peer
/// with [`PAYER`] alone, so that what node 0 pays them crosses it. Then the
/// network forms, every peering over the hub.
fn form_with_newcomers(sim: &mut Sim) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    for index in NEWCOMER..count {
        sim.step(index, |core| {
            core.set_hub_automatic(None, false).unwrap();
        });
    }
    let members_joined = |sim: &Sim| {
        (0..NEWCOMER).all(|index| {
            sim.core(index).hub_status().is_some_and(|status| {
                status.account.is_some() && status.status == dassie_settlement::LedgerStatus::Synced
            })
        })
    };
    if !sim.run_until_or(START + JOINED_WITHIN_MS, members_joined) {
        return Err(Outcome::liveness(
            "joined",
            format!(
                "the members did not join the hub in time: {}",
                hub_standing(sim)
            ),
        ));
    }
    for index in NEWCOMER..count {
        for other in (0..count).filter(|other| *other != index && *other != PAYER) {
            sim.step(index, |core| core.ban_node(key(other).node_id()).unwrap());
            sim.step(other, |core| core.ban_node(key(index).node_id()).unwrap());
        }
    }
    sim.join_network(PAYER + 1);
    if !sim.run_until_or(sim.now + READY_WITHIN_MS, routed) {
        return Err(Outcome::liveness(
            "ready",
            format!("the network was not ready in time: {}", hub_standing(sim)),
        ));
    }
    over_hub(sim)
}

/// Joining a hub by being paid (`node-hub`, spec/designs/settlement-hubs.md#5-joining):
/// [`NEWCOMER`], with no account in the hub, peers with [`PAYER`], a member,
/// over it. Paid less than the hub's minimum for a new account, it is not
/// paid in; paid more, its payer pays it in, though both crash around the
/// `pay`. It sets its disbursal address first, never deposits, and pays
/// and is paid over the hub from then on, each node's books holding to the
/// hub's.
#[must_use]
pub fn paid_in(seed: u64) -> Run {
    let mut rng = SeededRng::new(seed ^ 0x6a6f_696e_6564);
    let mut sim = Sim::with(seed, 3, 0, BTreeMap::new(), Settling::Hub(4));
    let result = script_paid_in(&mut sim, &mut rng);
    ended(sim, result)
}

fn script_paid_in(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    form_with_newcomers(sim)?;
    let minimum = minimum(sim);
    // Short of the minimum: what the payer owes it is not paid in.
    let short = minimum / 2 + rng.below(minimum / 4);
    sim.pay(0, NEWCOMER, short, None, None);
    if !sim.run_until_or(sim.now + PAID_WITHIN_MS, paid) {
        return Err(Outcome::liveness(
            "paid-short",
            "a payment short of the minimum did not complete",
        ));
    }
    sim.run_until(sim.now + CALM_MS);
    if has_account(sim, NEWCOMER) {
        return Err(Outcome::safety(
            "paid-in-early",
            format!("node {NEWCOMER} was paid in, owed {short} of {minimum}"),
        ));
    }
    sim.count("newcomers not paid in short of the minimum");
    // Past it: paid in, while the payer crashes with its `pay` on its way,
    // and the newcomer once its account is open.
    sim.pay(0, NEWCOMER, minimum, None, None);
    let newcomer = key(NEWCOMER).node_id();
    let paying = |sim: &Sim| {
        sim.nodes[PAYER]
            .core
            .as_ref()
            .is_some_and(|core| core.settling_with(&newcomer))
    };
    if sim.run_until_or(sim.now + PAID_WITHIN_MS, paying) {
        sim.run_until(sim.now + rng.below(250));
        if paying(sim) {
            sim.count("payers crashed paying a newcomer in");
        }
        sim.crash(PAYER);
        sim.run_until(sim.now + 1_000 + rng.below(9_000));
        sim.start(PAYER);
    }
    if !sim.run_until_or(sim.now + PAID_WITHIN_MS, |sim| has_account(sim, NEWCOMER)) {
        return Err(Outcome::liveness(
            "paid-in",
            format!(
                "node {NEWCOMER} was never paid in, owed {}: {}",
                sim.owes(PAYER, &key(NEWCOMER).node_id()),
                hub_standing(sim)
            ),
        ));
    }
    sim.run_until(sim.now + rng.below(5_000));
    sim.crash(NEWCOMER);
    sim.run_until(sim.now + 1_000 + rng.below(9_000));
    sim.start(NEWCOMER);
    sim.count("newcomers paid in");
    // Over the hub from then on, both ways: what it is paid it takes from
    // its payer's hold, and spends from its own, out of its account, which
    // its payer's `pay` left it little beyond the account reserve.
    let more = minimum / 4 + rng.below(minimum);
    sim.pay(0, NEWCOMER, more, None, None);
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    sim.run_until(sim.now + CALM_MS);
    let back = more / 4 + rng.below(more / 4);
    sim.pay(NEWCOMER, 0, back, None, None);
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    settle_down(sim, None);
    first_failure([
        check_calm_with(sim, None, &[NEWCOMER]),
        check_paid_in(sim).map_err(|problem| Outcome::safety("paid-in", problem)),
    ])
}

/// [`NEWCOMER`]'s account in the hub was opened by its payer's `pay`, at
/// the minimum, never by a deposit, has its disbursal address and made
/// operations besides. That setting it came first is not read from the
/// hub, which keeps no history here: the hub fails every other operation of
/// a member's account without one (`Failure::NoDisbursal`), and the
/// member's account sends none before it.
fn check_paid_in(sim: &Sim) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    let key = dassie_hub::member::member_key(&key(NEWCOMER), &hub.id()).public_key();
    let number = ledger
        .account_of(&key)
        .ok_or_else(|| format!("node {NEWCOMER} has no account in the hub"))?;
    let account = ledger.account(number).unwrap();
    if account.deposited != 0 {
        return Err(format!(
            "node {NEWCOMER} deposited {} into the hub",
            account.deposited
        ));
    }
    if account.disbursal.is_none() {
        return Err(format!("node {NEWCOMER} has no disbursal address"));
    }
    // Its disbursal address, and the hold it paid back over.
    if account.sequence < 2 {
        return Err(format!(
            "node {NEWCOMER} made {} operations in the hub",
            account.sequence
        ));
    }
    Ok(())
}

/// Fresh keys cycled through a member (`node-hub`,
/// spec/decisions/hubs-only.md): nodes from [`NEWCOMER`] on, each with no
/// account in the hub, peer with [`PAYER`], are paid less than the hub's
/// minimum for a new account, try to spend more than they are owed, and go
/// away. Each spends some of what it is owed, none more, none is paid in,
/// and the member pays nothing into the hub for them.
#[must_use]
pub fn cycled(seed: u64) -> Run {
    let mut rng = SeededRng::new(seed ^ 0x6379_636c_6564);
    let mut sim = Sim::with(seed, 4, 0, BTreeMap::new(), Settling::Hub(4));
    let result = script_cycled(&mut sim, &mut rng);
    ended(sim, result)
}

fn script_cycled(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    form_with_newcomers(sim)?;
    let minimum = minimum(sim);
    let count = sim.nodes.len();
    for index in NEWCOMER..count {
        let owed = minimum / 4 + rng.below(minimum / 2);
        sim.pay(0, index, owed, None, None);
        if !sim.run_until_or(sim.now + PAID_WITHIN_MS, paid) {
            return Err(Outcome::liveness(
                "paid-short",
                format!("a payment to node {index} did not complete"),
            ));
        }
        let first = sim.payments.len();
        sim.pay(index, 0, 2 * owed, None, None);
        sim.run_until(sim.now + PAID_WITHIN_MS);
        let spent = sim.payments[first..]
            .iter()
            .map(|payment| received(sim, payment))
            .sum::<u64>();
        if spent > owed {
            return Err(Outcome::safety(
                "overspent",
                format!("node {index} spent {spent}, owed {owed}"),
            ));
        }
        // It spends what it is owed through the payer before it is paid
        // in (spec/designs/settlement-hubs.md#5-joining), or the bound
        // above says nothing.
        if spent == 0 {
            return Err(Outcome::liveness(
                "unspent",
                format!("node {index} spent nothing of the {owed} it is owed"),
            ));
        }
        let peer = key(index).node_id();
        if sim.owes(PAYER, &peer) < 0 {
            return Err(Outcome::safety(
                "trusted",
                format!("node {PAYER} let node {index} owe it"),
            ));
        }
        sim.crash(index);
        sim.trace
            .push(format!("{} node {index} goes away", sim.now));
    }
    sim.run_until(sim.now + CALM_MS);
    if let Some(index) = (NEWCOMER..count).find(|index| has_account(sim, *index)) {
        return Err(Outcome::safety(
            "paid-in",
            format!("node {index} was paid in, owed less than the minimum"),
        ));
    }
    sim.count("fresh keys cycled");
    first_failure([
        check_validators_in(sim, 0).map_err(|problem| Outcome::safety("hub-agreement", problem)),
        super::checks::check_hub_packets(sim)
            .map_err(|problem| Outcome::safety("hub-packets", problem)),
        super::checks::check_hub_credit(sim)
            .map_err(|problem| Outcome::safety("hub-credit", problem)),
        check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::hub_checks::check_escrow_in;

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
        // The owner puts enough in for the least payout, whatever the
        // node's own moves keep there.
        let least = sim.hub.as_deref().unwrap().rules.terms.fees.least_payout;
        sim.run_until(sim.now + 30_000);
        sim.step(0, |core| {
            core.hub_deposit(2 * least * dassie_xrpl::UNITS_PER_DROP)
                .unwrap();
        });
        let can_pay = |sim: &Sim| {
            let core = sim.nodes[0].core.as_ref().unwrap();
            core.hub_status().is_some_and(|status| {
                status.moving.is_none() && status.balance > 2 * least * dassie_xrpl::UNITS_PER_DROP
            })
        };
        assert!(
            sim.run_until_or(sim.now + 120_000, can_pay),
            "the deposit did not land"
        );
        check_escrow_in(&sim, 0).unwrap();
        let hub = sim.hub.as_deref().unwrap();
        let before = sim.ledger.balance(hub.escrow()).unwrap_or(0);
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
        check_escrow_in(&sim, 0).unwrap();
    }
}
