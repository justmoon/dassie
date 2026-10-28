//! The node's state, shared by its listeners and services.
//!
//! [`Node`] holds what the database holds, cached where every request needs
//! it (the configuration, the node key, the certificate), plus what lives
//! only in memory: the setup token, the login rate limiter, the ACME
//! challenge tokens and the renewal schedule. The decisions are
//! `dassie-node-core`'s; this carries them out.

mod access;
mod certificates;
mod devices;
mod rates;
mod settings;

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use dassie_identity::IdentityKey;
use dassie_kit::ClientKey;
use dassie_node_core::acme::{ChallengeTokens, Renewal, RenewalPolicy};
use dassie_node_core::config::NodeConfig;
use dassie_node_core::connections::{ConnectionLimits, Refused};
use dassie_node_core::rate_limit::{RateLimitPolicy, RateLimiter};
use dassie_node_core::session::{SessionId, SessionPolicy};
use dassie_node_core::setup::SetupToken;
use dassie_node_core::urls;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, broadcast, watch};

pub use access::{AuthError, NewSession, SessionView};
pub use certificates::{ObtainError, ObtainRequest};
pub use devices::{
    MAX_CODE_RELAYS, PAIRING_LIFETIME_MS, PairingError, Reach, StartedPairing, pairing_code,
};
pub use settings::SettingsError;

use crate::acme::AcmeClient;
use crate::clock::{Clock, SharedRng};
use crate::engine::Engine;
use crate::outbound::{OutboundClient, Resolver};
use crate::quic::places::Classes;
use crate::slots::{ClientSlot, ClientSlots};
use crate::store::{Store, StoreError};
use crate::tls::{CertificateSlot, TlsError};

/// How the daemon got its sockets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runtime {
    /// It bound them itself, on 80 and 443 or the ports its flags give.
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
    /// A browser's RPC socket without a session, or a device's stream that
    /// was not paired when it connected, closes after this long, whatever it
    /// sends: it only asks for the node's basic state, or claims a pairing.
    pub anonymous_socket_lifetime_ms: u64,
    /// A browser's RPC socket with no call in flight closes after this long
    /// without a message either way.
    pub socket_idle_ms: u64,
    /// Most connections one client (an IPv4 address or an IPv6 /56) holds
    /// on each listener at once. Our outbound client keeps at most 16
    /// requests in flight to one host and reuses its idle connections (4 at
    /// most), so four nodes behind one address fit; a browser opens about 6.
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
    /// The credit by which a payer over a settlement hub sizes what one
    /// request to settle has it fund its hold with, in its ledger's units:
    /// 1.1 units at scale 9 on the test network
    /// (spec/areas/settlement.md#credit). Large enough that a request
    /// funds at least the least settlement
    /// (spec/decisions/thresholds-in-packets.md).
    pub peer_credit: u64,
    /// How often peers are checked, to ask those near the credit we extend
    /// to settle (spec/decisions/own-fulfills.md).
    pub settlement_interval_ms: u64,
    /// How often a peer that did not settle since it was last asked is
    /// asked again, at most.
    pub settlement_request_every_ms: u64,
    /// Open Payments' incoming payments.
    pub incoming: crate::engine::IncomingPolicy,
    /// How often exchange rates are fetched
    /// (spec/decisions/exchange-rates.md).
    pub rates_every_ms: u64,
    /// How often a dollar token's peg guard reads its pool, and how long
    /// its holds are (spec/areas/xrpl.md#the-peg-guard).
    pub peg: dassie_xrpl::peg::Timings,
    /// The budget on answers to requests for the node's snapshot
    /// (spec/areas/rate-feed.md#requests).
    pub feed: dassie_feed::AnswerLimits,
    /// How long a tunnel taken may hold its place among the QUIC
    /// connections being set up (spec/areas/peer-protocol.md#quic-over-websocket).
    pub tunnel_bounds: crate::quic::tunnel::TunnelBounds,
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
        // Two accounts for each core node and downlink ever peered with:
        // they stay when its channel closes (spec/roadmap.md#not-built-yet).
        ledger: dassie_ledger::Config {
            max_accounts: 8192,
            max_pending: 391_432,
            max_journal: 1024,
        },
        // Every BTP client, the 18 local endpoints, every downlink and every
        // core peer, as many as a periphery node pinned to the most
        // uplinks holds, can fill their share of the packet table at once:
        // 1082 × 60 + 128 × 512 fits its 130 456, and the ledger has three
        // transfers for each (across ledgers, with our fee), and 64 more.
        // Each of them is an endpoint. The tables fill only as they are
        // used.
        connector: dassie_connector::Config {
            max_endpoints: 1218,
            max_pending: 130_456,
            max_pending_per_endpoint: 60,
            ..dassie_connector::Config::DEFAULT
        },
        peer: dassie_peer::Config::DEFAULT,
        peer_credit: 1_100_000_000,
        settlement_interval_ms: 4_000,
        settlement_request_every_ms: 60_000,
        incoming: crate::engine::IncomingPolicy::DEFAULT,
        rates_every_ms: 60_000,
        peg: dassie_xrpl::peg::Timings::DEFAULT,
        feed: dassie_feed::AnswerLimits::DEFAULT,
        tunnel_bounds: crate::quic::tunnel::TunnelBounds::DEFAULT,
    };

    /// The production values with the peer protocol's dev timers, and the
    /// peg guard's, for what runs on a real clock: dev environments and the
    /// end-to-end tests. The network simulation runs [`Policies::DEFAULT`].
    pub const DEV: Self = Self {
        peer: dassie_peer::Config::DEV,
        settlement_interval_ms: 1_000,
        settlement_request_every_ms: 5_000,
        peg: dassie_xrpl::peg::Timings::DEV,
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
    /// The wallet the node serves, which the binary embeds.
    pub wallet: crate::http::assets::Wallet,
    /// A development build of the wallet to serve instead of the embedded
    /// one.
    pub dev_wallet_dir: Option<PathBuf>,
    /// The names the owner added, which a node without a hostname answers
    /// Open Payments to.
    pub public_names: Vec<dassie_node_core::config::Hostname>,
    /// Whether the live realm may be set and run: by the dev switch only
    /// (spec/decisions/mainnet-schemes.md).
    pub live_realm: dassie_node_core::config::LiveRealm,
}

