//! Starting and stopping the daemon (spec/areas/node.md#startup,
//! spec/areas/node.md#shutdown).
//!
//! In order: the environment is checked, the database opened and migrated,
//! the sockets taken from systemd or bound, the certificate loaded or made,
//! and the services started. Readiness is reported once the engine has
//! committed its first step, or at once on a node not set up yet, without
//! waiting on the network; the original waited for its first ACME check,
//! and in its standalone runtime ran that check before anything listened on
//! port 80 (RT-8). An engine that fails [`supervisor::START_ATTEMPTS`] times
//! before that ends the daemon with an error, so a release that cannot run
//! its engine fails its start (spec/areas/node.md#readiness). On SIGTERM it
//! stops accepting, lets requests finish, closes the database and removes
//! the socket it made (RT-7).
//!
//! The steps that adopt sockets run before the tokio runtime exists
//! ([`Prepared::new`]); the rest runs inside it ([`run`]).

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, UdpSocket};
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
use crate::node::{Node, NodeError, NodeOptions, PeerOptions, Policies, Runtime};
use crate::store::{Store, StoreError};
use crate::supervisor::{BeforeUp, Start};
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
    /// A service the node needs failed every attempt before the node was up.
    StartFailed {
        /// The service.
        service: &'static str,
        /// Why it last stopped.
        error: String,
    },
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
            Self::StartFailed { service, error } => write!(
                f,
                "the {service} service failed {} times while starting; last: {error}",
                supervisor::START_ATTEMPTS
            ),
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
    /// The one address the standalone runtime listens on, instead of all:
    /// the development environment keeps its nodes, whose keys are public,
    /// on loopback.
    pub listen: Option<IpAddr>,
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
    extras: Extras,
    /// The IPC socket the daemon made, published once it is ready.
    ipc_socket: Option<ipc::Unpublished>,
    notifier: Option<Notifier>,
}

/// What the daemon may have beside the three sockets it needs.
#[derive(Debug, Default)]
struct Extras {
    /// The UDP socket for QUIC, where there is one.
    quic: Option<UdpSocket>,
    /// For tests: where the QUIC endpoint goes once it is made.
    hand_out: Option<tokio::sync::oneshot::Sender<Arc<crate::quic::Quic>>>,
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
        let (http, https, ipc, quic, ipc_socket) = match runtime {
            Runtime::Systemd => {
                let sockets = systemd::take_sockets(env).map_err(DaemonError::Activation)?;
                (sockets.http, sockets.https, sockets.ipc, sockets.quic, None)
            }
            Runtime::Standalone => {
                let ports = match options.ports {
                    Some(ports) => ports,
                    None => store.config().map_err(DaemonError::Store)?.ports,
                };
                let (http, https) = bind_ports(ports, options.listen)?;
                let quic = bind_quic(&https);
                let path = standalone_ipc_path(&settings, env);
                let (ipc, unpublished) = ipc::bind_unpublished(&path)
                    .map_err(|e| DaemonError::Bind(path.display().to_string(), e))?;
                (http, https, ipc, quic, Some(unpublished))
            }
        };
        Ok(Self {
            settings,
            store,
            runtime,
            http,
            https,
            ipc,
            extras: Extras {
                quic,
                hand_out: None,
            },
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
            extras: Extras::default(),
            ipc_socket: None,
            notifier: None,
        }
    }

    /// For tests: the QUIC endpoint, once it is made, goes to `to`.
    #[must_use]
    pub fn handing_out(mut self, to: tokio::sync::oneshot::Sender<Arc<crate::quic::Quic>>) -> Self {
        self.extras.hand_out = Some(to);
        self
    }

    /// For tests: QUIC on `socket`, as if systemd had passed it.
    #[must_use]
    pub fn with_quic(mut self, socket: UdpSocket) -> Self {
        self.extras.quic = Some(socket);
        self
    }

