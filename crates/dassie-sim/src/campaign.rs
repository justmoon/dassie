//! The campaign (D27): random seeds of the simulations, on several
//! threads, for a while, each finished run reported as one line of JSON.
//! What the lines hold is the runner's contract with the campaign's
//! supervisor, in the runner's docs.
//!
//! The threads run the seeds and the main thread writes every line, so
//! lines never interleave. Every simulation bounds its steps, so every run
//! ends; a run that still takes far too long in wall time, such as one
//! stuck in a loop inside a single step, is reported as a liveness failure
//! at `wall-time` and left behind on its thread, which a fresh thread
//! replaces. A thread cannot be stopped from outside: the one left behind
//! runs on until its run ends, or until the campaign does.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
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

/// How long a run may take in wall time before the campaign reports it and
/// leaves it behind. Far longer than any run takes (the longest take
/// seconds), so that only a run that is stuck reaches it, and a slow
/// machine never does.
pub const RUN_LIMIT: Duration = Duration::from_secs(10 * 60);

/// The most threads a campaign runs seeds on at once.
pub const MOST_JOBS: usize = 1_024;

/// The longest a campaign runs: a week.
pub const MOST_MINUTES: u64 = 7 * 24 * 60;

/// How many messages each thread may have waiting for the main thread: a
/// thread that runs instant seeds, as the tripwire's, waits for the lines
/// to be written rather than queueing them without bound.
const MESSAGES_PER_JOB: usize = 16;

/// How often the campaign looks for runs past [`RUN_LIMIT`].
const LOOK: Duration = Duration::from_secs(1);

/// Each thread's stack: the size of the main thread's on Linux, where
/// `dassie-sim run` replays a seed, so that a run overflows its stack in a
/// campaign only where its replay does.
const STACK_BYTES: usize = 8 << 20;

/// Seeds are drawn below 2^53, so that a record's seed is exact in any
/// JSON parser, JavaScript's included. That still leaves more seeds than
/// any campaign can run.
const SEED_BITS: u32 = 53;

/// What a campaign runs.
pub struct Plan {
    /// The simulations it draws from, by their weights.
    pub sims: Vec<&'static Sim>,
    /// How long it starts new runs.
    pub minutes: u64,
    /// How many threads run seeds at once.
    pub jobs: usize,
}

/// What a thread tells the main thread.
enum Message {
    Started {
        slot: usize,
        sim: &'static Sim,
        seed: u64,
        options: String,
    },
    Finished {
        slot: usize,
        run: Run,
        ms: u64,
    },
    Done {
        slot: usize,
    },
    NoRandomness(String),
}

/// A run under way.
struct Flight {
    sim: &'static Sim,
    seed: u64,
    options: String,
    since: Instant,
}

/// Runs `plan`, writing a record per finished run and a summary to `out`.
/// Exits 0 whatever the runs found, since the records say that, and 3 when
/// the runner itself failed.
pub fn campaign(out: &mut impl Write, plan: &Plan) -> io::Result<ExitCode> {
    assert!(!plan.sims.is_empty() && plan.sims.iter().all(|sim| sim.weight > 0));
    assert!((1..=MOST_JOBS).contains(&plan.jobs));
    assert!((1..=MOST_MINUTES).contains(&plan.minutes));
    let started = Instant::now();
    let (sender, receiver) = mpsc::sync_channel(plan.jobs * MESSAGES_PER_JOB);
    let mut crew = Crew::new(plan, started, sender, Arc::new(from_the_os));
    for _ in 0..plan.jobs {
        crew.spawn()?;
    }
    let mut tally = Tally::default();
    while !crew.live.is_empty() {
        match receiver.recv_timeout(LOOK) {
            Ok(Message::NoRandomness(problem)) => {
                eprintln!("dassie-sim: no random seed: {problem}");
                return Ok(ExitCode::from(3));
            }
            Ok(message) => crew.take(message, out, &mut tally)?,
            Err(RecvTimeoutError::Timeout) => {}
            // The crew holds a sender, so the channel stays open.
            Err(RecvTimeoutError::Disconnected) => break,
        }
        crew.leave_overdue(out, &mut tally)?;
    }
    writeln!(out, "{}", tally.summary(started.elapsed()))?;
    out.flush()?;
    Ok(ExitCode::SUCCESS)
}

/// Where a crew's threads get each run's seed, and the number that picks
/// its simulation: the OS in a campaign, a fixed list in a test, which
/// never runs random seeds. `None` ends a thread's work.
type Draw = Arc<dyn Fn() -> Option<Result<(u64, u64), String>> + Send + Sync>;

/// A seed and a pick from the OS: the one place a campaign draws
/// randomness.
fn from_the_os() -> Option<Result<(u64, u64), String>> {
    let drawn = getrandom::u64().and_then(|seed| Ok((seed >> (64 - SEED_BITS), getrandom::u64()?)));
    Some(drawn.map_err(|error| error.to_string()))
}

