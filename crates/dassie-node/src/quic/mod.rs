//! The peer protocol over QUIC (spec/areas/peer-protocol.md#quic).
//!
//! One UDP socket, on the HTTPS listener's port number, both takes and makes
//! connections. Every bidirectional stream starts with a byte that says
//! what it carries ([`StreamKind`]). A peer request is the envelope one
//! way, then a two-byte status and the body back, with the same bytes and
//! statuses as over HTTPS. The other kinds carry relayed streams
//! ([`relay`]) and settlement hubs' messages (`hub.rs`,
//! spec/areas/peer-protocol.md#the-hubs-stream). Each side proves its node
//! key in the TLS handshake
//! ([`keys`]), so a dialler knows it reached the node it meant, and a
//! connection takes a place by who opened it ([`places`]).
//!
//! A pair of nodes keeps one connection, which both use: the one the lower
//! node ID opened, when both dialled. A node whose QUIC cannot be reached
//! (UDP filtered, say) is asked over HTTPS for a while instead, until every
//! node takes QUIC and `POST /peer` goes ([`send_peer`]).

mod hub;
pub mod keys;
pub mod places;
pub mod relay;

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use dassie_identity::{NodeId, NodeKey, PublicKey, SeedPath};
use dassie_node_core::address::is_public;
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use dassie_peer::wire::MAX_MESSAGE_BYTES;
use dassie_peer::{Failure, Request, Response};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{Connection, Endpoint, EndpointConfig, ReadToEndError, TransportConfig, VarInt};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use self::keys::{AnyNodeKey, Keys, PinnedServer, Present};
use self::places::{Counts, Peerings, Places};
use self::relay::{Purpose, Relaying, ServedStream};
use crate::http::peer::{Answer, BodyError};
use crate::log::target;
use crate::peer_client::PeerClient;
use crate::tls;

/// The protocol a connection speaks, by ALPN.
pub const ALPN: &[u8] = b"dassie/1";

/// Most time to set up a connection. A request gets at most half its own
/// time for it, so that HTTPS has the rest when QUIC cannot be reached.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection nothing was sent on for this long closes. Peers send
/// heartbeats more often.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How often, in each idle timeout, a connection kept open for its own sake,
/// such as a node's to the relay it chose, shows it is alive: well within
/// the idle timeout, so that a lost packet or two do not close it (every
/// 10 s, with the idle timeout of 30 s).
const KEEP_ALIVES_PER_IDLE_TIMEOUT: u32 = 3;

/// How long a node whose QUIC could not be reached is asked over HTTPS.
const UNREACHABLE_FOR: Duration = Duration::from_secs(10 * 60);

/// Least time an attempt to connect must have had for its failure to say
/// the node cannot be reached: a packet near its expiry may have too little.
const MIN_TO_JUDGE: Duration = Duration::from_millis(500);

/// Longest response read: a node list of 4096 IDs fits, as over HTTPS.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Most requests open at once on one connection: a client's share of the
/// requests the node takes at once
/// ([`MAX_PEER_REQUESTS_PER_CLIENT`](crate::node::MAX_PEER_REQUESTS_PER_CLIENT)).
const MAX_STREAMS: u32 = crate::node::MAX_PEER_REQUESTS_PER_CLIENT;

/// What one connection may have unread, in all: its streams' worth of the
/// largest message, so memory per connection is bounded
/// (spec/decisions/tigerstyle.md).
const CONNECTION_WINDOW: u32 = 4 * 1024 * 1024;

/// Most addresses of a name tried, in turn, within the time to connect.
const MAX_ADDRESSES: usize = 4;

/// Most nodes with a connection, or one being made, at once.
const MAX_LINKS: usize = 4096;

/// Time to write an answer to a client that does not read it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// Time for open connections to finish their requests at shutdown.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Time for the endpoint to tell its peers it closed.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// The code a server closes a connection with when it has no place for it
/// once it knows who opened it (spec/areas/peer-protocol.md#quic): before
/// it read any stream, so nothing sent on it was delivered, and the client
/// takes it as a dial that failed. Shutdown and every other close use 0.
pub const NO_ROOM: VarInt = VarInt::from_u32(1);

/// The status ahead of a response's body.
const STATUS_BYTES: usize = 2;

/// Where the key for stateless resets hangs in the node's key tree
/// (spec/areas/identity.md#seed-paths).
const RESET_KEY_PATH: &str = "node/quic/stateless-reset";

const _: () = assert!(MAX_MESSAGE_BYTES <= MAX_RESPONSE_BYTES);
const _: () = assert!(CONNECTION_WINDOW as usize >= 4 * MAX_RESPONSE_BYTES);

