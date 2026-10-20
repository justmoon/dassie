//! `dassie-sim`: runs Dassie's simulations from seeds (D27).
//!
//! ```text
//! dassie-sim list                                  the simulations
//! dassie-sim run <sim> [<seeds>]                   runs seeds, a random one by default
//! dassie-sim explain <sim> <seed> [--grep <text>]  runs a seed and prints its trace
//! dassie-sim campaign [--minutes <n>] [--jobs <n>] [--sims <sim>,...]
//!                                                  runs random seeds, a line of JSON each
//! dassie-sim record <sim> <seed>                   runs a seed as a campaign does, and
//!                                                  prints its line
//! ```
//!
//! Seeds are a number in decimal, as the runner prints them, a range
//! (`0..1000`, which runs 0 to 999) or a git commit's full SHA. Build it with
//! `--profile sim`, which keeps assertions on, so that every invariant is
//! checked before every commit, as in tests.
//!
//! The exit status of `run` and `explain` is 1 when a run broke safety, 2
//! when one broke liveness and none safety, 3 when the runner itself
//! failed, 64 for a usage error, and 0 otherwise. That of `record` is 0
//! once it printed its line, whatever the run found, as a campaign's is.
//!
//! # The campaign
//!
//! `campaign` runs random seeds, drawn from the OS, `--jobs` at once (by
//! default one per core) for `--minutes` (by default 10), each run's
//! simulation picked by its weight among `--sims` (by default all of them,
//! the tripwire included). Each run is a process of its own, `record`, so
//! that runs share nothing and take as long as each alone. Its output is
//! the contract with the supervisor that runs it on the campaign's machines
//! and files what it finds (D27):
//!
//! - **One JSON object per line on stdout, per finished run,** written as
//!   the run ends:
//!   `{"sim": "node", "seed": 123, "outcome": "pass", "signature": "node/pass", "ms": 812, "counts": {"crashes": 2}}`.
//!   - `outcome` is `pass`, `safety`, `liveness` or `unsupported`; `ms`
//!     the run's wall time; `counts` what it counted, by name.
//!   - `seed` replays the run, at the same commit: `dassie-sim run <sim>
//!     <seed>`. Campaign seeds are below 2^53, so that any JSON parser
//!     reads them exactly.
//!   - `signature` is `<sim>/<outcome>/<where>`, or `<sim>/pass`. `<where>`
//!     is the same for every seed that ends at the same place: for a panic,
//!     its file and line (`crates/dassie-node/src/sim/mod.rs:993`), which
//!     an edit above it in the file moves; otherwise the name of the check
//!     that failed or ruled the run out, words in lower case joined by
//!     hyphens (`settle-down`), which changes only with the check's name
//!     (`spec/areas/testing.md#outcomes`). A file holds slashes, so
//!     `<where>` is everything after the second slash. A run still going
//!     after the campaign's wall-time limit is stopped, and is a liveness
//!     failure at `wall-time`; a run whose process ends without its
//!     record, as one that overflows its stack does, is a safety failure
//!     at `abort`. Neither has a trace.
//!   - A failure (`safety` or `liveness`) adds `message`, its message with
//!     this run's numbers; `options`, what its seed drew, as `run` prints
//!     it after the seed; and `trace_tail`, the last 40 lines of its trace
//!     in one string, joined by newlines (empty after a panic, which loses
//!     the trace). An `unsupported` run adds `message` only.
//!   - `message` and `options` keep at most their first 16 KiB, and
//!     `trace_tail` its last (`TEXT_BYTES`), so that a line stays well
//!     under a megabyte.
//! - **After the last run, one summary line:**
//!   `{"summary": {"runs": 1520, "by_sim": {"node": {"runs": 900, "ms": 541000, "pass": 899, "safety": 1, "liveness": 0, "unsupported": 0}}, "failures": 1, "seconds": 600}}`.
//!   `runs` is the number of run lines printed before it, `wall-time` ones
//!   included; `failures` the number of those whose outcome is `safety` or
//!   `liveness`, the tripwire's included and `unsupported` ones not; and
//!   `seconds` the campaign's wall time, in whole seconds. `by_sim` holds a
//!   simulation once it has a run: its runs, how many ended each way, and
//!   `ms`, the sum of their wall times.
//! - **The exit status** is 0 when the campaign ran to its end, whatever
//!   its runs found, since the records say that; 3 when the runner itself
//!   failed, and 64 for a usage error. Panic messages go to stderr.
//!
//! The campaign stops starting runs once its minutes are up, and ends when
//! the runs under way have ended, or have been stopped at the wall-time
//! limit.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::process::ExitCode;
use std::time::Instant;

