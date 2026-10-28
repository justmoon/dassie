//! A seeded RNG for tests and simulations, and what the simulations
//! share: how a run ended, and catching where a run panicked
//! (spec/decisions/random-seed-simulations.md).

use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use core::convert::Infallible;
use core::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

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

/// How a simulated run ended (spec/decisions/random-seed-simulations.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every check held.
    Pass,
    /// Something that must never happen did: an assertion or invariant
    /// broke, or a check of what the run did failed.
    Safety(Finding),
    /// Something that must happen did not, once the network was calm.
    Liveness(Finding),
    /// The run drew a situation that no code could recover from, so it
    /// neither passes nor fails.
    Unsupported(Finding),
}

/// What a run that did not pass found: where, the same for every seed
/// that ends there, and what, with this run's details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Where: the name of the check that failed or ruled the run out, or
    /// the file and line of a panic. It holds nothing a seed draws, so
    /// runs that end the same way share it; with the simulation and the
    /// outcome, it is a failure's signature, and a campaign files one
    /// issue per signature (spec/decisions/random-seed-simulations.md).
    pub at: String,
    /// What happened, with this run's numbers.
    pub message: String,
}

impl Outcome {
    /// A safety failure that the check called `check` found.
    #[must_use]
    pub fn safety(check: &'static str, message: impl Into<String>) -> Self {
        Self::Safety(Finding::checked(check, message.into()))
    }

    /// A liveness failure that the check called `check` found.
    #[must_use]
    pub fn liveness(check: &'static str, message: impl Into<String>) -> Self {
        Self::Liveness(Finding::checked(check, message.into()))
    }

    /// A run that the check called `check` ruled out.
    #[must_use]
    pub fn unsupported(check: &'static str, reason: impl Into<String>) -> Self {
        Self::Unsupported(Finding::checked(check, reason.into()))
    }

    /// Whether the run found a bug.
    #[must_use]
    pub const fn is_failure(&self) -> bool {
        matches!(self, Self::Safety(_) | Self::Liveness(_))
    }

    /// What it is called: `pass`, `safety`, `liveness` or `unsupported`.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Safety(_) => "safety",
            Self::Liveness(_) => "liveness",
            Self::Unsupported(_) => "unsupported",
        }
    }

    /// What the run found, unless it passed.
    #[must_use]
    pub const fn finding(&self) -> Option<&Finding> {
        match self {
            Self::Pass => None,
            Self::Safety(finding) | Self::Liveness(finding) | Self::Unsupported(finding) => {
                Some(finding)
            }
        }
    }
}

impl Finding {
    /// What the check called `check` found. A check's name is words in
    /// lower case joined by hyphens, so that it never holds a number a
    /// seed drew.
    fn checked(check: &'static str, message: String) -> Self {
        assert!(
            !check.is_empty()
                && check
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
            "a check's name is lower-case words joined by hyphens: {check:?}"
        );
        Self {
            at: check.to_owned(),
            message,
        }
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.finding() {
            None => f.write_str(self.name()),
            Some(finding) => write!(f, "{}: {}", self.name(), finding.message),
        }
    }
}

/// A simulated run: how it ended, what happened in it, a line per event,
/// for comparing and explaining runs, and how often it reached what it
/// counts (spec/decisions/random-seed-simulations.md).
#[derive(Clone, Debug)]
pub struct Run {
    /// How it ended.
    pub outcome: Outcome,
    /// What happened.
    pub trace: Vec<String>,
    /// How many times it reached each thing it counts, such as crashes or
    /// settlements, so that a generator that stops reaching one shows.
    pub counts: BTreeMap<&'static str, u64>,
}

impl Run {
    /// A run that counts nothing.
    #[must_use]
    pub const fn new(outcome: Outcome, trace: Vec<String>) -> Self {
        Self {
            outcome,
            trace,
            counts: BTreeMap::new(),
        }
    }
}

/// Runs `run`, and turns a panic into a safety failure with the panic's
/// message, found at the panic's file and line. The trace of a run that
/// panicked is lost.
pub fn caught(run: impl FnOnce() -> Run) -> Run {
    record_panics();
    PANICKED_AT.set(None);
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|payload| {
        let message = payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic without a message".to_owned());
        let at = PANICKED_AT
            .take()
            .unwrap_or_else(|| "a panic of unknown place".to_owned());
        Run::new(Outcome::Safety(Finding { at, message }), Vec::new())
    })
}

