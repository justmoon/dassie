//! A range of seeds of one simulation on several cores, as `dassie-sim run
//! <sim> <seeds>` runs it: the campaign's crew makes the runs, each in a
//! process of its own, `dassie-sim record`, for the reasons in the
//! campaign's docs, and they are reported in the order of their seeds, as
//! if made one after another. Each seed is independent of the others, so
//! running them at once changes how long the range takes, not what it
//! finds.
//!
//! A run that ends before the runs of lower seeds waits for them to be
//! reported. So that few wait, a seed is handed out only while it is less
//! than [`AHEAD_PER_JOB`] per job past the lowest seed still to be
//! reported: a run that takes long holds the others back, rather than
//! leaving them to pile up.
//!
//! As in a campaign, a run still going after [`RUN_LIMIT`] of wall time is
//! stopped and is a liveness failure at `wall-time`, and a run whose
//! process ends without its record a safety failure at `abort`, so that
//! neither ends the range.
//!
//! [`RUN_LIMIT`]: crate::campaign::RUN_LIMIT

use std::collections::BTreeMap;
use std::io;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use dassie_kit::sim::Finding;
use dassie_sim::{Outcome, Sim};
use serde_json::Value;

use crate::campaign::{Crew, LOOK, OUTCOMES, Ran, Runner, in_a_process};

/// How many seeds per job a range hands out past the lowest seed still to
/// be reported: enough that a run as long as a few others keeps no job
/// waiting, and few enough that the runs waiting to be reported take
/// little memory.
pub const AHEAD_PER_JOB: u64 = 64;

/// A run of a range, as its process's record says it: how it ended, what
/// it counted, and how long it took. Its trace stayed with its process.
pub struct Recorded {
    /// The seed it ran.
    pub seed: u64,
    /// How it ended, its message cut as a record cuts it.
    pub outcome: Outcome,
    /// What it counted, by name.
    pub counts: BTreeMap<String, u64>,
    /// Its wall time.
    pub ms: u64,
}

/// Runs `seeds` of `sim` on `jobs` threads, each run in a process of its
/// own, and hands each run to `each` in the order of its seed. Says what
/// failed if the runner itself did. When `each` fails, as when the output
/// is closed, it stops the runs under way and returns the error.
pub fn range(
    sim: &'static Sim,
    seeds: RangeInclusive<u64>,
    jobs: usize,
    each: impl FnMut(Recorded) -> io::Result<()>,
) -> io::Result<Result<(), String>> {
    let ahead = u64::try_from(jobs)
        .unwrap_or(u64::MAX)
        .saturating_mul(AHEAD_PER_JOB);
    in_order(sim, seeds, jobs, ahead, Arc::new(in_a_process), each)
}

/// Runs `seeds` of `sim` on `jobs` threads, each run made by `run`, none
/// more than `ahead` seeds past the lowest still to be handed to `each`.
fn in_order(
    sim: &'static Sim,
    seeds: RangeInclusive<u64>,
    jobs: usize,
    ahead: u64,
    run: Runner,
    mut each: impl FnMut(Recorded) -> io::Result<()>,
) -> io::Result<Result<(), String>> {
    assert!(sim.weight > 0 && jobs >= 1 && !seeds.is_empty());
    assert!(ahead >= u64::try_from(jobs).unwrap_or(u64::MAX));
    let queue = Arc::new(Queue::new(seeds.clone(), ahead));
    let handing = Arc::clone(&queue);
    let crew = Crew {
        sims: vec![sim].into(),
        end: None,
        // The one simulation is picked whatever the number.
        draw: Arc::new(move |stop| handing.next(stop).map(|seed| Ok((seed, 0)))),
        run,
    };
    let mut waiting: BTreeMap<u64, Ran> = BTreeMap::new();
    let mut lowest = Some(*seeds.start());
    let ended = crew.run(jobs, &mut |_, ran| {
        assert!(seeds.contains(&ran.seed) && lowest.is_some_and(|lowest| ran.seed >= lowest));
        let past = ran.seed - lowest.unwrap_or_default();
        assert!(
            past < ahead,
            "seed {} was handed out too far ahead",
            ran.seed
        );
        waiting.insert(ran.seed, ran);
        while let Some(ran) = lowest.and_then(|seed| waiting.remove(&seed)) {
            each(recorded(ran))?;
            queue.reported();
            lowest = lowest
                .filter(|seed| seed < seeds.end())
                .map(|seed| seed + 1);
        }
        Ok(())
    })?;
    // A crew that ran to its end made every seed's run, and each was
    // handed on.
    assert!(ended.is_err() || (lowest.is_none() && waiting.is_empty()));
    Ok(ended)
}

