//! The node simulation's runs over a settlement hub on the XRP Ledger's
//! mainnet, in the live realm (spec/areas/testing.md#over-a-settlement-hub):
//! members of a hub of 4 to 7 honest validators run in the simulation, its
//! escrow on the fake ledger's mainnet, which has no faucet. The nodes run
//! in the live realm by the dev switch, as tests do
//! (spec/decisions/mainnet-schemes.md). No default mainnet hub is founded
//! yet, so each run founds its own, as the sponsor does on the fake ledger,
//! and its members name its rules in their schemes' configs.
//!
//! - **`node-hub-live`:** a hub of XRP, which members join over
//!   `hub-xrpl-xrp`, booking on `hub-xrpl-xrp+xrp`. Each runs the owner
//!   book of XRP on its mainnet account, `xrpl-xrp`, peering with no one,
//!   which its owner's deposits fund, and funds its account in the hub from
//!   it.
//! - **`node-hub-rlusd`:** a hub of RLUSD, over `hub-xrpl-rlusd`, booking
//!   on `hub-xrpl-rlusd+usd`, its escrow holding RLUSD on its trust line to
//!   the fake ledger's issuer. Each member runs the owner books of XRP and
//!   RLUSD on its mainnet account, peering on neither; the issuer mints it
//!   RLUSD, and it funds its account in the hub from the RLUSD book, and
//!   the escrow's gas from the XRP book.
//!
//! In both, once the members joined the hub and then the network, two of
//! them never peering, they pay each other across a hop over the hub. Then
//! each member's connection to the ledger comes back to a server that
//! tells a lie drawn from every kind, while its owner deposits, and the
//! servers of one to all of the hub's validators lie as well, each a lie
//! drawn, on every connection their watchers make, while the members pay
//! each other. After every ledger meanwhile, the validators attested
//! nothing false: the escrow holds what the hub owes, but for what is on
//! its way between them. In the RLUSD hub, the token's pool then moves off
//! its peg: each member's peg guard must halt the conversions of
//! `hub-xrpl-rlusd+usd`, while the members still pay each other over it
//! (spec/areas/xrpl.md#the-peg-guard).
//!
//! Once the network is calm, each node is judged by its own books
//! (spec/decisions/own-books.md): they hold what its schemes hold, its XRP
//! book exactly its account's XRP less the reserve, its RLUSD book its
//! line, and in the hub what its account and its peers' holds toward it
//! hold; its owner's funds in the book that funds the hub and in the hub
//! together moved by what its receivers took; nothing booked that the
//! ledger did not do; no loss booked on a peer over the hub but the fee of
//! a claim of its own; and every member anchored. The escrow holds what the
//! hub owes, and an XRP hub's what its sponsor left, but for what is on its
//! way, and the fake ledger made or lost no XRP and no RLUSD.

use std::collections::BTreeMap;

use dassie_hub::rules::Asset;
use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_settlement::PegPause;
use dassie_xrpl::DROPS_PER_XRP;
use dassie_xrpl::account::STALL_MS;
use dassie_xrpl::codec::TokenValue;
use dassie_xrpl::fake::{FAUCET_DROPS, Lie};
use dassie_xrpl::peg::TRIP_HOLD_MS;

use super::checks::{
    check_anchored, check_events, check_finished, check_funded, check_hub_credit,
    check_hub_packets, check_incoming, check_true, check_withdrawn, first_failure,
};
use super::hub::{PREPARES_OVER_LIVE_XRP, PREPARES_OVER_RLUSD};
use super::hub_checks::{
    check_each, check_escrow_in, check_losses, check_settled_with_honest, hub_standing, idle_in,
    joined,
};
use super::hubbed::{over_hub, paid, pay, routed};
use super::scripted::{READY_WITHIN_MS, ended};
use super::{
    HUB_RLUSD, LEDGER_CLOSE_MS, LEDGER_RECONNECT_MS, LONGEST_LIE_MS, POOL_TOKENS, START, Settling,
    Sim, key,
};
use crate::node::Policies;

