//! The checks the node simulation judges a run over settlement hubs by
//! (spec/areas/testing.md#over-a-settlement-hub), shared by every run over
//! hubs run in the simulation: the scripted ones (`node-hub` and the runs
//! beside it, over one hub or two, of XRP or of a token, on the testnet or
//! on mainnet) and the `node` simulation's runs over hubs
//! ([`swarm`](super::swarm)). Each hub is judged on its own, named by its
//! place among the run's hubs ([`HubSpec`](super::hub::HubSpec)): its
//! validators, its escrow, and each member by its own books
//! (spec/decisions/own-books.md), in the hub and in the owner book it funds
//! its account there from. What a member lost on a peer, and whether it
//! settled, are judged over every method of its peerings together. And the
//! payout of a hub of XRP, once it stalls.

use std::collections::BTreeMap;

use dassie_hub::escrow::PayoutTransaction;
use dassie_hub::ledger::TESTNET_NETWORK;
use dassie_hub::rules::Asset;
use dassie_kit::sim::Outcome;
use dassie_ledger::LedgerId;
use dassie_settlement::LedgerStatus;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::api;
use dassie_xrpl::codec::Amount;
use dassie_xrpl::fake::{FAUCET_DROPS, RESERVE_BASE};

use super::checks::{
    check_anchored, check_delivered, check_funded, check_held, check_true, first_failure,
};
use super::hub::{HubNet, owed};
use super::{
    DSUSD, HUB, HUB_DSUSD, HUB_LIVE, HUB_RLUSD, LIVE_XRP, RLUSD, Sim, XRP, key, received,
    xrpl_account,
};
use crate::engine::core::{Direction, HubStatus};

/// The ledger a hub's members book in it.
#[must_use]
pub fn ledger_of(hub: &HubNet) -> LedgerId {
    match (hub.spec.asset, hub.spec.network == TESTNET_NETWORK) {
        (Asset::Xrp, true) => HUB,
        (Asset::Token { .. }, true) => HUB_DSUSD,
        (Asset::Xrp, false) => HUB_LIVE,
        (Asset::Token { .. }, false) => HUB_RLUSD,
    }
}

/// The owner book a member funds its account in the hub of `ledger` from
/// (spec/areas/xrpl.md#owner-books), and the token the hub holds, if it is
/// not XRP.
fn book_of(ledger: LedgerId) -> (LedgerId, Option<IssuedAsset>) {
    match ledger {
        LedgerId::HubXrplTestnetDsusdUsd => (DSUSD, Some(IssuedAsset::DSUSD)),
        LedgerId::HubXrplXrp => (LIVE_XRP, None),
        LedgerId::HubXrplRlusdUsd => (RLUSD, Some(IssuedAsset::RLUSD)),
        _ => (XRP, None),
    }
}

/// The places of the run's hubs.
#[must_use]
pub fn places(sim: &Sim) -> Vec<usize> {
    sim.hubs().map(|hub| hub.spec.place).collect()
}

/// The members of the hub at `place`: the nodes with its ledger.
pub fn members_of(sim: &Sim, place: usize) -> impl Iterator<Item = usize> + '_ {
    let ledger = ledger_of(sim.hub_at(place));
    (0..sim.nodes.len()).filter(move |index| sim.settling.ledgers(*index).contains(&ledger))
}

/// Node `index`'s account in the hub at `place` and its funding, while the
/// node runs and the hub's scheme with it.
#[must_use]
pub fn status_in(sim: &Sim, index: usize, place: usize) -> Option<HubStatus> {
    let id = *sim.hub_at(place).id().0.as_bytes();
    sim.nodes[index]
        .core
        .as_ref()?
        .hub_statuses()
        .into_iter()
        .find(|status| status.hub_id == id)
}

/// Whether node `index`'s account in the hub at `place` is in step with
/// it.
#[must_use]
pub fn in_step_in(sim: &Sim, index: usize, place: usize) -> bool {
    status_in(sim, index, place).is_some_and(|status| status.status == LedgerStatus::Synced)
}