/// Why a request over QUIC was not answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// No connection could be made: nothing was sent, and HTTPS may try.
    Unreachable,
    /// The request was sent, or lost on a connection that was up.
    Failed(Failure),
}

/// The transport that carried a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// QUIC.
    Quic,
    /// HTTPS, `POST /peer`.
    Https,
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Quic => "quic",
            Self::Https => "https",
        })
    }
}

/// A request's body, being read.
pub type Body<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, BodyError>> + Send + 'a>>;

/// What a bidirectional stream carries: its first byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    /// A peer request, then its answer.
    Peer = 0,
    /// A relay request, from a client to a relay ([`relay`]).
    Relay = 1,
    /// A relayed stream, from a relay to the node it reaches ([`relay`]).
    Relayed = 2,
    /// A settlement hub's message, then its reply
    /// ([`crate::hub::transport`]).
    Hub = 3,
    /// A hub validator's request for the state after a final block, then
    /// the state ([`crate::hub::transport`]).
    HubState = 4,
}

impl StreamKind {
    /// The kind a stream's first byte names, if any.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Peer),
            1 => Some(Self::Relay),
            2 => Some(Self::Relayed),
            3 => Some(Self::Hub),
            4 => Some(Self::HubState),
            _ => None,
        }
    }

    /// The byte that names it.
    #[must_use]
    pub const fn byte(self) -> u8 {
        self as u8
    }
}

/// What answers the requests the endpoint takes.
pub trait Handler: Send + Sync + 'static {
    /// The share of connections `address` counts in; `None` for the total
    /// only.
    fn client(&self, address: IpAddr) -> Option<ClientKey>;

    /// How many connections the endpoint holds from anyone, in all and per
    /// client, and how many it sets up at once.
    fn limits(&self) -> ConnectionLimits;

    /// The node's peerings, which give the connections of its core peers
    /// and its downlinks places of their own ([`places`]). None, unless a
    /// handler says so: every connection is then anyone's.
    fn peerings(&self) -> Option<Peerings> {
        None
    }

    /// Answers a request from `client` whose body `body` reads.
    fn answer<'a>(
        &'a self,
        client: IpAddr,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>>;

    /// Answers `request` on a settlement hub's stream from the node `from`,
    /// whose key the connection proved ([`crate::hub::transport`]). `None`
    /// resets the stream: a node that validates no hub, unless a handler
    /// says otherwise.
    fn hub<'a>(
        &'a self,
        _from: PublicKey,
        _request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<crate::hub::HubAnswer>> + Send + 'a>> {
        Box::pin(async { None })
    }

    /// Answers a hub validator's request for the state after a final block
    /// from the node `from` ([`crate::hub::transport`]): the reply's bytes.
    /// `None` resets the stream, unless a handler says otherwise.
    fn hub_state<'a>(
        &'a self,
        _from: PublicKey,
        _request: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<Vec<u8>>> + Send + 'a>> {
        Box::pin(async { None })
    }

    /// Whether the node serves relayed streams for `purpose`. One it does
    /// not serve is refused before any TLS. None, unless a handler says so.
    fn serves(&self, _purpose: Purpose) -> bool {
        false
    }

    /// Whether the endpoint carries relay requests to the nodes connected
    /// to it. Every node does; a front door, which opens relayed streams
    /// itself ([`Quic::open_held`]), refuses them.
    fn relays(&self) -> bool {
        true
    }

    /// Serves a relayed stream for `purpose` until it ends or `shutdown`:
    /// `client` is the node the TLS inside the stream proved, `relay` the
    /// address of the node that carried it.
    fn serve_relayed<'a>(
        &'a self,
        _purpose: Purpose,
        _client: NodeId,
        _relay: SocketAddr,
        _stream: ServedStream,
        _shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}

/// Sends a peer request as the engine does: over QUIC when the node's link
/// state says it takes it, and its QUIC was not out of reach lately;
/// otherwise, or when no connection could be made in time, over HTTPS in the
/// time left. Both transports share the bounds on requests in flight.
pub async fn send_peer(
    quic: Option<Arc<Quic>>,
    client: &PeerClient,
    request: Request,
) -> (Result<Response, Failure>, Transport) {
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_millis(request.timeout_ms.max(1));
    let _slot = match client.hold(&request) {
        Ok(slot) => slot,
        Err(failure) => return (Err(failure), Transport::Https),
    };
    // A periphery node has no URL: it is reached only over the connection
    // it opened to us, never dialled, never over HTTPS.
    if request.url.is_empty() {
        let Some(quic) = quic else {
            return (Err(Failure::NotDelivered), Transport::Quic);
        };
        return match tokio::time::timeout_at(deadline, quic.send_held(&request)).await {
            Ok(result) => (result, Transport::Quic),
            Err(_) => (Err(Failure::Unknown), Transport::Quic),
        };
    }
    if let Some(quic) = quic.filter(|_| request.quic) {
        let within = CONNECT_TIMEOUT.min(Duration::from_millis(request.timeout_ms / 2));
        match tokio::time::timeout_at(deadline, quic.send(&request, client, within)).await {
            Ok(Err(SendError::Unreachable)) => {}
            Ok(Ok(response)) => return (Ok(response), Transport::Quic),
            Ok(Err(SendError::Failed(failure))) => return (Err(failure), Transport::Quic),
            Err(_) => return (Err(Failure::Unknown), Transport::Quic),
        }
    }
    let left = deadline.saturating_duration_since(tokio::time::Instant::now());
    let left_ms = u64::try_from(left.as_millis()).unwrap_or(u64::MAX);
    if left_ms == 0 {
        return (Err(Failure::NotDelivered), Transport::Https);
    }
    let request = Request {
        timeout_ms: left_ms,
        ..request
    };
    (client.send_held(request).await, Transport::Https)
}

