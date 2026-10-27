//! The node simulation's run over a DSUSD settlement hub (`node-hub-dsusd`,
//! spec/areas/testing.md#over-a-settlement-hub, spec/decisions/token-hubs.md):
//! members of a hub of 4 to 7 validators run in the simulation, whose escrow
//! holds DSUSD on the fake XRP Ledger, each with the owner books of XRP and
//! DSUSD on its XRP Ledger testnet account, peering on neither. The fake
//! ledger's issuer mints each member DSUSD; each opens its account in the
//! hub by a deposit from its DSUSD book, and the members pay each other
//! across hops over the hub.
//!
//! The token's pool moves off its peg: the members' peg guards halt the
//! conversions of the hub's ledger, and the members still pay each other
//! over the hub, in the token as it is. The escrow's XRP is brought below
//! the least the hub's crate fixes, as the fees of a long life would bring
//! it, and a member's withdrawal first tops it up from its XRP book, and
//! goes only once that applied. Then someone sends the escrow XRP, as
//! anyone may: it is gas, and credits no account. The run keeps that gift
//! for the payout at its end: the least gas does not cover the reserve of
//! the escrow's signer list and trust line, which the payout set's change
//! of signer list needs, so an escrow topped up only to it could not pay
//! out (spec/roadmap.md#token-hubs).
//!
//! Before the escrow's XRP falls, the issuer holds the escrow, a moment
//! into a round of payments, by a seed's draw: it freezes the escrow's
//! line, deep-freezes it, freezes
//! its token for every holder, or charges a transfer fee. Once every
//! member read the freeze from what the validators state of the issuer's
//! hold, none sends a Prepare over the hub, a payment made meanwhile waits,
//! and no account is in step, until the issuer lets go and every member
//! read the thaw; meanwhile only claims within the freeze's window take
//! from the holds. Then every payment completes, and no member booked a
//! loss on a peer but the fees of its own claims. A payee refuses a
//! commitment that expires further ahead than a connector holds a packet
//! and the hub's precision, and its payer pays it as before. The hub then
//! ends: in three runs of four the
//! issuer claws back part of what the escrow holds, the hub shuts down,
//! and its escrow pays the withdrawals already sequenced in full and each
//! account its share of what is left, checked to the unit against the
//! hub's books and the escrow; otherwise it loses f + 1 validators and
//! stalls, and its escrow pays out the final balances in full.
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
use dassie_hub::ledger::Phase;
use dassie_hub::member::{DSUSD_SCHEME_ID, GAS_CHECK_LASTS_MS, LEAST_ESCROW_GAS_DROPS, member_key};
use dassie_hub::promise::{Commitment, Signed};
use dassie_hub::rules::{Asset, MAX_HOLD_MS};
use dassie_ilp::ErrorCode;
use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_oer::{Decode, Encode};
use dassie_peer::InboundId;
use dassie_peer::wire::{Content, SchemeId};
use dassie_settlement::PegPause;
use dassie_xrpl::DROPS_PER_XRP;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::api;
use dassie_xrpl::codec::{Amount, TokenValue};
use dassie_xrpl::peg::TRIP_HOLD_MS;

use super::checks::{check_events, check_funded, check_held, check_incoming, first_failure};
use super::hub::owed;
use super::scripted::{READY_WITHIN_MS, ended};
use super::{
    DSUSD, Delivery, HUB_DSUSD, POOL_TOKENS, START, Settling, Sim, XRP, key, received, xrpl_account,
};
use crate::engine::EngineError;
use crate::engine::core::Direction;
use crate::hub::escrow::ISSUER_CHECK_EVERY_MS;

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

/// How the issuer holds the escrow in a run: each freezes the hub as a
/// freeze does (spec/decisions/token-hubs.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hold {
    /// It freezes the escrow's line: the escrow pays only the issuer.
    Line,
    /// It deep-freezes the escrow's line: it neither pays nor receives.
    Deep,
    /// It freezes its token for every holder.
    Global,
    /// It charges a transfer fee, which the escrow's payments would pay.
    Fee,
}

/// What the issuer charges while a run's [`Hold::Fee`] lasts, in
/// billionths: 0.2% on every payment of its token between two holders.
const TRANSFER_RATE: u32 = api::TRANSFER_RATE_PAR + 2_000_000;

