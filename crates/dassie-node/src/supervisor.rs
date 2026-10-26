//! Restarting services that stop when they should not.
//!
//! In the original an error stopped an actor for good, and the daemon ran on
//! without it. A service here runs until shutdown; if it returns or
//! panics before that, it is restarted after a backoff that doubles from one
//! second to a minute, and the failure is logged. A [`Registry`] keeps each
//! service's state for the wallet's State page
//! (spec/areas/wallet.md#debug-pages).
//!
//! Until the node is up, a service that must work for it to be ([`BeforeUp`])
//! is not restarted for ever: after [`START_ATTEMPTS`] failures the start
//! has failed, and the daemon exits, so systemd and `dassie update` see a
//! release that does not work (spec/areas/node.md#readiness).

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::clock::Clock;
use crate::log::target;

/// Longest error kept for a service.
const MAX_ERROR_BYTES: usize = 256;

/// Where a service is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    /// Running.
    Running,
    /// Stopped when it should not have, and waiting to start again.
    Restarting,
    /// Stopped at shutdown.
    Stopped,
}

/// A service as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceStatus {
    /// Its name.
    pub name: &'static str,
    /// Where it is.
    pub state: ServiceState,
    /// When it last started.
    pub started_at_ms: u64,
    /// How often it was restarted.
    pub restarts: u32,
    /// Why it last stopped, if it did.
    pub last_error: Option<String>,
    /// When it last stopped.
    pub last_error_at_ms: Option<u64>,
    /// When it starts again, while restarting.
    pub restart_at_ms: Option<u64>,
}

/// Where the node's start is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// The engine has not committed a step yet.
    Starting,
    /// The engine committed its first step, or waits for the owner to set
    /// the node up: readiness may be reported.
    Up,
    /// A service that the node needs failed [`START_ATTEMPTS`] times before
    /// the node was up.
    Failed {
        /// The service.
        service: &'static str,
        /// Why it last stopped.
        error: String,
    },
}

/// What a service's failures do before the node is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeforeUp {
    /// The service is restarted, as after.
    Restart,
    /// The node needs it: [`START_ATTEMPTS`] failures fail the start.
    FailStart,
}

/// How often a service the node needs may fail before the node is up.
pub const START_ATTEMPTS: u32 = 3;

/// The services and what became of them.
pub struct Registry {
    clock: Arc<dyn Clock>,
    services: Mutex<BTreeMap<&'static str, ServiceStatus>>,
    start: watch::Sender<Start>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").finish_non_exhaustive()
    }
}

impl Registry {
    /// No services yet.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            services: Mutex::new(BTreeMap::new()),
            start: watch::Sender::new(Start::Starting),
        }
    }

    /// Where the node's start is, and each change to it.
    #[must_use]
    pub fn start(&self) -> watch::Receiver<Start> {
        self.start.subscribe()
    }

    /// The node is up. Only the first call, before any failed start, counts.
    pub fn up(&self) {
        self.start.send_if_modified(|start| {
            let starting = *start == Start::Starting;
            if starting {
                *start = Start::Up;
            }
            starting
        });
    }

    /// `service` failed the start, unless the node is up already. Returns
    /// whether it did.
    fn fail_start(&self, service: &'static str, error: &str) -> bool {
        self.start.send_if_modified(|start| {
            let starting = *start == Start::Starting;
            if starting {
                *start = Start::Failed {
                    service,
                    error: error.to_owned(),
                };
            }
            starting
        })
    }

    fn is_starting(&self) -> bool {
        *self.start.borrow() == Start::Starting
    }

    /// Every service, by name.
    #[must_use]
    pub fn services(&self) -> Vec<ServiceStatus> {
        self.lock().values().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<&'static str, ServiceStatus>> {
        self.services.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn started(&self, name: &'static str) {
        let now = self.clock.now_ms();
        let mut services = self.lock();
        let status = services.entry(name).or_insert(ServiceStatus {
            name,
            state: ServiceState::Running,
            started_at_ms: now,
            restarts: 0,
            last_error: None,
            last_error_at_ms: None,
            restart_at_ms: None,
        });
        status.state = ServiceState::Running;
        status.started_at_ms = now;
        status.restart_at_ms = None;
    }

    fn failed(&self, name: &'static str, error: String, backoff: Duration) {
        let now = self.clock.now_ms();
        let wait = u64::try_from(backoff.as_millis()).unwrap_or(u64::MAX);
        if let Some(status) = self.lock().get_mut(name) {
            status.state = ServiceState::Restarting;
            status.restarts = status.restarts.saturating_add(1);
            status.last_error = Some(error);
            status.last_error_at_ms = Some(now);
            status.restart_at_ms = Some(now.saturating_add(wait));
        }
    }

    fn stopped(&self, name: &'static str) {
        if let Some(status) = self.lock().get_mut(name) {
            status.state = ServiceState::Stopped;
            status.restart_at_ms = None;
        }
    }
}

/// Why a service's task ended: its panic's message, if it had one.
fn failure(outcome: Result<(), tokio::task::JoinError>) -> String {
    let text = match outcome {
        Ok(()) => String::from("returned"),
        Err(error) if error.is_panic() => {
            let payload = error.into_panic();
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| String::from("panicked"))
        }
        Err(error) => error.to_string(),
    };
    let mut end = text.len().min(MAX_ERROR_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_string()
}

/// The first wait before a restart.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// The longest wait before a restart.
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A service that ran this long before failing starts the backoff over.
const HEALTHY_AFTER: Duration = Duration::from_secs(300);