/// A connection to a node, and who opened it.
#[derive(Clone, Debug)]
struct Link {
    connection: Connection,
    /// Whether we dialled it.
    ours: bool,
}

impl Link {
    fn is_open(&self) -> bool {
        self.connection.close_reason().is_none()
    }
}

/// Holds `link`, which did not get `place` or lost it to another, ready to
/// take it: whenever the link there closes while this one is open, this
/// one takes its place. Returns once it does, or once `link` closes. Each
/// connection stands by at most once at a time, since one in the place does
/// not, so the waits are bounded as the connections are.
async fn stand_by(place: &tokio::sync::Mutex<Option<Link>>, link: Link) {
    loop {
        let kept = {
            let mut held = place.lock().await;
            match held.as_ref() {
                Some(kept) if kept.is_open() => {
                    if kept.connection.stable_id() == link.connection.stable_id() {
                        return;
                    }
                    kept.connection.clone()
                }
                _ => {
                    if link.is_open() {
                        *held = Some(link);
                    }
                    return;
                }
            }
        };
        tokio::select! {
            _ = kept.closed() => {}
            _ = link.connection.closed() => return,
        }
    }
}

/// Why [`Quic::connection`] has no connection to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoConnection {
    /// The time ran out waiting for another's dial, or was too short to
    /// judge the node by.
    NoTime,
    /// A dial of its own had time enough and failed.
    DialFailed,
}

/// Requests over QUIC, both ways, since start.
#[derive(Debug, Default)]
pub struct Stats {
    /// Answered requests we sent.
    pub sent: AtomicU64,
    /// Requests we answered.
    pub answered: AtomicU64,
}

/// What answers requests, once the endpoint serves: on the connections it
/// takes, and on those it makes too, since the other node sends on them.
struct Serving {
    handler: Arc<dyn Handler>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
    places: Arc<Places>,
}

type Place = Arc<tokio::sync::Mutex<Option<Link>>>;

/// The node's QUIC endpoint.
pub struct Quic {
    endpoint: Endpoint,
    keys: Arc<Keys>,
    links: Mutex<BTreeMap<NodeId, Place>>,
    unreachable: Mutex<BTreeMap<NodeId, Instant>>,
    serving: Mutex<Option<Serving>>,
    relaying: Relaying,
    /// The relay the node keeps a connection to while it serves.
    chosen_relay: Mutex<Option<relay::Chosen>>,
    /// How the node stands with the chosen relay now, a
    /// [`relay::RelayLink`] as its byte.
    relay_link: AtomicU8,
    allow_private: bool,
    /// How long a connection lasts without traffic: [`IDLE_TIMEOUT`],
    /// except in tests.
    idle: Duration,
    /// Whether the connections it dials for requests keep themselves alive,
    /// as the one to a chosen relay always does: on a periphery node.
    keep_alive: AtomicBool,
    stats: Stats,
}

impl fmt::Debug for Quic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Quic")
            .field("local_addr", &self.endpoint.local_addr().ok())
            .finish_non_exhaustive()
    }
}

impl Quic {
    /// An endpoint on `socket`, presenting the node key `identity` holds once
    /// the node is set up. Call it within the async runtime.
    ///
    /// # Errors
    ///
    /// The socket cannot be adopted, or there is no async runtime.
    pub fn new(
        socket: std::net::UdpSocket,
        identity: watch::Receiver<Option<Arc<NodeKey>>>,
        allow_private: bool,
    ) -> io::Result<Self> {
        let config = endpoint_config(identity.borrow().as_deref());
        let keys = Arc::new(Keys::new(identity));
        let server = server_config(&keys, IDLE_TIMEOUT).map_err(io::Error::other)?;
        let runtime = quinn::default_runtime().ok_or_else(|| io::Error::other("no runtime"))?;
        let endpoint = Endpoint::new(config, Some(server), socket, runtime)?;
        Ok(Self {
            endpoint,
            keys,
            links: Mutex::new(BTreeMap::new()),
            unreachable: Mutex::new(BTreeMap::new()),
            serving: Mutex::new(None),
            relaying: Relaying::new(),
            chosen_relay: Mutex::new(None),
            relay_link: AtomicU8::new(relay::RelayLink::Connecting.byte()),
            allow_private,
            idle: IDLE_TIMEOUT,
            keep_alive: AtomicBool::new(false),
            stats: Stats::default(),
        })
    }