std::thread_local! {
    /// Where the last panic on this thread happened, for [`caught`]: runs
    /// on several threads each catch their own.
    static PANICKED_AT: Cell<Option<String>> = const { Cell::new(None) };
}

/// Has every panic note where it happened, on its own thread, and then be
/// reported as before. The hook is the process's, so it is set once, and
/// keeps the one it replaces.
fn record_panics() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let report = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Some(location) = info.location() {
                let at = format!("{}:{}", stable_file(location.file()), location.line());
                // A thread that is ending has no place to note it, and
                // no run left to catch it.
                let _ = PANICKED_AT.try_with(|cell| cell.set(Some(at)));
            }
            report(info);
        }));
    });
}

/// `file` without the part that differs between machines: a dependency's
/// or the standard library's path from its crate's directory on, as
/// `rusqlite-0.40.2/src/lib.rs` or `core/src/option.rs`. The workspace's
/// own files are relative to it already, as `crates/dassie-node/src/lib.rs`.
fn stable_file(file: &str) -> &str {
    if !file.starts_with('/') {
        return file;
    }
    let Some(sources) = file.rfind("/src/") else {
        return file.rsplit('/').next().unwrap_or(file);
    };
    let directory = file
        .get(..sources)
        .and_then(|crate_path| crate_path.rfind('/'))
        .map_or(0, |slash| slash + 1);
    file.get(directory..).unwrap_or(file)
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    fn ended(outcome: Outcome) -> Run {
        Run::new(outcome, vec!["a line".to_string()])
    }

    #[test]
    fn a_panic_is_a_safety_failure_found_where_it_panicked() {
        let broke = |seed: u64| caught(|| panic!("seed {seed} broke"));
        let line = line!() - 1;
        let (seven, eight) = (broke(7), broke(8));
        let found = |message: &str| Finding {
            at: format!("crates/dassie-kit/src/sim.rs:{line}"),
            message: message.into(),
        };
        assert_eq!(seven.outcome, Outcome::Safety(found("seed 7 broke")));
        assert_eq!(eight.outcome, Outcome::Safety(found("seed 8 broke")));
        assert!(seven.trace.is_empty());
        assert_eq!(format!("{}", seven.outcome), "safety: seed 7 broke");
    }

    #[test]
    fn a_run_that_returns_keeps_its_outcome_and_trace() {
        let run = caught(|| ended(Outcome::liveness("settled", "stuck")));
        let found = Finding {
            at: "settled".into(),
            message: "stuck".into(),
        };
        assert_eq!(run.outcome, Outcome::Liveness(found));
        assert_eq!(run.trace, vec!["a line".to_string()]);
        assert_eq!(format!("{}", run.outcome), "liveness: stuck");
        assert_eq!(run.outcome.name(), "liveness");
    }

    #[test]
    #[should_panic(expected = "lower-case words")]
    fn a_check_is_named_without_numbers() {
        let _ = Outcome::safety("node-7", "broke");
    }

    #[test]
    fn a_panic_is_found_where_no_machine_differs() {
        let cases = [
            (
                "crates/dassie-node/src/sim/mod.rs",
                "crates/dassie-node/src/sim/mod.rs",
            ),
            (
                "/home/a/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rusqlite-0.40.2/src/lib.rs",
                "rusqlite-0.40.2/src/lib.rs",
            ),
            (
                "/rustc/0123456789abcdef/library/core/src/option.rs",
                "core/src/option.rs",
            ),
            ("/build/out/generated.rs", "generated.rs"),
        ];
        for (file, stable) in cases {
            assert_eq!(stable_file(file), stable, "{file}");
        }
    }

    #[test]
    fn only_safety_and_liveness_are_failures() {
        assert!(!Outcome::Pass.is_failure());
        assert!(!Outcome::unsupported("quorum", "no quorum").is_failure());
        assert!(Outcome::safety("books", "").is_failure());
        assert!(Outcome::liveness("settled", "").is_failure());
    }

    #[test]
    fn below_stays_below_its_bound() {
        let mut rng = SeededRng::new(1);
        assert!((0..1_000).all(|_| rng.below(10) < 10));
        assert_eq!(rng.below(0), 0);
    }
}