/// The seeds of a range, handed out in order, each once, and none
/// `ahead` or more past the lowest seed still to be reported.
struct Queue {
    seeds: RangeInclusive<u64>,
    ahead: u64,
    places: Mutex<Places>,
    /// Signalled when a seed is reported, which may let another out.
    moved: Condvar,
}

/// How far a queue has got, in seeds from the start of its range.
struct Places {
    handed: u64,
    reported: u64,
}

impl Queue {
    fn new(seeds: RangeInclusive<u64>, ahead: u64) -> Self {
        Self {
            seeds,
            ahead,
            places: Mutex::new(Places {
                handed: 0,
                reported: 0,
            }),
            moved: Condvar::new(),
        }
    }

    /// The next seed, once it is less than `ahead` past the lowest seed
    /// still to be reported; `None` when every seed has been handed out,
    /// or once `stop` says the crew stopped.
    fn next(&self, stop: &AtomicBool) -> Option<u64> {
        let (first, last) = (*self.seeds.start(), *self.seeds.end());
        let mut places = self.places.lock().ok()?;
        loop {
            if stop.load(Ordering::Relaxed) || places.handed > last - first {
                return None;
            }
            if places.handed - places.reported < self.ahead {
                let seed = first + places.handed;
                places.handed += 1;
                return Some(seed);
            }
            places = self.moved.wait_timeout(places, LOOK).ok()?.0;
        }
    }

    /// The lowest seed still to be reported has been.
    fn reported(&self) {
        if let Ok(mut places) = self.places.lock() {
            assert!(places.reported < places.handed);
            places.reported += 1;
        }
        self.moved.notify_all();
    }
}