    /// For tests: connections close after `idle` without traffic, rather
    /// than [`IDLE_TIMEOUT`], and those kept alive are kept so within it.
    ///
    /// # Errors
    ///
    /// The server's configuration cannot be made.
    #[doc(hidden)]
    pub fn with_idle_timeout(mut self, idle: Duration) -> io::Result<Self> {
        let server = server_config(&self.keys, idle).map_err(io::Error::other)?;
        self.endpoint.set_server_config(Some(server));
        self.idle = idle;
        Ok(self)
    }

    /// Whether the connections dialled from now on keep themselves alive,
    /// as a periphery node's to its uplinks must.
    pub fn set_keep_alive(&self, on: bool) {
        self.keep_alive.store(on, Ordering::Relaxed);
    }

    /// Sends `request` on the open connection its node made to us, and reads
    /// the response: how a periphery node, which has no URL, is reached.
    ///
    /// # Errors
    ///
    /// [`Failure::NotDelivered`] when no such connection is open, and
    /// nothing was sent; otherwise the failure, as over HTTPS.
    pub async fn send_held(&self, request: &Request) -> Result<Response, Failure> {
        let place = self
            .links
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&request.to)
            .cloned()
            .ok_or(Failure::NotDelivered)?;
        let connection = place
            .lock()
            .await
            .as_ref()
            .filter(|link| link.is_open())
            .map(|link| link.connection.clone())
            .ok_or(Failure::NotDelivered)?;
        let result = exchange(&connection, &request.body).await;
        if result.is_ok() {
            self.stats.sent.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    /// The address the endpoint is bound to.
    ///
    /// # Errors
    ///
    /// The socket cannot say.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }

    /// Requests over QUIC since start.
    #[must_use]
    pub const fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Connections open now, both ways.
    #[must_use]
    pub fn open_connections(&self) -> usize {
        self.endpoint.open_connections()
    }

    /// The places the connections it took hold now, by kind, while it
    /// serves.
    #[must_use]
    pub fn places(&self) -> Option<Counts> {
        self.serving
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|serving| serving.places.counts())
    }