/// How a run ends its hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The issuer claws back part of what the escrow holds: the hub shuts
    /// down and pays out pro rata.
    Clawback,
    /// The hub loses f + 1 validators and stalls, and pays out in full.
    Stall,
}

/// What a seed draws.
struct Setup {
    validators: usize,
    members: usize,
    /// The member that withdraws once the escrow's XRP is low.
    withdrawer: usize,
    /// How the issuer holds the escrow, from the freeze to the thaw.
    hold: Hold,
    /// How the hub ends.
    ending: Ending,
}

fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ 0x6875_622d_6473_7573);
    let validators = 4 + usize::try_from(rng.below(4)).unwrap();
    let members = 3 + usize::try_from(rng.below(2)).unwrap();
    let withdrawer = usize::try_from(rng.below(members as u64)).unwrap();
    let hold =
        [Hold::Line, Hold::Deep, Hold::Global, Hold::Fee][usize::try_from(rng.below(4)).unwrap()];
    // A clawback in three runs of four: the stall's payout in full is the
    // `hub` simulation's and `node-hub`'s too.
    let ending = if rng.below(4) == 0 {
        Ending::Stall
    } else {
        Ending::Clawback
    };
    (
        rng,
        Setup {
            validators,
            members,
            withdrawer,
            hold,
            ending,
        },
    )
}

/// What a seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    format!(
        "{} members, {} validators, node {} withdraws once the escrow's XRP is low, \
         the issuer holds the escrow by {:?}, the hub ends by {:?}",
        setup.members, setup.validators, setup.withdrawer, setup.hold, setup.ending
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
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
    off_peg(sim, rng)?;
    // The freeze comes before the owner's moves, which pause the node's own
    // for longer than the run (`OWNER_PAUSE_MS`): a member that pays after
    // them could not fund its holds again.
    freeze(sim, rng, setup.hold)?;
    refuse_far_ahead(sim)?;
    let asked = withdraw_low(sim, setup)?;
    // The least gas a member tops the escrow up to is below the reserve
    // of the escrow's signer list and trust line, which the payout set's
    // change of signer list needs: someone sends the escrow more, as
    // anyone may (spec/roadmap.md#token-hubs).
    gift(sim, setup);
    settle(sim);
    check_topped_up(sim, setup.withdrawer, asked)?;
    check_calm(sim)?;
    match setup.ending {
        Ending::Clawback => claw_back(sim, rng),
        Ending::Stall => stall(sim, rng),
    }?;
    check_no_losses(sim, "paid-out")
}

/// Whether every payment sent all it meant to.
fn paid(sim: &Sim) -> bool {
    sim.payments.iter().all(|payment| payment.done)
}

/// Every peering is over the DSUSD hub: the owner books peer with nobody.
fn over_hub(sim: &Sim) -> Result<(), Outcome> {
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            let scheme = peer.peering.first().scheme;
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
    Ok(())
}

/// How long the members' peg guards take to halt the conversions of the
/// hub's ledger once the token's pool is off its peg: the guard's trip hold,
/// and the minute in which every node learns the rates again and reads the
/// pool once more.
const OFF_PEG_WITHIN_MS: u64 = TRIP_HOLD_MS + 2 * crate::node::Policies::DEFAULT.rates_every_ms;

