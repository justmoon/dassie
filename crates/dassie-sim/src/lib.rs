//! Runs Dassie's simulations from seeds (D27).
//!
//! Each simulation lives in the crate it simulates, behind that crate's
//! `sim` feature, and is a function from a seed to a [`Run`]: the same seed
//! gives the same run, byte for byte (D16). `cargo test` runs each on a few
//! fixed seeds. This crate runs them on any seed: one to replay a failure,
//! a range to search for one, or a commit's, as TigerBeetle's VOPR does.

use std::ops::Range;

pub use dassie_node_core::testing::{Outcome, Run, caught};

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
}

/// Every simulation the runner knows.
pub static SIMS: [Sim; 3] = [
    Sim {
        name: "peer",
        about: "peer networks through chaos and calm: membership, peering, routes and settlements",
        describe: dassie_peer::sim::chaos::describe,
        run: dassie_peer::sim::chaos::run,
    },
    Sim {
        name: "connector",
        about: "the connector under random packets, answers and disconnections",
        describe: dassie_connector::sim::traffic::describe,
        run: dassie_connector::sim::traffic::run,
    },
    Sim {
        name: "stream",
        about: "a STREAM payment over a lossy path with an exchange rate",
        describe: dassie_stream::sim::payment::describe,
        run: dassie_stream::sim::payment::run,
    },
];

/// The simulation called `name`.
#[must_use]
pub fn find(name: &str) -> Option<&'static Sim> {
    SIMS.iter().find(|sim| sim.name == name)
}

/// Parses which seeds to run: one seed (`42`), a range (`0..1000`), or a
/// git commit (`3c3db0fe…`, at least 16 hex digits), whose first 16 hex
/// digits are its seed, so that a commit names its own run.
///
/// # Errors
///
/// When `text` is none of these, or an empty range.
pub fn parse_seeds(text: &str) -> Result<Range<u64>, String> {
    let seeds = if let Some((from, to)) = text.split_once("..") {
        number(from)?..number(to)?
    } else if text.len() >= 16 && text.chars().all(|c| c.is_ascii_hexdigit()) {
        let digits = text.get(..16).unwrap_or(text);
        let seed = u64::from_str_radix(digits, 16).map_err(|error| error.to_string())?;
        seed..seed.saturating_add(1)
    } else {
        let seed = number(text)?;
        seed..seed.saturating_add(1)
    };
    if seeds.is_empty() {
        return Err(format!("{text} holds no seeds"));
    }
    Ok(seeds)
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
        assert_eq!(parse_seeds("42"), Ok(42..43));
        assert_eq!(parse_seeds("10..20"), Ok(10..20));
        assert_eq!(
            parse_seeds("3c3db0fe0123456789abcdef0123456789abcdef"),
            Ok(0x3c3d_b0fe_0123_4567..0x3c3d_b0fe_0123_4568)
        );
        assert!(parse_seeds("20..10").is_err());
        assert!(parse_seeds("abc").is_err());
        assert!(parse_seeds("").is_err());
    }

    #[test]
    fn every_simulation_has_its_own_name() {
        let mut names: Vec<_> = SIMS.iter().map(|sim| sim.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SIMS.len());
    }
}