use dassie_sim::{Outcome, Run, SIMS, Sim, caught, find, parse_seeds};

mod campaign;

use campaign::{MOST_JOBS, MOST_MINUTES, Plan};

const USAGE: &str = "\
usage: dassie-sim list
       dassie-sim run <sim> [<seeds>]
       dassie-sim explain <sim> <seed> [--grep <text>]
       dassie-sim campaign [--minutes <n>] [--jobs <n>] [--sims <sim>,...]
       dassie-sim record <sim> <seed>

<seeds> is a number, a range such as 0..1000, or a git commit's full SHA.
A campaign runs random seeds, 10 minutes on every core by default, and
prints a line of JSON per run.";

/// How long a campaign runs when not told.
const DEFAULT_MINUTES: u64 = 10;

/// What to do.
enum Command {
    Help,
    List,
    Run {
        sim: &'static Sim,
        seeds: Option<RangeInclusive<u64>>,
    },
    Explain {
        sim: &'static Sim,
        seed: u64,
        grep: String,
    },
    Campaign(Plan),
    Record {
        sim: &'static Sim,
        seed: u64,
    },
}

/// The options given, each only for the command it belongs to.
#[derive(Default)]
struct Flags {
    grep: Option<String>,
    minutes: Option<u64>,
    jobs: Option<usize>,
    sims: Option<Vec<&'static Sim>>,
}

fn main() -> ExitCode {
    match parse(lexopt::Parser::from_env()) {
        Ok(command) => execute(command),
        Err(problem) => {
            eprintln!("dassie-sim: {problem}\n\n{USAGE}");
            ExitCode::from(64)
        }
    }
}

fn parse(mut parser: lexopt::Parser) -> Result<Command, String> {
    use lexopt::prelude::*;
    let mut words = Vec::new();
    let mut flags = Flags::default();
    while let Some(arg) = parser.next().map_err(|error| error.to_string())? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Long("grep") => flags.grep = Some(text(parser.value())?),
            Long("minutes") => {
                let minutes = bounded(&text(parser.value())?, 1, MOST_MINUTES, "--minutes")?;
                flags.minutes = Some(minutes);
            }
            Long("jobs") => {
                let most = u64::try_from(MOST_JOBS).unwrap_or(u64::MAX);
                let jobs = bounded(&text(parser.value())?, 1, most, "--jobs")?;
                flags.jobs = Some(usize::try_from(jobs).unwrap_or(MOST_JOBS));
            }
            Long("sims") => flags.sims = Some(listed(&text(parser.value())?)?),
            Value(value) => words.push(text(Ok(value))?),
            _ => return Err(arg.unexpected().to_string()),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let command = match words.as_slice() {
        ["campaign"] => return campaign_plan(flags),
        ["list"] => Command::List,
        ["run", sim] => Command::Run {
            sim: named(sim)?,
            seeds: None,
        },
        ["run", sim, seeds] => Command::Run {
            sim: named(sim)?,
            seeds: Some(parse_seeds(seeds)?),
        },
        ["explain", sim, seed] => Command::Explain {
            sim: named(sim)?,
            seed: one_seed(seed)?,
            grep: flags.grep.take().unwrap_or_default(),
        },
        ["record", sim, seed] => Command::Record {
            sim: named(sim)?,
            seed: one_seed(seed)?,
        },
        _ => return Err("expected a command".into()),
    };
    if flags.grep.is_some()
        || flags.minutes.is_some()
        || flags.jobs.is_some()
        || flags.sims.is_some()
    {
        return Err(
            "--grep is only for explain, and --minutes, --jobs and --sims for campaign".into(),
        );
    }
    Ok(command)
}