    /// For tests: readiness goes to `notifier`, as to systemd's.
    #[must_use]
    pub fn notifying(mut self, notifier: Notifier) -> Self {
        self.notifier = Some(notifier);
        self
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

/// Binds both ports on `listen`, or on every address: IPv6 and IPv4 where
/// the system has IPv6, IPv4 only where it does not.
fn bind_ports(
    ports: Ports,
    listen: Option<IpAddr>,
) -> Result<(TcpListener, TcpListener), DaemonError> {
    let bind = |port: u16| {
        match listen {
            Some(address) => TcpListener::bind(SocketAddr::from((address, port))),
            None => TcpListener::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)))
                .or_else(|_| TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)))),
        }
        .map_err(|error| DaemonError::Bind(format!("port {port}"), error))
    };
    Ok((bind(ports.http())?, bind(ports.https())?))
}

/// The UDP socket for QUIC, on the HTTPS listener's address and port. A node
/// that cannot have one takes the peer protocol over HTTPS only.
fn bind_quic(https: &TcpListener) -> Option<UdpSocket> {
    let address = https.local_addr().ok()?;
    match UdpSocket::bind(address) {
        Ok(socket) => Some(socket),
        Err(error) => {
            tracing::warn!(target: target::DAEMON, %address, %error, "no UDP socket for QUIC: peers reach this node over HTTPS only");
            None
        }
    }
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
        extras,
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
    let (node, listeners, tls) = match make_node(parts, (http, https, ipc), extras) {
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
    let outcome = match until_up(&node, &shutdown).await {
        Some(Start::Up) => {
            report_ready(&node, notifier.as_ref());
            shutdown.cancelled().await;
            Ok(())
        }
        Some(Start::Failed { service, error }) => {
            shutdown.cancel();
            Err(DaemonError::StartFailed { service, error })
        }
        Some(Start::Starting) | None => Ok(()),
    };
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
    outcome
}

/// Waits until the node is up or its start failed; `None` if shutdown came
/// first.
async fn until_up(node: &Node, shutdown: &CancellationToken) -> Option<Start> {
    let mut start = node.services().start();
    tokio::select! {
        () = shutdown.cancelled() => None,
        start = start.wait_for(|start| *start != Start::Starting) => {
            start.ok().map(|start| start.clone())
        }
    }
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
    extras: Extras,
) -> Result<(Arc<Node>, Listeners, Arc<rustls::ServerConfig>), DaemonError> {
    let (http, https, ipc) = sockets;
    let listeners = adopt(http, https, ipc).map_err(|e| DaemonError::Bind("sockets".into(), e))?;
    let http_port = listeners.0.local_addr().map_or(0, |address| address.port());
    let https_port = listeners.1.local_addr().map_or(0, |address| address.port());
    let settings = &parts.settings;
    let policies = if settings.dev_timings {
        Policies {
            peer: Policies::DEV.peer,
            settlement_interval_ms: Policies::DEV.settlement_interval_ms,
            settlement_request_every_ms: Policies::DEV.settlement_request_every_ms,
            ..parts.policies
        }
    } else {
        parts.policies
    };
    let node = Node::new(NodeOptions {
        store: parts.store,
        clock: parts.clock,
        rng: parts.rng,
        runtime: parts.runtime,
        http_port,
        https_port,
        acme_ca_file: settings.acme_ca_file.clone(),
        policies,
        peer: peer_options(settings),
        dev_wallet_dir: settings.dev_wallet_dir.clone(),
        public_names: settings.public_names.clone(),
    })
    .map_err(DaemonError::Node)?;
    if let Some(socket) = extras.quic {
        let address = socket.local_addr().ok();
        match crate::quic::Quic::new(socket, node.identity(), node.allows_private_peers()) {
            Ok(quic) => {
                let quic = Arc::new(quic);
                node.attach_quic(Arc::clone(&quic));
                if let Some(to) = extras.hand_out {
                    let _ = to.send(quic);
                }
            }
            Err(error) => {
                tracing::warn!(target: target::DAEMON, ?address, %error, "no QUIC endpoint: peers reach this node over HTTPS only");
            }
        }
    }
    if let Some(dir) = &settings.dev_wallet_dir {
        tracing::warn!(target: target::HTTP, dir = %dir.display(), "serving a development build of the wallet");
    }
    let bound = [bound_ip(&listeners.0), bound_ip(&listeners.1)];
    for flag in exposed_dev_flags(settings, &bound) {
        tracing::warn!(target: target::DAEMON, "{} is set: {}. It is for development; unset it on a public node", flag.variable, flag.effect);
    }
    if settings.bootstrap_nodes.is_empty() {
        tracing::warn!(target: target::PEER, "no bootstrap nodes: the node runs alone (set DASSIE_BOOTSTRAP_NODES)");
    }
    let tls = tls::server_config(Arc::clone(node.certificate())).map_err(DaemonError::Tls)?;
    Ok((node, listeners, tls))
}

/// A development switch: its variable, and what it does to the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DevFlag {
    variable: &'static str,
    effect: &'static str,
}

