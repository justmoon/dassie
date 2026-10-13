//! Restarting services that stop when they should not.
//!
//! In the original an error stopped an actor for good, and the daemon ran on
//! without it (RT-24). A service here runs until shutdown; if it returns or
//! panics before that, it is restarted after a backoff that doubles from
//! one second to a minute, and the failure is logged.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::log::target;

/// The first wait before a restart.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// The longest wait before a restart.
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A service that ran this long before failing starts the backoff over.
const HEALTHY_AFTER: Duration = Duration::from_secs(300);

/// Runs `service` under `tasks` until `shutdown`, restarting it as needed.
pub fn spawn<F, Fut>(
    tasks: &TaskTracker,
    name: &'static str,
    shutdown: CancellationToken,
    service: F,
) where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tasks.spawn(async move {
        let mut backoff = BACKOFF_MIN;
        loop {
            let started = Instant::now();
            let outcome = tokio::spawn(service()).await;
            if shutdown.is_cancelled() {
                return;
            }
            match outcome {
                Ok(()) => tracing::error!(target: target::DAEMON, service = name, "service stopped; restarting"),
                Err(error) => tracing::error!(target: target::DAEMON, service = name, %error, "service failed; restarting"),
            }
            if started.elapsed() >= HEALTHY_AFTER {
                backoff = BACKOFF_MIN;
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    });
}
