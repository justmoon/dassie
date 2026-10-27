//! Runs Dassie's simulations from seeds
//! (spec/decisions/random-seed-simulations.md).
//!
//! Each simulation lives in the crate it simulates, in its `sim` module,
//! behind that crate's `sim` feature (`testing` for `dassie-node`), and is
//! a function from a seed to a [`Run`]: the same seed
//! gives the same run, byte for byte (spec/decisions/tigerstyle.md). `cargo
//! test` runs scenarios on fixed seeds. This crate runs them on any seed:
//! one to replay a failure, a range to search for one, a commit's, or random
//! ones for as long as a campaign lasts, as TigerBeetle's VOPR does.

use std::ops::RangeInclusive;

pub use dassie_kit::sim::{Outcome, Run, caught};

/// A simulation the runner knows.
#[derive(Clone, Copy, Debug)]
pub struct Sim {
    /// What to call it on the command line.
    pub name: &'static str,
    /// What it simulates.
    pub about: &'static str,
    /// What a seed draws, before the run starts.
    pub describe: fn(u64) -> String,
    /// Runs a seed. A panic is a safety failure; see [`caught`].
    pub run: fn(u64) -> Run,
    /// How many of a campaign's runs it gets, relative to the others.
    pub weight: u64,
}

/// Every simulation the runner knows. A weight counts runs, not time: a
/// run that takes a millisecond costs a campaign almost nothing, so the
/// time goes to the simulations whose runs take seconds, and most of it to
/// the `node` simulation. A slice, so that adding one changes no length.
pub static SIMS: &[Sim] = &[
    Sim {
        name: "node",
        about: "whole nodes' engine cores, with a network, faults and payments drawn per seed, \
                through chaos, then healed, on the stub with unlimited trust or with \
                commitments, or over a settlement hub run in the simulation",
        describe: dassie_node::sim::swarm::describe,
        run: dassie_node::sim::swarm::run,
        // The most, and most of the time: it runs the node whole, and each
        // seed draws a situation of its own.
        weight: 50,
    },
    Sim {
        name: "node-stub",
        about: "whole nodes' engine cores paying and crashing over the stub with unlimited \
                trust, as scripted",
        describe: dassie_node::sim::scripted::describe,
        run: dassie_node::sim::scripted::run,
        // A script: its seeds vary the detail, not the situation.
        weight: 5,
    },
    Sim {
        name: "node-stub-hub",
        about: "the same over the stub with commitments, its packets taking the path over a \
                settlement hub",
        describe: dassie_node::sim::scripted::describe_stub_hub,
        run: dassie_node::sim::scripted::run_stub_hub,
        weight: 5,
    },
    Sim {
        name: "node-hub",
        about: "whole nodes' engine cores peered over a settlement hub whose validators run in \
                the simulation, faulty ones and a lying member among them, until the hub stalls \
                and its escrow on a fake XRP Ledger pays out, or colluders fork it",
        describe: dassie_node::sim::hubbed::describe,
        run: dassie_node::sim::hubbed::run,
        // A script, its runs among the longest: a hub's validators make a
        // block every tenth of a second.
        weight: 5,
    },
    Sim {
        name: "node-hub-lies",
        about: "the same with every validator honest, its members' XRP Ledger servers telling \
                each kind of lie in turn, and some of its validators' too, while the ledger's \
                validators rotate a key, withhold a quorum and let a list lapse",
        describe: dassie_node::sim::hub_runs::describe_lies,
        run: dassie_node::sim::hub_runs::run_lies,
        // A script over a hub, as long as `node-hub`'s runs: the only runs
        // of lying servers once the direct schemes go.
        weight: 5,
    },
    Sim {
        name: "node-hub-currencies",
        about: "across dollars in a DSUSD hub and XRP in an XRP hub, through nodes in both \
                that convert, one of them charging",
        describe: dassie_node::sim::hub_runs::describe_across,
        run: dassie_node::sim::hub_runs::run_across,
        // A script over a hub, as long as `node-hub`'s runs: the only runs
        // across currencies over hubs.
        weight: 5,
    },
    Sim {
        name: "node-hubs",
        about: "whole nodes' engine cores in an XRP hub and a DSUSD hub, each pair peered over \
                both, paying in XRP and in dollars, each over its own hub, until the hub of the \
                method the peerings were not made on stalls and pays out while the other \
                carries on",
        describe: dassie_node::sim::hub_pairs::describe,
        run: dassie_node::sim::hub_pairs::run,
        // A script over two hubs, as long as `node-hub`'s runs.
        weight: 5,
    },
    Sim {
        name: "node-hub-routes",
        about: "whole nodes' engine cores in an XRP hub and a DSUSD hub, paying across nodes in \
                both without converting, one node with two first hops drawn by the fees each \
                earned it: a hop that refuses, an attacker's rejected packets and a hop short of \
                room",
        describe: dassie_node::sim::hub_routes::describe,
        run: dassie_node::sim::hub_routes::run,
        // A script over two hubs, as long as `node-hub`'s runs.
        weight: 5,
    },
    Sim {
        name: "node-hub-dsusd",
        about: "whole nodes' engine cores peered over a settlement hub of DSUSD, funding it from \
                their DSUSD owner books, paying off the token's peg, the escrow's XRP sent to it \
                as gas and topped up before a withdrawal, through a freeze by the issuer and \
                its thaw, until a clawback or a stall ends the hub and its escrow pays out in \
                DSUSD",
        describe: dassie_node::sim::hub_token::describe,
        run: dassie_node::sim::hub_token::run,
        // A script as long as node-hub's.
        weight: 5,
    },
    Sim {
        name: "node-hub-live",
        about: "whole nodes' engine cores in the live realm peered over a settlement hub of XRP \
                on the fake XRP Ledger's mainnet, funding it from their XRP owner books, which \
                their owners' deposits fund, while their servers and the hub's validators' \
                servers lie",
        describe: dassie_node::sim::hub_live::describe_live,
        run: dassie_node::sim::hub_live::run_live,
        // A script over a hub, shorter than node-hub's.
        weight: 5,
    },
    Sim {
        name: "node-hub-rlusd",
        about: "the same over a hub of RLUSD, funded from the members' RLUSD owner books and its \
                escrow's gas from their XRP books, paying each other over it while the token is \
                off its peg",
        describe: dassie_node::sim::hub_live::describe_rlusd,
        run: dassie_node::sim::hub_live::run_rlusd,
        // A script over a hub, shorter than node-hub's.
        weight: 5,
    },
    Sim {
        name: "node-owner-books",
        about: "whole nodes' engine cores paying over the stub with unlimited trust, as \
                scripted, each keeping its owner's books of XRP and DSUSD on a fake XRP \
                Ledger, paying out of them while the issuer claws back and freezes lines, \
                its servers telling each kind of lie in turn, while its validators rotate a \
                key, withhold a quorum and let a list lapse",
        describe: dassie_node::sim::owner_books::describe,
        run: dassie_node::sim::owner_books::run,
        // A script: its seeds vary the detail, not the situation.
        weight: 5,
    },
    Sim {
        name: "node-owner-books-live",
        about: "the owner's books of XRP and RLUSD in the live realm on the fake XRP Ledger's \
                mainnet, funded by deposits, while the issuer claws back and freezes lines \
                and a server lies, its nodes peering with no one",
        describe: dassie_node::sim::owner_books::describe_live,
        run: dassie_node::sim::owner_books::run_live,
        weight: 5,
    },
    Sim {
        name: "peer",
        about: "peer networks through chaos and calm: membership, peering, routes and settlements",
        describe: dassie_peer::sim::chaos::describe,
        run: dassie_peer::sim::chaos::run,
        // Its runs and the `names` simulation's take the longest on
        // average, some eight to ten seconds each, though a `node` run
        // split for days takes longer: a few of them take about an eighth
        // of the time, more than the scripts together.
        weight: 4,
    },
    Sim {
        name: "names",
        about: "names registered with four bootstrap nodes, one of them perhaps faulty, through \
                chaos and calm: one holder per name, no front-running, splits released",
        describe: dassie_peer::sim::names::describe,
        run: dassie_peer::sim::names::run,
        // Its runs take some ten seconds each, as long as the peer
        // simulation's, or a little longer.
        weight: 4,
    },
    Sim {
        name: "connector",
        about: "the connector under random packets, answers and disconnections",
        describe: dassie_connector::sim::traffic::describe,
        run: dassie_connector::sim::traffic::run,
        // Its runs take milliseconds, and the stream's less: many of them
        // cost almost no time.
        weight: 10,
    },
    Sim {
        name: "stream",
        about: "a STREAM payment over a lossy path with an exchange rate",
        describe: dassie_stream::sim::payment::describe,
        run: dassie_stream::sim::payment::run,
        weight: 10,
    },
    Sim {
        name: "hub",
        about: "a settlement hub's validators running Simplex through chaos, Byzantine \
                validators among them, then healed",
        describe: dassie_hub::sim::chaos::describe,
        run: dassie_hub::sim::chaos::run,
        // Its runs take a fraction of a second.
        weight: 10,
    },
    Sim {
        name: "tripwire",
        about: "fails on purpose, on one seed in a thousand, so that a silent campaign shows",
        describe: tripwire::describe,
        run: tripwire::run,
        // Its runs cost no time, and a tenth of the runs make it trip
        // about once an hour on eight cores: often enough to see a
        // campaign stop, rarely enough not to bury the real failures.
        weight: 10,
    },
];

