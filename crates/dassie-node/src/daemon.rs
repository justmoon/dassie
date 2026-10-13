//! Starting and stopping the daemon (spec 7 §4.1).
//!
//! In order: the environment is checked, the database opened and migrated,
//! the sockets taken from systemd or bound, the certificate loaded or made,
//! and the services started. Readiness is reported then, without waiting on
//! the network; the original waited for its first ACME check, and in its
//! standalone runtime ran that check before anything listened on port 80
//! (RT-8). On SIGTERM it stops accepting, lets requests finish, closes the
//! database and removes the socket it made (RT-7).
//!
//! The steps that adopt sockets run before the tokio runtime exists
//! ([`Prepared::new`]); the rest runs inside it ([`run`]).

use std::fmt;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dassie_node_core::config::Ports;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::clock::{Clock, SharedRng};
use crate::env::{Env, EnvError, Settings};
use crate::log::target;
use crate::node::{Node, NodeError, NodeOptions, Policies, Runtime};
use crate::store::{Store, StoreError};
use crate::systemd::{self, ActivationError, Notifier};
use crate::{http, ipc, supervisor, tls};

/// How often expired sessions are deleted.
const PRUNE_INTERVAL: Duration = Duration::from_secs(3_600);
/// Time services get to stop at shutdown.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// The daemon could not start.
#[derive(Debug)]
pub enum DaemonError {
    /// The environment is unusable.
    Env(EnvError),
    /// The database failed.
    Store(StoreError),
    /// systemd's sockets could not be taken.
    Activation(ActivationError),
    /// A socket could not be bound.
    Bind(String, io::Error),
    /// The node could not start.
    Node(NodeError),
    /// SIGTERM and SIGINT could not be handled.
    Signals,
    /// TLS could not be set up.
    Tls(rustls::Error),
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(error) => error.fmt(f),
            Self::Store(error) => error.fmt(f),
            Self::Activation(error) => error.fmt(f),
            Self::Bind(what, error) => write!(f, "binding {what}: {error}"),
            Self::Node(error) => error.fmt(f),
            Self::Signals => f.write_str("could not listen for SIGTERM and SIGINT"),
            Self::Tls(error) => write!(f, "TLS: {error}"),
        }
    }
}

impl std::error::Error for DaemonError {}

/// How `dassie daemon` was asked to run.
#[derive(Clone, Copy, Debug, Default)]
pub struct DaemonOptions {
    /// Force a runtime; otherwise systemd's is used when it passed sockets
    /// to this process.
    pub runtime: Option<Runtime>,
    /// Ports for this run of the standalone runtime, instead of the
    /// configured ones: for running unprivileged, and for the development
    /// environment's many nodes.
    pub ports: Option<Ports>,
}

/// Everything the daemon needs before the async runtime starts.
#[derive(Debug)]
pub struct Prepared {
    /// The environment's settings.
    pub settings: Settings,
    /// The open database.
    pub store: Store,
    /// How the sockets came.
    pub runtime: Runtime,
    http: TcpListener,
    https: TcpListener,
    ipc: UnixListener,
    /// The IPC socket the daemon made, published once it is ready.
    ipc_socket: Option<ipc::Unpublished>,
    notifier: Option<Notifier>,
}

impl Prepared {
    /// Checks the environment, opens the database and takes or binds the
    /// sockets.
    pub fn new(env: &Env, options: DaemonOptions) -> Result<Self, DaemonError> {
        let settings = Settings::from_env(env).map_err(DaemonError::Env)?;
        let notifier = Notifier::from_env(env).map_err(DaemonError::Env)?;
        let runtime = options.runtime.unwrap_or(if systemd::is_activated(env) {
            Runtime::Systemd
        } else {
            Runtime::Standalone
        });
        let store = Store::open(&settings.data_dir).map_err(DaemonError::Store)?;
        let (http, https, ipc, ipc_socket) = match runtime {
            Runtime::Systemd => {
                let sockets = systemd::take_sockets(env).map_err(DaemonError::Activation)?;
                (sockets.http, sockets.https, sockets.ipc, None)
            }
            Runtime::Standalone => {
                let ports = match options.ports {
                    Some(ports) => ports,
                    None => store.config().map_err(DaemonError::Store)?.ports,
                };
                let (http, https) = bind_ports(ports)?;
                let path = standalone_ipc_path(&settings, env);
                let (ipc, unpublished) = ipc::bind_unpublished(&path)
                    .map_err(|e| DaemonError::Bind(path.display().to_string(), e))?;
                (http, https, ipc, Some(unpublished))
            }
        };
        Ok(Self {
            settings,
            store,
            runtime,
            http,
            https,
            ipc,
            ipc_socket,
            notifier,
        })
    }

