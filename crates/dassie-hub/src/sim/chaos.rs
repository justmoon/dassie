//! The hub's random run (spec/areas/testing.md#the-simulations): a set of 4
//! to 7 validators, up to f of them Byzantine (silent, withholding their
//! proposals, proposing invalid payloads, or equivocating), through a
//! stretch of chaos (loss, duplication, slow messages, partitions, crashes
//! and restarts from what each persisted, clocks off within the precision),
//! then healed. One seed in ten has f + 1 validators collude to fork the
//! chain instead.
//!
//! Checks: no evidence ever names an honest validator; with at most f
//! faulty, no two honest validators finalize different blocks at one height,
//! and blocks finalize again within the heal's deadline; with f + 1
//! colluding, a fork may happen, but then evidence must appear.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_node_core::testing::{Outcome, Run, SeededRng};

use super::{Faults, Network, Role, START, Schedule, key, roles, set};
use crate::consensus::Config;
use crate::consensus::wire::MAX_DELIVERED_BLOCKS;
use crate::validators::ValidatorSet;

/// The production settings: nothing is scaled down.
const CONFIG: Config = Config::DEFAULT;

/// How long the network is rough: long enough for views to stall and
/// restart many times over, at a block every interval.
const CHAOS_MS: u64 = 20_000;

/// How many blocks every honest validator must finalize after the heal.
const LIVE_BLOCKS: u64 = 3;

/// What a seed draws.
struct Setup {
    roles: Vec<Role>,
    collusion: bool,
    /// Whether one more validator follows the hub from the start, and
    /// replaces validator 0 at `from`, a change the block at `at`
    /// schedules.
    reconfigure: bool,
    at: u64,
    from: u64,
    /// Whether the network splits in two just before `from`, where the
    /// votes on the epoch's last blocks are most likely to split: a knob
    /// for coverage of the handover, not a production value.
    split_at_boundary: bool,
    /// Whether every vote to finalize the epoch's last block itself is lost
    /// during the chaos, so that only a repeat can make it final: a knob
    /// too.
    lose_last_finalize: bool,
    /// How many states after blocks that are not final each validator's
    /// application keeps, if it is bounded: the engine must verify again
    /// what it let go of.
    keep_states: Option<usize>,
}

/// The block that schedules a change of validators, in seeds that draw one.
const RECONFIGURE_AT: u64 = 30;

/// The height the new set takes over at, with a notice the ledger might
/// give; seeds that split at the boundary draw the least notice too.
const RECONFIGURE_FROM: u64 = 60;

fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ 0x6875_625f_7369_6d00);
    let count = 4 + usize::try_from(rng.below(4)).unwrap_or(0);
    let faults = (count - 1) / 3;
    let collusion = rng.below(10) == 0;
    let byzantine: Vec<Role> = if collusion {
        alloc::vec![Role::Collude; faults + 1]
    } else {
        let kinds = [
            Role::Silent,
            Role::Withhold,
            Role::Invalid,
            Role::Equivocate,
            Role::Flood,
        ];
        let drawn = usize::try_from(rng.below(faults as u64 + 1)).unwrap_or(0);
        (0..drawn)
            .map(|_| kinds[usize::try_from(rng.below(kinds.len() as u64)).unwrap_or(0)])
            .collect()
    };
    let mut roles = roles(count, &byzantine);
    let reconfigure = rng.below(3) == 0;
    if reconfigure {
        roles.push(Role::Honest);
    }
    let split_at_boundary = reconfigure && rng.below(2) == 0;
    // The least notice the ledger gives: the change takes over two blocks
    // after the one that schedules it.
    let least = split_at_boundary && rng.below(2) == 0;
    let from = if least {
        RECONFIGURE_AT + 2
    } else {
        RECONFIGURE_FROM
    };
    let lose_last_finalize = split_at_boundary && rng.below(2) == 0;
    let keep_states = (rng.below(3) == 0).then(|| 1 + usize::try_from(rng.below(8)).unwrap_or(0));
    let setup = Setup {
        roles,
        collusion,
        reconfigure,
        at: RECONFIGURE_AT,
        from,
        split_at_boundary,
        lose_last_finalize,
        keep_states,
    };
    (rng, setup)
}

