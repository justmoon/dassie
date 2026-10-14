//! The node's state, shared by its listeners and services.
//!
//! [`Node`] holds what the database holds, cached where every request needs
//! it (the configuration, the node key, the certificate), plus what lives
//! only in memory: the setup token, the login rate limiter, the ACME
//! challenge tokens and the renewal schedule. The decisions are
//! `dassie-node-core`'s; this carries them out.

mod access;
mod certificates;
mod settings;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use dassie_identity::NodeKey;
use dassie_node_core::acme::{ChallengeTokens, Renewal, RenewalPolicy};
use dassie_node_core::config::NodeConfig;
use dassie_node_core::rate_limit::{RateLimitPolicy, RateLimiter};
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
    /// A browser's RPC socket with no call in flight closes after this long
    /// without a message either way.
    pub socket_idle_ms: u64,
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
}

impl Policies {
    /// The production values.
    pub const DEFAULT: Self = Self {
        sessions: SessionPolicy::DEFAULT,
        login_attempts: RateLimitPolicy::DEFAULT,
        renewal: RenewalPolicy::DEFAULT,
        max_websockets: 256,
        socket_idle_ms: 10 * 60 * 1_000,
        body_timeout_ms: 10_000,
        max_btp_connections: 64,
        btp_auth_timeout_ms: 10_000,
        btp_auth_attempts: RateLimitPolicy::DEFAULT,
        ledger: dassie_ledger::Config {
            max_accounts: 4096,
            max_pending: 9216,
            max_journal: 1024,
        },
        // Every BTP client and the receiver can fill their share of the
        // packet table at once: 65 × 60 fits its 4096.
        connector: dassie_connector::Config {
            max_pending_per_endpoint: 60,
            ..dassie_connector::Config::DEFAULT
        },
    };
}

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
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(f),
            Self::Tls(error) => error.fmt(f),
            Self::Acme(error) => write!(f, "ACME client: {error}"),
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
    runtime: Runtime,
    http_port: u16,
    https_port: u16,
    started_at_ms: u64,
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
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

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
        } = options;
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
        let setup_token = SetupToken::generate(rng.as_mut());
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
            policies,
            runtime,
            http_port,
            https_port,
            started_at_ms: now_ms,
            clock,
        }))
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
