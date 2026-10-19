//! The node's state, shared by its listeners and services.
//!
//! [`Node`] holds what the database holds, cached where every request needs
//! it (the configuration, the node key, the certificate), plus what lives
//! only in memory: the setup token, the login rate limiter, the ACME
//! challenge tokens and the renewal schedule. The decisions are
//! `dassie-node-core`'s; this carries them out.

mod access;
mod certificates;
mod rates;
mod settings;

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use dassie_identity::NodeKey;
use dassie_node_core::acme::{ChallengeTokens, Renewal, RenewalPolicy};
use dassie_node_core::config::NodeConfig;
use dassie_node_core::connections::{ConnectionLimits, Refused};
use dassie_node_core::rate_limit::{ClientKey, RateLimitPolicy, RateLimiter};
use dassie_node_core::session::{SessionId, SessionPolicy};
use dassie_node_core::setup::SetupToken;
use dassie_node_core::urls;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, broadcast, watch};

pub use access::{AuthError, NewSession, SessionView};
pub use certificates::{ObtainError, ObtainRequest};
pub use settings::SettingsError;

use crate::acme::AcmeClient;
use crate::clock::{Clock, SharedRng};
use crate::engine::Engine;
use crate::peer_client::PeerClient;
use crate::slots::{ClientSlot, ClientSlots};
use crate::store::{Store, StoreError};
use crate::tls::{CertificateSlot, TlsError};

/// How the daemon got its sockets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    /// It bound them itself, on the configured ports.
    Standalone,
    /// systemd passed them in.
    Systemd,
}

/// The limits and lifetimes the node applies; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct Policies {
    /// Sessions.
    pub sessions: SessionPolicy,
    /// Login and setup attempts.
    pub login_attempts: RateLimitPolicy,
    /// Certificate renewal.
    pub renewal: RenewalPolicy,
    /// Most RPC WebSockets open at once.
    pub max_websockets: usize,
    /// Most RPC WebSockets without a session open at once: fewer than
    /// `max_websockets`, so that sockets with a session always find room.
    pub max_anonymous_websockets: usize,
    /// Most RPC WebSockets without a session open at once from one client
    /// (an IPv4 address or an IPv6 /56). A wallet page holds one.
    pub anonymous_websockets_per_client: u32,
    /// A browser's RPC socket without a session closes after this long,
    /// whatever it sends: it only asks for the node's basic state.
    pub anonymous_socket_lifetime_ms: u64,
    /// A browser's RPC socket with no call in flight closes after this long
    /// without a message either way.
    pub socket_idle_ms: u64,
    /// Most connections one client (an IPv4 address or an IPv6 /56) holds
    /// on each listener at once. Our peer client keeps at most 16 requests
    /// in flight to one node and reuses its idle connections (4 at most),
    /// so four nodes behind one address fit; a browser opens about 6.
    pub http_connections_per_client: u32,
    /// Time to send a request body to the HTTP API.
    pub body_timeout_ms: u64,
    /// Most BTP clients connected at once.
    pub max_btp_connections: usize,
    /// Time a BTP client has to authenticate after connecting.
    pub btp_auth_timeout_ms: u64,
    /// Failed BTP authentications, per client address.
    pub btp_auth_attempts: RateLimitPolicy,
    /// The ledger's bounds.
    pub ledger: dassie_ledger::Config,
    /// The connector's bounds and timing.
    pub connector: dassie_connector::Config,
    /// The peer protocol's bounds and timers.
    pub peer: dassie_peer::Config,
    /// The credit a junior extends to the node that accepted its peering
    /// request, in its ledger's units: 1.1 units at scale 9 on the test
    /// network (spec/areas/settlement.md#credit, D20). At least what survives
    /// a crash (D25).
    pub peer_credit: u64,
    /// The credit a senior extends to the node whose peering request it
    /// accepted, at first: nothing, so a newcomer pays in advance until the
    /// peering earned it more (D20). Otherwise at least what survives a
    /// crash, as `peer_credit`.
    pub senior_credit: u64,
    /// How often peers are checked, to ask those near the credit we extend
    /// to settle (D23).
    pub settlement_interval_ms: u64,
    /// How often a peer that did not settle since it was last asked is
    /// asked again, at most.
    pub settlement_request_every_ms: u64,
    /// Open Payments' incoming payments.
    pub incoming: crate::engine::IncomingPolicy,
    /// How often exchange rates are fetched (D7).
    pub rates_every_ms: u64,
}