/// The tripwire (spec/decisions/random-seed-simulations.md): a simulation
/// that fails on purpose, for safety, on one seed in [`tripwire::ONE_IN`],
/// and passes on the others, at once. A campaign runs it now and then, and
/// files its failures apart, so that a campaign whose tripwire stops failing
/// shows that it has stopped. No test expects it to pass on a seed where it
/// fails.
pub mod tripwire {
    use super::{Outcome, Run};

    /// One seed in this many fails: every multiple of it, so that
    /// `dassie-sim run tripwire 0` shows a failure whenever one is wanted.
    pub const ONE_IN: u64 = 1_000;

    /// What every seed draws: nothing.
    #[must_use]
    pub fn describe(_seed: u64) -> String {
        format!("fails every seed that is a multiple of {ONE_IN}")
    }

    /// Fails on a multiple of [`ONE_IN`], and passes on the other seeds.
    #[must_use]
    pub fn run(seed: u64) -> Run {
        let trips = seed.is_multiple_of(ONE_IN);
        let trace = vec![format!("seed {seed} trips: {trips}")];
        let outcome = if trips {
            Outcome::safety("on-purpose", "the tripwire tripped, on purpose")
        } else {
            Outcome::Pass
        };
        Run::new(outcome, trace)
    }
}

/// The simulation called `name`.
#[must_use]
pub fn find(name: &str) -> Option<&'static Sim> {
    SIMS.iter().find(|sim| sim.name == name)
}

