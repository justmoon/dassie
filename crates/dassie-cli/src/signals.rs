//! SIGTERM and SIGINT, which stop the daemon, a front door and the market
//! maker.

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
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    shutdown.cancel();
}