impl Policies {
    /// The production values.
    pub const DEFAULT: Self = Self {
        sessions: SessionPolicy::DEFAULT,
        login_attempts: RateLimitPolicy::DEFAULT,
        renewal: RenewalPolicy::DEFAULT,
        max_websockets: 256,
        max_anonymous_websockets: 128,
        anonymous_websockets_per_client: 4,
        anonymous_socket_lifetime_ms: 60_000,
        socket_idle_ms: 10 * 60 * 1_000,
        http_connections_per_client: 64,
        body_timeout_ms: 10_000,
        max_btp_connections: 64,
        btp_auth_timeout_ms: 10_000,
        btp_auth_attempts: RateLimitPolicy::DEFAULT,
        ledger: dassie_ledger::Config {
            max_accounts: 4096,
            max_pending: 25_408,
            max_journal: 1024,
        },
        // Every BTP client, the 8 local endpoints and every peer can fill
        // their share of the packet table at once: 72 × 60 + 8 × 512 fits
        // its 8448, and the ledger has three transfers for each (across
        // ledgers, with our fee), and 64 more.
        connector: dassie_connector::Config {
            max_pending: 8448,
            max_pending_per_endpoint: 60,
            ..dassie_connector::Config::DEFAULT
        },
        peer: dassie_peer::Config::DEFAULT,
        peer_credit: 1_100_000_000,
        senior_credit: 0,
        settlement_interval_ms: 4_000,
        settlement_request_every_ms: 60_000,
        incoming: crate::engine::IncomingPolicy::DEFAULT,
        rates_every_ms: 60_000,
    };

    /// The production values with the peer protocol's dev timers, for what
    /// runs on a real clock: dev environments and the end-to-end tests. The
    /// network simulation runs [`Policies::DEFAULT`].
    pub const DEV: Self = Self {
        peer: dassie_peer::Config::DEV,
        settlement_interval_ms: 1_000,
        settlement_request_every_ms: 5_000,
        ..Self::DEFAULT
    };

    /// The places for RPC sockets without a session.
    #[must_use]
    pub const fn anonymous_socket_limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: self.max_anonymous_websockets,
            max_per_client: self.anonymous_websockets_per_client,
        }
    }
}

// Sockets with a session always find room beside those without.
const _: () =
    assert!(Policies::DEFAULT.max_anonymous_websockets < Policies::DEFAULT.max_websockets);
const _: () = assert!(Policies::DEV.max_anonymous_websockets < Policies::DEV.max_websockets);

/// What the node is made from.
pub struct NodeOptions {
    /// The database.
    pub store: Store,
    /// Wall-clock time.
    pub clock: Arc<dyn Clock>,
    /// Randomness for tokens.
    pub rng: Box<dyn SharedRng>,
    /// How the sockets came.
    pub runtime: Runtime,
    /// The port the HTTP listener actually has.
    pub http_port: u16,
    /// The port the HTTPS listener actually has.
    pub https_port: u16,
    /// An extra root for the ACME client (tests).
    pub acme_ca_file: Option<PathBuf>,
    /// Limits and lifetimes.
    pub policies: Policies,
    /// The peer protocol's settings.
    pub peer: PeerOptions,
    /// A development build of the wallet to serve instead of the embedded
    /// one.
    pub dev_wallet_dir: Option<PathBuf>,
}

/// The peer protocol's settings, from the environment.
#[derive(Clone, Debug, Default)]
pub struct PeerOptions {
    /// The bootstrap nodes.
    pub bootstrap: Vec<dassie_peer::Bootstrap>,
    /// Extra roots for peers' certificates (dev and tests).
    pub ca_file: Option<PathBuf>,
    /// Whether peers may be at private addresses (dev and tests).
    pub allow_private: bool,
}

/// Most inbound peer requests waiting on the engine at once.
const MAX_PEER_REQUESTS: usize = 256;