/// Parses which seeds to run: one seed in decimal, as the runner prints it
/// (`42`), a range (`0..1000`), or a git commit's full 40-digit SHA, whose
/// first 16 hex digits are its seed, so that a commit names its own run.
/// Only a full SHA is read as hex: a decimal seed has at most 20 digits.
///
/// # Errors
///
/// When `text` is none of these, or an empty range.
pub fn parse_seeds(text: &str) -> Result<RangeInclusive<u64>, String> {
    if let Some((from, to)) = text.split_once("..") {
        let (from, to) = (number(from)?, number(to)?);
        let last = to
            .checked_sub(1)
            .filter(|last| *last >= from)
            .ok_or_else(|| format!("{text} holds no seeds"))?;
        return Ok(from..=last);
    }
    let seed = if text.len() == 40 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        let digits = text.get(..16).unwrap_or(text);
        u64::from_str_radix(digits, 16).map_err(|error| error.to_string())?
    } else {
        number(text)?
    };
    Ok(seed..=seed)
}

fn number(text: &str) -> Result<u64, String> {
    text.parse()
        .map_err(|_| format!("{text:?} is not a seed: give a number, a range or a commit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_a_number_a_range_or_a_commit() {
        assert_eq!(parse_seeds("42"), Ok(42..=42));
        assert_eq!(parse_seeds("10..20"), Ok(10..=19));
        assert_eq!(
            parse_seeds("3c3db0fe0123456789abcdef0123456789abcdef"),
            Ok(0x3c3d_b0fe_0123_4567..=0x3c3d_b0fe_0123_4567)
        );
        assert!(parse_seeds("20..10").is_err());
        assert!(parse_seeds("10..10").is_err());
        assert!(parse_seeds("abc").is_err());
        assert!(parse_seeds("3c3db0fe0123456789").is_err());
        assert!(parse_seeds("").is_err());
    }

    #[test]
    fn every_seed_the_runner_prints_parses_back() {
        for seed in [
            0,
            9,
            1_234_567_890_123_456,
            13_838_123_456_789_012_345,
            u64::MAX,
        ] {
            assert_eq!(parse_seeds(&seed.to_string()), Ok(seed..=seed), "{seed}");
        }
    }

    #[test]
    fn every_simulation_has_its_own_name_and_a_share_of_a_campaign() {
        let mut names: Vec<_> = SIMS.iter().map(|sim| sim.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SIMS.len());
        assert!(SIMS.iter().all(|sim| sim.weight > 0));
        let most = SIMS.iter().max_by_key(|sim| sim.weight).unwrap();
        assert_eq!(most.name, "node");
    }

    #[test]
    fn the_tripwire_fails_one_seed_in_a_thousand_at_one_place() {
        let tripped: Vec<u64> = (0..10_000)
            .filter(|seed| tripwire::run(*seed).outcome.is_failure())
            .collect();
        assert_eq!(tripped.len(), 10);
        let wire = find("tripwire").unwrap();
        let run = caught(|| (wire.run)(7_000));
        assert_eq!(run.outcome.name(), "safety");
        assert_eq!(run.outcome.finding().unwrap().at, "on-purpose");
        assert_eq!(caught(|| (wire.run)(7_001)).outcome, Outcome::Pass);
    }
}