/// A campaign's plan: the flags given, or by default 10 minutes on every
/// core, of every simulation.
fn campaign_plan(flags: Flags) -> Result<Command, String> {
    if flags.grep.is_some() {
        return Err("--grep is only for explain".into());
    }
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    Ok(Command::Campaign(Plan {
        sims: flags.sims.unwrap_or_else(|| SIMS.iter().collect()),
        minutes: flags.minutes.unwrap_or(DEFAULT_MINUTES),
        jobs: flags.jobs.unwrap_or(cores).min(MOST_JOBS),
    }))
}

/// A number from `least` to `most`, for `flag`.
fn bounded(text: &str, least: u64, most: u64, flag: &str) -> Result<u64, String> {
    text.parse()
        .ok()
        .filter(|number| (least..=most).contains(number))
        .ok_or_else(|| format!("{flag} takes a number from {least} to {most}, not {text:?}"))
}

/// Simulations named in a list such as `node,peer`, each once.
fn listed(text: &str) -> Result<Vec<&'static Sim>, String> {
    let mut sims: Vec<&'static Sim> = Vec::new();
    for name in text.split(',') {
        let sim = named(name)?;
        if sims.iter().any(|known| known.name == sim.name) {
            return Err(format!("--sims names {name} twice"));
        }
        sims.push(sim);
    }
    Ok(sims)
}

fn text(value: Result<std::ffi::OsString, lexopt::Error>) -> Result<String, String> {
    let value = value.map_err(|error| error.to_string())?;
    value
        .into_string()
        .map_err(|value| format!("{} is not UTF-8", value.display()))
}

fn named(name: &str) -> Result<&'static Sim, String> {
    find(name)
        .ok_or_else(|| format!("no simulation is called {name:?}; `dassie-sim list` lists them"))
}

fn one_seed(text: &str) -> Result<u64, String> {
    let seeds = parse_seeds(text)?;
    if seeds.start() != seeds.end() {
        return Err(format!("{text} is more than one seed"));
    }
    Ok(*seeds.start())
}

fn execute(command: Command) -> ExitCode {
    let mut out = io::stdout().lock();
    let mut tally = Tally::default();
    let printed = match command {
        Command::Help => writeln!(out, "{USAGE}"),
        Command::List => list(&mut out),
        Command::Run { sim, seeds } => match seeds.map_or_else(random_seed, Ok) {
            Ok(seeds) => run(&mut out, &mut tally, sim, seeds),
            Err(problem) => {
                eprintln!("dassie-sim: {problem}");
                return ExitCode::from(3);
            }
        },
        Command::Explain { sim, seed, grep } => explain(&mut out, &mut tally, sim, seed, &grep),
        // A campaign's run: its record says how it ended, as the
        // campaign's own status does not.
        Command::Record { sim, seed } => writeln!(out, "{}", campaign::here(sim, seed)),
        // A campaign's records say how its runs ended, and its status only
        // whether it ran to its end: a reader that stops reading leaves it
        // nobody to report to, which fails the runner.
        Command::Campaign(plan) => {
            return campaign::campaign(&mut out, &plan).unwrap_or_else(|error| {
                eprintln!("dassie-sim: {error}");
                ExitCode::from(3)
            });
        }
    };
    match printed {
        Ok(()) => tally.status(),
        // A reader that stops reading, such as `head`, ends the output
        // without the runner having failed, and stops the runs; the status
        // still says how the runs so far ended, so a run that broke safety
        // is never reported as passed.
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => tally.status(),
        Err(error) => {
            eprintln!("dassie-sim: {error}");
            ExitCode::from(3)
        }
    }
}

fn list(out: &mut impl Write) -> io::Result<()> {
    for sim in &SIMS {
        writeln!(out, "{:<16} {}", sim.name, sim.about)?;
    }
    Ok(())
}

/// A seed from the OS: the one place the runner draws randomness, and
/// only to pick which run to make.
fn random_seed() -> Result<RangeInclusive<u64>, String> {
    let seed = getrandom::u64().map_err(|error| format!("no random seed: {error}"))?;
    Ok(seed..=seed)
}