/// Runs `service` under `tasks` until `shutdown`, restarting it as needed,
/// and keeps its state in `registry`. With [`BeforeUp::FailStart`], its
/// [`START_ATTEMPTS`]th failure before the node is up fails the start, and
/// it is not started again.
pub fn spawn<F, Fut>(
    tasks: &TaskTracker,
    registry: &Arc<Registry>,
    name: &'static str,
    before_up: BeforeUp,
    shutdown: CancellationToken,
    service: F,
) where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let registry = Arc::clone(registry);
    tasks.spawn(async move {
        let mut backoff = BACKOFF_MIN;
        let mut failed_before_up: u32 = 0;
        loop {
            let started = Instant::now();
            registry.started(name);
            let outcome = tokio::spawn(service()).await;
            if shutdown.is_cancelled() {
                registry.stopped(name);
                return;
            }
            let error = failure(outcome);
            if before_up == BeforeUp::FailStart && registry.is_starting() {
                failed_before_up += 1;
                assert!(failed_before_up <= START_ATTEMPTS);
                if failed_before_up == START_ATTEMPTS && registry.fail_start(name, &error) {
                    tracing::error!(target: target::DAEMON, service = name, %error, attempts = START_ATTEMPTS, "service failed while starting; giving up");
                    registry.failed(name, error, Duration::ZERO);
                    registry.stopped(name);
                    return;
                }
            }
            tracing::error!(target: target::DAEMON, service = name, %error, "service stopped; restarting");
            if started.elapsed() >= HEALTHY_AFTER {
                backoff = BACKOFF_MIN;
            }
            registry.failed(name, error, backoff);
            tokio::select! {
                () = shutdown.cancelled() => {
                    registry.stopped(name);
                    return;
                }
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    });
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::clock::ManualClock;

    // A service that panics is restarted, and the registry says why.
    #[tokio::test(start_paused = true)]
    async fn the_registry_follows_a_failing_service() {
        let registry = Arc::new(Registry::new(Arc::new(ManualClock::new(1_000))));
        let (tasks, shutdown) = (TaskTracker::new(), CancellationToken::new());
        let runs = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&runs);
        let stop = shutdown.clone();
        spawn(
            &tasks,
            &registry,
            "flaky",
            BeforeUp::Restart,
            shutdown.clone(),
            move || {
                let (counter, stop) = (Arc::clone(&counter), stop.clone());
                async move {
                    assert!(
                        counter.fetch_add(1, Ordering::SeqCst) > 0,
                        "first run fails"
                    );
                    stop.cancelled().await;
                }
            },
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        let status = &registry.services()[0];
        assert_eq!(status.state, ServiceState::Restarting);
        assert_eq!(status.restarts, 1);
        assert!(
            status
                .last_error
                .as_deref()
                .unwrap()
                .contains("first run fails")
        );
        tokio::time::sleep(BACKOFF_MIN).await;
        assert_eq!(registry.services()[0].state, ServiceState::Running);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        shutdown.cancel();
        tasks.close();
        tasks.wait().await;
        assert_eq!(registry.services()[0].state, ServiceState::Stopped);
    }

    // A release whose engine fails at every start must not count as up: the
    // daemon exits, and the update rolls back.
    #[tokio::test(start_paused = true)]
    async fn a_needed_service_that_keeps_failing_before_the_node_is_up_fails_the_start() {
        let registry = Arc::new(Registry::new(Arc::new(ManualClock::new(1_000))));
        let (tasks, shutdown) = (TaskTracker::new(), CancellationToken::new());
        let runs = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&runs);
        spawn(
            &tasks,
            &registry,
            "engine",
            BeforeUp::FailStart,
            shutdown.clone(),
            move || {
                let counter = Arc::clone(&counter);
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    panic!("broken release");
                }
            },
        );
        let mut start = registry.start();
        let failed = start
            .wait_for(|start| *start != Start::Starting)
            .await
            .unwrap()
            .clone();
        assert_eq!(
            failed,
            Start::Failed {
                service: "engine",
                error: "broken release".into()
            }
        );
        // It is not started again, and the task ends without a shutdown.
        tasks.close();
        tasks.wait().await;
        assert_eq!(runs.load(Ordering::SeqCst), START_ATTEMPTS);
        assert_eq!(registry.services()[0].state, ServiceState::Stopped);
        // The start stays failed.
        registry.up();
        assert!(matches!(*registry.start().borrow(), Start::Failed { .. }));
    }

    // Once up, the engine is restarted however often it fails, as before.
    #[tokio::test(start_paused = true)]
    async fn once_the_node_is_up_a_needed_service_is_restarted_as_any() {
        let registry = Arc::new(Registry::new(Arc::new(ManualClock::new(1_000))));
        let (tasks, shutdown) = (TaskTracker::new(), CancellationToken::new());
        let runs = Arc::new(AtomicU32::new(0));
        let (counter, up) = (Arc::clone(&runs), Arc::clone(&registry));
        let stop = shutdown.clone();
        spawn(
            &tasks,
            &registry,
            "engine",
            BeforeUp::FailStart,
            shutdown.clone(),
            move || {
                let (counter, up, stop) = (Arc::clone(&counter), Arc::clone(&up), stop.clone());
                async move {
                    up.up();
                    assert!(
                        counter.fetch_add(1, Ordering::SeqCst) > START_ATTEMPTS,
                        "fails after the node is up"
                    );
                    stop.cancelled().await;
                }
            },
        );
        tokio::time::sleep(BACKOFF_MAX * (START_ATTEMPTS + 2)).await;
        assert_eq!(runs.load(Ordering::SeqCst), START_ATTEMPTS + 2);
        assert_eq!(*registry.start().borrow(), Start::Up);
        assert_eq!(registry.services()[0].state, ServiceState::Running);
        shutdown.cancel();
        tasks.close();
        tasks.wait().await;
    }
}
