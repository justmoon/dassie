//! The owner's node, in this process (spec/areas/desktop.md#the-node).
//!
//! The daemon runs as `dassie daemon` runs it, on a runtime of its own on a
//! thread of its own, so that it never shares Tauri's. Its listeners stay on
//! loopback, where only this machine reaches the wallet; its QUIC socket
//! listens on every address, since a periphery node dials its relay and
//! uplinks and is never dialled.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;

use dassie_node::clock::{SystemClock, os_rng};
use dassie_node::daemon::{self, DaemonOptions, Prepared};
use dassie_node::env::Env;
use dassie_node::node::{Node, Policies, Runtime};
use dassie_node::quic::relay::RelayLink;
use dassie_node_core::config::Ports;
use tauri::Url;
use tokio_util::sync::CancellationToken;

/// The node, while it runs.
#[derive(Debug)]
pub struct RunningNode {
    /// The port of its HTTPS listener on loopback, the wallet's.
    pub https_port: u16,
    node: Weak<Node>,
    shutdown: CancellationToken,
    thread: std::sync::Mutex<Option<JoinHandle<Result<(), String>>>>,
}

/// How the node stands, as the tray shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The owner has not set the node up yet.
    NotSetUp,
    /// It keeps a connection to its relay, which stands so.
    Relay(RelayLink),
    /// It keeps no relay: the owner opted out, or it has no QUIC socket.
    NoRelay,
    /// It stopped.
    Stopped,
}

impl Status {
    /// The tray's line for it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NotSetUp => "Not set up yet",
            Self::Relay(RelayLink::Connected) => "Online",
            Self::Relay(RelayLink::Connecting) => "Connecting…",
            Self::Relay(RelayLink::Unreachable) => "Offline: the relay is unreachable",
            Self::NoRelay => "Running, without a relay",
            Self::Stopped => "Stopped",
        }
    }
}

/// Starts the node on `ports`; `on_end` runs once it stops, with why if it
/// failed, and `on_signal` on SIGTERM or SIGINT. Returns once the node is
/// made, or with why it could not start.
pub fn start(
    env: &Env,
    ports: Ports,
    on_end: impl FnOnce(Option<String>) + Send + 'static,
    on_signal: impl FnOnce() + Send + 'static,
) -> Result<RunningNode, String> {
    let options = DaemonOptions {
        runtime: Some(Runtime::Standalone),
        ports: Some(ports),
        listen: Some(Ipv4Addr::LOCALHOST.into()),
        quic: Some(quic_address()),
    };
    let (to, from) = tokio::sync::oneshot::channel();
    let prepared = Prepared::new(env, options)
        .map_err(|error| error.to_string())?
        .handing_out_node(to);
    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    // `on_end` is for a node that started; one that did not is this
    // function's error.
    let started = Arc::new(AtomicBool::new(false));
    let ended = Arc::clone(&started);
    let thread = std::thread::Builder::new()
        .name("dassie-node".into())
        .spawn(move || {
            let outcome = run(prepared, token, on_signal);
            if ended.load(Ordering::Acquire) {
                on_end(outcome.clone().err());
            }
            outcome
        })
        .map_err(|error| error.to_string())?;
    // The node is made before anything is served, or the daemon ended
    // without one and its thread says why.
    let Ok(node) = from.blocking_recv() else {
        return Err(match thread.join() {
            Ok(Err(error)) => error,
            Ok(Ok(())) | Err(_) => "the node stopped while it started".into(),
        });
    };
    started.store(true, Ordering::Release);
    Ok(RunningNode {
        https_port: ports.https(),
        node,
        shutdown,
        thread: std::sync::Mutex::new(Some(thread)),
    })
}

/// Runs the daemon to its end on a runtime of its own. SIGTERM and SIGINT,
/// which a session sends when it ends, go to `on_signal`, which quits the
/// app and so stops the node, as they stop `dassie daemon`.
fn run(
    prepared: Prepared,
    shutdown: CancellationToken,
    on_signal: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    let clock = Arc::new(SystemClock);
    runtime
        .block_on(async move {
            match signals() {
                Some((mut terminate, mut interrupt)) => {
                    tokio::spawn(async move {
                        tokio::select! {
                            _ = terminate.recv() => {}
                            _ = interrupt.recv() => {}
                        }
                        on_signal();
                    });
                }
                None => {
                    tracing::warn!(target: "dassie_desktop", "SIGTERM and SIGINT end the app without stopping its node");
                }
            }
            daemon::run(prepared, clock, os_rng(), Policies::DEFAULT, shutdown).await
        })
        .map_err(|error| {
            tracing::error!(target: "dassie_desktop", %error, "the node stopped");
            error.to_string()
        })
}

/// SIGTERM and SIGINT, handled from now on.
fn signals() -> Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)> {
    use tokio::signal::unix::{SignalKind, signal};
    Some((
        signal(SignalKind::terminate()).ok()?,
        signal(SignalKind::interrupt()).ok()?,
    ))
}

/// Every address, IPv6 and IPv4 where the system has IPv6, IPv4 only where
/// it does not, as the standalone daemon listens by default.
fn quic_address() -> SocketAddr {
    let dual = SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0));
    if UdpSocket::bind(dual).is_ok() {
        dual
    } else {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))
    }
}

impl RunningNode {
    /// How the node stands now.
    #[must_use]
    pub fn status(&self) -> Status {
        let Some(node) = self.node.upgrade() else {
            return Status::Stopped;
        };
        if !node.is_set_up() {
            return Status::NotSetUp;
        }
        match node.quic().and_then(|quic| quic.chosen_relay()) {
            Some((_, _, link)) => Status::Relay(link),
            None => Status::NoRelay,
        }
    }

    /// Where the window opens: the setup page until the node is set up,
    /// then the wallet.
    #[must_use]
    pub fn start_url(&self) -> Option<Url> {
        let node = self.node.upgrade()?;
        let text = node
            .setup_url()
            .unwrap_or_else(|| format!("https://localhost:{}/", self.https_port));
        Url::parse(&text).ok()
    }

    /// The certificate the node serves now, as PEM.
    #[must_use]
    pub fn certificate_pem(&self) -> Option<String> {
        Some(self.node.upgrade()?.certificate().chain_pem())
    }

    /// The node, weakly, for what outlives this call.
    #[must_use]
    pub fn weak(&self) -> Weak<Node> {
        self.node.clone()
    }

    /// Whether it was asked to stop.
    #[must_use]
    pub fn stopping(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// Stops the node and waits for it: the daemon bounds how long its
    /// services get.
    pub fn stop(&self) {
        self.shutdown.cancel();
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_its_own_line() {
        let all = [
            Status::NotSetUp,
            Status::Relay(RelayLink::Connected),
            Status::Relay(RelayLink::Connecting),
            Status::Relay(RelayLink::Unreachable),
            Status::NoRelay,
            Status::Stopped,
        ];
        let labels: std::collections::BTreeSet<_> = all.iter().map(|s| s.label()).collect();
        assert_eq!(labels.len(), all.len());
    }
}