/// Runs every seed, and prints how each ended. Each run is in `tally`
/// before anything is printed about it.
fn run(
    out: &mut impl Write,
    tally: &mut Tally,
    sim: &Sim,
    seeds: RangeInclusive<u64>,
) -> io::Result<()> {
    for seed in seeds {
        writeln!(out, "{} {seed}: {}", sim.name, (sim.describe)(seed))?;
        out.flush()?;
        let started = Instant::now();
        let run = caught(|| (sim.run)(seed));
        tally.add(&run);
        writeln!(out, "  {} in {:.2?}", run.outcome, started.elapsed())?;
        if !run.counts.is_empty() {
            writeln!(out, "  reached {}", counted(&run.counts))?;
        }
    }
    if tally.runs > 1 {
        writeln!(
            out,
            "{} runs: {} passed, {} unsupported, {} broke safety, {} broke liveness",
            tally.runs, tally.passed, tally.unsupported, tally.safety, tally.liveness
        )?;
        if !tally.counts.is_empty() {
            writeln!(out, "together they reached {}", counted(&tally.counts))?;
        }
    }
    Ok(())
}

/// Runs a seed, and prints its trace, or the lines of it holding `grep`.
fn explain(
    out: &mut impl Write,
    tally: &mut Tally,
    sim: &Sim,
    seed: u64,
    grep: &str,
) -> io::Result<()> {
    writeln!(out, "{} {seed}: {}", sim.name, (sim.describe)(seed))?;
    let run = caught(|| (sim.run)(seed));
    tally.add(&run);
    for line in run.trace.iter().filter(|line| line.contains(grep)) {
        writeln!(out, "{line}")?;
    }
    if run.trace.is_empty() {
        writeln!(out, "(no trace: a run that panics loses it)")?;
    }
    writeln!(out, "{}", run.outcome)?;
    if !run.counts.is_empty() {
        writeln!(out, "reached {}", counted(&run.counts))?;
    }
    Ok(())
}

/// What runs counted, as `crashes 3, settlements taken 12`.
fn counted(counts: &BTreeMap<&'static str, u64>) -> String {
    let counts: Vec<String> = counts
        .iter()
        .map(|(what, count)| format!("{what} {count}"))
        .collect();
    counts.join(", ")
}

/// How a set of runs ended.
#[derive(Default)]
struct Tally {
    runs: u64,
    passed: u64,
    unsupported: u64,
    safety: u64,
    liveness: u64,
    counts: BTreeMap<&'static str, u64>,
}

impl Tally {
    fn add(&mut self, run: &Run) {
        self.runs += 1;
        for (what, count) in &run.counts {
            *self.counts.entry(what).or_default() += count;
        }
        match run.outcome {
            Outcome::Pass => self.passed += 1,
            Outcome::Unsupported(_) => self.unsupported += 1,
            Outcome::Safety(_) => self.safety += 1,
            Outcome::Liveness(_) => self.liveness += 1,
        }
    }

    fn status(&self) -> ExitCode {
        if self.safety > 0 {
            ExitCode::from(1)
        } else if self.liveness > 0 {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A simulation whose every seed breaks safety.
    static BROKEN: Sim = Sim {
        name: "broken",
        about: "breaks safety",
        describe: |_| "nothing drawn".to_owned(),
        run: |_| Run::new(Outcome::safety("broken", "broken"), Vec::new()),
        weight: 1,
    };

    /// A reader that stops reading after `left` bytes, as `head` does.
    struct Closing {
        left: usize,
    }

    impl Write for Closing {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if buf.len() > self.left {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.left -= buf.len();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_run_counts_before_its_outcome_is_printed() {
        let first = "broken 0: nothing drawn\n".len();
        let mut tally = Tally::default();
        let printed = run(&mut Closing { left: first }, &mut tally, &BROKEN, 0..=9);
        assert_eq!(printed.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!((tally.runs, tally.safety), (1, 1));
        assert_eq!(tally.status(), ExitCode::from(1));
    }
}