/// What the seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    let byzantine: Vec<_> = setup
        .roles
        .iter()
        .filter(|role| !role.is_honest())
        .collect();
    if setup.reconfigure {
        let count = setup.roles.len() - 1;
        format!(
            "{count} validators, Byzantine: {byzantine:?}, validator {count} replacing 0 \
             from height {}{}",
            setup.from,
            if setup.split_at_boundary {
                ", split at the boundary"
            } else {
                ""
            }
        )
    } else {
        format!("{} validators, Byzantine: {byzantine:?}", setup.roles.len())
    }
}

/// The longest blocks may take to finalize again after the heal, from the
/// production rules: a view stuck in the chaos rebroadcasts its nullify
/// within `max_retry_ms`; a validator `behind` blocks catches up a delivery
/// of blocks per view, at worst an advance timeout each; then each of
/// [`LIVE_BLOCKS`] blocks takes at most a rotation of `count` leaders, each
/// faulty one's view lost to its timeout.
fn heal_deadline(count: u64, behind: u64) -> u64 {
    let catch_up = (behind / MAX_DELIVERED_BLOCKS as u64 + 1) * CONFIG.advance_timeout_ms;
    CONFIG.max_retry_ms + catch_up + LIVE_BLOCKS * count * CONFIG.advance_timeout_ms
}

/// Runs the seed and checks it.
#[must_use]
pub fn run(seed: u64) -> Run {
    let (mut rng, setup) = setup(seed);
    let mut network = network(seed, &setup);
    for sim in &mut network.validators {
        sim.app.keep_states = setup.keep_states;
    }
    let mut counts = alloc::collections::BTreeMap::new();
    let mut at = START;
    let mut split = setup.split_at_boundary;
    if setup.lose_last_finalize {
        network.lose_finalize_at = Some(setup.from - 1);
    }
    let mut heal_at = None;
    while at < START + CHAOS_MS {
        // Waiting to split at the boundary, the network is watched closely.
        at += if split { 20 } else { 100 + rng.below(1_500) };
        network.run_until(at);
        if heal_at.is_some_and(|heal| at >= heal) {
            network.heal();
            heal_at = None;
        }
        if split {
            let top = network
                .honest()
                .map(|i| network.height(i))
                .max()
                .unwrap_or(0);
            if top.saturating_add(3) >= setup.from {
                split = false;
                split_in_two(&mut network, &mut rng);
                heal_at = Some(at + 300 + rng.below(1_700));
                counts.insert("boundary-split", 1);
            }
            continue;
        }
        let action = chaos(&mut network, &mut rng);
        *counts.entry(action).or_insert(0) += 1;
    }
    network.faults = Faults::CALM;
    network.lose_finalize_at = None;
    network.heal();
    for index in 0..network.validators.len() {
        if !network.is_up(index) {
            network.start(index);
        }
    }
    let floor = network.honest_floor();
    let top = network
        .honest()
        .map(|index| network.height(index))
        .max()
        .unwrap_or(0);
    let count = network.validators.len() as u64;
    let deadline = network.now + heal_deadline(count, top - floor);
    network.run_until(deadline);
    let outcome = check(&network, &setup, top);
    let mut run = Run::new(outcome, core::mem::take(&mut network.trace));
    if network.evidence_found() {
        counts.insert("evidence", 1);
    }
    if network.disagreement().is_some() {
        counts.insert("fork", 1);
    }
    if setup.reconfigure && network.honest_floor() >= setup.from {
        counts.insert("reconfigured", 1);
    }
    if network.state_loads > 0 {
        counts.insert("state-load", network.state_loads);
    }
    if network.repeat_handovers > 0 {
        counts.insert("repeat-handover", network.repeat_handovers);
    }
    run.counts = counts;
    run
}