/// The peer protocol's settings, from the environment.
#[derive(Clone, Debug, Default)]
pub struct PeerOptions {
    /// The bootstrap nodes.
    pub bootstrap: Vec<dassie_peer::Bootstrap>,
    /// Extra roots for the certificates of the servers the node reaches
    /// (dev and tests).
    pub ca_file: Option<PathBuf>,
    /// Whether peers may be at private addresses (dev and tests).
    pub allow_private: bool,
    /// The only nodes the node asks to be its uplinks while it runs as a
    /// periphery node; empty for any (spec/areas/network.md#pinned-uplinks).
    pub uplinks: Vec<dassie_identity::NodeId>,
    /// The nodes a dev network makes core outright; empty outside one
    /// (spec/decisions/promotion-to-core.md).
    pub dev_core: Vec<dassie_identity::NodeId>,
}

/// Most inbound peer requests waiting on the engine at once.
const MAX_PEER_REQUESTS: usize = 256;

/// Most of those one client may hold: a share, so that a single address
/// cannot take every place, though one QUIC connection carries many
/// requests at once.
pub const MAX_PEER_REQUESTS_PER_CLIENT: u32 = 64;

const _: () = assert!(MAX_PEER_REQUESTS_PER_CLIENT as usize * 4 <= MAX_PEER_REQUESTS);

/// The places each client holds.
type HeldByClient = Arc<Mutex<BTreeMap<ClientKey, u32>>>;

/// One inbound peer request's place, given back when it drops.
#[derive(Debug)]
pub struct PeerPermit {
    _permit: OwnedSemaphorePermit,
    client: Option<(HeldByClient, ClientKey)>,
}

impl Drop for PeerPermit {
    fn drop(&mut self) {
        if let Some((by_client, key)) = &self.client {
            let mut by_client = lock(by_client);
            if let Some(count) = by_client.get_mut(key) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    by_client.remove(key);
                }
            }
        }
    }
}

/// Failed peer requests, per key a connection proved
/// ([`Node::peer_charge`]).
const PEER_ANONYMOUS: RateLimitPolicy = RateLimitPolicy {
    client_burst: 20,
    client_refill_ms: 1_000,
    client_slots: 1024,
};