/// Requests without authentication, per client address: they are for tools
/// and debugging, since nodes authenticate every request.
const PEER_ANONYMOUS: RateLimitPolicy = RateLimitPolicy {
    client_burst: 20,
    client_refill_ms: 1_000,
    client_slots: 1024,
};

/// The node's side of the peer protocol, beside the engine.
struct PeerSide {
    bootstrap: Vec<dassie_peer::Bootstrap>,
    /// Made on first use, as the ACME client is: a machine without the
    /// system's roots still serves, and says why its peer requests fail.
    client: std::sync::OnceLock<Result<PeerClient, String>>,
    allow_private: bool,
    permits: Arc<Semaphore>,
    limiter: Mutex<RateLimiter>,
}

impl PeerSide {
    fn new(options: PeerOptions) -> Result<Self, NodeError> {
        let client = std::sync::OnceLock::new();
        if options.ca_file.is_some() {
            // A dev or test CA's root that cannot be read is a mistake in
            // the environment: refuse to start, rather than fail later.
            let made = PeerClient::new(options.ca_file.as_deref(), options.allow_private)
                .map_err(NodeError::Peer)?;
            let _ = client.set(Ok(made));
        }
        Ok(Self {
            bootstrap: options.bootstrap,
            client,
            allow_private: options.allow_private,
            permits: Arc::new(Semaphore::new(MAX_PEER_REQUESTS)),
            limiter: Mutex::new(RateLimiter::new(PEER_ANONYMOUS)),
        })
    }
}

/// The node could not start.
#[derive(Debug)]
pub enum NodeError {
    /// The database failed.
    Store(StoreError),
    /// The stored certificate is unusable and none could be made.
    Tls(TlsError),
    /// The ACME client could not be set up (a bad `DASSIE_ACME_CA_FILE`).
    Acme(String),
    /// The peer client could not be set up (a bad `DASSIE_PEER_CA_FILE`).
    Peer(String),
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(f),
            Self::Tls(error) => error.fmt(f),
            Self::Acme(error) => write!(f, "ACME client: {error}"),
            Self::Peer(error) => write!(f, "peer client: {error}"),
        }
    }
}

impl std::error::Error for NodeError {}

impl From<StoreError> for NodeError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Sessions to end on every socket that holds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revocation {
    /// One session.
    Session(SessionId),
    /// Every session but one, or all.
    AllExcept(Option<SessionId>),
    /// Some sessions ended (expired, or evicted for newer ones) without
    /// being named: each socket checks its own.
    Recheck,
}

impl Revocation {
    /// Whether this ends `session`.
    #[must_use]
    pub fn ends(&self, session: &SessionId) -> bool {
        match self {
            Self::Session(id) => id == session,
            Self::AllExcept(keep) => keep.as_ref() != Some(session),
            Self::Recheck => false,
        }
    }
}

