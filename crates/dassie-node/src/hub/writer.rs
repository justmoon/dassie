//! One writer for the steps of every hub the node validates
//! (spec/areas/hubs.md#the-validator-in-the-node). A validator's step is on
//! disk before any of its messages go out, and every commit waits for the
//! disk: so each hub's validator hands its step's [`Write`] here and waits,
//! and the writer commits in one transaction every step waiting when it
//! starts, on a blocking task off the async runtime, then tells each it is
//! written. The validators of several hubs then wait for one commit, not
//! one each, and the node's other commits wait on its store for fewer.
//!
//! The writer adds no wait of its own: a step that finds it idle is
//! committed at once, alone. A validator waits for its step before it takes
//! its next input, so a commit holds at most one step per hub. A commit
//! that fails writes none of its steps, and each validator that waited for
//! it stops, as its own failed write stopped it before.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{mpsc, oneshot};

use super::config::MAX_VALIDATED_HUBS;
use super::driver::Write;
use crate::log::target;
use crate::store::{Store, StoreError};

/// The most steps one commit holds: one per hub the node validates.
const MAX_STEPS: usize = MAX_VALIDATED_HUBS;

/// How the validators of the node's hubs wrote their steps, since the node
/// started.
#[derive(Debug, Default)]
pub struct WriteCounters {
    steps: AtomicU64,
    commits: AtomicU64,
    commit_micros: AtomicU64,
    longest_commit_micros: AtomicU64,
}

/// What [`WriteCounters`] counted, read at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteCounts {
    /// Steps written.
    pub steps: u64,
    /// Commits that wrote them.
    pub commits: u64,
    /// Microseconds the commits took, all together.
    pub commit_micros: u64,
    /// Microseconds the longest commit took.
    pub longest_commit_micros: u64,
}

impl WriteCounters {
    /// What they counted.
    #[must_use]
    pub fn counts(&self) -> WriteCounts {
        WriteCounts {
            steps: self.steps.load(Ordering::Relaxed),
            commits: self.commits.load(Ordering::Relaxed),
            commit_micros: self.commit_micros.load(Ordering::Relaxed),
            longest_commit_micros: self.longest_commit_micros.load(Ordering::Relaxed),
        }
    }

    fn commit(&self, steps: usize, micros: u64) {
        let steps = u64::try_from(steps).unwrap_or(u64::MAX);
        self.steps.fetch_add(steps, Ordering::Relaxed);
        self.commits.fetch_add(1, Ordering::Relaxed);
        self.commit_micros.fetch_add(micros, Ordering::Relaxed);
        self.longest_commit_micros
            .fetch_max(micros, Ordering::Relaxed);
    }
}

/// A step waiting to be written, and whom to tell once it is.
struct Job {
    write: Write,
    written: oneshot::Sender<bool>,
}

/// Where the validators of the node's hubs hand their steps to be written.
/// It runs until the last of them is dropped.
#[derive(Clone, Debug)]
pub struct HubWriter {
    queue: mpsc::Sender<Job>,
}

impl HubWriter {
    /// Starts a writer to `store`, which counts into `counters`.
    #[must_use]
    pub fn spawn(store: Arc<Store>, counters: Arc<WriteCounters>) -> Self {
        let (queue, jobs) = mpsc::channel(MAX_STEPS);
        tokio::spawn(run(store, counters, jobs));
        Self { queue }
    }

    /// Writes `write`, with the steps of other hubs waiting then, and
    /// returns once it is on disk.
    ///
    /// # Errors
    ///
    /// The commit failed, and wrote nothing; the writer logged why.
    pub async fn write(&self, write: Write) -> Result<(), StoreError> {
        let (written, done) = oneshot::channel();
        self.queue
            .send(Job { write, written })
            .await
            .map_err(|_| StoreError::Corrupt("the hubs' writer stopped"))?;
        match done.await {
            Ok(true) => Ok(()),
            Ok(false) => Err(StoreError::Corrupt("the hubs' steps could not be written")),
            Err(_) => Err(StoreError::Corrupt("the hubs' writer stopped")),
        }
    }
}

