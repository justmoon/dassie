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

use super::{Faults, Network, Role, START, roles};
use crate::consensus::Config;
use crate::consensus::wire::MAX_DELIVERED_BLOCKS;

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
}

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
        ];
        let drawn = usize::try_from(rng.below(faults as u64 + 1)).unwrap_or(0);
        (0..drawn)
            .map(|_| kinds[usize::try_from(rng.below(4)).unwrap_or(0)])
            .collect()
    };
    let roles = roles(count, &byzantine);
    (rng, Setup { roles, collusion })
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
    format!("{} validators, Byzantine: {byzantine:?}", setup.roles.len())
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
    let mut network = Network::new(seed, &setup.roles, CONFIG);
    let mut counts = alloc::collections::BTreeMap::new();
    let mut at = START;
    while at < START + CHAOS_MS {
        at += 100 + rng.below(1_500);
        network.run_until(at);
        let action = chaos(&mut network, &mut rng);
        *counts.entry(action).or_insert(0) += 1;
    }
    network.faults = Faults::CALM;
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
    if network.state_loads > 0 {
        counts.insert("state-load", network.state_loads);
    }
    run.counts = counts;
    run
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
