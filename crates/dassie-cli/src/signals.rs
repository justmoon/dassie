//! SIGTERM and SIGINT, which stop the daemon, a front door and the market
//! maker.

use dassie_node::daemon::Shutdown;
use tokio_util::sync::CancellationToken;

/// The two signals' streams.
pub type Signals = (tokio::signal::unix::Signal, tokio::signal::unix::Signal);

/// SIGTERM and SIGINT, handled from now on.
#[must_use]
pub fn listen_for_signals() -> Option<Signals> {
    use tokio::signal::unix::{SignalKind, signal};
    let terminate = signal(SignalKind::terminate()).ok()?;
    let interrupt = signal(SignalKind::interrupt()).ok()?;
    Some((terminate, interrupt))
}

/// Cancels `shutdown` at the first of the two.
pub async fn stop_on_signal(signals: Signals, shutdown: CancellationToken) {
    let (mut terminate, mut interrupt) = signals;
    next(&mut terminate, &mut interrupt).await;
    shutdown.cancel();
}

/// Stops the daemon: at the first of the two once it drained its packets in
/// flight, at the second at once (spec/areas/node.md#shutdown). Later ones
/// change nothing.
pub async fn drain_on_signal(signals: Signals, shutdown: Shutdown) {
    let (mut terminate, mut interrupt) = signals;
    next(&mut terminate, &mut interrupt).await;
    shutdown.drain.cancel();
    next(&mut terminate, &mut interrupt).await;
    tracing::warn!(target: dassie_node::log::target::DAEMON, "stopping at once: the answers to packets still in flight are lost");
    shutdown.now.cancel();
}

/// Until the next of either.
async fn next(
    terminate: &mut tokio::signal::unix::Signal,
    interrupt: &mut tokio::signal::unix::Signal,
) {
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
}
