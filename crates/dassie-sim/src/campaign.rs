//! The campaign (D27): random seeds of the simulations, several at once,
//! for a while, each finished run reported as one line of JSON. What the
//! lines hold is the runner's contract with the campaign's supervisor, in
//! the runner's docs.
//!
//! Each run is a process of its own, `dassie-sim record`, which one of the
//! campaign's threads starts and waits for; the main thread writes every
//! line, so lines never interleave. Runs in one process would share what
//! is global to it: SQLite, which every node's store is, takes one lock
//! for each allocation of every connection, to count them. A `node` run
//! among seven others in one process took three and a half times as long
//! as alone, and among the 55 others of a campaign of 56 jobs longer than
//! the wall-time limit. In processes of their own, eight take as long as
//! one.
//!
//! Every simulation bounds its steps, so every run ends. A run that still
//! takes far too long in wall time, such as one stuck in a loop inside a
//! single step, is stopped and reported as a liveness failure at
//! `wall-time`. A run that ends without its record, as one that overflows
//! its stack aborts, is a safety failure at `abort`.

use std::collections::BTreeMap;
use std::env;
use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use dassie_sim::{Outcome, Run, Sim, caught};
use serde_json::{Map, Value, json};

/// How many of a failed run's last trace lines its record holds: enough to
/// see what led to the failure, little enough for an issue.
pub const TRACE_TAIL_LINES: usize = 40;

/// The most bytes a record keeps of a message, of what a seed drew, and of
/// a trace's tail. JSON escapes a byte into six at worst, so a record stays
/// well under the megabyte the supervisor reads of a line (D27).
pub const TEXT_BYTES: usize = 16 * 1024;

/// The most bytes the campaign reads of what a run's process writes: the
/// megabyte the supervisor reads of a line, which a record stays well
/// under.
const RECORD_BYTES: u64 = 1 << 20;

/// How long a run may take in wall time before the campaign stops it and
/// reports it. Far longer than any run takes, so that only a run that is
/// stuck reaches it, and a slow machine never does: the longest, `node`
/// runs of twelve nodes whose network stays split for two days, take a
/// minute and a half alone on a fast core, and twice that with another
/// run on the core's other hyperthread, as on a campaign's VM.
pub const RUN_LIMIT: Duration = Duration::from_secs(10 * 60);

/// The most runs a campaign makes at once.
pub const MOST_JOBS: usize = 1_024;

/// The longest a campaign runs: a week.
pub const MOST_MINUTES: u64 = 7 * 24 * 60;

/// How many messages each thread may have waiting for the main thread: a
/// thread whose runs are instant, as the tripwire's, waits for the lines
/// to be written rather than queueing them without bound.
const MESSAGES_PER_JOB: usize = 16;

/// Seeds are drawn below 2^53, so that a record's seed is exact in any
/// JSON parser, JavaScript's included. That still leaves more seeds than
/// any campaign can run.
const SEED_BITS: u32 = 53;

/// How a run can end, as a record names it.
const OUTCOMES: [&str; 4] = ["pass", "safety", "liveness", "unsupported"];

/// What a campaign runs.
pub struct Plan {
    /// The simulations it draws from, by their weights.
    pub sims: Vec<&'static Sim>,
    /// How long it starts new runs.
    pub minutes: u64,
    /// How many runs it makes at once.
    pub jobs: usize,
}

