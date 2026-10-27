//! The node simulation's runs of owner books (spec/areas/xrpl.md#owner-books,
//! spec/decisions/hubs-only.md): each node keeps its owner's books of the
//! XRP Ledger on its account against the fake ledger, and peers on none of
//! them.
//!
//! `node-owner-books` runs on the testnet: each node runs the owner books
//! of XRP and DSUSD (`xrpl-testnet-xrp`, `xrpl-testnet-dsusd`, each
//! `{"peers": false}`), funded from the faucet, which the fake ledger's
//! issuer mints DSUSD into, and peers and pays over the stub with unlimited
//! trust as the scripted runs do (`scripted.rs`). While payments are under
//! way, each owner pays out of its XRP book to an address outside the node,
//! through the owner's payments, and the issuer claws back and freezes
//! lines, deep or for every holder, and lifts the freeze. Then each node's
//! connection to the ledger comes back to servers that tell each kind of
//! lie in turn, its owner asking the faucet for more and paying out again
//! meanwhile; a validator rotates its key, a quorum of the validators is
//! withheld, and the publisher's list lapses before a new one comes.
//!
//! `node-owner-books-live` runs the same books in the live realm on the fake
//! ledger's mainnet (`xrpl-xrp`, `xrpl-rlusd`), funded by the owners'
//! deposits, since mainnet has no faucet, through the RLUSD issuer's acts and
//! a lying server for each node. The stub runs only in the test realm, so
//! these nodes peer with no one, and the run judges their books alone.
//!
//! Each node is judged by its own books (spec/decisions/own-books.md): its
//! owner books hold to its account, the XRP book its account's XRP less the
//! reserve and the token's book its line; what the issuer did is booked as
//! its owner's deposits and withdrawals; its XRP book moved by what its
//! owner deposited and paid out, as the fake ledger has it; no XRP and no
//! token was made or lost; the token's peg guard reads its pool at a
//! dollar; every node anchored; and every ledger of every node is synced
//! at the end.

use std::collections::BTreeMap;

use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_ledger::{LedgerId, Name};
use dassie_settlement::{LedgerStatus, OwnerPaymentState};
use dassie_xrpl::DROPS_PER_XRP;
use dassie_xrpl::account::{LIST_REFRESH_MS, LIST_RETRY_MS, STALL_MS};
use dassie_xrpl::fake::{FAUCET_DROPS, Lie, RESERVE_BASE};

use super::checks::{
    check, check_anchored, check_events, check_funded, check_held, check_ledgers, check_pegged,
    check_settled, check_true, check_withdrawn, first_failure,
};
use super::scripted::{PAID_WITHIN_MS, READY_WITHIN_MS, ended, paid, script, size};
use super::{IssuerFreeze, LONGEST_LIE_MS, START, Settling, Sim};

/// How long a server lies to a node: the longest a drawn one does.
const LIE_MS: u64 = LONGEST_LIE_MS;

/// How long a quorum of the validators is withheld: longer than the account
/// waits before it connects anew.
const WITHHELD_MS: u64 = 4 * STALL_MS;

/// How long the list a publisher lets lapse lasts: longer than a node takes
/// to fetch it again, so that every node holds it when it expires.
const LAPSING_LIST_MS: u64 = 2 * LIST_REFRESH_MS;

/// The most the issuer claws back at once, in dollars: a few payments'
/// worth, well within what it minted.
const MOST_CLAWED_DOLLARS: u64 = 5;

/// The least an owner pays out of its XRP book at once, in drops: the base
/// reserve, which the outside address it pays needs to exist.
const LEAST_PAID_OUT_DROPS: u64 = DROPS_PER_XRP;

/// The owners' payments out of their XRP books: each node's last one, to
/// follow it until its ledger decided it.
#[derive(Default)]
struct PaidOut {
    last: BTreeMap<usize, Name>,
}

/// The address outside the node that node `index`'s owner pays out to:
/// one of its own, which no node and no issuer holds.
fn outside(index: usize) -> dassie_xrpl::AccountId {
    dassie_xrpl::AccountId([0xe0 + u8::try_from(index).unwrap(); 20])
}

impl PaidOut {
    /// Node `index`'s owner pays `drops` out of its XRP book to its
    /// address outside the node, through the owner's payments, unless its
    /// last one is still on its way. The node may refuse it, as it does
    /// while its book cannot sign, such as while its server lies.
    fn pay(&mut self, sim: &mut Sim, index: usize, drops: u64) {
        if self.on_its_way(sim, index) {
            return;
        }
        let destination = outside(index).to_address();
        let mut paid = None;
        sim.step(index, |core| {
            paid = Some(core.pay_from_xrpl(&destination, drops));
        });
        match paid {
            Some(Ok(id)) => {
                self.last.insert(index, id);
                sim.count("owner payments made");
                sim.trace.push(format!(
                    "{} {index}'s owner pays out {drops} drops",
                    sim.now
                ));
            }
            Some(Err(refused)) => {
                sim.count("owner payments refused");
                sim.trace.push(format!(
                    "{} {index}'s owner may not pay out now: {refused:?}",
                    sim.now
                ));
            }
            None => {}
        }
    }