/// The run `ran` holds, as its record says it.
fn recorded(ran: Ran) -> Recorded {
    assert!(OUTCOMES.contains(&ran.outcome));
    let text = |key: &str| {
        let text = ran.record.get(key).and_then(Value::as_str);
        text.unwrap_or_default().to_owned()
    };
    // `<sim>/<outcome>/<where>`, and `<where>` may hold slashes.
    let signature = text("signature");
    let at = signature
        .splitn(3, '/')
        .nth(2)
        .unwrap_or_default()
        .to_owned();
    let finding = Finding {
        at,
        message: text("message"),
    };
    let outcome = match ran.outcome {
        "pass" => Outcome::Pass,
        "safety" => Outcome::Safety(finding),
        "liveness" => Outcome::Liveness(finding),
        _ => Outcome::Unsupported(finding),
    };
    let counts = ran.record.get("counts").and_then(Value::as_object);
    let counts = counts.into_iter().flatten();
    let counts = counts
        .filter_map(|(what, count)| Some((what.clone(), count.as_u64()?)))
        .collect();
    Recorded {
        seed: ran.seed,
        outcome,
        counts,
        ms: ran.ms,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::thread;
    use std::time::Duration;

    use dassie_sim::{caught, find};

    use super::*;
    use crate::campaign::ran_here;

    /// What `seeds` of `sim` gave, in the order handed on, made by `run`
    /// on `jobs` threads.
    fn ranged(
        sim: &'static Sim,
        seeds: RangeInclusive<u64>,
        jobs: usize,
        ahead: u64,
        run: Runner,
    ) -> Vec<Recorded> {
        let mut handed = Vec::new();
        let ended = in_order(sim, seeds, jobs, ahead, run, |recorded| {
            handed.push(recorded);
            Ok(())
        });
        assert_eq!(ended.unwrap(), Ok(()));
        handed
    }

    #[test]
    fn a_range_hands_on_every_seed_once_in_order() {
        // Runs made here, a test's process not being the runner's, and
        // the low seeds slower, so that they end out of order.
        let wire = find("tripwire").unwrap();
        let run: Runner = Arc::new(|sim, seed, _, _| {
            thread::sleep(Duration::from_millis(
                20u64.saturating_sub(seed % 1_000 / 50),
            ));
            Ok(ran_here(sim, seed))
        });
        let handed = ranged(wire, 1_000..=1_999, 8, 16, run);
        let seeds: Vec<u64> = handed.iter().map(|recorded| recorded.seed).collect();
        assert_eq!(seeds, (1_000..=1_999).collect::<Vec<_>>());
        for recorded in &handed {
            let alone = caught(|| (wire.run)(recorded.seed));
            assert_eq!(recorded.outcome, alone.outcome, "{}", recorded.seed);
        }
        assert!(handed.iter().any(|recorded| recorded.outcome.is_failure()));
    }

    #[test]
    fn a_range_of_one_seed_runs_it() {
        let wire = find("tripwire").unwrap();
        let run: Runner = Arc::new(|sim, seed, _, _| Ok(ran_here(sim, seed)));
        let handed = ranged(wire, u64::MAX - 1..=u64::MAX - 1, 4, 4, run);
        assert_eq!(handed.len(), 1);
        assert_eq!(handed[0].seed, u64::MAX - 1);
    }

    #[test]
    fn no_seed_is_handed_out_too_far_ahead() {
        // Seed 0 takes long; while it runs, only the next three seeds may
        // be handed out, however fast they are.
        let highest = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&highest);
        let run: Runner = Arc::new(move |sim, seed, _, _| {
            if seed == 0 {
                thread::sleep(Duration::from_millis(300));
                let before = seen.swap(u64::MAX, Ordering::Relaxed);
                assert!(before < 4, "seed {before} ran while seed 0 did");
            } else {
                seen.fetch_max(seed, Ordering::Relaxed);
            }
            Ok(ran_here(sim, seed))
        });
        let handed = ranged(find("tripwire").unwrap(), 0..=99, 2, 4, run);
        assert_eq!(handed.len(), 100);
        assert_eq!(highest.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn a_record_gives_back_its_outcome_and_counts() {
        let mut run = dassie_sim::Run::new(Outcome::safety("settle-down", "never settled"), vec![]);
        run.counts.insert("crashes", 3);
        let record = crate::campaign::record("node", 7, "3 nodes", &run, 812);
        let back = recorded(Ran {
            seed: 7,
            record,
            outcome: "safety",
            ms: 812,
        });
        assert_eq!(back.outcome, run.outcome);
        assert_eq!(back.counts, BTreeMap::from([("crashes".to_owned(), 3)]));
        assert_eq!((back.seed, back.ms), (7, 812));
        // A panic's `<where>` is a file, which holds slashes.
        let panicked = Outcome::Safety(Finding {
            at: "crates/dassie-node/src/sim/mod.rs:993".into(),
            message: "boom".into(),
        });
        let record = crate::campaign::record(
            "node",
            8,
            "",
            &dassie_sim::Run::new(panicked.clone(), vec![]),
            1,
        );
        let ran = Ran {
            seed: 8,
            record,
            outcome: "safety",
            ms: 1,
        };
        assert_eq!(recorded(ran).outcome, panicked);
    }

    #[test]
    fn a_range_whose_output_closes_stops() {
        let run: Runner = Arc::new(|sim, seed, _, _| Ok(ran_here(sim, seed)));
        let mut handed = 0;
        let ended = in_order(find("tripwire").unwrap(), 0..=9_999, 4, 16, run, |_| {
            handed += 1;
            if handed == 3 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            Ok(())
        });
        assert_eq!(ended.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(handed, 3);
    }
}