/// What the RLUSD hub holds: RLUSD, of its mainnet issuer.
pub const RLUSD_ASSET: Asset = Asset::Token {
    currency: dassie_xrpl::rlusd::CURRENCY,
    issuer: dassie_xrpl::rlusd::ISSUER,
};

/// How long a move may be on its way before it is late
/// (`CREDIT_WAIT_MS`, `engine/core/hub_funding.rs`).
const CREDIT_WAIT_MS: u64 = 600_000;

/// How long the payments may take, at production timings.
const PAID_WITHIN_MS: u64 = 10 * 60_000;

/// How long the network is left calm before the books are checked.
const CALM_MS: u64 = 60_000;

/// How long the network may then take to settle down: a peer short of room
/// is asked again once a minute, and may have to move funds into the hub
/// before it can settle, which is late only after `CREDIT_WAIT_MS`.
const SETTLED_WITHIN_MS: u64 = Policies::DEFAULT.settlement_request_every_ms + CREDIT_WAIT_MS;

/// How long a server lies to a member, and its validators' servers to
/// them: the longest a drawn lie lasts.
const LIE_MS: u64 = LONGEST_LIE_MS;

/// How long the members' peg guards take to halt the conversions of the
/// hub's ledger once the token's pool is off its peg: the guard's trip hold,
/// and the minute in which every node learns the rates again and reads the
/// pool once more.
const OFF_PEG_WITHIN_MS: u64 = TRIP_HOLD_MS + 2 * Policies::DEFAULT.rates_every_ms;

/// The hub a run founds on mainnet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveHub {
    /// A hub of XRP: `node-hub-live`.
    Xrp,
    /// A hub of RLUSD: `node-hub-rlusd`.
    Rlusd,
}

impl LiveHub {
    /// What the run settles over, with `validators` validators.
    const fn settling(self, validators: usize) -> Settling {
        match self {
            Self::Xrp => Settling::HubLive(validators),
            Self::Rlusd => Settling::HubRlusd(validators),
        }
    }

    /// The salt of its generator: "hub-live" or "hub-rlus".
    const fn salt(self) -> u64 {
        match self {
            Self::Xrp => 0x6875_622d_6c69_7665,
            Self::Rlusd => 0x6875_622d_726c_7573,
        }
    }

    /// What it counts once the members joined.
    const fn joined(self) -> &'static str {
        match self {
            Self::Xrp => "members joined the mainnet XRP hub",
            Self::Rlusd => "members joined the RLUSD hub",
        }
    }

    /// What it counts of the Prepares over its method.
    const fn prepares(self) -> &'static str {
        match self {
            Self::Xrp => PREPARES_OVER_LIVE_XRP,
            Self::Rlusd => PREPARES_OVER_RLUSD,
        }
    }

    /// How long the members may take to join the hub: as long as a move
    /// into it may take before it is late, and on RLUSD the issuer's mint
    /// first, once each member's line exists.
    const fn joined_within_ms(self) -> u64 {
        match self {
            Self::Xrp => CREDIT_WAIT_MS,
            Self::Rlusd => 2 * CREDIT_WAIT_MS,
        }
    }
}

/// What a seed draws: its generator, how many validators the hub has (4
/// to 7, all honest), and how many members.
fn size(seed: u64, hub: LiveHub) -> (SeededRng, usize, usize) {
    let mut rng = SeededRng::new(seed ^ hub.salt());
    let validators = 4 + usize::try_from(rng.below(4)).unwrap();
    let members = 3 + usize::try_from(rng.below(2)).unwrap();
    (rng, validators, members)
}

/// What a seed of `node-hub-live` draws, before the run starts.
#[must_use]
pub fn describe_live(seed: u64) -> String {
    let (_, validators, members) = size(seed, LiveHub::Xrp);
    format!("{members} members of a mainnet XRP hub of {validators} validators")
}