    /// Whether the endpoint takes connections: from [`serve`](Self::serve)'s
    /// start until its shutdown, while it drains.
    #[must_use]
    pub fn is_serving(&self) -> bool {
        self.serving
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Whether `node`'s QUIC failed lately, so that it is asked over HTTPS.
    #[must_use]
    pub fn is_unreachable(&self, node: &NodeId) -> bool {
        let mut unreachable = self
            .unreachable
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match unreachable.get(node) {
            Some(until) if *until > Instant::now() => true,
            Some(_) => {
                unreachable.remove(node);
                false
            }
            None => false,
        }
    }

    fn mark_unreachable(&self, node: NodeId) {
        let mut unreachable = self
            .unreachable
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if unreachable.len() >= MAX_LINKS {
            unreachable.retain(|_, until| *until > now);
        }
        if unreachable.len() < MAX_LINKS {
            unreachable.insert(node, now + UNREACHABLE_FOR);
        }
    }

    /// Sends `request` and reads its response, making a connection within
    /// `connect_within` if there is none. The caller bounds the time the
    /// whole exchange takes.
    ///
    /// # Errors
    ///
    /// [`SendError::Unreachable`] when no connection could be made in time,
    /// and nothing was sent. The node is then asked over HTTPS for a while,
    /// if a dial of its own had at least half a second and failed; not
    /// when the time ran out waiting for another request's dial, or was too
    /// short to judge. Otherwise the failure, as over HTTPS.
    pub async fn send(
        self: &Arc<Self>,
        request: &Request,
        resolver: &PeerClient,
        connect_within: Duration,
    ) -> Result<Response, SendError> {
        // Without its key the node cannot prove who it is. TLS 1.3 would
        // only say so after the handshake, on the first stream.
        if self.is_unreachable(&request.to) || self.keys.current().is_none() {
            return Err(SendError::Unreachable);
        }
        let Some(place) = self.place(request.to) else {
            return Err(SendError::Unreachable);
        };
        // A connection kept from before may have died since: then the request
        // was not sent, and a fresh one is made once.
        for attempt in 0..2 {
            let connection = self
                .connection(&place, (request.to, &request.url), resolver, connect_within)
                .await;
            let connection = match connection {
                Ok(connection) => connection,
                Err(missed) => {
                    if missed == NoConnection::DialFailed {
                        self.mark_unreachable(request.to);
                    }
                    return Err(SendError::Unreachable);
                }
            };
            let result = exchange(&connection, &request.body).await;
            // A node with no room for us closes before it reads anything:
            // nothing was delivered, and it is asked over HTTPS for a while,
            // as one whose QUIC refused the connection outright.
            if result.is_err() && no_room(&connection) {
                self.mark_unreachable(request.to);
                return Err(SendError::Unreachable);
            }
            if result == Err(Failure::NotDelivered) && attempt == 0 {
                let mut held = place.lock().await;
                if held
                    .as_ref()
                    .is_some_and(|kept| kept.connection.stable_id() == connection.stable_id())
                {
                    *held = None;
                }
                continue;
            }
            if result.is_ok() {
                self.stats.sent.fetch_add(1, Ordering::Relaxed);
            }
            return result.map_err(SendError::Failed);
        }
        Err(SendError::Failed(Failure::NotDelivered))
    }

    /// The open connection in `place`, or a new one, within `within`. Only
    /// making one holds the place, so requests to a node wait for its
    /// connection, never for each other; one that waits past its time for
    /// another's dial goes over HTTPS, nothing sent. The caller decides
    /// whether a dial that failed marks the node as out of reach.
    async fn connection(
        self: &Arc<Self>,
        place: &tokio::sync::Mutex<Option<Link>>,
        (to, url): (NodeId, &str),
        resolver: &PeerClient,
        within: Duration,
    ) -> Result<Connection, NoConnection> {
        let deadline = tokio::time::Instant::now() + within;
        let mut held = tokio::time::timeout_at(deadline, place.lock())
            .await
            .map_err(|_| NoConnection::NoTime)?;
        if let Some(kept) = held.as_ref().filter(|kept| kept.is_open()) {
            return Ok(kept.connection.clone());
        }
        *held = None;
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        // A periphery node keeps its connections to its uplinks alive: they
        // are the only way its uplinks reach it.
        let keep_alive = self.keep_alive.load(Ordering::Relaxed);
        let Some(connection) = self.dial((to, url), resolver, left, keep_alive).await else {
            return Err(if left >= MIN_TO_JUDGE {
                NoConnection::DialFailed
            } else {
                NoConnection::NoTime
            });
        };
        self.answer_on(&connection);
        *held = Some(Link {
            connection: connection.clone(),
            ours: true,
        });
        Ok(connection)
    }

    /// The place for `node`'s connection, if the table has room.
    fn place(&self, node: NodeId) -> Option<Place> {
        let mut links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(place) = links.get(&node) {
            return Some(Arc::clone(place));
        }
        if links.len() >= MAX_LINKS {
            // Places nobody holds and whose connection is gone are free.
            links.retain(|_, place| {
                Arc::strong_count(place) > 1
                    || place
                        .try_lock()
                        .map_or(true, |held| held.as_ref().is_some_and(Link::is_open))
            });
            if links.len() >= MAX_LINKS {
                return None;
            }
        }
        let place = Arc::new(tokio::sync::Mutex::new(None));
        links.insert(node, Arc::clone(&place));
        Some(place)
    }

    /// Makes a connection to the node `to` at its URL's host and port,
    /// within `within`, checking that the server holds that node's key. The
    /// name's addresses are tried in turn, each with a share of the time, so
    /// one that drops UDP does not take it all. With `keep_alive`, our side
    /// keeps the connection from going idle.
    async fn dial(
        &self,
        (to, url): (NodeId, &str),
        resolver: &PeerClient,
        within: Duration,
        keep_alive: bool,
    ) -> Option<Connection> {
        let deadline = tokio::time::Instant::now() + within;
        let (host, port) = authority(url)?;
        if let Ok(ip) = host.parse::<IpAddr>()
            && !self.allow_private
            && !is_public(ip)
        {
            return None;
        }
        let resolving = resolver.resolve(host, port);
        let addresses = tokio::time::timeout_at(deadline, resolving)
            .await
            .ok()?
            .ok()?;
        // An endpoint on IPv4 alone cannot reach an IPv6 address.
        let ipv6 = self
            .endpoint
            .local_addr()
            .is_ok_and(|address| address.is_ipv6());
        let addresses: Vec<SocketAddr> = addresses
            .into_iter()
            .filter(|address| ipv6 || address.is_ipv4())
            .take(MAX_ADDRESSES)
            .collect();
        let config = client_config(&self.keys, to, self.idle, keep_alive).ok()?;
        for (index, address) in addresses.iter().enumerate() {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let tries_left = u32::try_from(addresses.len() - index).unwrap_or(1).max(1);
            let share = left / tries_left;
            if share.is_zero() {
                break;
            }
            let Ok(connecting) = self.endpoint.connect_with(config.clone(), *address, host) else {
                continue;
            };
            match tokio::time::timeout(share, connecting).await {
                Ok(Ok(connection)) => return Some(connection),
                Ok(Err(error)) => {
                    tracing::debug!(target: target::PEER, %to, %address, %error, "QUIC connection failed");
                }
                Err(_) => {
                    tracing::debug!(target: target::PEER, %to, %address, "QUIC connection timed out");
                }
            }
        }
        None
    }

    /// Keeps a connection a node opened to us, for our requests to it too:
    /// unless we hold one already that the lower node ID opened. It waits
    /// for a dial of ours to the same node, so that crossed dials end with
    /// both nodes using the same connection. Whichever of the two is not
    /// kept, the new one or the one it replaced, stands by ([`stand_by`]),
    /// so that a node that keeps its connection to us open (one that chose
    /// us as its relay, say) stays reachable whatever other connections
    /// come and go.
    async fn keep_inbound(&self, peer: NodeId, connection: Connection) {
        let Some((own, _)) = self.keys.current() else {
            return;
        };
        let Some(place) = self.place(peer) else {
            return;
        };
        let preferred = |link: &Link| link.ours == (own < peer);
        let link = Link {
            connection,
            ours: false,
        };
        let waiting = {
            let mut held = place.lock().await;
            match held.as_ref() {
                Some(kept) if kept.is_open() && preferred(kept) => link,
                _ => match held.replace(link) {
                    Some(replaced) if replaced.is_open() => replaced,
                    _ => return,
                },
            }
        };
        stand_by(&place, waiting).await;
    }

    /// Answers the requests the other node sends on a connection we made,
    /// once the endpoint serves.
    fn answer_on(self: &Arc<Self>, connection: &Connection) {
        let serving = self.serving.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(serving) = serving.as_ref() else {
            return;
        };
        let quic = Arc::clone(self);
        let connection = connection.clone();
        let client = connection.remote_address().ip().to_canonical();
        let handler = Arc::clone(&serving.handler);
        let shutdown = serving.shutdown.clone();
        serving.tasks.spawn(async move {
            quic.requests(&connection, client, &handler, &shutdown)
                .await;
        });
    }

    /// Takes connections and answers their requests with `handler`, until
    /// `shutdown`; and answers on the connections the node makes. At
    /// shutdown it stops taking connections, lets requests in progress
    /// finish for a bounded time, and only then closes the endpoint, which
    /// tells peers the connections are gone.
    pub async fn serve(self: Arc<Self>, handler: Arc<dyn Handler>, shutdown: CancellationToken) {
        let places = Arc::new(Places::new(handler.limits(), handler.peerings()));
        let tasks = TaskTracker::new();
        *self.serving.lock().unwrap_or_else(PoisonError::into_inner) = Some(Serving {
            handler: Arc::clone(&handler),
            shutdown: shutdown.clone(),
            tasks: tasks.clone(),
            places: Arc::clone(&places),
        });
        let relay = self.keep_chosen_relay(&tasks, &shutdown);
        loop {
            let incoming = tokio::select! {
                incoming = self.endpoint.accept() => incoming,
                () = shutdown.cancelled() => break,
            };
            let Some(incoming) = incoming else {
                break;
            };
            // Once places run short, a client must show it receives at its
            // address before it holds one: spoofed handshakes cannot.
            if !incoming.remote_address_validated() && places.short() {
                let _ = incoming.retry();
                continue;
            }
            let client = incoming.remote_address().ip().to_canonical();
            let key = handler.client(client);
            let setting_up = match places.set_up(key) {
                Ok(slot) => slot,
                Err(refused) => {
                    tracing::debug!(target: target::PEER, %client, ?refused, "too many QUIC connections being set up; refusing");
                    incoming.refuse();
                    continue;
                }
            };
            let quic = Arc::clone(&self);
            let (handler, places) = (Arc::clone(&handler), Arc::clone(&places));
            let shutdown = shutdown.clone();
            tasks.spawn(async move {
                let Ok(connecting) = incoming.accept() else {
                    return;
                };
                let Ok(Ok(connection)) = tokio::time::timeout(CONNECT_TIMEOUT, connecting).await
                else {
                    return;
                };
                // Who opened it is known now, and so its class.
                let Some(held) = places.settle(&connection, key, setting_up) else {
                    return;
                };
                if let Some(peer) = keys::peer_node(&connection) {
                    let (quic, connection) = (Arc::clone(&quic), connection.clone());
                    tokio::spawn(async move { quic.keep_inbound(peer, connection).await });
                }
                tokio::select! {
                    () = quic.requests(&connection, client, &handler, &shutdown) => {}
                    () = places.follow(&connection, held) => {}
                }
            });
        }
        *self.serving.lock().unwrap_or_else(PoisonError::into_inner) = None;
        relay.cancel();
        tasks.close();
        if tokio::time::timeout(DRAIN_TIMEOUT, tasks.wait())
            .await
            .is_err()
        {
            tracing::warn!(target: target::PEER, "QUIC requests still open after the drain timeout");
        }
        if shutdown.is_cancelled() {
            self.endpoint.close(0u32.into(), b"shutting down");
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, self.endpoint.wait_idle()).await;
        }
    }