/// The token's pool moves off its peg, 5% under a dollar: once each
/// member's peg guard has held it off for its trip hold, the hub's ledger
/// converts nothing, while the members still pay each other over the hub,
/// in the token as it is (spec/decisions/token-hubs.md). Then the pool is
/// back at a dollar; the guards resume after their recovery hold, which
/// the run does not wait for: nothing in it converts.
fn off_peg(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let token = sim.token();
    let tokens = TokenValue::from_units(POOL_TOKENS, 0).unwrap();
    let par = 2 * POOL_TOKENS * DROPS_PER_XRP;
    sim.ledger
        .set_pool(token, par / 100 * 95, tokens, 300)
        .unwrap();
    sim.trace.push(format!(
        "{} the token's pool moves 5% under a dollar",
        sim.now
    ));
    let halted = |sim: &Sim| {
        (0..sim.nodes.len()).all(|index| {
            let core = sim.core(index);
            let paused = core
                .ledger_views()
                .into_iter()
                .find(|view| view.id == HUB_DSUSD)
                .and_then(|view| view.peg)
                .and_then(|peg| peg.paused);
            paused == Some(PegPause::OffPeg) && core.price_table().halted(HUB_DSUSD)
        })
    };
    if !sim.run_until_or(sim.now + OFF_PEG_WITHIN_MS, halted) {
        return Err(Outcome::liveness(
            "off-peg",
            "the members' peg guards did not halt the hub's ledger in time",
        ));
    }
    sim.count("conversions halted off the peg");
    pay(sim, rng);
    if !sim.run_until_or(sim.now + PAID_WITHIN_MS, paid) {
        return Err(Outcome::liveness(
            "off-peg",
            format!(
                "the members did not pay each other over the hub off its peg: {}",
                standing(sim)
            ),
        ));
    }
    if !halted(sim) {
        return Err(Outcome::safety(
            "off-peg",
            "a member's guard resumed while the pool was still off its peg",
        ));
    }
    sim.count("members paid each other off the peg");
    sim.ledger.set_pool(token, par, tokens, 300).unwrap();
    sim.trace
        .push(format!("{} the token's pool is back at a dollar", sim.now));
    Ok(())
}

/// How long an act of the issuer takes to reach every member, at
/// production timings: the ledger that applies it, two of the
/// validators' proofs of the issuer's state (the first may be of the
/// ledger before), and a minute for the block that takes the quorum's
/// attestations to become final and for every member to read it.
const KNOWN_WITHIN_MS: u64 = super::LEDGER_CLOSE_MS + 2 * ISSUER_CHECK_EVERY_MS + 60_000;

/// The issuer holds the escrow as `hold` says, in the next ledger, or
/// lets go of it.
fn issuer_holds(sim: &mut Sim, hold: Hold, on: bool) {
    let token = sim.token();
    let escrow = sim.hub.as_deref().unwrap().escrow();
    let ledger = &mut sim.ledger;
    match hold {
        Hold::Line => ledger.freeze(escrow, token, on),
        Hold::Deep if on => ledger.deep_freeze(escrow, token, true),
        // Lifted whole, the freeze with the deep freeze.
        Hold::Deep => ledger.freeze(escrow, token, false),
        Hold::Global => ledger.global_freeze(token.issuer, on),
        Hold::Fee => ledger.set_transfer_rate(token.issuer, if on { TRANSFER_RATE } else { 0 }),
    }
    .unwrap();
    sim.count(if on {
        "issuer holds the escrow"
    } else {
        "issuer lets go of the escrow"
    });
    sim.trace.push(format!(
        "{} the issuer {} the escrow by {hold:?}",
        sim.now,
        if on { "holds" } else { "lets go of" }
    ));
}

/// Whether every member knows the hub frozen, or every one knows it not.
fn all_know(sim: &Sim, frozen: bool) -> bool {
    (0..sim.nodes.len()).all(|index| {
        sim.core(index)
            .hub_status()
            .is_some_and(|status| status.frozen_since_ms.is_some() == frozen)
    })
}