/// The run's network: with a change of validators scheduled, validator
/// `count` follows the hub and replaces validator 0, keeping the others'
/// places in the set, so that evidence names the same validators.
fn network(seed: u64, setup: &Setup) -> Network {
    if !setup.reconfigure {
        return Network::new(seed, &setup.roles, CONFIG);
    }
    let count = setup.roles.len() - 1;
    let mut network = Network::with_genesis(seed, &setup.roles, CONFIG, count);
    let keys = core::iter::once(count)
        .chain(1..count)
        .map(|index| key(index).public_key())
        .collect();
    let set = ValidatorSet::new(keys).unwrap_or_else(|| set(0..count));
    for sim in &mut network.validators {
        sim.app.schedule = Some(Schedule {
            at: setup.at,
            from: setup.from,
            set: set.clone(),
        });
    }
    network
}

/// Splits the network into two halves, drawn at random.
fn split_in_two(network: &mut Network, rng: &mut SeededRng) {
    let count = network.validators.len();
    let mut side: Vec<usize> = (0..count).collect();
    for at in 0..count {
        let other = usize::try_from(rng.below(count as u64)).unwrap_or(0);
        side.swap(at, other);
    }
    side.truncate(count / 2);
    network.partition(&side);
}

/// One stroke of chaos, and its name.
fn chaos(network: &mut Network, rng: &mut SeededRng) -> &'static str {
    let count = network.validators.len();
    let pick = usize::try_from(rng.below(count as u64)).unwrap_or(0);
    let honest = network.validators[pick].role.is_honest();
    match rng.below(8) {
        0 if honest && network.is_up(pick) => {
            network.crash(pick);
            "crash"
        }
        1 | 2 if !network.is_up(pick) => {
            network.start(pick);
            "restart"
        }
        3 => {
            let size = 1 + usize::try_from(rng.below(count as u64 / 2)).unwrap_or(0);
            let mut side: Vec<usize> = (0..count).collect();
            for at in 0..count {
                let other = usize::try_from(rng.below(count as u64)).unwrap_or(0);
                side.swap(at, other);
            }
            side.truncate(size);
            network.partition(&side);
            "partition"
        }
        4 | 5 => {
            network.heal();
            "heal"
        }
        6 => {
            network.faults = if network.faults == Faults::CALM {
                Faults::ROUGH
            } else {
                Faults::CALM
            };
            "faults"
        }
        _ => "calm",
    }
}

/// Whether every honest validator past the boundary of a scheduled change
/// runs the new set there: a problem if one does not.
fn reconfigured(network: &Network, setup: &Setup) -> Option<String> {
    if !setup.reconfigure {
        return None;
    }
    network.honest().find_map(|index| {
        let schedule = network.validators[index].app.schedule.as_ref()?;
        let engine = network.validators[index].engine.as_ref()?;
        let past = engine.tip().block.height >= setup.from;
        (past && engine.set_for_height(setup.from) != &schedule.set).then(|| {
            format!(
                "validator {index} is past height {} with the old set",
                setup.from
            )
        })
    })
}

fn check(network: &Network, setup: &Setup, top: u64) -> Outcome {
    if let Some(problem) = network.blamed_honest() {
        return Outcome::safety("blameless", problem);
    }
    let disagreement = network.disagreement();
    if setup.collusion {
        return match disagreement {
            Some(problem) if !network.evidence_found() => {
                Outcome::safety("evidence", format!("a fork without evidence: {problem}"))
            }
            _ => Outcome::Pass,
        };
    }
    if let Some(problem) = disagreement {
        return Outcome::safety("agreement", problem);
    }
    if let Some(problem) = reconfigured(network, setup) {
        return Outcome::safety("reconfigured", problem);
    }
    for index in network.honest() {
        let height = network.height(index);
        if height < top + LIVE_BLOCKS {
            return Outcome::liveness(
                "finality",
                format!(
                    "validator {index} finalized up to {height} after the heal, short of {}",
                    top + LIVE_BLOCKS
                ),
            );
        }
    }
    Outcome::Pass
}