    /// For tests: the given sockets, as if systemd had passed them.
    #[must_use]
    pub fn from_parts(
        settings: Settings,
        store: Store,
        http: TcpListener,
        https: TcpListener,
        ipc: UnixListener,
    ) -> Self {
        Self {
            settings,
            store,
            runtime: Runtime::Systemd,
            http,
            https,
            ipc,
            ipc_socket: None,
            notifier: None,
        }
    }
}

/// `DASSIE_IPC_SOCKET_PATH`, or `$XDG_RUNTIME_DIR/dassie.sock`, or
/// `/run/dassie.sock`.
fn standalone_ipc_path(settings: &Settings, env: &Env) -> PathBuf {
    settings.ipc_socket_path.clone().unwrap_or_else(|| {
        env.path("XDG_RUNTIME_DIR").map_or_else(
            || PathBuf::from("/run/dassie.sock"),
            |dir| dir.join("dassie.sock"),
        )
    })
}

/// Binds both ports on every address, IPv6 and IPv4 where the system has
/// IPv6, IPv4 only where it does not.
fn bind_ports(ports: Ports) -> Result<(TcpListener, TcpListener), DaemonError> {
    let bind = |port: u16| {
        TcpListener::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
            .or_else(|_| TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))))
            .map_err(|error| DaemonError::Bind(format!("port {port}"), error))
    };
    Ok((bind(ports.http())?, bind(ports.https())?))
}

/// Runs the daemon until `shutdown`.
pub async fn run(
    prepared: Prepared,
    clock: Arc<dyn Clock>,
    rng: Box<dyn SharedRng>,
    policies: Policies,
    shutdown: CancellationToken,
) -> Result<(), DaemonError> {
    let Prepared {
        settings,
        store,
        runtime,
        http,
        https,
        ipc,
        ipc_socket,
        notifier,
    } = prepared;
    let parts = NodeParts {
        settings,
        store,
        runtime,
        clock,
        rng,
        policies,
    };
    let (node, listeners, tls) = match make_node(parts, (http, https, ipc)) {
        Ok(made) => made,
        Err(error) => {
            // A daemon that did not start leaves no socket behind.
            if let Some(socket) = ipc_socket {
                socket.discard();
            }
            return Err(error);
        }
    };
    let services = TaskTracker::new();
    start_services(&node, &services, &shutdown, listeners, tls);
    // Clients find the socket only now, when the daemon answers them and
    // handles signals; before, a CLI would hang or a SIGTERM go unhandled.
    let ipc_path = match ipc_socket.map(ipc::Unpublished::publish).transpose() {
        Ok(path) => path,
        Err(error) => {
            shutdown.cancel();
            services.close();
            services.wait().await;
            return Err(DaemonError::Bind("the IPC socket".into(), error));
        }
    };
    report_ready(&node, notifier.as_ref());
    shutdown.cancelled().await;
    tracing::info!(target: target::DAEMON, "shutting down");
    if let Some(notifier) = &notifier {
        let _ = notifier.notify("STOPPING=1");
    }
    services.close();
    if tokio::time::timeout(STOP_TIMEOUT, services.wait())
        .await
        .is_err()
    {
        tracing::warn!(target: target::DAEMON, "services did not stop in time");
    }
    drop(node);
    if let Some(path) = ipc_path {
        let _ = std::fs::remove_file(path);
    }
    tracing::info!(target: target::DAEMON, "stopped");
    Ok(())
}

/// What the node is made from, besides its sockets.
struct NodeParts {
    settings: Settings,
    store: Store,
    runtime: Runtime,
    clock: Arc<dyn Clock>,
    rng: Box<dyn SharedRng>,
    policies: Policies,
}

