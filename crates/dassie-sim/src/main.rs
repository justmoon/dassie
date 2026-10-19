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

use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::process::ExitCode;
use std::time::Instant;

use dassie_sim::{Outcome, SIMS, Sim, caught, find, parse_seeds};

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
    let printed = match command {
        Command::Help => writeln!(out, "{USAGE}").map(|()| ExitCode::SUCCESS),
        Command::List => list(&mut out),
        Command::Run { sim, seeds } => match seeds.map_or_else(random_seed, Ok) {
            Ok(seeds) => run(&mut out, sim, seeds),
            Err(problem) => {
                eprintln!("dassie-sim: {problem}");
                return ExitCode::from(3);
            }
        },
        Command::Explain { sim, seed, grep } => explain(&mut out, sim, seed, &grep),
    };
    printed.unwrap_or_else(|error| {
        // A reader that stops reading, such as `head`, ends the output
        // without the runner having failed.
        if error.kind() == io::ErrorKind::BrokenPipe {
            ExitCode::SUCCESS
        } else {
            eprintln!("dassie-sim: {error}");
            ExitCode::from(3)
        }
    })
}

fn list(out: &mut impl Write) -> io::Result<ExitCode> {
    for sim in &SIMS {
        writeln!(out, "{:<16} {}", sim.name, sim.about)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// A seed from the OS: the one place the runner draws randomness, and
/// only to pick which run to make.
fn random_seed() -> Result<RangeInclusive<u64>, String> {
    let seed = getrandom::u64().map_err(|error| format!("no random seed: {error}"))?;
    Ok(seed..=seed)
}

/// Runs every seed, and prints how each ended.
fn run(out: &mut impl Write, sim: &Sim, seeds: RangeInclusive<u64>) -> io::Result<ExitCode> {
    let mut tally = Tally::default();
    for seed in seeds {
        writeln!(out, "{} {seed}: {}", sim.name, (sim.describe)(seed))?;
        out.flush()?;
        let started = Instant::now();
        let outcome = caught(|| (sim.run)(seed)).outcome;
        writeln!(out, "  {outcome} in {:.2?}", started.elapsed())?;
        tally.add(&outcome);
    }
    if tally.runs > 1 {
        writeln!(
            out,
            "{} runs: {} passed, {} unsupported, {} broke safety, {} broke liveness",
            tally.runs, tally.passed, tally.unsupported, tally.safety, tally.liveness
        )?;
    }
    Ok(tally.status())
}

/// Runs a seed, and prints its trace, or the lines of it holding `grep`.
fn explain(out: &mut impl Write, sim: &Sim, seed: u64, grep: &str) -> io::Result<ExitCode> {
    writeln!(out, "{} {seed}: {}", sim.name, (sim.describe)(seed))?;
    let run = caught(|| (sim.run)(seed));
    for line in run.trace.iter().filter(|line| line.contains(grep)) {
        writeln!(out, "{line}")?;
    }
    if run.trace.is_empty() {
        writeln!(out, "(no trace: a run that panics loses it)")?;
    }
    writeln!(out, "{}", run.outcome)?;
    let mut tally = Tally::default();
    tally.add(&run.outcome);
    Ok(tally.status())
}

/// How a set of runs ended.
#[derive(Default)]
struct Tally {
    runs: u64,
    passed: u64,
    unsupported: u64,
    safety: u64,
    liveness: u64,
}

impl Tally {
    fn add(&mut self, outcome: &Outcome) {
        self.runs += 1;
        match outcome {
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