    /// Whether node `index`'s owner's last payment out is still on its way,
    /// as its node says; a node that is down says nothing.
    fn on_its_way(&self, sim: &Sim, index: usize) -> bool {
        let Some(&id) = self.last.get(&index) else {
            return false;
        };
        sim.nodes[index]
            .core
            .as_ref()
            .is_some_and(|core| core.xrpl_owner_payment(id) == OwnerPaymentState::InFlight)
    }
}

/// What the ledger's truth says node `index`'s owner paid out of its XRP
/// book, in the ledger's internal units: what its address outside the node
/// holds.
fn paid_out(sim: &Sim, index: usize) -> i128 {
    i128::from(sim.ledger.balance(outside(index)).unwrap_or(0)) * 1_000
}

/// What a seed draws for a run of owner books on the testnet, before it
/// starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    super::scripted::describe(seed)
}

/// A run of owner books on the testnet, as the module says.
#[must_use]
pub fn run(seed: u64) -> Run {
    let (mut rng, count, bootstraps) = size(seed);
    let mut sim = Sim::with(
        seed,
        count,
        bootstraps,
        BTreeMap::new(),
        Settling::OwnerBooks,
    );
    let mut owners = PaidOut::default();
    let result = script(&mut sim, &mut rng, |sim, rng| {
        for index in 0..sim.nodes.len() {
            let drops = LEAST_PAID_OUT_DROPS + rng.below(2 * DROPS_PER_XRP);
            owners.pay(sim, index, drops);
        }
        issuer_acts(sim, rng);
    })
    .and_then(|()| lies(&mut sim, &mut rng, &mut owners))
    .and_then(|()| {
        settle_down(&mut sim, &owners);
        first_failure([
            check(&sim),
            check_withdrawn(&sim).map_err(|problem| Outcome::liveness("withdrawn", problem)),
            check_owner_books(&sim, &owners),
        ])
    });
    ended(sim, result)
}

/// While payments are under way, the issuer claws some of a holder's token
/// back, then freezes a holder's line, deep or not, or every line, and
/// lifts it again. A freeze lasts up to the grace a peer short of room has
/// before it is withdrawn, drawn from the production timings. The issuer
/// acts only on lines it minted to: it waits for every holder's, and a
/// holder whose line it has not minted to by then is spared.
fn issuer_acts(sim: &mut Sim, rng: &mut SeededRng) {
    let Some((token, _)) = sim.settling.token() else {
        return;
    };
    let holders: Vec<usize> = (0..sim.nodes.len())
        .filter(|index| sim.settling.books(*index, token))
        .collect();
    sim.run_until_or(sim.now + READY_WITHIN_MS, |sim| {
        holders.iter().all(|index| sim.issued.contains_key(index))
    });
    let holders: Vec<usize> = holders
        .into_iter()
        .filter(|index| sim.issued.contains_key(index))
        .collect();
    if holders.is_empty() {
        return;
    }
    let pick =
        |rng: &mut SeededRng| holders[usize::try_from(rng.below(holders.len() as u64)).unwrap()];
    sim.run_until(sim.now + rng.below(4_000));
    let clawed = pick(rng);
    let dollars = 1 + rng.below(MOST_CLAWED_DOLLARS);
    sim.claw_back(clawed, i128::from(dollars) * 1_000_000_000);
    sim.run_until(sim.now + rng.below(4_000));
    let frozen = pick(rng);
    let freeze = match rng.below(3) {
        0 => IssuerFreeze::Line(frozen),
        1 => IssuerFreeze::Deep(frozen),
        _ => IssuerFreeze::Global,
    };
    sim.issuer_freeze(freeze, true);
    sim.run_until(sim.now + rng.below(sim.peer.advertise_short_grace_ms));
    sim.issuer_freeze(freeze, false);
}