/// Whether node `index` has a move of its funds into or out of the hub at
/// `place` on its way.
#[must_use]
pub fn moving_in(sim: &Sim, index: usize, place: usize) -> bool {
    status_in(sim, index, place).is_some_and(|status| status.moving.is_some())
}

/// Whether the hub at `place` is node `index`'s first, whose reading the
/// node shows a simulation (`Core::hub_sequence`, `Core::hub_reading`).
fn first_of(sim: &Sim, index: usize, place: usize) -> bool {
    let id = *sim.hub_at(place).id().0.as_bytes();
    sim.nodes[index]
        .core
        .as_ref()
        .and_then(crate::engine::Core::hub_status)
        .is_some_and(|status| status.hub_id == id)
}

/// Whether every member of each of the run's hubs has its account there,
/// in step with it.
#[must_use]
pub fn joined(sim: &Sim) -> bool {
    places(sim).into_iter().all(|place| {
        members_of(sim, place).all(|index| {
            status_in(sim, index, place).is_some_and(|status| {
                status.account.is_some() && status.status == LedgerStatus::Synced
            })
        })
    })
}

/// Whether every member of the hubs at `places` but the `liar` has its
/// account there in step with it, and no move into or out of it on its
/// way; and every member, the liar too, booked in its owner books what
/// their accounts hold ([`books_caught_up`]).
#[must_use]
pub fn idle_in(sim: &Sim, places: &[usize], liar: Option<usize>) -> bool {
    places.iter().all(|&place| {
        members_of(sim, place)
            .filter(|index| Some(*index) != liar)
            .all(|index| in_step_in(sim, index, place) && !moving_in(sim, index, place))
            && members_of(sim, place).all(|index| books_caught_up(sim, index))
    })
}

/// Whether node `index`'s owner books caught up with their accounts on
/// the XRP Ledger: no payment of its owner's is held on its way out of
/// any, and each holds what [`check_held`] wants of it. Each side of a move
/// into or out of a hub books its half when it sees it
/// (`engine/core/hub_funding.rs`), so the hub's side may show a move done
/// while the ledger already applied a payment the owner book has yet to
/// book: a run waits for that, within its deadline, before it judges the
/// books, which still fail if they never catch up.
fn books_caught_up(sim: &Sim, index: usize) -> bool {
    sim.nodes[index].core.as_ref().is_some_and(|core| {
        sim.settling.owner_books(index).iter().all(|&book| {
            !core.owner_payments_in_flight(book) && check_held(sim, index, book).is_ok()
        })
    })
}

/// What the escrow of the hub at `place` holds of the hub's asset: drops of
/// XRP, or millionths of its token.
#[must_use]
pub fn escrow_holds(sim: &Sim, place: usize) -> u64 {
    let hub = sim.hub_at(place);
    match book_of(ledger_of(hub)).1 {
        None => sim.ledger.balance(hub.escrow()).unwrap_or(0),
        Some(token) => sim
            .ledger
            .token_balance(hub.escrow(), token)
            .and_then(|value| value.floor_units(6))
            .and_then(|micros| u64::try_from(micros).ok())
            .unwrap_or(0),
    }
}

/// Each member's books over the run's hubs, on their own, once the network
/// is calm and no member lies: what a run on one ledger checks of them
/// ([`checks::check_books`](super::checks::check_books)), the hubs' way.
/// In each hub, its validators broke nothing, the escrow holds what the hub
/// owes, and each member's books hold what its schemes hold, in the hub as
/// its honest validators have it, its owner's funds in the hub and in the
/// book that funds it together moved by what its receivers took, its
/// account is in step with the hub and no move of its funds is on its way
/// ([`check_each`]). Over all of them, what a member booked as lost on a
/// peer is no more than the holds between them, its books hold to the fake
/// ledger's truth, and no honest peer is at its ask point.
pub(super) fn check_books(sim: &Sim) -> Result<(), Outcome> {
    let places = places(sim);
    first_failure(
        [check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem))]
            .into_iter()
            .chain(check_each(sim, &places, None, &[]))
            .chain([
                check_true(sim).map_err(|problem| Outcome::safety("proven", problem)),
                check_funded(sim).map_err(|problem| Outcome::liveness("funded", problem)),
                check_anchored(sim).map_err(|problem| Outcome::liveness("anchored", problem)),
                check_settled_with_honest(sim, None)
                    .map_err(|problem| Outcome::liveness("settled", problem)),
                check_delivered(sim).map_err(|problem| Outcome::liveness("delivered", problem)),
            ])
            .collect::<Vec<_>>(),
    )
}