    /// Answers the streams on `connection`, each on a task of its own, until
    /// it closes or `shutdown`, then lets those in progress finish. The
    /// connection's stream limit bounds how many run at once.
    async fn requests(
        self: &Arc<Self>,
        connection: &Connection,
        client: IpAddr,
        handler: &Arc<dyn Handler>,
        shutdown: &CancellationToken,
    ) {
        let streams = TaskTracker::new();
        loop {
            let stream = tokio::select! {
                stream = connection.accept_bi() => stream,
                () = shutdown.cancelled() => break,
            };
            let Ok((send, recv)) = stream else {
                break;
            };
            let on = On {
                quic: Arc::clone(self),
                connection: connection.clone(),
                client,
                handler: Arc::clone(handler),
                shutdown: shutdown.clone(),
            };
            streams.spawn(on.stream(send, recv));
        }
        streams.close();
        streams.wait().await;
    }
}

/// A stream the endpoint took, and where: what its kind needs.
struct On {
    quic: Arc<Quic>,
    connection: Connection,
    client: IpAddr,
    handler: Arc<dyn Handler>,
    shutdown: CancellationToken,
}

impl On {
    /// Reads the stream's kind, within the time for a header, and handles
    /// it as that kind. A stream of no known kind is reset.
    async fn stream(self, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(relay::HEADER_TIMEOUT, recv.read_exact(&mut byte)).await;
        let [byte] = byte;
        let kind = match read {
            Ok(Ok(())) => StreamKind::from_byte(byte),
            _ => None,
        };
        match kind {
            Some(StreamKind::Peer) => self.answer(send, recv).await,
            Some(StreamKind::Relay) => self.relay(send, recv).await,
            Some(StreamKind::Relayed) => self.serve_relayed(send, recv).await,
            Some(StreamKind::Hub) => self.hub(send, recv).await,
            Some(StreamKind::HubState) => self.hub_state(send, recv).await,
            None => {
                let _ = send.reset(0u32.into());
                let _ = recv.stop(0u32.into());
            }
        }
    }