/// What each hold of the honest validators' ledger holds, by its number.
fn hold_amounts(sim: &Sim) -> BTreeMap<u64, u64> {
    sim.hub
        .as_deref()
        .and_then(super::hub::HubNet::ledger)
        .map(|ledger| {
            ledger
                .holds()
                .map(|(number, hold)| (number.0, hold.amount))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the honest validators' ledger has the hub frozen.
fn hub_frozen(sim: &Sim) -> bool {
    sim.hub
        .as_deref()
        .and_then(super::hub::HubNet::ledger)
        .is_some_and(|ledger| ledger.frozen().is_some())
}

/// No member booked a loss on a peer over the hub but the hub's fee for a
/// claim it made there itself, when its peer's own claim was late
/// (`LossCause::ClaimFee`): the hub paid no peer for a packet the member
/// had rejected or forgotten, as it never does between honest members, a
/// freeze or not (spec/areas/hubs.md#a-frozen-hub). The run counts what
/// each booked (`Sim::hub_losses`) but not why, which its trace of each
/// node's events says.
fn check_no_losses(sim: &Sim, phase: &'static str) -> Result<(), Outcome> {
    let lost = sim
        .trace
        .iter()
        .find(|line| line.contains("Settlement(PeerLoss {") && !line.contains("cause: ClaimFee"));
    match lost {
        None => Ok(()),
        Some(line) => Err(Outcome::safety(
            phase,
            format!("a member lost more than a claim's fee over the hub: {line}"),
        )),
    }
}

/// The issuer holds the escrow while the members pay each other, and lets
/// go of it (spec/areas/hubs.md#a-frozen-hub). The members pay until the
/// freeze, which comes a moment into a round of payments, so that packets
/// are on their way, and fulfilled but not yet claimed on the hub, when it
/// comes. Once every member read the freeze from what its validators state
/// of the issuer's hold, none sends a Prepare over the hub, a payment made
/// meanwhile does not cross it, and no account is in step for moves or
/// packets, until the thaw. The hub's clock stands still while it lasts,
/// longer than any commitment taken before it runs.
///
/// The members keep the claims for packets they took before the freeze.
/// While the hub is frozen, nothing but a claim for commitments within the
/// freeze's window takes from a hold (no reduction falls due, and nothing
/// else applies), so what the holds lost between the freeze's block and
/// the thaw is what the frozen hub paid on such claims, counted when it
/// paid any, and no hold grew or opened meanwhile. After the thaw every
/// payment completes, the ones the freeze held up and new ones; no member
/// booked a loss on a peer over the hub but the fee of a claim it made
/// there ([`check_no_losses`]), and the checks of the calm network that
/// follow find each member's books whole.
fn freeze(sim: &mut Sim, rng: &mut SeededRng, hold: Hold) -> Result<(), Outcome> {
    pay(sim, rng);
    sim.run_until(sim.now + rng.below(super::LEDGER_CLOSE_MS));
    issuer_holds(sim, hold, true);
    // What the holds held at the freeze's block: the first state of the
    // honest validators' ledger that has the hub frozen.
    if !sim.run_until_or(sim.now + KNOWN_WITHIN_MS, hub_frozen) {
        return Err(Outcome::liveness(
            "frozen",
            "the validators did not freeze the hub in time",
        ));
    }
    let at_freeze = hold_amounts(sim);
    if !sim.run_until_or(sim.now + KNOWN_WITHIN_MS, |sim| all_know(sim, true)) {
        return Err(Outcome::liveness(
            "frozen",
            format!(
                "the members did not all learn the freeze in time: {}",
                standing(sim)
            ),
        ));
    }
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub
        .ledger()
        .ok_or_else(|| Outcome::liveness("frozen", "no honest validator runs"))?;
    if ledger.frozen().is_none() || !ledger.issuer().holds() {
        return Err(Outcome::safety(
            "frozen",
            "members took the hub as frozen where its validators have it running",
        ));
    }
    sim.count("members learned the freeze");
    let prepares = |sim: &Sim| sim.counts.get("prepares over a hub").copied().unwrap_or(0);
    let before = prepares(sim);
    // A payment across the hop while the freeze lasts: no path crosses
    // the hub until the thaw.
    let during = sim.payments.len();
    let amount = 100_000_000 + rng.below(1_000_000_000);
    sim.pay(0, 2, amount, None, None);
    let lasts = 2 * sim.hub.as_deref().unwrap().rules.terms.timers.hold_notice;
    if sim.run_until_or(sim.now + lasts, |sim| !all_know(sim, true)) {
        return Err(Outcome::safety(
            "frozen",
            "a member took the hub as thawed while the issuer held it",
        ));
    }
    let sent = prepares(sim) - before;
    if sent > 0 {
        return Err(Outcome::safety(
            "frozen",
            format!("members sent {sent} Prepares over the frozen hub"),
        ));
    }
    if sim.payments.get(during).is_some_and(|payment| payment.done) {
        return Err(Outcome::safety(
            "frozen",
            "a payment crossed the frozen hub",
        ));
    }
    if let Some(index) = (0..sim.nodes.len()).find(|index| in_step(sim, *index)) {
        return Err(Outcome::safety(
            "frozen",
            format!("node {index}'s hub account is in step for moves and packets while frozen"),
        ));
    }
    sim.count("no packet crossed the frozen hub");
    if !hub_frozen(sim) {
        return Err(Outcome::safety(
            "frozen",
            "the validators thawed the hub while the issuer held it",
        ));
    }
    let now = hold_amounts(sim);
    let mut claimed = 0;
    for (number, before) in &at_freeze {
        let after = now.get(number).copied().unwrap_or(0);
        if after > *before {
            return Err(Outcome::safety(
                "frozen",
                format!("hold {number} was funded while the hub was frozen"),
            ));
        }
        claimed += before - after;
    }
    if now.keys().any(|number| !at_freeze.contains_key(number)) {
        return Err(Outcome::safety(
            "frozen",
            "a hold was opened while the hub was frozen",
        ));
    }
    if claimed > 0 {
        sim.count("claims the frozen hub paid");
        sim.trace.push(format!(
            "{} the frozen hub paid {claimed} on claims within its window",
            sim.now
        ));
    }
    issuer_holds(sim, hold, false);
    if !sim.run_until_or(sim.now + KNOWN_WITHIN_MS, |sim| all_know(sim, false)) {
        return Err(Outcome::liveness(
            "thawed",
            format!(
                "the members did not all learn the thaw in time: {}",
                standing(sim)
            ),
        ));
    }
    sim.count("members learned the thaw");
    pay(sim, rng);
    if !sim.run_until_or(sim.now + PAID_WITHIN_MS, paid) {
        return Err(Outcome::liveness(
            "thawed",
            format!(
                "the members did not all pay each other after the thaw: {}",
                standing(sim)
            ),
        ));
    }
    sim.count("members paid each other after the thaw");
    check_no_losses(sim, "thawed")
}

/// The request ID of the Prepare [`refuse_far_ahead`] makes up: one no
/// node sends, since each counts its own from zero.
const FAR_AHEAD_REQUEST: u32 = u32::MAX - 7;

/// A payee refuses a commitment that expires further ahead of its clock
/// than a connector holds a packet and the hub's precision, and the
/// Prepare with it, as one beyond its payer's credit (T04): a later one
/// would stretch the window in which a frozen hub still pays claims
/// (spec/areas/hubs.md#packets-promises-and-claims). Node 1, which peers
/// with every other member, sends node 0 what no node of ours sends: a
/// Prepare whose commitment, signed with its key in the hub on its hold
/// toward node 0, holds in every other respect, and expires a second past
/// that bound.
fn refuse_far_ahead(sim: &mut Sim) -> Result<(), Outcome> {
    let (payer, payee) = (1, 0);
    let fail = |problem: String| Outcome::liveness("far-ahead", problem);
    let hub = sim.hub.as_deref().unwrap();
    let id = hub.id();
    let margin = hub.rules.terms.timers.margin;
    let ledger = hub
        .ledger()
        .ok_or_else(|| fail("no honest validator runs".into()))?;
    let member = |index| member_key(&key(index), &id);
    let account = |index| ledger.account_of(&member(index).public_key());
    let (Some(from), Some(to)) = (account(payer), account(payee)) else {
        return Err(fail("node 0 or node 1 has no account in the hub".into()));
    };
    let Some((hold, _)) = ledger
        .holds()
        .find(|(_, hold)| hold.payer == from && hold.payee == to)
    else {
        return Err(fail("node 1 has no hold toward node 0".into()));
    };
    let fulfillment = [7; 32];
    let condition = dassie_ilp::condition_for(&fulfillment);
    let ahead = MAX_HOLD_MS + dassie_hub::consensus::Config::DEFAULT.precision_ms;
    let expiry = sim.local_now(payee) + ahead + 1_000;
    let prepare = dassie_ilp::Prepare {
        amount: dassie_hub::member::UNITS_PER_DROP,
        expires_at: dassie_ilp::Timestamp {
            unix_ms: expiry + margin,
        },
        execution_condition: condition,
        destination: dassie_ilp::Address::parse("test.far-ahead").unwrap(),
        data: &[],
    };
    let packet = prepare.to_vec();
    let commitment = Signed::sign(
        Commitment {
            hold,
            // Above any it took or was claimed through.
            sequence: u64::MAX / 2,
            amount: 1,
            condition,
            expiry,
        },
        &id,
        &member(payer),
    )
    .to_vec();
    let body = Content::MethodPacket {
        request_id: FAR_AHEAD_REQUEST,
        scheme: SchemeId::parse(DSUSD_SCHEME_ID).unwrap(),
        packet: &packet,
        commitment: &commitment,
    }
    .to_vec();
    // Its answer goes to no one; the Reject goes to node 1, lost to none.
    let loss = sim.loss_per_mille;
    sim.loss_per_mille = 0;
    let node = &mut sim.nodes[payee];
    let inbound = InboundId(node.next_inbound);
    node.next_inbound += 1;
    let sender = key(payer).public_key();
    sim.step(payee, |core| {
        core.peer_inbound(inbound, sender, &body, None)
    });
    sim.loss_per_mille = loss;
    let refused = sim.queue.values().any(|delivery| match delivery {
        Delivery::Request { from, to, body, .. } if *from == payee && *to == payer => {
            matches!(
                Content::decode_exact(body),
                Ok(Content::InterledgerPacket { request_id, packet })
                    if request_id == FAR_AHEAD_REQUEST
                        && matches!(
                            dassie_ilp::Packet::decode_exact(packet),
                            Ok(dassie_ilp::Packet::Reject(reject))
                                if reject.code == ErrorCode::T04_INSUFFICIENT_LIQUIDITY
                        )
            )
        }
        _ => false,
    });
    if !refused {
        return Err(Outcome::safety(
            "far-ahead",
            "node 0 took a commitment that expires past the longest a connector holds a packet",
        ));
    }
    sim.count("commitments too far ahead refused");
    sim.trace.push(format!(
        "{} node 0 refuses node 1's commitment that expires {}ms ahead",
        sim.now,
        ahead + 1_000
    ));
    // What node 0 refused leaves node 1's commitments to it as they were:
    // node 1 pays it as before.
    sim.pay(payer, payee, FAR_AHEAD_THEN_PAID, None, None);
    if !sim.run_until_or(sim.now + PAID_WITHIN_MS, paid) {
        return Err(fail(format!(
            "node 1 did not pay node 0 after its refusal: {}",
            standing(sim)
        )));
    }
    Ok(())
}

/// What node 1 pays node 0 after [`refuse_far_ahead`]'s refusal: a dollar.
const FAR_AHEAD_THEN_PAID: u64 = 1_000_000_000;

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
            check_no_losses(sim, "hub-losses"),
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

/// The issuer claws back part of what the escrow holds, a quarter to a
/// half: the hub shuts down at once, ends after its notice, and its
/// validators sign the payout set of its last block, which pays the
/// withdrawals already sequenced in full and each account its share of
/// what the escrow holds beyond them (spec/decisions/token-hubs.md). The
/// members' own moves are off first, so nothing is on its way into or out
/// of the escrow. Each member's DSUSD book then holds its line.
fn claw_back(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    for index in 0..sim.nodes.len() {
        sim.step(index, |core| {
            core.set_hub_automatic(None, false).unwrap();
        });
    }
    let held = escrow_tokens(sim);
    let part = held / 4 + rng.below(held / 4);
    let token = sim.token();
    let escrow = sim.hub.as_deref().unwrap().escrow();
    let value = TokenValue::from_units(part, 6).unwrap();
    sim.ledger.claw_back(escrow, token.amount(value)).unwrap();
    sim.count("issuer clawbacks from the escrow");
    sim.trace.push(format!(
        "{} the issuer claws back {part} of the escrow's {held} millionths",
        sim.now
    ));
    let notice = sim
        .hub
        .as_deref()
        .unwrap()
        .rules
        .terms
        .timers
        .shutdown_notice;
    let gathered = |sim: &Sim| sim.hub.as_deref().unwrap().payout.is_some();
    if !sim.run_until_or(sim.now + KNOWN_WITHIN_MS + notice, gathered) {
        return Err(Outcome::liveness(
            "clawback",
            "the hub did not end and sign its payout in time after the clawback",
        ));
    }
    // Nothing of the set has gone to the ledger yet.
    let before = escrow_tokens(sim);
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
    check_pro_rata(sim, part, before).map_err(|problem| Outcome::safety("payout", problem))?;
    sim.run_until(sim.now + CALM_MS);
    for index in 0..sim.nodes.len() {
        check_held(sim, index, DSUSD).map_err(|problem| Outcome::safety("paid-out", problem))?;
        check_held(sim, index, XRP).map_err(|problem| Outcome::safety("paid-out", problem))?;
    }
    Ok(())
}

/// After a clawback of `part`, the escrow paid out what the hub's books and
/// the escrow say, to the unit: the hub ended, counting `part` clawed back;
/// the escrow held, before the set, what the hub owes less `part`; the set
/// paid each withdrawal the hub had sequenced in full, and each account
/// with somewhere to go its share of what the escrow held beyond them, in
/// proportion to what the hub owed it, worked out apart from the set and
/// rounded down, less the underlying ledger's fee, if that leaves at
/// least the least payout; every transaction of
/// the set applied, and it paid out no more than the escrow held.
fn check_pro_rata(sim: &mut Sim, part: u64, before: u64) -> Result<(), String> {
    let hub = sim.hub.as_deref().unwrap();
    let payout = hub.payout.as_ref().unwrap();
    let ledger = &payout.ledger;
    if ledger.phase() != Phase::Ended {
        return Err(format!(
            "the payout set is of a hub in {:?}",
            ledger.phase()
        ));
    }
    let escrow = ledger.escrow();
    if escrow.clawed_back != part {
        return Err(format!(
            "the hub counts {} clawed back, where the issuer took {part}",
            escrow.clawed_back
        ));
    }
    if escrow.held() != before {
        return Err(format!(
            "the hub has the escrow holding {}, where it held {before}",
            escrow.held()
        ));
    }
    let terms = ledger.terms().clone();
    let fees = terms.fees;
    let sequenced: BTreeMap<dassie_xrpl::AccountId, u64> =
        ledger
            .withdrawals()
            .fold(BTreeMap::new(), |mut sum, (_, withdrawal)| {
                *sum.entry(dassie_xrpl::AccountId(withdrawal.to.account))
                    .or_default() += withdrawal.amount;
                sum
            });
    let out: u64 = ledger
        .withdrawals()
        .map(|(_, withdrawal)| withdrawal.amount + withdrawal.ledger_fee)
        .sum();
    let due = u128::from(ledger.supply() - out);
    let have = u128::from(before - out).min(due);
    let mut expected: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    for (number, account) in ledger.accounts() {
        let Some(disbursal) = account.disbursal else {
            continue;
        };
        let share = u64::try_from(u128::from(owed(ledger, number)) * have / due).unwrap();
        // The escrow pays the underlying ledger's fee of each payout out of
        // the share it pays.
        let Some(amount) = share.checked_sub(fees.ledger) else {
            continue;
        };
        if amount >= fees.least_payout {
            *expected
                .entry(dassie_xrpl::AccountId(disbursal.account))
                .or_default() += amount;
        }
    }
    let hashes = payout.hashes.clone();
    let transactions = payout.set.transactions.clone();
    let mut paid: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    let mut withdrawn: BTreeMap<dassie_xrpl::AccountId, u64> = BTreeMap::new();
    for (hash, transaction) in hashes.iter().zip(&transactions) {
        let record = transaction_record(sim, hash)
            .ok_or_else(|| format!("{transaction:?} was never applied"))?;
        if record.result != "tesSUCCESS" {
            return Err(format!("{transaction:?} ended {}", record.result));
        }
        let (PayoutTransaction::Payout(payment) | PayoutTransaction::Withdrawal(payment)) =
            transaction
        else {
            continue;
        };
        let named = terms
            .hub_amount(&payment.amount)
            .ok_or("a payment of something else than DSUSD")?;
        let got = record
            .delivered_token
            .and_then(|token| terms.hub_amount(&Amount::Token(token)));
        if got != Some(named) {
            return Err(format!("a payment of {named} delivered {got:?}"));
        }
        let into = if matches!(transaction, PayoutTransaction::Payout(_)) {
            &mut paid
        } else {
            &mut withdrawn
        };
        *into.entry(payment.destination).or_default() += named;
    }
    if withdrawn != sequenced {
        return Err(format!(
            "the escrow paid the sequenced withdrawals {withdrawn:?}, where they are {sequenced:?}"
        ));
    }
    if paid != expected {
        return Err(format!(
            "the escrow paid out {paid:?}, where the shares of what it holds are {expected:?}"
        ));
    }
    let total: u64 = paid.values().sum();
    if total + out > before {
        return Err(format!(
            "the escrow paid out {total} and {out} of withdrawals, more than its {before}"
        ));
    }
    for index in 0..sim.nodes.len() {
        if !paid.contains_key(&xrpl_account(index)) {
            return Err(format!("node {index} was paid nothing out"));
        }
    }
    sim.count("pro rata payouts checked");
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