/// The node's side of the peer protocol, beside the engine.
struct PeerSide {
    bootstrap: Vec<dassie_peer::Bootstrap>,
    uplinks: Vec<dassie_identity::NodeId>,
    dev_core: Vec<dassie_identity::NodeId>,
    /// Made on first use, as the ACME client is: a machine without the
    /// system's roots still serves, and says why its peer requests fail.
    client: std::sync::OnceLock<Result<OutboundClient, String>>,
    allow_private: bool,
    permits: Arc<Semaphore>,
    /// Places held by each client, for its share.
    by_client: HeldByClient,
    limiter: Mutex<RateLimiter>,
    /// The nodes peered with us, by kind, as the engine last said: what
    /// gives their QUIC connections places of their own.
    peerings: watch::Sender<Arc<Classes>>,
    /// The node's snapshot, signed, as the engine last made it: what its
    /// UDP endpoint answers requests with (spec/areas/rate-feed.md#requests).
    snapshot: watch::Sender<Option<Arc<[u8]>>>,
}

impl PeerSide {
    fn new(options: PeerOptions) -> Result<Self, NodeError> {
        let client = std::sync::OnceLock::new();
        if options.ca_file.is_some() {
            // A dev or test CA's root that cannot be read is a mistake in
            // the environment: refuse to start, rather than fail later.
            let made = OutboundClient::new(options.ca_file.as_deref(), options.allow_private)
                .map_err(NodeError::Peer)?;
            let _ = client.set(Ok(made));
        }
        Ok(Self {
            bootstrap: options.bootstrap,
            uplinks: options.uplinks,
            dev_core: options.dev_core,
            client,
            allow_private: options.allow_private,
            permits: Arc::new(Semaphore::new(MAX_PEER_REQUESTS)),
            by_client: Arc::new(Mutex::new(BTreeMap::new())),
            limiter: Mutex::new(RateLimiter::new(PEER_ANONYMOUS)),
            peerings: watch::Sender::new(Arc::new(Classes::default())),
            snapshot: watch::Sender::new(None),
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
    /// The outbound client could not be set up (a bad `DASSIE_EXTRA_CA_FILE`).
    Peer(String),
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => error.fmt(f),
            Self::Tls(error) => error.fmt(f),
            Self::Acme(error) => write!(f, "ACME client: {error}"),
            Self::Peer(error) => write!(f, "outbound client: {error}"),
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
    /// A paired device was revoked.
    Device(dassie_identity::NodeId),
}

impl Revocation {
    /// Whether this ends `session`.
    #[must_use]
    pub fn ends(&self, session: &SessionId) -> bool {
        match self {
            Self::Session(id) => id == session,
            Self::AllExcept(keep) => keep.as_ref() != Some(session),
            Self::Recheck | Self::Device(_) => false,
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
    node_key: watch::Sender<Option<Arc<IdentityKey>>>,
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
    /// The pairing waiting to be claimed, if any.
    pairing: Mutex<Option<devices::Pairing>>,
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
    wallet: crate::http::assets::Wallet,
    dev_wallet_dir: Option<PathBuf>,
    open_payments: crate::open_payments::State,
    /// Wakes the rate fetcher.
    rates_wake: Notify,
    /// The QUIC endpoint, where the node has a UDP socket.
    quic: std::sync::OnceLock<Arc<crate::quic::Quic>>,
    /// The UDP socket that answers requests for the node's snapshot, where
    /// the node has one.
    feed: std::sync::OnceLock<Arc<crate::feed::FeedEndpoint>>,
    /// The settlement hubs the node validates, which the hub's stream
    /// reaches (spec/areas/hubs.md#the-validator-in-the-node).
    hubs: Arc<crate::hub::Hubs>,
    /// The validator's side of founding hubs, where the owner turned
    /// validating on (spec/areas/hubs.md#founding-a-hub).
    founding: std::sync::OnceLock<Arc<crate::hub::founding::Founding>>,
    /// The hubs the node founds as a sponsor.
    sponsoring: crate::hub::sponsor::Sponsoring,
    /// Whether the live realm may be set and run.
    live_realm: dassie_node_core::config::LiveRealm,
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

/// The rate limiter's key for the node of `key`: the first half of its
/// public key, which two nodes share only by chance.
fn key_client(key: &dassie_identity::NodeIdentity) -> ClientKey {
    ClientKey::from_bytes(
        key.key()
            .as_bytes()
            .first_chunk()
            .copied()
            .unwrap_or_default(),
    )
}

/// Locks a mutex, carrying on past a panic of another holder: none of the
/// node's mutexes guard an update that a panic could leave half done.
pub(crate) fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Most live streams the node serves at once, over all connections.
pub const MAX_LIVE_STREAMS: usize = 64;

/// The token setup asks for: the stored one, or a new one stored. A node
/// set up already gets one that is never shown, and refused.
fn setup_token(
    store: &Store,
    set_up: bool,
    rng: &mut dyn SharedRng,
) -> Result<SetupToken, NodeError> {
    if set_up {
        return Ok(SetupToken::generate(rng));
    }
    if let Some(bytes) = store.setup_token()? {
        return Ok(SetupToken::from_bytes(bytes));
    }
    let token = SetupToken::generate(rng);
    store.set_setup_token(token.as_bytes())?;
    Ok(token)
}

/// The ACME client, made at once where a test CA's root is given: one that
/// cannot be read is a mistake in the environment, so the node refuses to
/// start rather than fail the first order. Otherwise it is made at the
/// first order.
fn acme_with(
    ca_file: Option<&std::path::Path>,
) -> Result<std::sync::OnceLock<Result<AcmeClient, String>>, NodeError> {
    let acme = std::sync::OnceLock::new();
    if ca_file.is_some() {
        let client = AcmeClient::new(ca_file).map_err(|e| NodeError::Acme(e.to_string()))?;
        let _ = acme.set(Ok(client));
    }
    Ok(acme)
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
            peer,
            wallet,
            dev_wallet_dir,
            public_names,
            live_realm,
        } = options;
        let peer = PeerSide::new(peer)?;
        let now_ms = clock.now_ms();
        let config = store.config()?;
        // With its identity, whose SLH-DSA key takes a key generation once.
        let node_key = store.node_key()?.map(|key| Arc::new(IdentityKey::new(key)));
        let certificate = certificates::initial(&store, config.hostname.as_ref(), now_ms)?;
        let acme = acme_with(acme_ca_file.as_deref())?;
        let setup_token = setup_token(&store, node_key.is_some(), rng.as_mut())?;
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
            pairing: Mutex::new(None),
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
            wallet,
            dev_wallet_dir,
            open_payments: crate::open_payments::State::new(public_names),
            rates_wake: Notify::new(),
            quic: std::sync::OnceLock::new(),
            feed: std::sync::OnceLock::new(),
            hubs: Arc::new(crate::hub::Hubs::default()),
            founding: std::sync::OnceLock::new(),
            sponsoring: crate::hub::sponsor::Sponsoring::default(),
            live_realm,
        }))
    }

    /// Whether the live realm may be set and run: by the dev switch, in a
    /// build with debug assertions only (spec/decisions/mainnet-schemes.md).
    #[must_use]
    pub const fn live_realm(&self) -> dassie_node_core::config::LiveRealm {
        self.live_realm
    }

    /// The validator's side of founding hubs, where the owner turned
    /// validating on.
    #[must_use]
    pub fn founding(&self) -> Option<&Arc<crate::hub::founding::Founding>> {
        self.founding.get()
    }

    /// Gives the node the validator's side of founding hubs, before its
    /// services start.
    pub fn attach_founding(&self, founding: Arc<crate::hub::founding::Founding>) {
        let _ = self.founding.set(founding);
    }

    /// The hubs the node founds as a sponsor.
    #[must_use]
    pub const fn sponsoring(&self) -> &crate::hub::sponsor::Sponsoring {
        &self.sponsoring
    }

    /// The settlement hubs the node validates.
    #[must_use]
    pub const fn hubs(&self) -> &Arc<crate::hub::Hubs> {
        &self.hubs
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

    /// The uplinks the node is pinned to while it runs as a periphery
    /// node; empty for any.
    #[must_use]
    pub fn pinned_uplinks(&self) -> Vec<dassie_identity::NodeId> {
        self.peer.uplinks.clone()
    }

    /// The nodes a dev network makes core outright; empty outside one.
    #[must_use]
    pub fn dev_core(&self) -> Vec<dassie_identity::NodeId> {
        self.peer.dev_core.clone()
    }

    /// The client for servers that are not nodes (Open Payments, rate
    /// sources, ledgers), made on first use; `None` when it cannot be, for
    /// want of roots to trust. Peer requests need none of it: they go over
    /// QUIC, which pins each node's key, by [`Self::resolver`].
    #[must_use]
    pub fn outbound(&self) -> Option<&OutboundClient> {
        let allow_private = self.peer.allow_private;
        let made = self.peer.client.get_or_init(|| {
            let made = OutboundClient::new(None, allow_private);
            if let Err(error) = &made {
                tracing::error!(target: crate::log::target::PEER, %error, "no requests to other servers can be sent");
            }
            made
        });
        made.as_ref().ok()
    }

    /// The resolver every outbound connection of the node's goes by: public
    /// addresses only, unless private ones are allowed.
    #[must_use]
    pub const fn resolver(&self) -> Resolver {
        Resolver::new(self.peer.allow_private)
    }

    /// A place for one more inbound peer request from `client`, if any is
    /// left and the client has not used up its share.
    #[must_use]
    pub fn peer_permit(&self, client: IpAddr) -> Option<PeerPermit> {
        let Some(key) = self.counted_client(client) else {
            let permit = Arc::clone(&self.peer.permits).try_acquire_owned().ok()?;
            return Some(PeerPermit {
                _permit: permit,
                client: None,
            });
        };
        let mut by_client = lock(&self.peer.by_client);
        let held = by_client.get(&key).copied().unwrap_or(0);
        if held >= MAX_PEER_REQUESTS_PER_CLIENT {
            return None;
        }
        let permit = Arc::clone(&self.peer.permits).try_acquire_owned().ok()?;
        by_client.insert(key, held + 1);
        Some(PeerPermit {
            _permit: permit,
            client: Some((Arc::clone(&self.peer.by_client), key)),
        })
    }

    /// Whether the node of `key` may send another failing peer request
    /// now.
    #[must_use]
    pub fn peer_limited(&self, key: &dassie_identity::NodeIdentity) -> bool {
        let now = self.now_ms();
        lock(&self.peer.limiter).peek(key_client(key), now).is_err()
    }

    /// Charges the node of `key` for a failed peer request: by the key its
    /// connection proved, not its address, so one node's failures never
    /// cost another behind the same address. A node that makes new keys
    /// to escape it needs a connection for each, which takes a place by its
    /// address (spec/areas/peer-protocol.md#transport).
    pub fn peer_charge(&self, key: &dassie_identity::NodeIdentity) {
        let now = self.now_ms();
        let _ = lock(&self.peer.limiter).check(key_client(key), now);
    }

    /// The nodes peered with us, by kind, as they change
    /// (spec/areas/peer-protocol.md#quic).
    #[must_use]
    pub fn peerings(&self) -> watch::Receiver<Arc<Classes>> {
        self.peer.peerings.subscribe()
    }

    /// Says who is peered with us now; those who follow hear of it only if
    /// that changed.
    pub fn set_peerings(&self, classes: Classes) {
        self.peer.peerings.send_if_modified(|held| {
            let changed = **held != classes;
            if changed {
                *held = Arc::new(classes);
            }
            changed
        });
    }

    /// The node's snapshot as it changes: none while the node offers no
    /// swaps, or its engine is down (spec/areas/rate-feed.md#requests).
    #[must_use]
    pub fn snapshots(&self) -> watch::Receiver<Option<Arc<[u8]>>> {
        self.peer.snapshot.subscribe()
    }

    /// The node's snapshot now, if it has one that has not expired: whether
    /// its UDP endpoint answers requests.
    #[must_use]
    pub fn current_snapshot(&self) -> Option<Arc<[u8]>> {
        let snapshot = self.peer.snapshot.borrow().clone()?;
        let valid_until_ms = crate::feed::valid_until_ms(&snapshot)?;
        (self.now_ms() < valid_until_ms).then_some(snapshot)
    }

    /// Says what the node's snapshot is now; those who follow hear of it
    /// only if that changed.
    pub fn set_snapshot(&self, snapshot: Option<&Arc<[u8]>>) {
        self.peer.snapshot.send_if_modified(|held| {
            let changed = match (&*held, snapshot) {
                (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
                (None, None) => false,
                _ => true,
            };
            if changed {
                *held = snapshot.cloned();
            }
            changed
        });
    }

    /// The configuration as it changes.
    #[must_use]
    pub fn subscribe_config(&self) -> watch::Receiver<NodeConfig> {
        self.config.subscribe()
    }

    /// What our link state says about us: the public URL, once there is a
    /// hostname and the QUIC endpoint is at its port, and the alias. A node
    /// without a URL runs as a periphery node
    /// (spec/areas/network.md#periphery-nodes).
    #[must_use]
    pub fn own_info(&self) -> dassie_peer::OwnInfo {
        let config = self.config();
        // Peers reach QUIC at the URL's port. Under systemd the daemon does
        // not start otherwise; a host that embeds the node may bind QUIC
        // apart, and such a node only dials, from wherever its socket is,
        // with no URL peers would look for it at.
        let https_port = self.ports().1;
        let at_url = self.quic().is_none_or(|quic| {
            quic.local_addr()
                .is_ok_and(|address| address.port() == https_port)
        });
        let url = dassie_node_core::urls::node_url(config.hostname.as_ref(), https_port)
            .filter(|_| at_url);
        // Readers find the endpoint at the URL's host, so only a node with
        // a URL names its port (spec/areas/rate-feed.md#finding-a-node); the
        // engine names it in the link state only while the node offers
        // swaps.
        let feed_port = self
            .feed()
            .and_then(|feed| feed.local_addr().ok())
            .map(|address| address.port())
            .filter(|_| url.is_some());
        dassie_peer::OwnInfo {
            url,
            alias: config.alias.as_str().to_owned(),
            feed_port,
        }
    }

    /// The rate feed's socket, where the node has one.
    #[must_use]
    pub fn feed(&self) -> Option<&Arc<crate::feed::FeedEndpoint>> {
        self.feed.get()
    }

    /// Gives the node its rate feed's socket, before its engine starts: the
    /// link state it issues then names the feed's port.
    pub fn attach_feed(&self, feed: Arc<crate::feed::FeedEndpoint>) {
        let _ = self.feed.set(feed);
    }

    /// The QUIC endpoint, where the node has one.
    #[must_use]
    pub fn quic(&self) -> Option<&Arc<crate::quic::Quic>> {
        self.quic.get()
    }

    /// Gives the node its QUIC endpoint, before its engine starts.
    pub fn attach_quic(&self, quic: Arc<crate::quic::Quic>) {
        let _ = self.quic.set(quic);
    }

    /// Whether private peers are allowed (dev and tests).
    #[must_use]
    pub const fn allows_private_peers(&self) -> bool {
        self.peer.allow_private
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
    pub fn node_key(&self) -> Option<Arc<IdentityKey>> {
        self.node_key.borrow().clone()
    }

    /// The node key as it changes: `None` until setup.
    #[must_use]
    pub fn identity(&self) -> watch::Receiver<Option<Arc<IdentityKey>>> {
        self.node_key.subscribe()
    }

    /// The ledger's engine, while it runs.
    #[must_use]
    pub fn engine(&self) -> Option<Engine> {
        self.engine.borrow().clone()
    }

    /// The ledger's engine as it starts and stops.
    #[must_use]
    pub fn engines(&self) -> watch::Receiver<Option<Engine>> {
        self.engine.subscribe()
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
        let client = dassie_kit::ClientKey::from_ip(client);
        lock(&self.btp_auth_limiter)
            .peek(client, self.now_ms())
            .is_err()
    }

    /// Counts a failed BTP authentication from `client`.
    pub fn btp_auth_failed(&self, client: std::net::IpAddr) {
        let client = dassie_kit::ClientKey::from_ip(client);
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

    /// The wallet the node serves, unless it serves a development build.
    #[must_use]
    pub const fn wallet(&self) -> crate::http::assets::Wallet {
        self.wallet
    }

    /// The development build of the wallet, if it serves one.
    #[must_use]
    pub fn dev_wallet_dir(&self) -> Option<&std::path::Path> {
        self.dev_wallet_dir.as_deref()
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

    /// The network a request from `address` counts against, for the
    /// shares of a pool one network may hold (an IPv4 /24, an IPv6 /56);
    /// `None` counts in the totals only, as loopback does for
    /// [`counted_client`](Self::counted_client).
    #[must_use]
    pub fn counted_network(&self, address: IpAddr) -> Option<ClientKey> {
        let address = address.to_canonical();
        if self.peer.allow_private && address.is_loopback() {
            return None;
        }
        Some(ClientKey::network_of(address))
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
