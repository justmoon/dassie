//! Restarting services that stop when they should not.
//!
//! In the original an error stopped an actor for good, and the daemon ran on
//! without it (RT-24). A service here runs until shutdown; if it returns or
//! panics before that, it is restarted after a backoff that doubles from one
//! second to a minute, and the failure is logged. A [`Registry`] keeps each
//! service's state for the wallet's State page
//! (spec/areas/wallet.md#debug-pages).

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

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

/// The services and what became of them.
pub struct Registry {
    clock: Arc<dyn Clock>,
    services: Mutex<BTreeMap<&'static str, ServiceStatus>>,
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
        }
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
/// and keeps its state in `registry`.
pub fn spawn<F, Fut>(
    tasks: &TaskTracker,
    registry: &Arc<Registry>,
    name: &'static str,
    shutdown: CancellationToken,
    service: F,
) where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let registry = Arc::clone(registry);
    tasks.spawn(async move {
        let mut backoff = BACKOFF_MIN;
        loop {
            let started = Instant::now();
            registry.started(name);
            let outcome = tokio::spawn(service()).await;
            if shutdown.is_cancelled() {
                registry.stopped(name);
                return;
            }
            match &outcome {
                Ok(()) => tracing::error!(target: target::DAEMON, service = name, "service stopped; restarting"),
                Err(error) => tracing::error!(target: target::DAEMON, service = name, %error, "service failed; restarting"),
            }
            if started.elapsed() >= HEALTHY_AFTER {
                backoff = BACKOFF_MIN;
            }
            registry.failed(name, failure(outcome), backoff);
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
        spawn(&tasks, &registry, "flaky", shutdown.clone(), move || {
            let (counter, stop) = (Arc::clone(&counter), stop.clone());
            async move {
                assert!(
                    counter.fetch_add(1, Ordering::SeqCst) > 0,
                    "first run fails"
                );
                stop.cancelled().await;
            }
        });
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
}
