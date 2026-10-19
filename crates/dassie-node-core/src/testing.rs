//! A seeded RNG for tests and simulations, and what the simulations
//! share: how a run ended, and running a range of seeds (D27).

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use core::convert::Infallible;
use core::fmt;
use core::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

use rand_core::{TryCryptoRng, TryRng};

/// xoshiro256**, seeded with SplitMix64. Fast and reproducible; **not** a
/// cryptographic generator, despite implementing [`TryCryptoRng`] so that
/// it can stand in for one in tests.
#[derive(Clone, Debug)]
pub struct SeededRng {
    state: [u64; 4],
}

impl SeededRng {
    /// A generator whose whole output follows from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut mix = seed;
        let mut next = || {
            mix = mix.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = mix;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        Self {
            state: [next(), next(), next(), next()],
        }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        let [a, b, c, d] = &mut self.state;
        let result = b.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = *b << 17;
        *c ^= *a;
        *d ^= *b;
        *b ^= *c;
        *a ^= *d;
        *c ^= t;
        *d = d.rotate_left(45);
        result
    }

    /// Uniform enough in `0..bound`, and 0 for a bound of 0.
    pub fn below(&mut self, bound: u64) -> u64 {
        self.next_u64().checked_rem(bound).unwrap_or(0)
    }
}

impl TryRng for SeededRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        // The high half, which is the better half of xoshiro's output.
        Ok(u32::try_from(self.next_u64() >> 32).unwrap_or_default())
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        Ok(self.next_u64())
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), Infallible> {
        for chunk in destination.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            for (out, byte) in chunk.iter_mut().zip(bytes) {
                *out = byte;
            }
        }
        Ok(())
    }
}

impl TryCryptoRng for SeededRng {}

/// How a simulated run ended (D27).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every check held.
    Pass,
    /// Something that must never happen did: an assertion or invariant
    /// broke, or a check of what the run did failed.
    Safety(String),
    /// Something that must happen did not, once the network was calm.
    Liveness(String),
    /// The run drew a situation that no code could recover from, so it
    /// neither passes nor fails.
    Unsupported(String),
}

impl Outcome {
    /// Whether the run found a bug.
    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Safety(_) | Self::Liveness(_))
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pass => f.write_str("pass"),
            Self::Safety(problem) => write!(f, "safety: {problem}"),
            Self::Liveness(problem) => write!(f, "liveness: {problem}"),
            Self::Unsupported(reason) => write!(f, "unsupported: {reason}"),
        }
    }
}

/// A simulated run: how it ended, and what happened in it, a line per
/// event, for comparing and explaining runs.
#[derive(Clone, Debug)]
pub struct Run {
    /// How it ended.
    pub outcome: Outcome,
    /// What happened.
    pub trace: Vec<String>,
}

/// Runs `run`, and turns a panic into a safety failure with the panic's
/// message. The trace of a run that panicked is lost.
pub fn caught(run: impl FnOnce() -> Run) -> Run {
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic without a message".to_owned());
        Run {
            outcome: Outcome::Safety(message),
            trace: Vec::new(),
        }
    })
}

/// Runs every seed, and fails naming the first one that found a bug.
pub fn assert_seeds_pass(seeds: Range<u64>, run: impl Fn(u64) -> Run) {
    for seed in seeds {
        let outcome = caught(|| run(seed)).outcome;
        assert!(!outcome.is_failure(), "seed {seed}: {outcome}");
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    fn ended(outcome: Outcome) -> Run {
        Run {
            outcome,
            trace: vec!["a line".to_string()],
        }
    }

    #[test]
    fn a_panic_is_a_safety_failure_with_its_message() {
        let seed = 7;
        let formatted = caught(|| panic!("seed {seed} broke"));
        let fixed = caught(|| panic!("broke"));
        assert_eq!(formatted.outcome, Outcome::Safety("seed 7 broke".into()));
        assert!(formatted.trace.is_empty());
        assert_eq!(fixed.outcome, Outcome::Safety("broke".into()));
    }

    #[test]
    fn a_run_that_returns_keeps_its_outcome_and_trace() {
        let run = caught(|| ended(Outcome::Liveness("stuck".into())));
        assert_eq!(run.outcome, Outcome::Liveness("stuck".into()));
        assert_eq!(run.trace, vec!["a line".to_string()]);
        assert_eq!(format!("{}", run.outcome), "liveness: stuck");
    }

    #[test]
    fn only_safety_and_liveness_are_failures() {
        assert!(!Outcome::Pass.is_failure());
        assert!(!Outcome::Unsupported("no quorum".into()).is_failure());
        assert!(Outcome::Safety(String::new()).is_failure());
        assert!(Outcome::Liveness(String::new()).is_failure());
        assert_seeds_pass(0..3, |_| ended(Outcome::Unsupported("never".into())));
    }

    #[test]
    fn below_stays_below_its_bound() {
        let mut rng = SeededRng::new(1);
        assert!((0..1_000).all(|_| rng.below(10) < 10));
        assert_eq!(rng.below(0), 0);
    }
}