/// What a seed of `node-hub-rlusd` draws, before the run starts.
#[must_use]
pub fn describe_rlusd(seed: u64) -> String {
    let (_, validators, members) = size(seed, LiveHub::Rlusd);
    format!("{members} members of a mainnet RLUSD hub of {validators} validators")
}

/// A run over a mainnet hub of XRP, as the module says.
#[must_use]
pub fn run_live(seed: u64) -> Run {
    run(seed, LiveHub::Xrp)
}

/// A run over a mainnet hub of RLUSD, as the module says.
#[must_use]
pub fn run_rlusd(seed: u64) -> Run {
    run(seed, LiveHub::Rlusd)
}

fn run(seed: u64, hub: LiveHub) -> Run {
    let (mut rng, validators, members) = size(seed, hub);
    let mut sim = Sim::with(seed, members, 0, BTreeMap::new(), hub.settling(validators));
    let result = script(&mut sim, &mut rng, hub).and_then(|()| check_calm(&sim));
    ended(sim, result)
}

fn script(sim: &mut Sim, rng: &mut SeededRng, hub: LiveHub) -> Result<(), Outcome> {
    if !sim.run_until_or(START + hub.joined_within_ms(), joined) {
        return Err(Outcome::liveness(
            "joined",
            format!(
                "the members did not all join the hub in time: {}",
                hub_standing(sim)
            ),
        ));
    }
    sim.count(hub.joined());
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
    lie(sim, rng)?;
    if !sim.run_until_or(sim.now + PAID_WITHIN_MS, paid) {
        return Err(Outcome::liveness(
            "lied-to",
            format!(
                "the members did not pay each other under lying servers: {}",
                hub_standing(sim)
            ),
        ));
    }
    sim.count("members paid each other under lying servers");
    if hub == LiveHub::Rlusd {
        off_peg(sim, rng)?;
    }
    settle(sim);
    if sim.counts.get(hub.prepares()).copied().unwrap_or(0) == 0 {
        return Err(Outcome::liveness(
            "over-hub",
            "no Prepare crossed the hub's method",
        ));
    }
    Ok(())
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

/// A lie drawn from every kind.
fn draw(rng: &mut SeededRng) -> Lie {
    Lie::ALL[usize::try_from(rng.below(Lie::ALL.len() as u64)).unwrap()]
}

/// Every member's connection to the ledger comes back to a server that
/// tells a lie drawn from every kind, as its owner deposits, and its owner
/// deposits again while the server lies, so that the history the server
/// gives, and its stream, have something to leave out. The servers of one
/// to all of the hub's validators lie as well, each a lie drawn, on every
/// connection their watchers make, until the members' lies end. The members
/// pay each other meanwhile; after every ledger, the validators attested
/// nothing false.
fn lie(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    let validators = sim.hub.as_deref().unwrap().validators.len();
    let lying = 1 + usize::try_from(rng.below(validators as u64)).unwrap();
    let first = usize::try_from(rng.below(validators as u64)).unwrap();
    let liars: Vec<usize> = (0..lying).map(|turn| (first + turn) % validators).collect();
    for &validator in &liars {
        let lie = draw(rng);
        sim.lie_to_validator(validator, Some(lie));
        sim.count("validator servers lying to the hub");
    }
    for member in 0..count {
        let lie = draw(rng);
        deposit(sim, member);
        // Only the liar's pairs may be withdrawn meanwhile: it sees nothing
        // of its ledger until its next connection anchors.
        sim.outages.push((
            Some(member),
            sim.now,
            sim.now + LEDGER_RECONNECT_MS + LIE_MS + STALL_MS,
        ));
        sim.lie_to(member, lie, LIE_MS);
    }
    pay(sim, rng);
    let again = sim.now + STALL_MS / 2;
    let end = sim.now + LIE_MS + STALL_MS / 2;
    let mut deposited = false;
    while sim.now < end {
        if !deposited && sim.now >= again {
            for member in 0..count {
                deposit(sim, member);
            }
            deposited = true;
        }
        sim.run_until(end.min(sim.now + LEDGER_CLOSE_MS));
        attested_truly(sim)?;
    }
    for validator in liars {
        sim.lie_to_validator(validator, None);
    }
    sim.end_lies();
    Ok(())
}

/// What the validators attested holds after a ledger of lies, as a safety
/// failure: the escrow holds what the hub owes, and an XRP hub's what its
/// sponsor left, but for what is on its way between them, and the hub
/// wrote off no reset, which mainnet never makes
/// ([`check_escrow_in`]). A lying server makes a validator's watcher attest
/// nothing, never something false (spec/areas/hubs.md#what-validators-attest).
/// While no honest validator runs, nobody attested anything to check.
fn attested_truly(sim: &Sim) -> Result<(), Outcome> {
    if sim.hub.as_deref().unwrap().ledger().is_none() {
        return Ok(());
    }
    check_escrow_in(sim, 0).map_err(|problem| Outcome::safety("attested", problem))
}

/// The token's pool moves off its peg, 5% under a dollar: once each
/// member's peg guard has held it off for its trip hold, the hub's ledger
/// converts nothing, while the members still pay each other over the hub,
/// in the token as it is. Then the pool is back at a dollar; the guards
/// resume after their recovery hold, which the run does not wait for:
/// nothing in it converts.
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
                .find(|view| view.id == HUB_RLUSD)
                .and_then(|view| view.peg)
                .and_then(|peg| peg.paused);
            paused == Some(PegPause::OffPeg) && core.price_table().halted(HUB_RLUSD)
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
                hub_standing(sim)
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

/// The network calms down: every move done, every account in step with
/// the hub, and every peer that was asked to settle settled.
fn settle(sim: &mut Sim) {
    sim.run_until(sim.now + CALM_MS);
    let settled =
        |sim: &Sim| idle_in(sim, &[0], None) && check_settled_with_honest(sim, None).is_ok();
    sim.run_until_or(sim.now + SETTLED_WITHIN_MS, settled);
    // The last settlement may have executed a block before the accounts
    // read it: a while of quiet, then the accounts read the hub again.
    sim.run_until(sim.now + CALM_MS);
    sim.run_until_or(sim.now + CALM_MS, settled);
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
            check_hub_packets(sim).map_err(|problem| Outcome::safety("hub-packets", problem)),
            check_hub_credit(sim).map_err(|problem| Outcome::safety("hub-credit", problem)),
            check_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
            check_no_losses(sim).map_err(|problem| Outcome::safety("hub-losses", problem)),
        ]
        .into_iter()
        .chain(check_each(sim, &[0], None, &[]))
        .chain([
            check_true(sim).map_err(|problem| Outcome::safety("proven", problem)),
            check_funded(sim).map_err(|problem| Outcome::liveness("funded", problem)),
            check_anchored(sim).map_err(|problem| Outcome::liveness("anchored", problem)),
            check_withdrawn(sim).map_err(|problem| Outcome::liveness("withdrawn", problem)),
            check_finished(sim).map_err(|problem| Outcome::liveness("finished", problem)),
            check_settled_with_honest(sim, None)
                .map_err(|problem| Outcome::liveness("settled", problem)),
        ])
        .chain(incoming)
        .collect::<Vec<_>>(),
    )
}

/// No member booked a loss on a peer over the hub but the hub's fee for a
/// claim it made there itself, when its peer's own claim was late
/// (`LossCause::ClaimFee`): every member is honest, and the hub paid no
/// peer for a packet the member had rejected or forgotten. The run counts
/// what each booked (`Sim::hub_losses`) but not why, which its trace of
/// each node's events says.
fn check_no_losses(sim: &Sim) -> Result<(), String> {
    let lost = sim
        .trace
        .iter()
        .find(|line| line.contains("Settlement(PeerLoss {") && !line.contains("cause: ClaimFee"));
    match lost {
        None => Ok(()),
        Some(line) => Err(format!(
            "a member lost more than a claim's fee over the hub: {line}"
        )),
    }
}