/// What a thread tells the main thread.
enum Message {
    /// A run of a simulation ended.
    Finished(&'static str, Ran),
    /// The thread has no more runs to make.
    Done,
    /// The runner itself failed: it has no randomness, or cannot start a
    /// process.
    Failed(String),
}

/// Runs `plan`, writing a record per finished run and a summary to `out`.
/// Exits 0 whatever the runs found, since the records say that, and 3 when
/// the runner itself failed.
pub fn campaign(out: &mut impl Write, plan: &Plan) -> io::Result<ExitCode> {
    assert!(!plan.sims.is_empty() && plan.sims.iter().all(|sim| sim.weight > 0));
    assert!((1..=MOST_JOBS).contains(&plan.jobs));
    assert!((1..=MOST_MINUTES).contains(&plan.minutes));
    let started = Instant::now();
    let end = started + Duration::from_secs(plan.minutes * 60);
    let crew = Crew {
        sims: plan.sims.clone().into(),
        end,
        draw: Arc::new(from_the_os),
        run: Arc::new(in_a_process),
    };
    let mut tally = Tally::default();
    if let Err(problem) = crew.run(plan.jobs, out, &mut tally)? {
        eprintln!("dassie-sim: {problem}");
        return Ok(ExitCode::from(3));
    }
    writeln!(out, "{}", tally.summary(started.elapsed()))?;
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

/// Where a crew's threads get each run's seed, and the number that picks
/// its simulation: the OS in a campaign, a fixed list in a test, which
/// never runs random seeds. `None` ends a thread's work.
type Draw = Arc<dyn Fn() -> Option<Result<(u64, u64), String>> + Send + Sync>;

/// How a crew makes a run of a simulation, from its seed and what the seed
/// drew: in a process of its own in a campaign, and in the test's own
/// process in a test.
type Runner = Arc<dyn Fn(&'static Sim, u64, &str) -> io::Result<Ran> + Send + Sync>;

/// A finished run: its record, and what the tally counts of it.
#[derive(Debug)]
struct Ran {
    record: Value,
    outcome: &'static str,
    ms: u64,
}

impl Ran {
    /// The run `record` holds, if it holds one of `sim` and `seed`: a record
    /// another process wrote is believed only then.
    fn of(record: Value, sim: &str, seed: u64) -> Option<Self> {
        let outcome = record.get("outcome")?.as_str()?;
        let outcome = OUTCOMES.into_iter().find(|known| *known == outcome)?;
        let ms = record.get("ms")?.as_u64()?;
        let ours = record.get("sim").is_some_and(|named| named == sim)
            && record.get("seed").is_some_and(|drawn| drawn == seed);
        ours.then_some(Self {
            record,
            outcome,
            ms,
        })
    }

    /// The run of `run`, as a record says it.
    fn new(sim: &str, seed: u64, options: &str, run: &Run, ms: u64) -> Self {
        Self {
            record: record(sim, seed, options, run, ms),
            outcome: run.outcome.name(),
            ms,
        }
    }
}

/// A seed and a pick from the OS: the one place a campaign draws
/// randomness.
fn from_the_os() -> Option<Result<(u64, u64), String>> {
    let drawn = getrandom::u64().and_then(|seed| Ok((seed >> (64 - SEED_BITS), getrandom::u64()?)));
    Some(drawn.map_err(|error| error.to_string()))
}

/// The threads that make the runs.
struct Crew {
    sims: Arc<[&'static Sim]>,
    /// When the threads stop starting runs.
    end: Instant,
    draw: Draw,
    run: Runner,
}

impl Crew {
    /// Makes runs on `jobs` threads until the end, writing each run's
    /// record to `out` and counting it, until each thread is done. Says
    /// what failed if the runner itself did.
    fn run(
        &self,
        jobs: usize,
        out: &mut impl Write,
        tally: &mut Tally,
    ) -> io::Result<Result<(), String>> {
        let (sender, receiver) = mpsc::sync_channel(jobs * MESSAGES_PER_JOB);
        for job in 0..jobs {
            let (sims, end, sender) = (Arc::clone(&self.sims), self.end, sender.clone());
            let (draw, run) = (Arc::clone(&self.draw), Arc::clone(&self.run));
            thread::Builder::new()
                .name(format!("job {job}"))
                .spawn(move || work(&sims, end, &*draw, &*run, &sender))?;
        }
        drop(sender);
        let mut working = jobs;
        while working > 0 {
            // Each thread holds a sender until it is done.
            let Ok(message) = receiver.recv() else { break };
            match message {
                Message::Finished(sim, ran) => {
                    writeln!(out, "{}", ran.record)?;
                    out.flush()?;
                    tally.add(sim, ran.outcome, ran.ms);
                }
                Message::Done => working -= 1,
                Message::Failed(problem) => return Ok(Err(problem)),
            }
        }
        Ok(Ok(()))
    }
}

/// A thread's work: makes the runs `draw` gives until the campaign's end,
/// or until `draw` has no more.
fn work(
    sims: &[&'static Sim],
    end: Instant,
    draw: &(dyn Fn() -> Option<Result<(u64, u64), String>> + Send + Sync),
    run: &(dyn Fn(&'static Sim, u64, &str) -> io::Result<Ran> + Send + Sync),
    sender: &SyncSender<Message>,
) {
    while Instant::now() < end {
        let (seed, picked) = match draw() {
            Some(Ok(drawn)) => drawn,
            Some(Err(problem)) => {
                let _ = sender.send(Message::Failed(format!("no random seed: {problem}")));
                return;
            }
            None => break,
        };
        let Some(sim) = pick(sims, picked) else { break };
        let message = match run(sim, seed, &described(sim, seed)) {
            Ok(ran) => Message::Finished(sim.name, ran),
            Err(error) => Message::Failed(format!("cannot make a run: {error}")),
        };
        let failed = matches!(message, Message::Failed(_));
        if sender.send(message).is_err() || failed {
            return;
        }
    }
    let _ = sender.send(Message::Done);
}

/// What `seed` of `sim` draws, as `dassie-sim run` prints it.
fn described(sim: &Sim, seed: u64) -> String {
    catch_unwind(AssertUnwindSafe(|| (sim.describe)(seed)))
        .unwrap_or_else(|_| "(describing it panicked)".to_owned())
}

/// Makes a run here, in this process, and returns its record: what
/// `dassie-sim record` prints for the campaign.
pub fn here(sim: &'static Sim, seed: u64) -> Value {
    ran_here(sim, seed).record
}

fn ran_here(sim: &'static Sim, seed: u64) -> Ran {
    let options = described(sim, seed);
    let since = Instant::now();
    let run = caught(|| (sim.run)(seed));
    Ran::new(sim.name, seed, &options, &run, millis(since.elapsed()))
}

/// Makes a run in a process of its own, `dassie-sim record`: its record is
/// the one the process wrote, or, when it wrote none, or ran past
/// [`RUN_LIMIT`] and was stopped, one that says so.
fn in_a_process(sim: &'static Sim, seed: u64, options: &str) -> io::Result<Ran> {
    let mut command = Command::new(env::current_exe()?);
    command.args(["record", sim.name, &seed.to_string()]);
    let since = Instant::now();
    let ended = apart(command, RUN_LIMIT)?;
    let ms = millis(since.elapsed());
    Ok(ended_as(sim.name, seed, options, ended, ms))
}

/// The run whose process ended as `ended`, after `ms`.
fn ended_as(sim: &str, seed: u64, options: &str, ended: Ended, ms: u64) -> Ran {
    let replay = format!("`dassie-sim run {sim} {seed}` replays it");
    let outcome = match ended {
        Ended::Overdue => Outcome::liveness(
            "wall-time",
            format!(
                "still running after {} s of wall time, and stopped; {replay}",
                ms / 1_000
            ),
        ),
        Ended::Exited { status, out } => {
            let written = serde_json::from_slice::<Value>(&out).ok();
            let ran = written.and_then(|record| Ran::of(record, sim, seed));
            match ran.filter(|_| status.success()) {
                Some(ran) => return ran,
                None => Outcome::safety(
                    "abort",
                    format!("the run ended without its record, {status}; {replay}"),
                ),
            }
        }
    };
    Ran::new(sim, seed, options, &Run::new(outcome, Vec::new()), ms)
}

/// How a process ended.
enum Ended {
    /// On its own: its status, and the first [`RECORD_BYTES`] it wrote.
    Exited { status: ExitStatus, out: Vec<u8> },
    /// Still running at the limit, and stopped.
    Overdue,
}

/// Runs `command` until it ends, or stops it at `limit`.
fn apart(mut command: Command, limit: Duration) -> io::Result<Ended> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take();
    let (sender, receiver) = mpsc::sync_channel(1);
    // A thread reads what it writes until it closes its end; the read end
    // closes after the most that is read, so that a process that wrote
    // more does not wait on a full pipe.
    thread::Builder::new()
        .name("output".into())
        .spawn(move || {
            let mut out = Vec::new();
            if let Some(stdout) = stdout {
                let _ = stdout.take(RECORD_BYTES).read_to_end(&mut out);
            }
            let _ = sender.send(out);
        })?;
    if let Ok(out) = receiver.recv_timeout(limit) {
        let status = child.wait()?;
        return Ok(Ended::Exited { status, out });
    }
    // It may have ended just now: then there is nothing to stop.
    let _ = child.kill();
    child.wait()?;
    Ok(Ended::Overdue)
}

/// The simulation `draw` picks, each as likely as its weight. A draw from
/// 64 random bits is uniform enough for weights this small.
pub fn pick(sims: &[&'static Sim], draw: u64) -> Option<&'static Sim> {
    let total: u64 = sims.iter().map(|sim| sim.weight).sum();
    let mut drawn = draw.checked_rem(total)?;
    for sim in sims {
        if drawn < sim.weight {
            return Some(sim);
        }
        drawn -= sim.weight;
    }
    None
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// A failure's signature: `<sim>/<outcome>/<where>`, where `<where>` is the
/// check that failed or the panic's file and line, the same for every seed
/// that ends there. A run that passed has no `<where>`.
pub fn signature(sim: &str, outcome: &Outcome) -> String {
    match outcome.finding() {
        Some(finding) => format!("{sim}/{}/{}", outcome.name(), finding.at),
        None => format!("{sim}/{}", outcome.name()),
    }
}

/// A finished run's record, as the runner's docs give it.
pub fn record(sim: &str, seed: u64, options: &str, run: &Run, ms: u64) -> Value {
    let mut record = json!({
        "sim": sim,
        "seed": seed,
        "outcome": run.outcome.name(),
        "signature": signature(sim, &run.outcome),
        "ms": ms,
        "counts": run.counts,
    });
    if let (Some(finding), Some(fields)) = (run.outcome.finding(), record.as_object_mut()) {
        fields.insert("message".into(), start_of(&finding.message).into());
        if run.outcome.is_failure() {
            let tail = run.trace.len().saturating_sub(TRACE_TAIL_LINES);
            let tail = run.trace.get(tail..).unwrap_or_default().join("\n");
            fields.insert("options".into(), start_of(options).into());
            fields.insert("trace_tail".into(), end_of(&tail).into());
        }
    }
    record
}

/// At most [`TEXT_BYTES`] from the start of `text`.
fn start_of(text: &str) -> &str {
    let cut = text.floor_char_boundary(TEXT_BYTES);
    text.get(..cut).unwrap_or_default()
}

/// At most [`TEXT_BYTES`] from the end of `text`.
fn end_of(text: &str) -> &str {
    let cut = text.ceil_char_boundary(text.len().saturating_sub(TEXT_BYTES));
    text.get(cut..).unwrap_or_default()
}

/// How the campaign's runs ended, in all and by simulation.
#[derive(Default)]
struct Tally {
    runs: u64,
    failures: u64,
    /// Per simulation: its runs, the milliseconds they took, and how many
    /// ended each way.
    by_sim: BTreeMap<String, BTreeMap<&'static str, u64>>,
}

impl Tally {
    /// Counts a run of `sim` that ended as `outcome`, after `ms`.
    fn add(&mut self, sim: &str, outcome: &'static str, ms: u64) {
        self.runs += 1;
        if matches!(outcome, "safety" | "liveness") {
            self.failures += 1;
        }
        let counts = self.by_sim.entry(sim.to_owned()).or_insert_with(|| {
            ["runs", "ms"]
                .into_iter()
                .chain(OUTCOMES)
                .map(|key| (key, 0))
                .collect()
        });
        for (key, add) in [("runs", 1), ("ms", ms), (outcome, 1)] {
            let count = counts.entry(key).or_default();
            *count = count.saturating_add(add);
        }
    }

    fn summary(&self, elapsed: Duration) -> Value {
        let by_sim: Map<String, Value> = self
            .by_sim
            .iter()
            .map(|(sim, counts)| (sim.clone(), json!(counts)))
            .collect();
        json!({
            "summary": {
                "runs": self.runs,
                "by_sim": by_sim,
                "failures": self.failures,
                "seconds": elapsed.as_secs(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use dassie_sim::{SIMS, find};

    use super::*;

    /// A simulation that fails at one assertion, with the seed in its
    /// message, as a real failure's message holds the run's numbers.
    fn asserts(seed: u64) -> Run {
        assert!(seed.is_multiple_of(2), "seed {seed} is odd");
        Run::new(Outcome::Pass, vec!["even".into()])
    }

    #[test]
    fn two_seeds_that_fail_at_one_assertion_share_a_signature() {
        let (three, five) = (caught(|| asserts(3)), caught(|| asserts(5)));
        assert_ne!(three.outcome, five.outcome);
        let shared = signature("odd", &three.outcome);
        assert_eq!(shared, signature("odd", &five.outcome));
        assert!(
            shared.starts_with("odd/safety/crates/dassie-sim/src/campaign.rs:"),
            "{shared}"
        );
        let wire = find("tripwire").unwrap();
        let tripped = |seed| signature("tripwire", &caught(|| (wire.run)(seed)).outcome);
        assert_eq!(tripped(1_000), "tripwire/safety/on-purpose");
        assert_eq!(tripped(2_000), tripped(1_000));
        assert_eq!(signature("tripwire", &Outcome::Pass), "tripwire/pass");
    }

    #[test]
    fn a_passing_run_is_one_line_without_a_message() {
        let mut run = Run::new(Outcome::Pass, vec!["a line".into()]);
        run.counts.insert("crashes", 3);
        let line = record("node", 42, "3 nodes", &run, 120).to_string();
        assert!(!line.contains('\n'));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            parsed,
            json!({
                "sim": "node",
                "seed": 42,
                "outcome": "pass",
                "signature": "node/pass",
                "ms": 120,
                "counts": {"crashes": 3},
            })
        );
    }

    #[test]
    fn a_failure_holds_its_message_options_and_the_end_of_its_trace() {
        let trace = (0..100).map(|line| format!("line {line}")).collect();
        let outcome = Outcome::liveness("settled", "node 2 never settled\nwith 7");
        let run = Run::new(outcome, trace);
        let line = record("node", u64::MAX >> 11, "5 nodes", &run, 9).to_string();
        assert!(!line.contains('\n'));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["outcome"], "liveness");
        assert_eq!(parsed["signature"], "node/liveness/settled");
        assert_eq!(parsed["seed"], u64::MAX >> 11);
        assert_eq!(parsed["message"], "node 2 never settled\nwith 7");
        assert_eq!(parsed["options"], "5 nodes");
        let tail: Vec<&str> = parsed["trace_tail"].as_str().unwrap().lines().collect();
        assert_eq!(tail.len(), TRACE_TAIL_LINES);
        assert_eq!(tail[0], "line 60");
        assert_eq!(tail[39], "line 99");
        assert_eq!(parsed["counts"], json!({}));
    }

    #[test]
    fn an_unsupported_run_holds_its_message_only() {
        let run = Run::new(Outcome::unsupported("quorum", "no quorum"), vec![]);
        let parsed = record("node", 1, "2 nodes", &run, 1);
        assert_eq!(parsed["signature"], "node/unsupported/quorum");
        assert_eq!(parsed["message"], "no quorum");
        assert!(parsed.get("options").is_none());
        assert!(parsed.get("trace_tail").is_none());
    }

    #[test]
    fn each_simulation_is_picked_by_its_weight() {
        let sims: Vec<&'static Sim> = SIMS.iter().collect();
        let total: u64 = sims.iter().map(|sim| sim.weight).sum();
        let mut picked: BTreeMap<&str, u64> = BTreeMap::new();
        for draw in 0..total * 10 {
            *picked.entry(pick(&sims, draw).unwrap().name).or_default() += 1;
        }
        for sim in &sims {
            assert_eq!(picked[sim.name], sim.weight * 10, "{}", sim.name);
        }
        assert!(pick(&[], 7).is_none());
    }

    #[test]
    fn the_summary_counts_runs_by_simulation_and_outcome() {
        let mut tally = Tally::default();
        tally.add("node", "pass", 100);
        tally.add("node", "safety", 50);
        tally.add("stream", "pass", 1);
        let summary = tally.summary(Duration::from_secs(61));
        assert_eq!(summary["summary"]["runs"], 3);
        assert_eq!(summary["summary"]["failures"], 1);
        assert_eq!(summary["summary"]["seconds"], 61);
        let node = &summary["summary"]["by_sim"]["node"];
        assert_eq!(node["runs"], 2);
        assert_eq!(node["ms"], 150);
        assert_eq!(node["safety"], 1);
        assert_eq!(node["liveness"], 0);
    }

    #[test]
    fn a_record_stays_well_under_a_megabyte() {
        let long = "\u{1}é".repeat(100_000);
        let trace = vec![long.clone(); 100];
        let run = Run::new(Outcome::safety("events", long.clone()), trace);
        let line = record("node", 1, &long, &run, 1).to_string();
        assert!(line.len() < 512 * 1024, "{}", line.len());
        let parsed: Value = serde_json::from_str(&line).unwrap();
        let tail = parsed["trace_tail"].as_str().unwrap();
        assert!(long.ends_with(tail.rsplit('\n').next().unwrap()));
        assert!(parsed["message"].as_str().unwrap().len() <= TEXT_BYTES);
    }

    /// Seeds 0, 500, 1000, … for `runs` runs, then no more: half of them
    /// trip the tripwire.
    fn fixed(runs: u64) -> Draw {
        let next = Arc::new(std::sync::atomic::AtomicU64::new(0));
        Arc::new(move || {
            let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            (index < runs).then_some(Ok((index * 500, index)))
        })
    }

    /// A crew of the tripwire, whose runs are made by `run`.
    fn tripwire_crew(draw: Draw, run: Runner) -> Crew {
        Crew {
            sims: vec![find("tripwire").unwrap()].into(),
            end: Instant::now() + Duration::from_secs(60),
            draw,
            run,
        }
    }

    #[test]
    fn a_crew_reports_every_run_it_makes() {
        // The crew draws a fixed list of seeds and stops once it is spent,
        // well before its minute is up. Its runs are made here: a test's
        // process is not the runner's, so it cannot start `dassie-sim
        // record`.
        let crew = tripwire_crew(fixed(100), Arc::new(|sim, seed, _| Ok(ran_here(sim, seed))));
        let (mut out, mut tally) = (Vec::new(), Tally::default());
        assert_eq!(crew.run(4, &mut out, &mut tally).unwrap(), Ok(()));
        let mut lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        lines.sort_by_key(|line| line["seed"].as_u64());
        assert_eq!(lines.len(), 100);
        assert_eq!((tally.runs, tally.failures), (100, 50));
        for (index, line) in (0..).zip(&lines) {
            assert_eq!(line["sim"], "tripwire");
            assert_eq!(line["seed"], index * 500);
            let outcome = if index % 2 == 0 { "safety" } else { "pass" };
            assert_eq!(line["outcome"], outcome);
        }
    }

    #[test]
    fn a_crew_that_cannot_make_a_run_says_so() {
        let failing: Runner = Arc::new(|_, _, _| Err(io::ErrorKind::NotFound.into()));
        let crew = tripwire_crew(fixed(100), failing);
        let (mut out, mut tally) = (Vec::new(), Tally::default());
        let problem = crew.run(2, &mut out, &mut tally).unwrap().unwrap_err();
        assert!(problem.starts_with("cannot make a run"), "{problem}");
        assert!(out.is_empty());
    }

    /// `script` run by the shell, with `argument` as `$1`.
    fn shell(script: &str, argument: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script, "sh", argument]);
        command
    }

    #[test]
    fn the_record_a_run_writes_is_its_record() {
        let written = here(find("tripwire").unwrap(), 7_000);
        let ended = apart(shell("echo \"$1\"", &written.to_string()), RUN_LIMIT);
        let ran = ended_as("tripwire", 7_000, "", ended.unwrap(), 1);
        assert_eq!(ran.record, written);
        assert_eq!(ran.outcome, "safety");
        assert_eq!(ran.record["signature"], "tripwire/safety/on-purpose");
    }

    #[test]
    fn a_run_past_the_limit_is_stopped_and_reported() {
        let started = Instant::now();
        let ended = apart(shell("exec sleep 60", ""), Duration::from_millis(100)).unwrap();
        assert!(matches!(ended, Ended::Overdue));
        assert!(started.elapsed() < Duration::from_secs(30));
        let ran = ended_as("node", 42, "3 nodes", ended, 600_001);
        assert_eq!((ran.outcome, ran.ms), ("liveness", 600_001));
        let line = ran.record;
        assert_eq!(line["signature"], "node/liveness/wall-time");
        assert_eq!(line["seed"], 42);
        assert_eq!(line["options"], "3 nodes");
        assert_eq!(line["trace_tail"], "");
        assert_eq!(line["ms"], 600_001);
        let message = line["message"].as_str().unwrap();
        assert!(
            message.starts_with("still running after 600 s"),
            "{message}"
        );
    }

    #[test]
    fn a_run_that_ends_without_its_record_is_a_safety_failure() {
        // Killed, as one out of memory is, or aborted, as one that
        // overflows its stack is, a run's process writes no record.
        let killed = apart(shell("kill -KILL $$", ""), RUN_LIMIT).unwrap();
        let line = ended_as("tripwire", 42, "nothing", killed, 5).record;
        assert_eq!(line["signature"], "tripwire/safety/abort");
        assert_eq!(line["options"], "nothing");
        let message = line["message"].as_str().unwrap();
        assert!(message.contains("signal"), "{message}");
        assert!(message.ends_with("`dassie-sim run tripwire 42` replays it"));
        // Nor is a record that is not one, or one its process failed after
        // writing, or one of another seed.
        let written = here(find("tripwire").unwrap(), 42).to_string();
        for (script, seed) in [
            ("echo nothing", 42),
            ("echo \"$1\"; exit 1", 42),
            ("echo \"$1\"", 43),
        ] {
            let ended = apart(shell(script, &written), RUN_LIMIT).unwrap();
            let ran = ended_as("tripwire", seed, "nothing", ended, 5);
            assert_eq!(ran.outcome, "safety", "{script}");
            assert_eq!(ran.record["signature"], "tripwire/safety/abort");
        }
        let ended = apart(shell("echo \"$1\"", &written), RUN_LIMIT).unwrap();
        assert_eq!(ended_as("tripwire", 42, "", ended, 5).outcome, "pass");
    }

    #[test]
    fn what_a_run_writes_is_read_only_up_to_the_most() {
        // A process that would write without end is not waited for on a
        // full pipe: its output closes after the most, and it ends.
        let Ended::Exited { status, out } = apart(shell("exec yes", ""), RUN_LIMIT).unwrap() else {
            panic!("it ends once its output closes");
        };
        assert!(!status.success());
        assert_eq!(out.len(), usize::try_from(RECORD_BYTES).unwrap());
    }
}