/// Each node's connection to the ledger comes back to a server that tells
/// each kind of lie in turn, for [`LIE_MS`], its owner asking the faucet
/// for more as the connection drops and paying out while the server lies;
/// then a validator rotates its key, a quorum of the validators is
/// withheld for a while, and the publisher's list lapses before a new one
/// comes
/// (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation).
fn lies(sim: &mut Sim, rng: &mut SeededRng, owners: &mut PaidOut) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    for lie in Lie::ALL {
        for liar in 0..count {
            // Its owner asks the faucet for more as the node's connection
            // drops, and once more while the server lies: so the history
            // the server gives on connecting, and its stream, have
            // something to leave out, or to hint at in an old ledger.
            sim.fund(liar);
            sim.lie_to(liar, lie, LIE_MS);
        }
        sim.run_until(sim.now + STALL_MS / 2);
        for liar in 0..count {
            sim.fund(liar);
            let drops = LEAST_PAID_OUT_DROPS + rng.below(DROPS_PER_XRP);
            owners.pay(sim, liar, drops);
        }
        sim.run_until(sim.now + LIE_MS + STALL_MS / 2);
    }
    sim.end_lies();
    validators_fail(sim);
    Ok(())
}

/// A validator rotates its key, a quorum of the validators is withheld,
/// and the publisher's list lapses before a new one comes: every node waits
/// meanwhile, and goes on once it can prove what it reads again.
fn validators_fail(sim: &mut Sim) {
    // A validator's new key: the nodes learn it from the server's
    // `manifest` method before any list carries it.
    sim.ledger.rotate_validator(0);
    sim.count("validator keys rotated");
    sim.trace
        .push(format!("{} validator 0 rotates its key", sim.now));
    sim.run_until(sim.now + LIE_MS);
    // A quorum withheld: no ledger is validated, and every node waits.
    let quorum_short = [1, 2];
    for validator in quorum_short {
        sim.ledger.withhold_validator(validator, true);
    }
    sim.trace
        .push(format!("{} validators {quorum_short:?} withheld", sim.now));
    sim.count("quorums withheld");
    sim.run_until(sim.now + WITHHELD_MS);
    for validator in quorum_short {
        sim.ledger.withhold_validator(validator, false);
    }
    // A list that lapses: every node fetches it, it expires, and the nodes
    // wait until the publisher publishes another.
    let seconds = |ms: u64| u32::try_from(dassie_xrpl::unl::ledger_seconds(ms)).unwrap();
    let lapses = sim.now + LAPSING_LIST_MS;
    sim.ledger.publish_validator_list(seconds(lapses), None);
    sim.trace.push(format!("{} the list is to lapse", sim.now));
    sim.run_until(lapses + LIST_RETRY_MS);
    sim.count("validator lists lapsed");
    sim.ledger
        .publish_validator_list(seconds(sim.now + 30 * 24 * 3_600_000), None);
    sim.trace
        .push(format!("{} a new list is published", sim.now));
}

/// Whether every running node's every ledger is synced.
fn synced(sim: &Sim) -> bool {
    (0..sim.nodes.len()).all(|index| {
        sim.nodes[index].core.as_ref().is_some_and(|core| {
            (core.ledger_views().iter()).all(|view| view.status == LedgerStatus::Synced)
        })
    })
}

/// The payments left finish, every owner's payment out lands, and every
/// ledger is synced again; then a minute's calm.
fn settle_down(sim: &mut Sim, owners: &PaidOut) {
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    let calm = |sim: &Sim| {
        paid(sim)
            && check_settled(sim).is_ok()
            && synced(sim)
            && (0..sim.nodes.len()).all(|index| !owners.on_its_way(sim, index))
    };
    sim.run_until_or(sim.now + READY_WITHIN_MS, calm);
    sim.run_until(sim.now + 60_000);
    // What landed, as the ledger has it: so a run shows that the owners'
    // payments reached the books' checks.
    for index in 0..sim.nodes.len() {
        if paid_out(sim, index) > 0 {
            sim.count("owners whose payments out landed");
        }
    }
}

/// What node `index`'s owner deposited on `ledger`, an owner book of the
/// run: on XRP what the faucet, or on mainnet the owner, gave above the
/// reserve each time, or on the token what the issuer minted less what it
/// clawed back, which is the owner's withdrawal.
fn deposited(sim: &Sim, index: usize, ledger: LedgerId) -> i128 {
    if Some(ledger) == sim.settling.token().map(|(token, _)| token) {
        let (minted, clawed) = sim.issued.get(&index).copied().unwrap_or((0, 0));
        return minted - clawed;
    }
    let fundings = i128::from(sim.fundings.get(&index).copied().unwrap_or(0));
    (fundings * i128::from(FAUCET_DROPS) - i128::from(RESERVE_BASE)) * 1_000
}