/// Adopts the sockets and makes the node and its TLS configuration; nothing
/// is served yet.
fn make_node(
    parts: NodeParts,
    sockets: (TcpListener, TcpListener, UnixListener),
) -> Result<(Arc<Node>, Listeners, Arc<rustls::ServerConfig>), DaemonError> {
    let (http, https, ipc) = sockets;
    let listeners = adopt(http, https, ipc).map_err(|e| DaemonError::Bind("sockets".into(), e))?;
    let http_port = listeners.0.local_addr().map_or(0, |address| address.port());
    let https_port = listeners.1.local_addr().map_or(0, |address| address.port());
    let node = Node::new(NodeOptions {
        store: parts.store,
        clock: parts.clock,
        rng: parts.rng,
        runtime: parts.runtime,
        http_port,
        https_port,
        acme_ca_file: parts.settings.acme_ca_file.clone(),
        policies: parts.policies,
    })
    .map_err(DaemonError::Node)?;
    let tls = tls::server_config(Arc::clone(node.certificate())).map_err(DaemonError::Tls)?;
    Ok((node, listeners, tls))
}

type Listeners = (
    tokio::net::TcpListener,
    tokio::net::TcpListener,
    tokio::net::UnixListener,
);

fn adopt(http: TcpListener, https: TcpListener, ipc: UnixListener) -> io::Result<Listeners> {
    http.set_nonblocking(true)?;
    https.set_nonblocking(true)?;
    ipc.set_nonblocking(true)?;
    Ok((
        tokio::net::TcpListener::from_std(http)?,
        tokio::net::TcpListener::from_std(https)?,
        tokio::net::UnixListener::from_std(ipc)?,
    ))
}

fn start_services(
    node: &Arc<Node>,
    services: &TaskTracker,
    shutdown: &CancellationToken,
    listeners: Listeners,
    tls: Arc<rustls::ServerConfig>,
) {
    // A listener cannot be cloned for a restart, so the accept loops share
    // theirs; they only return at shutdown anyway.
    let (http, https, ipc) = (
        Arc::new(listeners.0),
        Arc::new(listeners.1),
        Arc::new(listeners.2),
    );
    let (node_, shutdown_) = (Arc::clone(node), shutdown.clone());
    supervisor::spawn(services, "http", shutdown.clone(), move || {
        http::serve(
            Arc::clone(&node_),
            Arc::clone(&http),
            None,
            shutdown_.clone(),
        )
    });
    let (node_, shutdown_) = (Arc::clone(node), shutdown.clone());
    supervisor::spawn(services, "https", shutdown.clone(), move || {
        http::serve(
            Arc::clone(&node_),
            Arc::clone(&https),
            Some(Arc::clone(&tls)),
            shutdown_.clone(),
        )
    });
    let (node_, shutdown_) = (Arc::clone(node), shutdown.clone());
    supervisor::spawn(services, "ipc", shutdown.clone(), move || {
        ipc::serve(Arc::clone(&node_), Arc::clone(&ipc), shutdown_.clone())
    });
    let (node_, shutdown_) = (Arc::clone(node), shutdown.clone());
    supervisor::spawn(services, "acme", shutdown.clone(), move || {
        Arc::clone(&node_).run_renewal(shutdown_.clone())
    });
    let (node_, shutdown_) = (Arc::clone(node), shutdown.clone());
    supervisor::spawn(services, "sessions", shutdown.clone(), move || {
        prune_sessions(Arc::clone(&node_), shutdown_.clone())
    });
}

async fn prune_sessions(node: Arc<Node>, shutdown: CancellationToken) {
    loop {
        match node.prune_sessions() {
            Ok(0) => {}
            Ok(count) => tracing::info!(target: target::AUTH, count, "expired sessions deleted"),
            Err(error) => {
                tracing::warn!(target: target::AUTH, %error, "could not delete expired sessions")
            }
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(PRUNE_INTERVAL) => {}
        }
    }
}

fn report_ready(node: &Node, notifier: Option<&Notifier>) {
    let (http_port, https_port) = node.ports();
    tracing::info!(target: target::DAEMON, version = crate::VERSION, runtime = ?node.runtime(), http_port, https_port, "listening");
    if let Some(url) = node.setup_url() {
        tracing::warn!(target: target::DAEMON, "Node identity is not configured, visit {url}");
    }
    if let Some(notifier) = notifier
        && let Err(error) = notifier.notify("READY=1")
    {
        tracing::warn!(target: target::SYSTEMD, %error, "could not report readiness");
    }
}