/// The node.
pub struct Node {
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
    rng: Arc<Mutex<Box<dyn SharedRng>>>,
    config: watch::Sender<NodeConfig>,
    /// Held across each read, change and write of the configuration, so
    /// that concurrent changes cannot undo each other.
    config_writer: Mutex<()>,
    node_key: watch::Sender<Option<Arc<NodeKey>>>,
    engine: watch::Sender<Option<Engine>>,
    btp_connections: Arc<Semaphore>,
    btp_auth_limiter: Mutex<RateLimiter>,
    setup_token: SetupToken,
    policies: Policies,
    login_limiter: Mutex<RateLimiter>,
    challenges: Mutex<ChallengeTokens>,
    certificate: Arc<CertificateSlot>,
    renewal: Mutex<Renewal>,
    renewal_wake: Notify,
    /// Made on first use: the system's root store is read then, so a
    /// machine without one still serves, and says so when ordering.
    acme: std::sync::OnceLock<Result<AcmeClient, String>>,
    acme_ca_file: Option<PathBuf>,
    acme_order: tokio::sync::Mutex<()>,
    acme_last_error: Mutex<Option<String>>,
    revocations: broadcast::Sender<Revocation>,
    websockets: Arc<Semaphore>,
    /// Places for RPC sockets without a session, within `websockets`.
    anonymous_sockets: ClientSlots,
    runtime: Runtime,
    http_port: u16,
    https_port: u16,
    started_at_ms: u64,
    peer: PeerSide,
    services: Arc<crate::supervisor::Registry>,
    logs: Arc<crate::log::LogRing>,
    live_streams: Arc<Semaphore>,
    dev_wallet_dir: Option<PathBuf>,
    stats_cache: crate::http::stats::StatsCache,
    open_payments: crate::open_payments::State,
    /// Wakes the rate fetcher.
    rates_wake: Notify,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

/// Locks a mutex, carrying on past a panic of another holder: none of the
/// node's mutexes guard an update that a panic could leave half done.
pub(crate) fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Most live streams the node serves at once, over all connections.
pub const MAX_LIVE_STREAMS: usize = 64;

impl Node {
    /// Loads the node from its database, making a self-signed certificate if
    /// the stored one won't do.
    pub fn new(options: NodeOptions) -> Result<Arc<Self>, NodeError> {
        let NodeOptions {
            store,
            clock,
            mut rng,
            runtime,
            http_port,
            https_port,
            acme_ca_file,
            policies,
            peer,
            dev_wallet_dir,
        } = options;
        let peer = PeerSide::new(peer)?;
        let now_ms = clock.now_ms();
        let config = store.config()?;
        let node_key = store.node_key()?.map(Arc::new);
        let certificate = certificates::initial(&store, config.hostname.as_ref(), now_ms)?;
        let acme = std::sync::OnceLock::new();
        if acme_ca_file.is_some() {
            // A test CA's root that cannot be read is a mistake in the
            // environment: refuse to start, rather than fail the first order.
            let client = AcmeClient::new(acme_ca_file.as_deref())
                .map_err(|e| NodeError::Acme(e.to_string()))?;
            let _ = acme.set(Ok(client));
        }
        let setup_token = if node_key.is_some() {
            // Never shown, and refused: the node is set up.
            SetupToken::generate(rng.as_mut())
        } else if let Some(bytes) = store.setup_token()? {
            SetupToken::from_bytes(bytes)
        } else {
            let token = SetupToken::generate(rng.as_mut());
            store.set_setup_token(token.as_bytes())?;
            token
        };
        Ok(Arc::new(Self {
            store: Arc::new(store),
            rng: Arc::new(Mutex::new(rng)),
            config: watch::Sender::new(config),
            config_writer: Mutex::new(()),
            node_key: watch::Sender::new(node_key),
            engine: watch::Sender::new(None),
            btp_connections: Arc::new(Semaphore::new(policies.max_btp_connections)),
            btp_auth_limiter: Mutex::new(RateLimiter::new(policies.btp_auth_attempts)),
            setup_token,
            login_limiter: Mutex::new(RateLimiter::new(policies.login_attempts)),
            challenges: Mutex::new(ChallengeTokens::default()),
            certificate: Arc::new(CertificateSlot::new(certificate)),
            renewal: Mutex::new(Renewal::new(policies.renewal, now_ms)),
            renewal_wake: Notify::new(),
            acme,
            acme_ca_file,
            acme_order: tokio::sync::Mutex::new(()),
            acme_last_error: Mutex::new(None),
            revocations: broadcast::Sender::new(16),
            websockets: Arc::new(Semaphore::new(policies.max_websockets)),
            anonymous_sockets: ClientSlots::new(policies.anonymous_socket_limits()),
            policies,
            runtime,
            http_port,
            https_port,
            started_at_ms: now_ms,
            services: Arc::new(crate::supervisor::Registry::new(Arc::clone(&clock))),
            clock,
            peer,
            logs: crate::log::ring(),
            live_streams: Arc::new(Semaphore::new(MAX_LIVE_STREAMS)),
            dev_wallet_dir,
            stats_cache: crate::http::stats::StatsCache::default(),
            open_payments: crate::open_payments::State::default(),
            rates_wake: Notify::new(),
        }))
    }

    /// What Open Payments keeps beside the database.
    #[must_use]
    pub const fn open_payments(&self) -> &crate::open_payments::State {
        &self.open_payments
    }