/// Each node's owner books, judged by themselves
/// (spec/decisions/own-books.md): each holds to its account, the XRP book
/// its account's XRP less the reserve and the token's book its line; each
/// owner's funds there moved by what its owner deposited, what the issuer
/// did and what the owner paid out as the ledger has it; and no payment
/// out is still on its way. And every ledger of every node is synced.
fn check_owner_books(sim: &Sim, owners: &PaidOut) -> Result<(), Outcome> {
    let books = || {
        for index in 0..sim.nodes.len() {
            for &ledger in sim.settling.owner_books() {
                check_held(sim, index, ledger)?;
                let owner = sim
                    .balances_on(index, ledger)
                    .get("equity/owner")
                    .copied()
                    .unwrap_or(0);
                let paid = if ledger.asset().code == "XRP" {
                    paid_out(sim, index)
                } else {
                    0
                };
                let deposited = deposited(sim, index, ledger);
                if owner != deposited - paid {
                    return Err(format!(
                        "node {index}'s owner holds {owner} on {ledger}, for {deposited} \
                         deposited and {paid} paid out; its books: {:?}",
                        sim.balances_on(index, ledger)
                    ));
                }
            }
        }
        Ok(())
    };
    let landed = (0..sim.nodes.len())
        .find(|index| owners.on_its_way(sim, *index))
        .map_or(Ok(()), |index| {
            Err(format!(
                "node {index}'s owner's payment out is still on its way"
            ))
        });
    let synced = if synced(sim) {
        Ok(())
    } else {
        Err("a node's ledger is not synced".to_owned())
    };
    first_failure([
        books().map_err(|problem| Outcome::safety("owner-books", problem)),
        landed.map_err(|problem| Outcome::liveness("paid-out", problem)),
        synced.map_err(|problem| Outcome::liveness("synced", problem)),
    ])
}

/// What a run in the live realm draws first: its generator, and how many
/// nodes.
fn size_live(seed: u64) -> (SeededRng, usize) {
    let mut rng = SeededRng::new(seed ^ 0x6c69_7665);
    let count = 2 + usize::try_from(rng.below(3)).unwrap();
    (rng, count)
}

/// What a seed draws for a run of owner books in the live realm, before it
/// starts.
#[must_use]
pub fn describe_live(seed: u64) -> String {
    let (_, count) = size_live(seed);
    format!("{count} nodes, peering with no one")
}

/// A run of owner books in the live realm, as the module says.
#[must_use]
pub fn run_live(seed: u64) -> Run {
    let (mut rng, count) = size_live(seed);
    let mut sim = Sim::with(seed, count, 0, BTreeMap::new(), Settling::OwnerBooksLive);
    let owners = PaidOut::default();
    let result = script_live(&mut sim, &mut rng).and_then(|()| {
        first_failure([
            check_events(&sim).map_err(|problem| Outcome::safety("events", problem)),
            check_ledgers(&sim).map_err(|problem| Outcome::safety("ledgers", problem)),
            check_true(&sim).map_err(|problem| Outcome::safety("proven", problem)),
            check_funded(&sim).map_err(|problem| Outcome::liveness("funded", problem)),
            check_anchored(&sim).map_err(|problem| Outcome::liveness("anchored", problem)),
            check_pegged(&sim).map_err(|problem| Outcome::liveness("pegged", problem)),
            check_owner_books(&sim, &owners),
        ])
    });
    ended(sim, result)
}

/// Node `index`'s owner deposits on mainnet, which has no faucet: as much
/// as the testnet's faucet gives, in the next ledger.
fn deposit(sim: &mut Sim, index: usize) {
    let account = sim.xrpl_account(index);
    sim.ledger.deposit(account, FAUCET_DROPS);
    *sim.fundings.entry(index).or_default() += 1;
    sim.trace
        .push(format!("{} {index}'s owner deposits", sim.now));
}

/// Whether every node's books are up: every ledger synced, and the issuer
/// minted to each node, once its line existed.
fn booked(sim: &Sim) -> bool {
    synced(sim) && (0..sim.nodes.len()).all(|index| sim.issued.contains_key(&index))
}

fn script_live(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    if !sim.run_until_or(START + READY_WITHIN_MS, booked) {
        return Err(Outcome::liveness(
            "booked",
            "the nodes' owner books were not up in time",
        ));
    }
    sim.loss_per_mille = 0;
    issuer_acts(sim, rng);
    // Each node's connection comes back to a server that tells a lie drawn
    // from every kind, while its owner deposits.
    for liar in 0..sim.nodes.len() {
        let lie = Lie::ALL[usize::try_from(rng.below(Lie::ALL.len() as u64)).unwrap()];
        deposit(sim, liar);
        sim.lie_to(liar, lie, LIE_MS);
        sim.run_until(sim.now + STALL_MS / 2);
        deposit(sim, liar);
        sim.run_until(sim.now + LIE_MS + STALL_MS / 2);
    }
    sim.end_lies();
    sim.run_until_or(sim.now + READY_WITHIN_MS, synced);
    sim.run_until(sim.now + 60_000);
    Ok(())
}