/// The threads that run seeds, each in its slot.
struct Crew {
    sims: Arc<[&'static Sim]>,
    end: Instant,
    sender: SyncSender<Message>,
    draw: Draw,
    /// The next slot's number.
    next: usize,
    /// The most slots there may be: each slot can leave a thread behind
    /// at most once per [`RUN_LIMIT`] of the campaign.
    most: usize,
    /// The slots whose threads still report.
    live: BTreeSet<usize>,
    /// Each live slot's run under way, if any.
    flights: BTreeMap<usize, Option<Flight>>,
    /// Each live slot's flag that tells its thread it was left behind.
    left: BTreeMap<usize, Arc<AtomicBool>>,
}

impl Crew {
    fn new(plan: &Plan, started: Instant, sender: SyncSender<Message>, draw: Draw) -> Self {
        let seconds = plan.minutes * 60;
        let renewals = usize::try_from(seconds / RUN_LIMIT.as_secs() + 1).unwrap_or(usize::MAX);
        Self {
            sims: plan.sims.clone().into(),
            end: started + Duration::from_secs(seconds),
            sender,
            draw,
            next: 0,
            most: plan.jobs.saturating_mul(renewals.saturating_add(1)),
            live: BTreeSet::new(),
            flights: BTreeMap::new(),
            left: BTreeMap::new(),
        }
    }

    /// Starts a thread in a new slot.
    fn spawn(&mut self) -> io::Result<()> {
        let slot = self.next;
        assert!(
            slot < self.most,
            "more threads than a campaign may leave behind"
        );
        self.next += 1;
        let (sims, end, sender) = (Arc::clone(&self.sims), self.end, self.sender.clone());
        let draw = Arc::clone(&self.draw);
        let left = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&left);
        thread::Builder::new()
            .name(format!("run {slot}"))
            .stack_size(STACK_BYTES)
            .spawn(move || work(slot, &sims, end, &*draw, &stop, &sender))?;
        self.live.insert(slot);
        self.flights.insert(slot, None);
        self.left.insert(slot, left);
        Ok(())
    }

    /// Takes in what a thread said. A thread left behind is no longer
    /// listened to.
    fn take(
        &mut self,
        message: Message,
        out: &mut impl Write,
        tally: &mut Tally,
    ) -> io::Result<()> {
        match message {
            Message::Started {
                slot,
                sim,
                seed,
                options,
            } => {
                if let Some(flight) = self.flights.get_mut(&slot) {
                    let since = Instant::now();
                    *flight = Some(Flight {
                        sim,
                        seed,
                        options,
                        since,
                    });
                }
            }
            Message::Finished { slot, run, ms } => {
                if let Some(flight) = self.flights.get_mut(&slot).and_then(Option::take) {
                    report(out, tally, &flight, &run, ms)?;
                }
            }
            Message::Done { slot } => self.forget(slot),
            Message::NoRandomness(_) => {}
        }
        Ok(())
    }

    /// Reports every run past [`RUN_LIMIT`], leaves its thread behind,
    /// and starts another in its place while there is time left.
    fn leave_overdue(&mut self, out: &mut impl Write, tally: &mut Tally) -> io::Result<()> {
        let overdue: Vec<usize> = self
            .flights
            .iter()
            .filter(|(_, flight)| {
                flight
                    .as_ref()
                    .is_some_and(|flight| flight.since.elapsed() > RUN_LIMIT)
            })
            .map(|(slot, _)| *slot)
            .collect();
        for slot in overdue {
            let flight = self.flights.get_mut(&slot).and_then(Option::take);
            if let Some(left) = self.left.get(&slot) {
                left.store(true, Ordering::Relaxed);
            }
            self.forget(slot);
            let Some(flight) = flight else { continue };
            let elapsed = flight.since.elapsed();
            let message = format!(
                "still running after {} s of wall time, and left behind; \
                 `dassie-sim run {} {}` replays it",
                elapsed.as_secs(),
                flight.sim.name,
                flight.seed
            );
            let run = Run::new(Outcome::liveness("wall-time", message), Vec::new());
            report(out, tally, &flight, &run, millis(elapsed))?;
            if Instant::now() < self.end {
                self.spawn()?;
            }
        }
        Ok(())
    }

    fn forget(&mut self, slot: usize) {
        self.live.remove(&slot);
        self.flights.remove(&slot);
        self.left.remove(&slot);
    }
}