const PEER_ALLOW_PRIVATE: DevFlag = DevFlag {
    variable: "DASSIE_PEER_ALLOW_PRIVATE",
    effect: "peers on private and loopback addresses are reachable",
};

const DEV_TIMINGS: DevFlag = DevFlag {
    variable: "DASSIE_DEV_TIMINGS",
    effect: "the peer protocol runs on dev timers, polling and settling every second",
};

/// The development switches that are set, if the node listens beyond
/// loopback. On loopback, as in dev and the tests, they are expected and
/// none are returned. An unspecified address (`0.0.0.0`, `::`) listens on
/// every interface, so it is beyond loopback.
fn exposed_dev_flags(settings: &Settings, bound: &[IpAddr]) -> Vec<DevFlag> {
    if bound.iter().all(|ip| ip.to_canonical().is_loopback()) {
        return Vec::new();
    }
    [
        (settings.peer_allow_private, PEER_ALLOW_PRIVATE),
        (settings.dev_timings, DEV_TIMINGS),
    ]
    .into_iter()
    .filter_map(|(set, flag)| set.then_some(flag))
    .collect()
}

/// The address a listener is bound to, whether `--listen`, the standalone
/// default or systemd chose it. One that cannot be read counts as every
/// address.
fn bound_ip(listener: &tokio::net::TcpListener) -> IpAddr {
    listener
        .local_addr()
        .map_or(IpAddr::V6(Ipv6Addr::UNSPECIFIED), |address| address.ip())
}

/// The peer protocol's settings from the environment.
fn peer_options(settings: &Settings) -> PeerOptions {
    PeerOptions {
        bootstrap: settings
            .bootstrap_nodes
            .iter()
            .map(|node| dassie_peer::Bootstrap {
                id: node.id,
                url: node.url.clone(),
                key: node.public_key,
            })
            .collect(),
        ca_file: settings.peer_ca_file.clone(),
        allow_private: settings.peer_allow_private,
    }
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
    let spawn =
        |name, service| supervise(node, services, shutdown, name, BeforeUp::Restart, service);
    spawn(
        "http",
        Box::new(move |node, shutdown| {
            Box::pin(http::serve(node, Arc::clone(&http), None, shutdown))
        }),
    );
    spawn(
        "https",
        Box::new(move |node, shutdown| {
            Box::pin(http::serve(
                node,
                Arc::clone(&https),
                Some(Arc::clone(&tls)),
                shutdown,
            ))
        }),
    );
    spawn(
        "ipc",
        Box::new(move |node, shutdown| Box::pin(ipc::serve(node, Arc::clone(&ipc), shutdown))),
    );
    if node.quic().is_some() {
        spawn(
            "quic",
            Box::new(|node, shutdown| {
                Box::pin(async move {
                    if let Some(quic) = node.quic().cloned() {
                        let handler = Arc::new(http::peer::QuicHandler(node));
                        quic.serve(handler, shutdown).await;
                    }
                })
            }),
        );
    }
    spawn(
        "acme",
        Box::new(|node, shutdown| Box::pin(node.run_renewal(shutdown))),
    );
    spawn(
        "sessions",
        Box::new(|node, shutdown| Box::pin(prune_sessions(node, shutdown))),
    );
    // The node is up once the engine works.
    supervise(
        node,
        services,
        shutdown,
        "engine",
        BeforeUp::FailStart,
        Box::new(|node, shutdown| Box::pin(crate::engine::run(node, shutdown))),
    );
    spawn(
        "rates",
        Box::new(|node, shutdown| Box::pin(node.run_rates(shutdown))),
    );
}