    /// The bootstrap nodes.
    #[must_use]
    pub fn bootstrap(&self) -> Vec<dassie_peer::Bootstrap> {
        self.peer.bootstrap.clone()
    }

    /// The client for peer requests, made on first use; `None` when it
    /// cannot be, for want of roots to trust.
    #[must_use]
    pub fn peer_client(&self) -> Option<&PeerClient> {
        let allow_private = self.peer.allow_private;
        let made = self.peer.client.get_or_init(|| {
            let made = PeerClient::new(None, allow_private);
            if let Err(error) = &made {
                tracing::error!(target: crate::log::target::PEER, %error, "no peer requests can be sent");
            }
            made
        });
        made.as_ref().ok()
    }

    /// A place for one more inbound peer request, if any is left.
    #[must_use]
    pub fn peer_permit(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.peer.permits).try_acquire_owned().ok()
    }

    /// Whether `client` may send another unauthenticated or failing peer
    /// request now.
    #[must_use]
    pub fn peer_limited(&self, client: IpAddr) -> bool {
        let now = self.now_ms();
        lock(&self.peer.limiter)
            .peek(ClientKey::from_ip(client), now)
            .is_err()
    }

    /// Charges `client` for an unauthenticated or failed peer request.
    pub fn peer_charge(&self, client: IpAddr) {
        let now = self.now_ms();
        let _ = lock(&self.peer.limiter).check(ClientKey::from_ip(client), now);
    }

    /// The configuration as it changes.
    #[must_use]
    pub fn subscribe_config(&self) -> watch::Receiver<NodeConfig> {
        self.config.subscribe()
    }

    /// What our link state says about us: the public URL, once there is a
    /// hostname, and the alias.
    #[must_use]
    pub fn own_info(&self) -> dassie_peer::OwnInfo {
        let config = self.config();
        dassie_peer::OwnInfo {
            url: dassie_node_core::urls::node_url(config.hostname.as_ref(), self.ports().1),
            alias: config.alias.as_str().to_owned(),
        }
    }

    /// Now, by the node's clock.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// The current configuration.
    #[must_use]
    pub fn config(&self) -> NodeConfig {
        self.config.borrow().clone()
    }

    /// The node key, once set up.
    #[must_use]
    pub fn node_key(&self) -> Option<Arc<NodeKey>> {
        self.node_key.borrow().clone()
    }

    /// The node key as it changes: `None` until setup.
    #[must_use]
    pub fn identity(&self) -> watch::Receiver<Option<Arc<NodeKey>>> {
        self.node_key.subscribe()
    }

    /// The ledger's engine, while it runs.
    #[must_use]
    pub fn engine(&self) -> Option<Engine> {
        self.engine.borrow().clone()
    }

    pub(crate) fn publish_engine(&self, engine: Option<Engine>) {
        self.engine.send_replace(engine);
    }