/// A thread's work: runs the seeds `draw` gives until the campaign's end,
/// until it is left behind, or until `draw` has no more.
fn work(
    slot: usize,
    sims: &[&'static Sim],
    end: Instant,
    draw: &(dyn Fn() -> Option<Result<(u64, u64), String>> + Send + Sync),
    left: &AtomicBool,
    sender: &SyncSender<Message>,
) {
    while Instant::now() < end && !left.load(Ordering::Relaxed) {
        let (seed, picked) = match draw() {
            Some(Ok(drawn)) => drawn,
            Some(Err(problem)) => {
                let _ = sender.send(Message::NoRandomness(problem));
                return;
            }
            None => break,
        };
        let Some(sim) = pick(sims, picked) else { break };
        let options = catch_unwind(AssertUnwindSafe(|| (sim.describe)(seed)))
            .unwrap_or_else(|_| "(describing it panicked)".to_owned());
        let started = Message::Started {
            slot,
            sim,
            seed,
            options,
        };
        if sender.send(started).is_err() {
            return;
        }
        let since = Instant::now();
        let run = caught(|| (sim.run)(seed));
        let ms = millis(since.elapsed());
        if sender.send(Message::Finished { slot, run, ms }).is_err() {
            return;
        }
    }
    let _ = sender.send(Message::Done { slot });
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

/// Writes a run's record, and counts it.
fn report(
    out: &mut impl Write,
    tally: &mut Tally,
    flight: &Flight,
    run: &Run,
    ms: u64,
) -> io::Result<()> {
    let record = record(flight.sim.name, flight.seed, &flight.options, run, ms);
    writeln!(out, "{record}")?;
    out.flush()?;
    tally.add(flight.sim.name, &run.outcome, ms);
    Ok(())
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
    by_sim: BTreeMap<&'static str, BTreeMap<&'static str, u64>>,
}

impl Tally {
    fn add(&mut self, sim: &'static str, outcome: &Outcome, ms: u64) {
        self.runs += 1;
        if outcome.is_failure() {
            self.failures += 1;
        }
        let counts = self.by_sim.entry(sim).or_insert_with(|| {
            ["runs", "ms", "pass", "safety", "liveness", "unsupported"]
                .into_iter()
                .map(|key| (key, 0))
                .collect()
        });
        for (key, add) in [("runs", 1), ("ms", ms), (outcome.name(), 1)] {
            let count = counts.entry(key).or_default();
            *count = count.saturating_add(add);
        }
    }

    fn summary(&self, elapsed: Duration) -> Value {
        let by_sim: Map<String, Value> = self
            .by_sim
            .iter()
            .map(|(sim, counts)| ((*sim).to_owned(), json!(counts)))
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
        tally.add("node", &Outcome::Pass, 100);
        tally.add("node", &Outcome::safety("events", "broke"), 50);
        tally.add("stream", &Outcome::Pass, 1);
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

    fn tripwire_plan() -> Plan {
        Plan {
            sims: vec![find("tripwire").unwrap()],
            minutes: 1,
            jobs: 1,
        }
    }

    /// Seeds 0, 500, 1000, … for `runs` runs, then no more: half of them
    /// trip the tripwire.
    fn fixed(runs: u64) -> Draw {
        let next = Arc::new(std::sync::atomic::AtomicU64::new(0));
        Arc::new(move || {
            let index = next.fetch_add(1, Ordering::Relaxed);
            (index < runs).then_some(Ok((index * 500, index)))
        })
    }

    #[test]
    fn a_crew_reports_every_run_it_makes() {
        let plan = tripwire_plan();
        // The crew draws a fixed list of seeds and stops once it is spent,
        // well before the campaign's minute is up.
        let (sender, receiver) = mpsc::sync_channel(MESSAGES_PER_JOB);
        let mut crew = Crew::new(&plan, Instant::now(), sender, fixed(100));
        crew.spawn().unwrap();
        let mut out = Vec::new();
        let mut tally = Tally::default();
        while !crew.live.is_empty() {
            let message = receiver.recv_timeout(Duration::from_secs(60)).unwrap();
            crew.take(message, &mut out, &mut tally).unwrap();
        }
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
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

    #[test]
    fn a_run_past_the_limit_is_reported_and_left_behind() {
        let plan = tripwire_plan();
        let (sender, _receiver) = mpsc::sync_channel(MESSAGES_PER_JOB);
        // A campaign whose time is up: no thread takes the place of the
        // one left behind.
        let mut crew = Crew::new(
            &plan,
            Instant::now() - Duration::from_secs(60),
            sender,
            fixed(0),
        );
        let (slot, left) = (7, Arc::new(AtomicBool::new(false)));
        let since = Instant::now() - RUN_LIMIT - Duration::from_secs(1);
        let flight = Flight {
            sim: find("node").unwrap(),
            seed: 42,
            options: "3 nodes".into(),
            since,
        };
        crew.live.insert(slot);
        crew.flights.insert(slot, Some(flight));
        crew.left.insert(slot, Arc::clone(&left));
        let (mut out, mut tally) = (Vec::new(), Tally::default());
        crew.leave_overdue(&mut out, &mut tally).unwrap();
        assert!(left.load(Ordering::Relaxed));
        assert!(crew.live.is_empty());
        assert_eq!(crew.next, 0);
        let line: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(line["signature"], "node/liveness/wall-time");
        assert_eq!(line["seed"], 42);
        assert_eq!(line["options"], "3 nodes");
        assert_eq!(line["trace_tail"], "");
        assert!(line["ms"].as_u64().unwrap() > 600_000);
        // What the thread says once its run ends is not listened to.
        let late = Message::Finished {
            slot,
            run: Run::new(Outcome::Pass, Vec::new()),
            ms: 1,
        };
        crew.take(late, &mut out, &mut tally).unwrap();
        assert_eq!(tally.runs, 1);
    }
}