    /// Answers a peer request: its envelope, then the stream's end.
    async fn answer(self, mut send: quinn::SendStream, mut recv: quinn::RecvStream) {
        let body: Body<'_> = Box::pin(async move {
            recv.read_to_end(MAX_MESSAGE_BYTES)
                .await
                .map_err(|error| match error {
                    ReadToEndError::TooLong => BodyError::TooLarge,
                    ReadToEndError::Read(_) => BodyError::Broken,
                })
        });
        let answer = self.handler.answer(self.client, body).await;
        let mut bytes = Vec::with_capacity(STATUS_BYTES + answer.body.len());
        bytes.extend_from_slice(&answer.status.to_be_bytes());
        bytes.extend_from_slice(&answer.body);
        // A client that does not read its answer holds nothing for longer
        // than this.
        match tokio::time::timeout(ANSWER_TIMEOUT, send.write_all(&bytes)).await {
            Ok(Ok(())) if send.finish().is_ok() => {
                self.quic.stats.answered.fetch_add(1, Ordering::Relaxed);
            }
            Ok(_) => {}
            Err(_) => {
                let _ = send.reset(0u32.into());
            }
        }
    }
}

/// Whether the other side closed `connection` for want of room
/// ([`NO_ROOM`]), before it read anything sent on it.
fn no_room(connection: &Connection) -> bool {
    matches!(
        connection.close_reason(),
        Some(quinn::ConnectionError::ApplicationClosed(close)) if close.error_code == NO_ROOM
    )
}