/// A service: started again, with the node and the shutdown token, after
/// it stops.
type Service = Box<
    dyn Fn(
            Arc<Node>,
            CancellationToken,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Runs `service` under the supervisor, as `name`.
fn supervise(
    node: &Arc<Node>,
    services: &TaskTracker,
    shutdown: &CancellationToken,
    name: &'static str,
    before_up: BeforeUp,
    service: Service,
) {
    let (node_, shutdown_) = (Arc::clone(node), shutdown.clone());
    supervisor::spawn(
        services,
        node.services(),
        name,
        before_up,
        shutdown.clone(),
        move || service(Arc::clone(&node_), shutdown_.clone()),
    );
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
    if !node.is_set_up() {
        // Not the URL itself: whoever opens it first claims the node, and the
        // journal is readable by groups that do not administer it (adm,
        // systemd-journal). The IPC socket, which only `dassie-users` reach,
        // hands it out.
        tracing::warn!(target: target::DAEMON, "Node identity is not configured; `sudo dassie setup-url` prints the setup URL");
    }
    if let Some(notifier) = notifier
        && let Err(error) = notifier.notify("READY=1")
    {
        tracing::warn!(target: target::SYSTEMD, %error, "could not report readiness");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [&str; 2] = ["DASSIE_PEER_ALLOW_PRIVATE", "DASSIE_DEV_TIMINGS"];

    /// The variables `exposed_dev_flags` names, with `flags` set to 1 and
    /// the listeners bound to `bound`.
    fn exposed(flags: &[&'static str], bound: &[&str]) -> Vec<&'static str> {
        let mut pairs = vec![("DASSIE_STATE_DIRECTORY", "/var/lib/dassie")];
        pairs.extend(flags.iter().map(|flag| (*flag, "1")));
        let settings = Settings::from_env(&Env::from_pairs(pairs)).unwrap();
        let bound: Vec<IpAddr> = bound.iter().map(|ip| ip.parse().unwrap()).collect();
        exposed_dev_flags(&settings, &bound)
            .into_iter()
            .map(|flag| flag.variable)
            .collect()
    }

    #[test]
    fn dev_flags_on_loopback_are_quiet() {
        assert!(exposed(&BOTH, &["127.0.0.1", "127.0.0.1"]).is_empty());
        assert!(exposed(&BOTH, &["::1", "::1"]).is_empty());
        assert!(exposed(&BOTH, &["::ffff:127.0.0.1", "127.0.0.2"]).is_empty());
    }

    #[test]
    fn dev_flags_beyond_loopback_are_named() {
        assert_eq!(exposed(&BOTH, &["0.0.0.0", "0.0.0.0"]), BOTH);
        assert_eq!(exposed(&BOTH, &["::", "::"]), BOTH);
        assert_eq!(
            exposed(&["DASSIE_DEV_TIMINGS"], &["203.0.113.7", "203.0.113.7"]),
            ["DASSIE_DEV_TIMINGS"]
        );
        // One listener beyond loopback is enough.
        assert_eq!(
            exposed(
                &["DASSIE_PEER_ALLOW_PRIVATE"],
                &["127.0.0.1", "192.168.1.2"]
            ),
            ["DASSIE_PEER_ALLOW_PRIVATE"]
        );
    }

    #[test]
    fn no_dev_flags_are_quiet() {
        assert!(exposed(&[], &["0.0.0.0", "::"]).is_empty());
        assert!(exposed(&[], &["203.0.113.7", "203.0.113.7"]).is_empty());
    }

    #[tokio::test]
    async fn the_bound_address_is_read_from_the_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        assert_eq!(bound_ip(&listener), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
}