/// What each hub at `places` is checked by once the network is calm, every
/// check of every hub, for [`first_failure`]: its validators
/// ([`check_validators_in`]), each member's books ([`check_books_in`]) and
/// its owner's ([`check_owners_in`]), its escrow ([`check_escrow_in`]), and
/// each member's account in step ([`check_in_step_in`]) with no move on its
/// way ([`check_moved_in`]). A `liar`'s reading and moves are its own; the
/// nodes `paid_in` by a peer paid no account reserve.
pub(super) fn check_each(
    sim: &Sim,
    places: &[usize],
    liar: Option<usize>,
    paid_in: &[usize],
) -> Vec<Result<(), Outcome>> {
    let mut checks = Vec::new();
    for &place in places {
        checks.extend([
            check_validators_in(sim, place)
                .map_err(|problem| Outcome::safety("hub-agreement", problem)),
            check_books_in(sim, place, liar).map_err(|problem| {
                Outcome::safety("hub-books", format!("{problem}; {}", hub_standing(sim)))
            }),
            check_owners_in(sim, place, paid_in)
                .map_err(|problem| Outcome::safety("hub-owner-books", problem)),
            check_escrow_in(sim, place).map_err(|problem| Outcome::safety("hub-escrow", problem)),
            check_in_step_in(sim, place, liar)
                .map_err(|problem| Outcome::liveness("hub-in-step", problem)),
            check_moved_in(sim, place, liar)
                .map_err(|problem| Outcome::liveness("hub-moves", problem)),
        ]);
    }
    checks
}

/// What a run over hubs still waits for before its books may be judged,
/// if anything: a member's account that has not read its hub up to a
/// recent block, a move of its funds into or out of a hub on its way, or,
/// in a hub other than its first, an account in step that does not yet
/// see what the hub last applied to it ([`read_up_in`]). In its first hub
/// [`check_books_in`] tells by itself whether the member read up to the
/// newest block; in another it cannot, so a run that judges the books as
/// soon as nothing waits, as the `node` simulation's do after the heal,
/// waits for that instead of judging a member a block behind.
pub(super) fn waiting(sim: &Sim) -> Option<String> {
    places(sim).into_iter().find_map(|place| {
        check_in_step_in(sim, place, None)
            .err()
            .or_else(|| check_moved_in(sim, place, None).err())
            .or_else(|| {
                members_of(sim, place)
                    .filter(|index| in_step_in(sim, *index, place) && !first_of(sim, *index, place))
                    .find(|index| !read_up_in(sim, *index, place))
                    .map(|index| {
                        format!(
                            "node {index} has not read its account in the hub at {place} up to \
                             what the hub applied to it"
                        )
                    })
            })
    })
}

/// Whether node `index` sees its account in the hub at `place` as the
/// hub's honest validators have it: its balance, and each of its peers'
/// holds toward it, the two its books in the hub hold
/// ([`check_books_in`]). With no honest validator running, or no account
/// there, there is nothing to read, and the checks say so.
fn read_up_in(sim: &Sim, index: usize, place: usize) -> bool {
    let hub = sim.hub_at(place);
    let Some(ledger) = hub.ledger() else {
        return true;
    };
    let member = dassie_hub::member::member_key(&key(index), &hub.id()).public_key();
    let (Some(status), Some(number)) = (status_in(sim, index, place), ledger.account_of(&member))
    else {
        return true;
    };
    let Some(account) = ledger.account(number) else {
        return true;
    };
    let units = dassie_hub::member::units;
    let mut hub_has: Vec<(u32, u64)> = ledger
        .holds()
        .filter(|(_, hold)| hold.payee == number)
        .map(|(_, hold)| (hold.payer.0, units(hold.amount)))
        .collect();
    let mut sees: Vec<(u32, u64)> = status
        .taking
        .iter()
        .map(|hold| (hold.account, hold.amount))
        .collect();
    hub_has.sort_unstable();
    sees.sort_unstable();
    status.balance == units(account.balance) && sees == hub_has
}