/// Commits what waits, one commit at a time, until no validator is left.
async fn run(store: Arc<Store>, counters: Arc<WriteCounters>, mut jobs: mpsc::Receiver<Job>) {
    let mut batch: Vec<Job> = Vec::with_capacity(MAX_STEPS);
    while let Some(job) = jobs.recv().await {
        batch.push(job);
        while batch.len() < MAX_STEPS {
            let Ok(job) = jobs.try_recv() else {
                break;
            };
            batch.push(job);
        }
        commit(&store, &counters, &mut batch).await;
    }
}

/// Commits the steps in `batch` in one transaction, and tells each whether
/// it is written.
async fn commit(store: &Arc<Store>, counters: &WriteCounters, batch: &mut Vec<Job>) {
    assert!(
        !batch.is_empty() && batch.len() <= MAX_STEPS,
        "a commit holds one to {MAX_STEPS} steps"
    );
    let (writes, waiting): (Vec<Write>, Vec<oneshot::Sender<bool>>) =
        batch.drain(..).map(|job| (job.write, job.written)).unzip();
    let hubs: std::collections::BTreeSet<&[u8; 32]> = writes.iter().map(Write::hub).collect();
    assert_eq!(
        hubs.len(),
        writes.len(),
        "a commit holds at most one step per hub: each validator waits for its step"
    );
    let steps = writes.len();
    let started = tokio::time::Instant::now();
    let store = Arc::clone(store);
    let written = tokio::task::spawn_blocking(move || Write::apply_all(&writes, &store)).await;
    let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
    let written = match written {
        Ok(Ok(())) => {
            counters.commit(steps, micros);
            true
        }
        Ok(Err(error)) => {
            tracing::error!(target: target::HUB, %error, steps, "the hubs' steps could not be written");
            false
        }
        Err(error) => {
            tracing::error!(target: target::HUB, %error, steps, "writing the hubs' steps did not finish");
            false
        }
    };
    for waiter in waiting {
        let _ = waiter.send(written);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Aim;

    fn write(hub: u8, state: &[u8]) -> Write {
        Write::of_state([hub; 32], 1_000, state.to_vec(), (1, state.to_vec()))
    }

    fn consensus(store: &Store, hub: u8) -> Option<Vec<u8>> {
        store
            .hub(&[hub; 32])
            .unwrap()
            .map(|stored| stored.consensus)
    }

    // Steps of several hubs that wait together are written in one commit,
    // each of them; a step that finds the writer idle goes alone.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn steps_that_wait_together_are_written_in_one_commit() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let counters = Arc::new(WriteCounters::default());
        let writer = HubWriter::spawn(Arc::clone(&store), Arc::clone(&counters));

        let (a, b, c) = tokio::join!(
            writer.write(write(1, b"a")),
            writer.write(write(2, b"b")),
            writer.write(write(3, b"c")),
        );
        assert!(a.is_ok() && b.is_ok() && c.is_ok());
        let counts = counters.counts();
        assert_eq!((counts.steps, counts.commits), (3, 1));
        for (hub, state) in [(1, b"a"), (2, b"b"), (3, b"c")] {
            assert_eq!(consensus(&store, hub).as_deref(), Some(&state[..]));
        }

        writer.write(write(1, b"d")).await.unwrap();
        let counts = counters.counts();
        assert_eq!((counts.steps, counts.commits), (4, 2));
        assert_eq!(consensus(&store, 1).as_deref(), Some(&b"d"[..]));
    }

    // A commit that fails writes none of its steps, and every validator
    // that waited for it is told; the writer goes on with the next.
    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn a_failed_commit_fails_every_step_in_it() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let counters = Arc::new(WriteCounters::default());
        let writer = HubWriter::spawn(Arc::clone(&store), Arc::clone(&counters));
        let failed = store.fail_commit(Aim::Hub, 0);

        let (a, b) = tokio::join!(writer.write(write(1, b"a")), writer.write(write(2, b"b")),);
        assert!(a.is_err() && b.is_err());
        assert!(failed.held().is_some_and(|held| held.hub));
        assert_eq!((consensus(&store, 1), consensus(&store, 2)), (None, None));
        assert_eq!(counters.counts(), WriteCounts::default());

        store.let_commits_through();
        writer.write(write(1, b"c")).await.unwrap();
        assert_eq!(consensus(&store, 1).as_deref(), Some(&b"c"[..]));
    }
}