/// Sends `body` on a new stream of `connection` and reads the answer.
async fn exchange(connection: &Connection, body: &[u8]) -> Result<Response, Failure> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|_| Failure::NotDelivered)?;
    send.write_all(&[StreamKind::Peer.byte()])
        .await
        .map_err(|_| Failure::Unknown)?;
    send.write_all(body).await.map_err(|_| Failure::Unknown)?;
    send.finish().map_err(|_| Failure::Unknown)?;
    let bytes = recv
        .read_to_end(MAX_RESPONSE_BYTES)
        .await
        .map_err(|_| Failure::Unknown)?;
    let Some((status, body)) = bytes.split_first_chunk::<STATUS_BYTES>() else {
        // Not a peer's answer: an error, which no status says arrived.
        return Ok(Response {
            status: 502,
            body: Vec::new(),
        });
    };
    Ok(Response {
        status: u16::from_be_bytes(*status),
        body: body.to_vec(),
    })
}

/// The host and port of a node URL, `https://host[:port]`; 443 when it
/// names none. An IPv6 literal comes without its brackets.
#[must_use]
pub fn authority(url: &str) -> Option<(&str, u16)> {
    let authority = url.strip_prefix("https://")?;
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        return match after {
            "" => Some((host, 443)),
            port => Some((host, port.strip_prefix(':')?.parse().ok()?)),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host, port.parse().ok()?)),
        None => Some((authority, 443)),
    }
}

/// The endpoint's configuration. Its key for stateless resets derives from
/// the node key, so it survives a restart: a node that crashed, and so
/// closed nothing, can still tell its peers their connections are gone, and
/// they connect again at once instead of waiting out the idle timeout. A
/// node not set up yet has no peers, and a random key.
fn endpoint_config(key: Option<&NodeKey>) -> EndpointConfig {
    let derived = SeedPath::parse(RESET_KEY_PATH)
        .ok()
        .and_then(|path| key?.derive(&path).ok());
    match derived {
        Some(secret) => {
            let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
            EndpointConfig::new(Arc::new(key))
        }
        None => EndpointConfig::default(),
    }
}

/// The transport's settings, with connections that close after `idle`
/// without traffic.
fn transport_config(idle: Duration) -> TransportConfig {
    let mut transport = TransportConfig::default();
    transport.max_idle_timeout(idle.try_into().ok());
    transport.max_concurrent_bidi_streams(MAX_STREAMS.into());
    transport.max_concurrent_uni_streams(0u8.into());
    // Bounded windows: a stream holds at most the largest message, and a
    // connection a few of them (spec/decisions/tigerstyle.md).
    let stream_window = u32::try_from(MAX_RESPONSE_BYTES).unwrap_or(u32::MAX);
    transport.stream_receive_window(VarInt::from_u32(stream_window));
    transport.receive_window(VarInt::from_u32(CONNECTION_WINDOW));
    transport.send_window(u64::from(CONNECTION_WINDOW));
    transport
}

fn server_config(keys: &Arc<Keys>, idle: Duration) -> Result<quinn::ServerConfig, String> {
    let mut config = rustls::ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(Arc::new(AnyNodeKey::new()))
        .with_cert_resolver(Arc::new(Present(Arc::clone(keys))));
    config.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicServerConfig::try_from(config).map_err(|e| e.to_string())?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config(idle)));
    Ok(config)
}

/// The configuration for dialling `expected`, with connections that close
/// after `idle` without traffic. With `keep_alive`, our side sends
/// something often enough that the connection never goes idle.
fn client_config(
    keys: &Arc<Keys>,
    expected: NodeId,
    idle: Duration,
    keep_alive: bool,
) -> Result<quinn::ClientConfig, String> {
    let mut config = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServer::new(expected)))
        .with_client_cert_resolver(Arc::new(Present(Arc::clone(keys))));
    config.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicClientConfig::try_from(config).map_err(|e| e.to_string())?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    let mut transport = transport_config(idle);
    if keep_alive {
        transport.keep_alive_interval(Some(idle / KEEP_ALIVES_PER_IDLE_TIMEOUT));
    }
    config.transport_config(Arc::new(transport));
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorities_parse_as_node_urls_name_them() {
        assert_eq!(authority("https://a.example"), Some(("a.example", 443)));
        assert_eq!(
            authority("https://a.example:8443"),
            Some(("a.example", 8443))
        );
        assert_eq!(authority("https://192.0.2.1:7"), Some(("192.0.2.1", 7)));
        assert_eq!(
            authority("https://[2001:db8::1]"),
            Some(("2001:db8::1", 443))
        );
        assert_eq!(
            authority("https://[2001:db8::1]:8443"),
            Some(("2001:db8::1", 8443))
        );
        assert_eq!(authority("http://a.example"), None);
        assert_eq!(authority("https://a.example:x"), None);
        assert_eq!(authority("https://[2001:db8::1]x"), None);
    }

    #[test]
    fn the_reset_key_path_is_under_the_node_key() {
        let key = NodeKey::from_bytes(&[1; 32]);
        let path = SeedPath::parse(RESET_KEY_PATH).unwrap();
        assert!(key.derive(&path).is_ok());
    }
}
