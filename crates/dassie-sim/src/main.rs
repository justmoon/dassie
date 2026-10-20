//! `dassie-sim`: runs Dassie's simulations from seeds (D27).
//!
//! ```text
//! dassie-sim list                                  the simulations
//! dassie-sim run <sim> [<seeds>]                   runs seeds, a random one by default
//! dassie-sim explain <sim> <seed> [--grep <text>]  runs a seed and prints its trace
//! ```
//!
//! Seeds are a number in decimal, as the runner prints them, a range
//! (`0..1000`, which runs 0 to 999) or a git commit's full SHA. Build it with
//! `--profile sim`, which keeps assertions on, so that every invariant is
//! checked before every commit, as in tests.
//!
//! The exit status is 1 when a run broke safety, 2 when one broke liveness
//! and none safety, 3 when the runner itself failed, 64 for a usage error,
//! and 0 otherwise.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::process::ExitCode;
use std::time::Instant;

use dassie_sim::{Outcome, Run, SIMS, Sim, caught, find, parse_seeds};

const USAGE: &str = "\
usage: dassie-sim list
       dassie-sim run <sim> [<seeds>]
       dassie-sim explain <sim> <seed> [--grep <text>]

<seeds> is a number, a range such as 0..1000, or a git commit's full SHA.";

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
    let mut grep = String::new();
    while let Some(arg) = parser.next().map_err(|error| error.to_string())? {
        match arg {
            Short('h') | Long("help") => return Ok(Command::Help),
            Long("grep") => grep = text(parser.value())?,
            Value(value) => words.push(text(Ok(value))?),
            _ => return Err(arg.unexpected().to_string()),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["list"] => Ok(Command::List),
        ["run", sim] => Ok(Command::Run {
            sim: named(sim)?,
            seeds: None,
        }),
        ["run", sim, seeds] => Ok(Command::Run {
            sim: named(sim)?,
            seeds: Some(parse_seeds(seeds)?),
        }),
        ["explain", sim, seed] => Ok(Command::Explain {
            sim: named(sim)?,
            seed: one_seed(seed)?,
            grep,
        }),
        _ => Err("expected a command".into()),
    }
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
        run: |_| Run::new(Outcome::Safety("broken".to_owned()), Vec::new()),
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