    /// The node's clock.
    #[must_use]
    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.clock)
    }

    /// Runs `f` with the node's RNG.
    pub fn with_rng<T>(&self, f: impl FnOnce(&mut dyn SharedRng) -> T) -> T {
        f(lock(&self.rng).as_mut())
    }

    /// The node's RNG, as one a task can own.
    #[must_use]
    pub fn shared_rng(&self) -> Box<dyn SharedRng> {
        Box::new(LockedRng(Arc::clone(&self.rng)))
    }

    /// A place for one more BTP client, if any is left.
    #[must_use]
    pub fn btp_permit(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.btp_connections).try_acquire_owned().ok()
    }

    /// Whether `client` failed to authenticate BTP connections too often to
    /// try again now.
    #[must_use]
    pub fn btp_auth_blocked(&self, client: std::net::IpAddr) -> bool {
        let client = dassie_node_core::rate_limit::ClientKey::from_ip(client);
        lock(&self.btp_auth_limiter)
            .peek(client, self.now_ms())
            .is_err()
    }

    /// Counts a failed BTP authentication from `client`.
    pub fn btp_auth_failed(&self, client: std::net::IpAddr) {
        let client = dassie_node_core::rate_limit::ClientKey::from_ip(client);
        let _ = lock(&self.btp_auth_limiter).check(client, self.now_ms());
    }

    /// Whether the node has an identity.
    #[must_use]
    pub fn is_set_up(&self) -> bool {
        self.node_key().is_some()
    }

    /// Where to complete setup, while that is still possible.
    #[must_use]
    pub fn setup_url(&self) -> Option<String> {
        if self.is_set_up() {
            return None;
        }
        let config = self.config.borrow();
        Some(urls::setup_url(
            config.hostname.as_ref(),
            self.https_port,
            &self.setup_token.to_hex(),
        ))
    }

    /// The certificate being served.
    #[must_use]
    pub fn certificate(&self) -> &Arc<CertificateSlot> {
        &self.certificate
    }

    /// The http-01 key authorization for `token`, if one is being served.
    #[must_use]
    pub fn challenge(&self, token: &str) -> Option<String> {
        lock(&self.challenges)
            .lookup(token, self.now_ms())
            .map(str::to_string)
    }

    /// Sessions ended from now on.
    #[must_use]
    pub fn revocations(&self) -> broadcast::Receiver<Revocation> {
        self.revocations.subscribe()
    }

    /// How the sockets came.
    #[must_use]
    pub const fn runtime(&self) -> Runtime {
        self.runtime
    }

    /// The listeners' ports: HTTP, then HTTPS.
    #[must_use]
    pub const fn ports(&self) -> (u16, u16) {
        (self.http_port, self.https_port)
    }

    /// The services and what became of them.
    #[must_use]
    pub const fn services(&self) -> &Arc<crate::supervisor::Registry> {
        &self.services
    }

    /// A place for one more live stream, if any is left.
    #[must_use]
    pub fn live_stream_permit(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.live_streams).try_acquire_owned().ok()
    }

    /// The development build of the wallet, if it serves one.
    #[must_use]
    pub fn dev_wallet_dir(&self) -> Option<&std::path::Path> {
        self.dev_wallet_dir.as_deref()
    }

    pub(crate) const fn stats_cache(&self) -> &crate::http::stats::StatsCache {
        &self.stats_cache
    }

    /// The node's last log lines.
    #[must_use]
    pub const fn logs(&self) -> &Arc<crate::log::LogRing> {
        &self.logs
    }

    /// When the node started, in ms since the Unix epoch.
    #[must_use]
    pub const fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }

    /// A place for one more RPC WebSocket, if any is left.
    #[must_use]
    pub fn websocket_permit(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.websockets).try_acquire_owned().ok()
    }

    /// A place for one more RPC WebSocket without a session from `client`,
    /// beside its [`websocket_permit`](Self::websocket_permit).
    ///
    /// # Errors
    ///
    /// All such places are taken, or the client has its share.
    pub fn anonymous_socket_slot(&self, client: IpAddr) -> Result<ClientSlot, Refused> {
        self.anonymous_sockets.take(self.counted_client(client))
    }

    /// Who a connection from `address` counts against, for the limits per
    /// client; `None` counts in the totals only. That is loopback where
    /// private peers are allowed: dev environments and tests run many
    /// nodes and their tools on 127.0.0.1, and production never allows
    /// them.
    #[must_use]
    pub fn counted_client(&self, address: IpAddr) -> Option<ClientKey> {
        let address = address.to_canonical();
        if self.peer.allow_private && address.is_loopback() {
            return None;
        }
        Some(ClientKey::from_ip(address))
    }

    /// The limits and lifetimes in force.
    #[must_use]
    pub const fn policies(&self) -> &Policies {
        &self.policies
    }

    /// The database.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The database, as a task can own it.
    #[must_use]
    pub fn store_handle(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }
}

/// The node's RNG behind its lock: each draw takes the lock, so a task may
/// own one while others draw too.
struct LockedRng(Arc<Mutex<Box<dyn SharedRng>>>);

impl rand_core::TryRng for LockedRng {
    type Error = std::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(rand_core::Rng::next_u32(&mut **lock(&self.0)))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(rand_core::Rng::next_u64(&mut **lock(&self.0)))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        rand_core::Rng::fill_bytes(&mut **lock(&self.0), dst);
        Ok(())
    }
}

impl rand_core::TryCryptoRng for LockedRng {}