/// How the run's hubs and their members stand, for a failure's message:
/// each validator's last final block, view, and whether a quorum checked
/// the escrow; each member's account in each hub and how it stands; what
/// each escrow holds; and each node's reading of its first hub, its XRP on
/// the ledger and its token's line.
pub(super) fn hub_standing(sim: &Sim) -> String {
    let hubs: Vec<String> = sim
        .hubs()
        .map(|hub| {
            let place = hub.spec.place;
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
            let members: Vec<String> = members_of(sim, place)
                .map(|index| match status_in(sim, index, place) {
                    None => format!("node {index}: no account"),
                    Some(status) => {
                        let key =
                            dassie_hub::member::member_key(&key(index), &hub.id()).public_key();
                        let known = hub.ledger().and_then(|ledger| ledger.account_of(&key));
                        format!(
                            "node {index}: {:?} (the hub's {known:?}) {:?} balance {} moving {:?}",
                            status.account,
                            status.status,
                            status.balance,
                            status.moving.map(|moving| (moving.direction, moving.drops)),
                        )
                    }
                })
                .collect();
            format!(
                "the {} hub at {place}: validators {validators:?}, members {members:?}, its \
                 escrow holding {} and {:?} drops",
                ledger_of(hub),
                escrow_holds(sim, place),
                sim.ledger.balance(hub.escrow()),
            )
        })
        .collect();
    let nodes: Vec<String> = (0..sim.nodes.len())
        .map(|index| {
            format!(
                "node {index} reading {:?}, XRP {:?}, line {:?}",
                sim.nodes[index]
                    .core
                    .as_ref()
                    .and_then(crate::engine::Core::hub_reading),
                sim.ledger.balance(sim.xrpl_account(index)),
                sim.settling.token().and_then(|_| sim.line(index)),
            )
        })
        .collect();
    format!("{}; {nodes:?}", hubs.join("; "))
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

/// The validators of the hub at `place` broke nothing: their invariants
/// held after every step, no two honest ones finalized different states at
/// one height, and no evidence named an honest one.
pub(super) fn check_validators_in(sim: &Sim, place: usize) -> Result<(), String> {
    match sim.hub_at(place).breaches.as_slice() {
        [] => Ok(()),
        breaches => Err(format!("the hub at {place}: {}", breaches.join("; "))),
    }
}

/// What each node booked as lost on a peer over a hub is no more than the
/// holds between them in that hub: its own toward the peer, which a payee
/// that claims what it should not takes from, and the peer's toward it,
/// from which it takes what a payer that sends no claims owes, paying a fee
/// per claim (spec/designs/settlement-hubs.md#13-how-it-fits-the-node). By
/// its own books: its losses as it booked them, its holds as its scheme
/// read them. Each method of a peering, each hub the two share, is judged
/// apart, in its own asset, which bounds what a peer can cost summed over
/// them.
pub(super) fn check_losses(sim: &Sim) -> Result<(), String> {
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

/// Each member's books hold what its schemes hold: each owner book its
/// account's XRP less the reserve or its token's line, and in the hub at
/// `place` what its account and its peers' holds toward it hold, as the
/// hub's honest validators have it once the network is calm and it read the
/// operations of its own they applied.
///
/// Once the network is calm and an honest node read up to the hub's newest
/// block, it has read every operation of its own the hub applied; only a
/// liar's claims, held back to just before their commitments expire, may
/// land after, and its books are checked once it read them. How far it
/// read, the node shows a simulation of its first hub only; in another,
/// its books are checked once its account is in step, so a run first leaves
/// the network calm long enough for it to read the hub, as the scripted
/// runs' settling down does, or waits until it sees its account as the hub
/// has it ([`waiting`]), as the `node` simulation does. One whose account
/// is not in step at all is left to [`check_in_step_in`]. Nor does a run
/// wait for a liar's moves and reading: its books in the hub are checked
/// once it read up to the newest block, its moves done.
pub(super) fn check_books_in(sim: &Sim, place: usize, liar: Option<usize>) -> Result<(), String> {
    let hub = sim.hub_at(place);
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    let ours = ledger_of(hub);
    for index in members_of(sim, place) {
        for &book in sim.settling.owner_books(index) {
            check_held(sim, index, book)?;
        }
        let booked = check_held(sim, index, ours)?;
        if !in_step_in(sim, index, place) {
            continue;
        }
        let member = dassie_hub::member::member_key(&key(index), &hub.id()).public_key();
        let number = ledger
            .account_of(&member)
            .ok_or_else(|| format!("node {index} has no account in the {ours} hub"))?;
        let account = ledger.account(number).unwrap();
        let first = first_of(sim, index, place);
        let cursor = sim
            .core(index)
            .hub_reading()
            .map_or(0, |(cursor, _, _)| cursor);
        // A liar's moves and reading are its own, which the run waits for
        // neither of: one with a move on its way, or that read the hub only
        // up to a block before the newest, may not have read a deposit the
        // hub credited it, which moves no sequence of its own. Its books in
        // the hub are checked once it read up to the newest block, or in a
        // hub other than its first sees its account as the hub has it, its
        // moves done, as [`check_owners_in`] checks a node's.
        let behind = if first {
            cursor < ledger.height()
        } else {
            !read_up_in(sim, index, place)
        };
        if Some(index) == liar && (moving_in(sim, index, place) || behind) {
            continue;
        }
        // An operation of its own the hub applied after the state it read
        // is booked once it reads it. An honest node may be a block or so
        // behind the newest, as it reads the hub every so often; once it
        // read up to it, it read all.
        if first {
            let read = sim.core(index).hub_sequence();
            if read != Some(account.sequence) {
                if Some(index) == liar || cursor < ledger.height() {
                    continue;
                }
                return Err(format!(
                    "node {index} read its account in the {ours} hub up to sequence {read:?}, \
                     where the hub applied its operations up to {}",
                    account.sequence
                ));
            }
        }
        let toward: u64 = ledger
            .holds()
            .filter(|(_, hold)| hold.payee == number)
            .map(|(_, hold)| hold.amount)
            .sum();
        let held = i128::from(dassie_hub::member::units(account.balance + toward));
        if booked != held {
            return Err(format!(
                "node {index} books {booked} in the {ours} hub, where its account holds {} and \
                 its peers' holds toward it {toward}",
                account.balance
            ));
        }
    }
    Ok(())
}

/// The ledger node `to`'s incoming payment `incoming` pays into.
fn into_ledger(sim: &Sim, to: usize, incoming: crate::store::PaymentId) -> Option<LedgerId> {
    sim.nodes[to]
        .store
        .incoming_payment(incoming)
        .unwrap()
        .map(|row| row.ledger)
}

/// What an outgoing payment moved, on the ledgers it moved it on.
struct Moved {
    /// The paying node.
    from: usize,
    /// The node paid.
    to: usize,
    /// The ledger that paid, and what it sent.
    sent: Option<(LedgerId, u64)>,
    /// The ledger paid into, and what its receiver got.
    got: Option<(LedgerId, u64)>,
}

/// What each outgoing payment moved.
fn outgoing_on(sim: &Sim) -> impl Iterator<Item = Moved> + '_ {
    sim.outgoing
        .iter()
        .map(|&(from, to, incoming, outgoing)| Moved {
            from,
            to,
            sent: sim.nodes[from]
                .store
                .outgoing_payment(outgoing)
                .unwrap()
                .map(|row| (row.ledger, row.sent_amount)),
            got: sim.nodes[to]
                .store
                .incoming_payment(incoming)
                .unwrap()
                .map(|row| (row.ledger, row.received_amount)),
        })
}

/// Each owner's funds in the hub at `place` and in the owner book it funds
/// its account there from together moved by what its receivers took on the
/// hub's ledger: on XRP, what the faucet gave or its owner deposited, less
/// the XRP Ledger's reserve; on a token, what the issuer minted, less what
/// it clawed back; either less the hub's account reserve, which neither the
/// owner nor anyone else can spend. The account reserve of a node
/// `paid_in` by a peer came out of that peer's `pay`, not out of its
/// owner's funds (spec/designs/settlement-hubs.md#5-joining). So do its
/// outgoing payments on the hub's ledger: what each received there counts
/// as its receiver's, and the payer's account paid what it sent. And each
/// payment's token on the hub's ledger, every node's, the liar's among
/// them, paid what it sent, as
/// [`checks::check_owner_books`](super::checks::check_owner_books) has it:
/// a token's books follow its packets, not what the node read of the hub.
///
/// It leaves out the XRP an owner's XRP book pays a token hub's escrow as
/// gas, which leaves that book for good (spec/decisions/token-hubs.md): a
/// run that has a member top a token escrow up must not judge that
/// member's XRP hub by it. None does: every run founds a token escrow with
/// as much XRP as the testnet's faucet gives, far above the least gas, and
/// only `node-hub-dsusd` drains it, which has no XRP hub.
pub(super) fn check_owners_in(sim: &Sim, place: usize, paid_in: &[usize]) -> Result<(), String> {
    let hub = sim.hub_at(place);
    let ledger = ledger_of(hub);
    let (book, token) = book_of(ledger);
    let account_reserve = i128::from(dassie_hub::member::units(
        hub.rules.terms.fees.account_reserve,
    ));
    let outgoing: Vec<_> = outgoing_on(sim).collect();
    // One behind the hub, or moving funds, has not booked all of it yet:
    // the checks of being in step and of moves say so.
    let done = |index: &usize| in_step_in(sim, *index, place) && !moving_in(sim, *index, place);
    for index in members_of(sim, place).filter(done) {
        let owner = |ledger| {
            sim.balances_on(index, ledger)
                .get("equity/owner")
                .copied()
                .unwrap_or(0)
        };
        let pool = owner(book) + owner(ledger);
        let deposit = match token {
            None => {
                let fundings = i128::from(sim.fundings.get(&index).copied().unwrap_or(0));
                (fundings * i128::from(FAUCET_DROPS) - i128::from(RESERVE_BASE)) * 1_000
            }
            Some(_) => {
                let (minted, clawed) = sim.issued.get(&index).copied().unwrap_or((0, 0));
                minted - clawed
            }
        };
        let reserve = if paid_in.contains(&index) {
            0
        } else {
            account_reserve
        };
        let received: i128 = sim
            .payments
            .iter()
            .chain(&sim.abandoned)
            .filter(|payment| {
                payment.to == index && into_ledger(sim, index, payment.incoming) == Some(ledger)
            })
            .map(|payment| i128::from(received(sim, payment)))
            .sum::<i128>()
            + outgoing
                .iter()
                .filter(|moved| moved.to == index)
                .filter_map(|moved| moved.got.filter(|(on, _)| *on == ledger))
                .map(|(_, got)| i128::from(got))
                .sum::<i128>();
        if pool - (deposit - reserve) != received {
            return Err(format!(
                "node {index}'s owner gained {} in the {ledger} hub for {received} received; its \
                 books: {:?} and {:?}",
                pool - (deposit - reserve),
                sim.balances_on(index, book),
                sim.balances_on(index, ledger)
            ));
        }
        let paid: i128 = outgoing
            .iter()
            .filter(|moved| moved.from == index)
            .filter_map(|moved| moved.sent.filter(|(on, _)| *on == ledger))
            .map(|(_, sent)| i128::from(sent))
            .sum();
        let payer = -sim
            .balances_on(index, ledger)
            .get("equity/owner/local/pay")
            .copied()
            .unwrap_or(0);
        if payer != paid {
            return Err(format!(
                "node {index}'s payer paid {payer} in the {ledger} hub for {paid} its outgoing \
                 payments sent"
            ));
        }
    }
    for payment in sim
        .payments
        .iter()
        .filter(|payment| payment.ledger == ledger)
    {
        let index = payment.from;
        let account = format!("equity/owner/btp/{}", payment.token.0);
        let token = |ledger| {
            sim.balances_on(index, ledger)
                .get(&account)
                .copied()
                .unwrap_or(0)
        };
        let paid = -(token(book) + token(ledger));
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

/// The escrow of the hub at `place` holds in the hub's asset what the hub
/// owes, and an XRP hub's what its sponsor left in it, but for what is on
/// its way between them; the hub wrote off no reset, which the fake ledger
/// never makes; and the fake ledger made or lost no XRP and no token. A
/// token escrow's XRP is gas, which credits no account.
///
/// A deposit on its way, a liar's among them, is in the escrow and not yet
/// in the hub: the escrow holds more than the hub owes, by no more than the
/// deposits the members are making. A withdrawal the hub queued is part of
/// what it owes until a quorum attests that the escrow paid it: once the
/// escrow paid it and until then, the escrow holds less than the hub owes,
/// by no more than the queued withdrawals and their ledger fees, whatever
/// the members think of their moves. A lying server makes a validator's
/// watcher attest nothing, never something false
/// (spec/areas/hubs.md#what-validators-attest), so this holds after every
/// ledger.
pub(super) fn check_escrow_in(sim: &Sim, place: usize) -> Result<(), String> {
    let hub = sim.hub_at(place);
    let ledger = hub.ledger().ok_or("no honest validator runs")?;
    let depositing: u64 = members_of(sim, place)
        .filter_map(|index| status_in(sim, index, place)?.moving)
        .filter(|moving| moving.direction == Direction::Deposit)
        .map(|moving| moving.drops)
        .sum();
    let withdrawing: u64 = ledger
        .withdrawals()
        .map(|(_, withdrawal)| withdrawal.amount + withdrawal.ledger_fee)
        .sum();
    let held = escrow_holds(sim, place);
    let left = match book_of(ledger_of(hub)).1 {
        None => hub.escrow_funding,
        Some(_) => 0,
    };
    let owed = left + ledger.supply();
    if held + withdrawing < owed || held > owed + depositing {
        return Err(format!(
            "the escrow of the {} hub at {place} holds {held}, while the hub owes {} and the \
             sponsor left {left}, with {depositing} on its way in and {withdrawing} queued out",
            ledger_of(hub),
            ledger.supply()
        ));
    }
    if let Some(at) = ledger.written_off_at() {
        return Err(format!(
            "the hub at {place} wrote off a reset at {at:?} that the ledger never made"
        ));
    }
    if sim.ledger.total() != dassie_xrpl::MAX_DROPS {
        return Err("the fake ledger made or lost XRP".into());
    }
    if !sim.ledger.tokens_conserved() {
        return Err("the fake ledger made or lost a token".into());
    }
    Ok(())
}

/// Once the network is calm, every honest member's account in the hub at
/// `place` is in step with it: it read what touched it up to a recent final
/// block. A liar's is its own affair.
pub(super) fn check_in_step_in(sim: &Sim, place: usize, liar: Option<usize>) -> Result<(), String> {
    match members_of(sim, place)
        .filter(|index| Some(*index) != liar)
        .find(|index| !in_step_in(sim, *index, place))
    {
        Some(index) => Err(format!(
            "node {index}'s account in the hub at {place} is not in step with it: {}",
            hub_standing(sim)
        )),
        None => Ok(()),
    }
}

/// Once the network is calm, no honest member has funds on their way into
/// or out of the hub at `place`: each move is done within its wait
/// (`CREDIT_WAIT_MS`). A liar's moves are its own affair: one that makes
/// operations of its own account behind its node's back holds up its
/// node's.
pub(super) fn check_moved_in(sim: &Sim, place: usize, liar: Option<usize>) -> Result<(), String> {
    match members_of(sim, place)
        .filter(|index| Some(*index) != liar)
        .find(|index| moving_in(sim, *index, place))
    {
        Some(index) => Err(format!(
            "node {index} still moves funds into or out of the hub at {place}: {:?}",
            status_in(sim, index, place).and_then(|status| status.moving)
        )),
        None => Ok(()),
    }
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
