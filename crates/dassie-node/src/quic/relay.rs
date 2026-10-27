//! Relayed streams (spec/areas/peer-protocol.md#relayed-streams): how a
//! client reaches a node it cannot dial, through a relay the node keeps a
//! connection to.
//!
//! The client sends the relay a relay request, a stream of kind
//! [`StreamKind::Relay`] naming the destination and a purpose. The relay
//! opens a stream of kind [`StreamKind::Relayed`] to the destination, on a
//! connection it holds already, naming the purpose and the client. The
//! destination answers whether it takes it, the relay passes the answer on
//! as an [`Outcome`], and from then on copies bytes both ways, within
//! bounds. Inside, the client runs TLS 1.3 with the destination, pinned to
//! its node key, so the relay sees only ciphertext and its size.
//!
//! The bytes, after the kind byte, each node ID in the peer protocol's OER
//! encoding (a length byte, then its characters):
//!
//! - relay request: destination node ID, purpose byte; answered with one
//!   [`Outcome`] byte;
//! - relayed stream: purpose byte, client node ID; answered the same way.

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_identity::{NODE_ID_BYTES, NodeId};
use dassie_kit::ClientKey;
use dassie_node_core::config::NodeConfig;
use dassie_node_core::connections::ConnectionLimits;
use quinn::{Connection, RecvStream, SendStream};
use rustls::pki_types::ServerName;
use tokio::io::Join;
use tokio::sync::watch;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::keys::{self, AnyNodeKey, Keys, PinnedServer, Present};
use super::{CONNECT_TIMEOUT, Handler, Link, MAX_STREAMS, On, Quic, StreamKind, no_room, stand_by};
use crate::log::target;
use crate::node::lock;
use crate::outbound::Resolver;
use crate::slots::{ClientSlot, ClientSlots};
use crate::tls;

/// Time to send a stream's header, and for each step of joining a relayed
/// stream: finding the destination's connection, opening a stream on it,
/// and hearing whether the destination takes it.
pub(super) const HEADER_TIMEOUT: Duration = Duration::from_secs(10);

/// Time a client waits for a relay to join it: each of the relay's steps.
const JOIN_TIMEOUT: Duration = Duration::from_secs(4 * HEADER_TIMEOUT.as_secs());

/// Time for the TLS handshake inside a relayed stream.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// A relayed stream that carries nothing either way for this long ends.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest a relayed stream lasts, however busy: a client that needs
/// longer opens another.
const LIFETIME: Duration = Duration::from_secs(15 * 60);

/// Bytes a relay holds for each direction of a relayed stream, beyond what
/// QUIC's windows hold.
const COPY_BUFFER_BYTES: usize = 16 * 1024;

/// Most relayed streams a relay carries to one destination at once.
pub const MAX_RELAYED_PER_DESTINATION: usize = 16;

/// Most relayed streams a relay carries for one client address to one
/// destination at once: less than the destination's share, so that one
/// address cannot shut every other client out of a node.
pub const MAX_RELAYED_PER_CLIENT_TO_DESTINATION: usize = 4;

/// Most relayed streams a relay carries for one client address at once, to
/// all destinations: a front door reaches every node it serves from one
/// address.
pub const MAX_RELAYED_PER_CLIENT: u32 = 256;

/// Most relayed streams a relay carries at once, in all.
pub const MAX_RELAYED: usize = 1024;

/// Most relayed streams a node takes at once, from all relays.
pub const MAX_SERVED: usize = 256;

/// Most relayed streams a node takes at once from one relay address: a
/// share, so that a few nodes that open relayed streams to it cannot take
/// every place.
pub const MAX_SERVED_PER_RELAY: u32 = 16;

/// Least time between attempts to reach the chosen relay.
const RELAY_BACKOFF_MIN: Duration = Duration::from_secs(1);

/// Most time between attempts to reach the chosen relay.
const RELAY_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// A connection to the chosen relay that lasted this long was not refused:
/// once it is lost, the node dials again after the least backoff.
const RELAY_HELD_FOR: Duration = RELAY_BACKOFF_MAX;

/// How the node stands with the relay it chose
/// (spec/areas/node.md#a-chosen-relay).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayLink {
    /// The node holds its connection to the relay.
    Connected,
    /// The node dials the relay, or waits to dial it again, and has not
    /// failed long enough for that to be a fault: right after it starts,
    /// and after a brief drop.
    Connecting,
    /// The node has failed to reach the relay for so long that the wait
    /// before its next dial has reached its cap, [`RELAY_BACKOFF_MAX`]; or
    /// the relay is the node itself.
    Unreachable,
}

impl RelayLink {
    /// How the node stands with its relay while it waits `wait` to dial it
    /// again. While the wait still grows, the node is connecting: a relay
    /// that restarts, or a path that drops for a moment, is dialled again
    /// within seconds of coming back. Once the wait has reached its cap,
    /// every dial has failed for over a minute (the waits before it add up
    /// to that), longer than a restart or a brief drop lasts, and the node
    /// dials only once per cap: the relay is unreachable. A relay that takes
    /// each connection and drops it soon after reaches the cap the same way.
    /// So the cap is the bound, and no other constant decides when a relay
    /// reads as a fault.
    #[must_use]
    pub fn waiting(wait: Duration) -> Self {
        if wait >= RELAY_BACKOFF_MAX {
            Self::Unreachable
        } else {
            Self::Connecting
        }
    }

    pub(super) const fn byte(self) -> u8 {
        match self {
            Self::Connected => 0,
            Self::Connecting => 1,
            Self::Unreachable => 2,
        }
    }

    const fn from_byte(byte: u8) -> Self {
        match byte {
            0 => Self::Connected,
            2 => Self::Unreachable,
            _ => Self::Connecting,
        }
    }
}

/// The name the TLS inside a relayed stream asks for. Nobody checks it: the
/// key is what counts.
const SERVER_NAME: &str = "dassie";

// Relayed streams take at most half of a connection's streams, so peer
// requests on it always have room.
const _: () = assert!(MAX_RELAYED_PER_DESTINATION * 2 <= MAX_STREAMS as usize);
// No one client address, nor one relay address, takes every place.
const _: () = assert!(MAX_RELAYED_PER_CLIENT_TO_DESTINATION < MAX_RELAYED_PER_DESTINATION);
const _: () = assert!((MAX_RELAYED_PER_CLIENT as usize) < MAX_RELAYED);
const _: () = assert!(MAX_SERVED_PER_RELAY as usize * 2 <= MAX_STREAMS as usize);
const _: () = assert!((MAX_SERVED_PER_RELAY as usize) < MAX_SERVED);

/// While a node keeps its chosen relay
/// (spec/areas/node.md#a-chosen-relay).
#[derive(Clone, Debug)]
pub enum KeepWhile {
    /// Always: the owner named it (`DASSIE_RELAY`).
    Always,
    /// While the node's configuration has no hostname: the default relay
    /// is the front door of the node's default name, which a node with a
    /// hostname does not answer under (spec/decisions/default-relay.md).
    NoHostname(watch::Receiver<NodeConfig>),
}

impl KeepWhile {
    /// Whether the node keeps the relay now.
    #[must_use]
    pub fn holds(&self) -> bool {
        match self {
            Self::Always => true,
            Self::NoHostname(config) => config.borrow().hostname.is_none(),
        }
    }

    /// Whether the node keeps the relay now, marking the configuration
    /// seen, so that [`changed`](Self::changed) waits for the next change.
    fn holds_seen(&mut self) -> bool {
        match self {
            Self::Always => true,
            Self::NoHostname(config) => config.borrow_and_update().hostname.is_none(),
        }
    }

    /// Returns once whether it holds may have changed: never for
    /// [`Always`](Self::Always), nor once the configuration has no writer.
    async fn changed(&mut self) {
        let changed = match self {
            Self::Always => false,
            Self::NoHostname(config) => config.changed().await.is_ok(),
        };
        if !changed {
            std::future::pending::<()>().await;
        }
    }

    /// Returns once it no longer holds.
    async fn ended(&mut self) {
        while self.holds_seen() {
            self.changed().await;
        }
    }
}

/// The relay a node chose: its ID and URL, how to resolve its name, and
/// while to keep it.
#[derive(Clone)]
pub(super) struct Chosen {
    relay: NodeId,
    url: String,
    resolver: Resolver,
    keep: KeepWhile,
}

/// How holding the connection to the chosen relay ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Held {
    /// The connection closed.
    Lost,
    /// The node no longer keeps the relay, and closed it.
    Released,
    /// The endpoint is shutting down.
    Shutdown,
}

/// What a client wants from the node it reaches through a relay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// The owner's app, speaking the RPC contract (`owner_app`).
    Owner = 1,
    /// Open Payments, over HTTP/1.1.
    Payments = 2,
}

impl Purpose {
    /// The purpose a byte names, if any.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Owner),
            2 => Some(Self::Payments),
            _ => None,
        }
    }

    /// The byte that names it.
    #[must_use]
    pub const fn byte(self) -> u8 {
        self as u8
    }

    /// The ALPN of the TLS inside a stream for this purpose.
    #[must_use]
    pub const fn alpn(self) -> &'static [u8] {
        match self {
            Self::Owner => b"dassie-owner/1",
            Self::Payments => b"dassie-pay/1",
        }
    }
}

/// What a relay answers a relay request, and a destination a relayed
/// stream: one byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Joined: the bytes that follow are the destination's.
    Joined = 0,
    /// The relay holds no connection to the destination.
    UnknownDestination = 1,
    /// A bound was reached, or the destination did not answer in time.
    Busy = 2,
    /// The destination does not serve the purpose.
    Refused = 3,
}

impl Outcome {
    /// The byte that says it.
    #[must_use]
    pub const fn byte(self) -> u8 {
        self as u8
    }

    /// What a destination's answer means to the client. Anything but the
    /// bytes for joined and busy is a refusal.
    const fn from_destination(byte: u8) -> Self {
        match byte {
            0 => Self::Joined,
            2 => Self::Busy,
            _ => Self::Refused,
        }
    }
}

/// Why a client could not open a relayed stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayError {
    /// No connection to the relay could be made, or we have no key yet.
    Unreachable,
    /// The relay holds no connection to the destination.
    UnknownDestination,
    /// The relay or the destination is at a bound; try later.
    Busy,
    /// The destination does not serve the purpose.
    Refused,
    /// The stream broke, or the relay did not answer in time.
    Broken,
    /// The TLS handshake with the destination failed: not the node asked
    /// for, or it did not finish in time.
    Handshake,
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreachable => "the relay cannot be reached",
            Self::UnknownDestination => "the relay holds no connection to the node",
            Self::Busy => "the relay or the node is busy",
            Self::Refused => "the node does not serve this purpose",
            Self::Broken => "the relayed stream broke",
            Self::Handshake => "the TLS handshake with the node failed",
        })
    }
}

impl std::error::Error for RelayError {}

/// A QUIC stream as one byte stream, both ways.
pub type StreamIo = Join<RecvStream, SendStream>;

/// A relayed stream, as its client holds it: TLS with the destination.
pub type RelayedStream = tokio_rustls::client::TlsStream<StreamIo>;

/// A relayed stream, as its destination holds it: TLS with the client.
pub type ServedStream = tokio_rustls::server::TlsStream<StreamIo>;

/// Sees every chunk a relay copies, for tests.
type Tap = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// What an endpoint counts of the relayed streams it carries and takes.
pub(super) struct Relaying {
    /// As relay: streams in all and per client address.
    clients: ClientSlots,
    /// As relay: streams per destination, and per client address to it.
    destinations: Mutex<BTreeMap<NodeId, Share>>,
    /// As destination: streams taken, in all and per relay address.
    served: ClientSlots,
    tap: Mutex<Option<Tap>>,
}

impl Relaying {
    pub(super) fn new() -> Self {
        Self {
            clients: ClientSlots::new(ConnectionLimits {
                max_total: MAX_RELAYED,
                max_per_client: MAX_RELAYED_PER_CLIENT,
            }),
            destinations: Mutex::new(BTreeMap::new()),
            served: ClientSlots::new(ConnectionLimits {
                max_total: MAX_SERVED,
                max_per_client: MAX_SERVED_PER_RELAY,
            }),
            tap: Mutex::new(None),
        }
    }

    /// A place for one more stream to `node` from `client`, unless the
    /// destination has its share, or the client its share of it. A client
    /// without a key (on loopback, where private peers are allowed) is
    /// bounded by the destination's share alone.
    fn to_destination(&self, node: NodeId, client: Option<ClientKey>) -> Option<ToDestination<'_>> {
        let mut destinations = lock(&self.destinations);
        // Each entry holds a stream, and a client slot is taken first.
        assert!(destinations.len() <= MAX_RELAYED);
        let share = destinations.entry(node).or_default();
        let from_client = client.map_or(0, |key| share.clients.get(&key).copied().unwrap_or(0));
        if share.streams >= MAX_RELAYED_PER_DESTINATION
            || from_client >= MAX_RELAYED_PER_CLIENT_TO_DESTINATION
        {
            if share.streams == 0 {
                destinations.remove(&node);
            }
            return None;
        }
        share.streams += 1;
        if let Some(key) = client {
            *share.clients.entry(key).or_insert(0) += 1;
        }
        assert!(share.clients.len() <= share.streams);
        Some(ToDestination {
            relaying: self,
            node,
            client,
        })
    }
}

/// The relayed streams a relay carries to one destination: in all, and per
/// client address.
#[derive(Debug, Default)]
struct Share {
    streams: usize,
    clients: BTreeMap<ClientKey, usize>,
}

/// One relayed stream's place in its destination's share.
struct ToDestination<'a> {
    relaying: &'a Relaying,
    node: NodeId,
    client: Option<ClientKey>,
}

impl Drop for ToDestination<'_> {
    fn drop(&mut self) {
        let mut destinations = lock(&self.relaying.destinations);
        let Some(share) = destinations.get_mut(&self.node) else {
            return;
        };
        share.streams = share.streams.saturating_sub(1);
        if let Some(key) = self.client
            && let Some(count) = share.clients.get_mut(&key)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                share.clients.remove(&key);
            }
        }
        if share.streams == 0 {
            destinations.remove(&self.node);
        }
    }
}

/// A stream to the destination that took it, and the places it holds.
struct Joined<'a> {
    send: SendStream,
    recv: RecvStream,
    _client: ClientSlot,
    _destination: ToDestination<'a>,
}

impl Quic {
    /// Opens a relayed stream to `destination` for `purpose`, through the
    /// relay `(node ID, URL)`: makes or reuses a connection to the relay,
    /// pinned to its key, sends the relay request and, once joined, runs
    /// TLS with the destination, pinned to its key. What comes back reaches
    /// only the destination.
    ///
    /// # Errors
    ///
    /// See [`RelayError`].
    pub async fn open_relayed(
        self: &Arc<Self>,
        relay: (NodeId, &str),
        destination: NodeId,
        purpose: Purpose,
        resolver: &Resolver,
    ) -> Result<RelayedStream, RelayError> {
        if self.keys.current().is_none() {
            return Err(RelayError::Unreachable);
        }
        let place = self.place(relay.0).ok_or(RelayError::Unreachable)?;
        // A relayed stream dials its relay whenever it is asked for, backoff
        // or not: its caller paces it. A relay out of reach is not backed
        // off from for peer requests either.
        let connection = self
            .connection(&place, relay, resolver, CONNECT_TIMEOUT, true)
            .await
            .map_err(|_| RelayError::Unreachable)?;
        // A relay with no room for us read nothing: it is busy, not broken.
        let stream = ask_relay(&connection, destination, purpose)
            .await
            .map_err(|error| {
                if no_room(&connection) {
                    RelayError::Busy
                } else {
                    error
                }
            })?;
        self.secure(destination, purpose, stream).await
    }

    /// Opens a relayed stream for `purpose` to `destination` on the
    /// connection it keeps to this endpoint, as relay and client in one:
    /// how a front door reaches the nodes that chose it as their relay
    /// (spec/areas/http.md#front-doors). The stream names this endpoint's
    /// own node ID as the client, and the TLS inside it is pinned to the
    /// destination's key, as through any relay. It never dials.
    ///
    /// # Errors
    ///
    /// [`RelayError::UnknownDestination`] when no connection to the
    /// destination is open; otherwise as [`open_relayed`](Self::open_relayed).
    pub async fn open_held(
        self: &Arc<Self>,
        destination: NodeId,
        purpose: Purpose,
    ) -> Result<RelayedStream, RelayError> {
        let Some((own, _)) = self.keys.current() else {
            return Err(RelayError::Unreachable);
        };
        let connection = self
            .held(destination)
            .await
            .ok_or(RelayError::UnknownDestination)?;
        let stream = open_relayed_on(&connection, own, purpose).await?;
        self.secure(destination, purpose, stream).await
    }

    /// Opens a relayed stream for `purpose` to `destination` at `url`
    /// directly, as relay and client in one: how an owner app reaches a node
    /// it can dial, on the local network or at the node's own URL. The
    /// stream names this endpoint's own node ID as the client, and the TLS
    /// inside it is pinned to the destination's key.
    ///
    /// # Errors
    ///
    /// [`RelayError::Unreachable`] when the node cannot be dialled;
    /// otherwise as [`open_relayed`](Self::open_relayed).
    pub async fn open_direct(
        self: &Arc<Self>,
        (destination, url): (NodeId, &str),
        purpose: Purpose,
        resolver: &Resolver,
    ) -> Result<RelayedStream, RelayError> {
        let Some((own, _)) = self.keys.current() else {
            return Err(RelayError::Unreachable);
        };
        let place = self.place(destination).ok_or(RelayError::Unreachable)?;
        let connection = self
            .connection(&place, (destination, url), resolver, CONNECT_TIMEOUT, true)
            .await
            .map_err(|_| RelayError::Unreachable)?;
        let stream = open_relayed_on(&connection, own, purpose).await?;
        self.secure(destination, purpose, stream).await
    }

    /// Runs TLS for `purpose` inside a joined stream, pinned to
    /// `destination`'s key and presenting our own.
    async fn secure(
        &self,
        destination: NodeId,
        purpose: Purpose,
        (send, recv): (SendStream, RecvStream),
    ) -> Result<RelayedStream, RelayError> {
        let config =
            client_tls(&self.keys, destination, purpose).map_err(|_| RelayError::Handshake)?;
        let name = ServerName::try_from(SERVER_NAME).map_err(|_| RelayError::Handshake)?;
        let connecting = TlsConnector::from(config).connect(name, tokio::io::join(recv, send));
        match tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting).await {
            Ok(Ok(stream)) if stream.get_ref().1.alpn_protocol() == Some(purpose.alpn()) => {
                Ok(stream)
            }
            _ => Err(RelayError::Handshake),
        }
    }

    /// Chooses the relay `relay` at `url`, which [`serve`](Self::serve)
    /// keeps a connection to while it runs and `keep` holds
    /// ([`keep_relay`](Self::keep_relay)), resolving its name with
    /// `resolver`.
    pub fn choose_relay(&self, relay: NodeId, url: String, resolver: Resolver, keep: KeepWhile) {
        *lock(&self.chosen_relay) = Some(Chosen {
            relay,
            url,
            resolver,
            keep,
        });
    }

    /// The relay the node chose and keeps now, its URL, and how the node
    /// stands with it now ([`keep_relay`](Self::keep_relay)). The default
    /// relay is no choice of a node with a hostname, which keeps none.
    #[must_use]
    pub fn chosen_relay(&self) -> Option<(NodeId, String, RelayLink)> {
        let link = self.relay_link();
        lock(&self.chosen_relay)
            .as_ref()
            .filter(|chosen| chosen.keep.holds())
            .map(|chosen| (chosen.relay, chosen.url.clone(), link))
    }

    /// How the node stands with the relay it keeps a connection to
    /// ([`keep_relay`](Self::keep_relay)).
    #[must_use]
    pub fn relay_link(&self) -> RelayLink {
        RelayLink::from_byte(self.relay_link.load(Ordering::Relaxed))
    }

    fn set_relay_link(&self, link: RelayLink) {
        self.relay_link.store(link.byte(), Ordering::Relaxed);
    }

    /// Keeps the chosen relay's connection on `tasks`, if a relay was
    /// chosen, until `shutdown` or the returned token is cancelled.
    pub(super) fn keep_chosen_relay(
        self: &Arc<Self>,
        tasks: &TaskTracker,
        shutdown: &CancellationToken,
    ) -> CancellationToken {
        let stop = shutdown.child_token();
        if let Some(chosen) = lock(&self.chosen_relay).clone() {
            tasks.spawn(Arc::clone(self).keep_relay(
                chosen.relay,
                chosen.url,
                chosen.resolver,
                chosen.keep,
                stop.clone(),
            ));
        }
        stop
    }

    /// Keeps a connection to the relay `relay` at `url`, which the node
    /// chose, so that the relay can reach it: dials it once the node has
    /// its key and serves, keeps it from going idle, answers on it, and
    /// dials again when it is lost, until `shutdown`. The wait before a
    /// dial doubles after each that fails or whose connection did not last
    /// [`RELAY_HELD_FOR`], so a relay that takes the connection and closes
    /// it at once is not dialled every second. A node never dials itself.
    /// While `keep` does not hold, it neither dials nor keeps the
    /// connection, and once it holds again it starts afresh. Meanwhile
    /// [`relay_link`](Self::relay_link) says how it stands with the relay
    /// ([`RelayLink::waiting`]).
    pub async fn keep_relay(
        self: Arc<Self>,
        relay: NodeId,
        url: String,
        resolver: Resolver,
        mut keep: KeepWhile,
        shutdown: CancellationToken,
    ) {
        let mut backoff = RELAY_BACKOFF_MIN;
        let mut warned = false;
        while !shutdown.is_cancelled() {
            if !keep.holds_seen() {
                backoff = RELAY_BACKOFF_MIN;
                self.set_relay_link(RelayLink::Connecting);
                tokio::select! {
                    () = keep.changed() => continue,
                    () = shutdown.cancelled() => return,
                }
            }
            let mut wait = RELAY_BACKOFF_MIN;
            match self.keys.current() {
                Some((own, _)) if own == relay => {
                    if !warned {
                        tracing::warn!(target: target::PEER, %relay, "DASSIE_RELAY names this node itself; it keeps no connection to a relay");
                        warned = true;
                    }
                    wait = RELAY_BACKOFF_MAX;
                }
                Some(_) if self.is_serving() => {
                    let dialled = self
                        .dial((relay, &url), &resolver, CONNECT_TIMEOUT, true)
                        .await;
                    if let Some(connection) = dialled {
                        let since = tokio::time::Instant::now();
                        match self
                            .hold_kept(relay, &connection, &mut keep, &shutdown)
                            .await
                        {
                            Held::Lost => {}
                            Held::Released => continue,
                            Held::Shutdown => return,
                        }
                        if since.elapsed() >= RELAY_HELD_FOR {
                            backoff = RELAY_BACKOFF_MIN;
                        }
                        tracing::info!(target: target::PEER, %relay, ?backoff, "lost the chosen relay; connecting again");
                    } else {
                        tracing::debug!(target: target::PEER, %relay, ?backoff, "the chosen relay cannot be reached");
                    }
                    wait = backoff;
                    backoff = backoff.saturating_mul(2).min(RELAY_BACKOFF_MAX);
                }
                _ => {}
            }
            self.set_relay_link(RelayLink::waiting(wait));
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = shutdown.cancelled() => return,
            }
        }
    }

    /// Holds the connection to the chosen relay ([`hold_relay`](Self::hold_relay))
    /// until it is lost, `keep` no longer holds, which closes it, or
    /// `shutdown`. A connection whose dial ended after `keep` stopped
    /// holding, when a hostname was set meanwhile, is closed at once: it
    /// is never reported as connected.
    async fn hold_kept(
        self: &Arc<Self>,
        relay: NodeId,
        connection: &Connection,
        keep: &mut KeepWhile,
        shutdown: &CancellationToken,
    ) -> Held {
        if !keep.holds_seen() {
            connection.close(0u32.into(), b"no longer chosen");
            return Held::Released;
        }
        tracing::info!(target: target::PEER, %relay, "connected to the chosen relay");
        self.set_relay_link(RelayLink::Connected);
        let held = tokio::select! {
            () = self.hold_relay(relay, connection) => Held::Lost,
            () = keep.ended() => Held::Released,
            () = shutdown.cancelled() => Held::Shutdown,
        };
        // Once lost, how it stands follows from the wait before the next
        // dial, which the caller knows.
        self.set_relay_link(RelayLink::Connecting);
        if held == Held::Released {
            connection.close(0u32.into(), b"no longer chosen");
            tracing::info!(target: target::PEER, %relay, "the node has a hostname now; it keeps no connection to the default relay");
        }
        held
    }

    /// Answers on the connection to the chosen relay, and uses it for
    /// requests to the relay too: at once, unless another is open, and
    /// otherwise once that one closes ([`stand_by`]). Returns once the
    /// connection closes.
    async fn hold_relay(self: &Arc<Self>, relay: NodeId, connection: &Connection) {
        self.answer_on(connection);
        if let Some(place) = self.place(relay) {
            let link = Link {
                connection: connection.clone(),
                ours: true,
            };
            stand_by(&place, link).await;
        }
        let _ = connection.closed().await;
    }

    /// For tests: `tap` sees every chunk of bytes this endpoint relays, as
    /// it relays it.
    #[doc(hidden)]
    pub fn tap_relayed(&self, tap: impl Fn(&[u8]) + Send + Sync + 'static) {
        *lock(&self.relaying.tap) = Some(Arc::new(tap));
    }

    /// Whether this endpoint holds an open connection to `node` now, which
    /// [`open_held`](Self::open_held) would open streams on.
    pub async fn holds_connection(&self, node: NodeId) -> bool {
        self.held(node).await.is_some()
    }

    /// The open connection to `node` this endpoint holds, whoever dialled
    /// it, if any. A relay never dials: it reaches only nodes connected to
    /// it.
    async fn held(&self, node: NodeId) -> Option<Connection> {
        let place = lock(&self.links).get(&node).cloned()?;
        let held = tokio::time::timeout(HEADER_TIMEOUT, place.lock())
            .await
            .ok()?;
        held.as_ref()
            .filter(|link| link.is_open())
            .map(|link| link.connection.clone())
    }

    /// Joins a stream from `client` at `address` to `destination`, within
    /// the bounds, if the destination takes it.
    async fn join(
        &self,
        handler: &dyn Handler,
        address: IpAddr,
        (client, destination): (NodeId, NodeId),
        purpose: u8,
    ) -> Result<Joined<'_>, Outcome> {
        let key = handler.client(address);
        let client_slot = self.relaying.clients.take(key).map_err(|_| Outcome::Busy)?;
        let to_destination = self
            .relaying
            .to_destination(destination, key)
            .ok_or(Outcome::Busy)?;
        let connection = self
            .held(destination)
            .await
            .ok_or(Outcome::UnknownDestination)?;
        let (mut send, mut recv) =
            match tokio::time::timeout(HEADER_TIMEOUT, connection.open_bi()).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(_)) => return Err(Outcome::UnknownDestination),
                Err(_) => return Err(Outcome::Busy),
            };
        let header = relayed_header(purpose, &client);
        let answer = tokio::time::timeout(HEADER_TIMEOUT, async {
            send.write_all(&header).await.ok()?;
            read_byte(&mut recv).await
        })
        .await;
        let outcome = match answer {
            Ok(Some(byte)) => Outcome::from_destination(byte),
            // A node that does not know relayed streams resets them.
            Ok(None) => Outcome::Refused,
            Err(_) => Outcome::Busy,
        };
        if outcome != Outcome::Joined {
            let _ = send.reset(0u32.into());
            return Err(outcome);
        }
        Ok(Joined {
            send,
            recv,
            _client: client_slot,
            _destination: to_destination,
        })
    }
}

impl On {
    /// As relay: reads a relay request, joins it to its destination if it
    /// can, answers the client, and copies bytes both ways.
    pub(super) async fn relay(self, mut send: SendStream, mut recv: RecvStream) {
        let header = tokio::time::timeout(HEADER_TIMEOUT, read_relay_request(&mut recv)).await;
        let (Ok(Some((destination, purpose))), Some(client)) =
            (header, keys::peer_node(&self.connection))
        else {
            let _ = send.reset(0u32.into());
            return;
        };
        if !self.handler.relays() {
            let refused = [Outcome::Refused.byte()];
            let _ = tokio::time::timeout(HEADER_TIMEOUT, send.write_all(&refused)).await;
            let _ = send.finish();
            return;
        }
        let joined = self
            .quic
            .join(&*self.handler, self.client, (client, destination), purpose)
            .await;
        let outcome = joined
            .as_ref()
            .map_or_else(|outcome| *outcome, |_| Outcome::Joined);
        tracing::debug!(target: target::PEER, %client, %destination, ?outcome, "relay request");
        let told = tokio::time::timeout(HEADER_TIMEOUT, send.write_all(&[outcome.byte()])).await;
        let Ok(joined) = joined else {
            let _ = send.finish();
            return;
        };
        if !matches!(told, Ok(Ok(()))) {
            let _ = send.reset(0u32.into());
            return;
        }
        let tap = lock(&self.quic.relaying.tap).clone();
        let Joined {
            send: to_send,
            recv: to_recv,
            _client,
            _destination,
        } = joined;
        pump((recv, send), (to_recv, to_send), tap, &self.shutdown).await;
    }

    /// As destination: reads a relayed stream's header, answers whether the
    /// node takes it, runs TLS with the client, and hands the decrypted
    /// stream to the handler. The client must prove the key of the node the
    /// relay named.
    pub(super) async fn serve_relayed(self, mut send: SendStream, mut recv: RecvStream) {
        let header = tokio::time::timeout(HEADER_TIMEOUT, read_relayed_header(&mut recv)).await;
        let Ok(Some((purpose, claimed))) = header else {
            let _ = send.reset(0u32.into());
            return;
        };
        let purpose = Purpose::from_byte(purpose).filter(|purpose| self.handler.serves(*purpose));
        let served = &self.quic.relaying.served;
        let counted = served.take(self.handler.client(self.client)).ok();
        let (outcome, purpose) = match (purpose, &counted) {
            (None, _) => (Outcome::Refused, None),
            (Some(_), None) => (Outcome::Busy, None),
            (Some(purpose), Some(_)) => (Outcome::Joined, Some(purpose)),
        };
        let told = tokio::time::timeout(HEADER_TIMEOUT, send.write_all(&[outcome.byte()])).await;
        let (Some(purpose), Ok(Ok(()))) = (purpose, told) else {
            tracing::debug!(target: target::PEER, client = %claimed, ?outcome, "relayed stream not taken");
            let _ = send.finish();
            return;
        };
        let Some(stream) = accept_tls(&self.quic.keys, purpose, claimed, (send, recv)).await else {
            return;
        };
        let relay = self.connection.remote_address();
        let serving =
            self.handler
                .serve_relayed(purpose, claimed, relay, stream, self.shutdown.clone());
        let _ = tokio::time::timeout(LIFETIME, serving).await;
        drop(counted);
    }
}

/// Opens a relayed stream for `purpose` on `connection` to its destination,
/// naming `own` as the client, and reads whether the destination takes it;
/// the stream, once joined.
async fn open_relayed_on(
    connection: &Connection,
    own: NodeId,
    purpose: Purpose,
) -> Result<(SendStream, RecvStream), RelayError> {
    let (mut send, mut recv) =
        match tokio::time::timeout(HEADER_TIMEOUT, connection.open_bi()).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => return Err(RelayError::UnknownDestination),
            Err(_) => return Err(RelayError::Busy),
        };
    let header = relayed_header(purpose.byte(), &own);
    let answer = tokio::time::timeout(HEADER_TIMEOUT, async {
        send.write_all(&header).await.ok()?;
        read_byte(&mut recv).await
    })
    .await;
    let failed = match answer.map(|byte| byte.map(Outcome::from_destination)) {
        Ok(Some(Outcome::Joined)) => None,
        Ok(Some(Outcome::Busy)) | Err(_) => Some(RelayError::Busy),
        Ok(Some(_)) => Some(RelayError::Refused),
        Ok(None) => Some(RelayError::Broken),
    };
    if let Some(failed) = failed {
        let _ = send.reset(0u32.into());
        return Err(failed);
    }
    Ok((send, recv))
}

/// Sends a relay request on a new stream of `connection` and reads the
/// relay's answer; the stream, once joined.
async fn ask_relay(
    connection: &Connection,
    destination: NodeId,
    purpose: Purpose,
) -> Result<(SendStream, RecvStream), RelayError> {
    let (mut send, mut recv) =
        match tokio::time::timeout(HEADER_TIMEOUT, connection.open_bi()).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => return Err(RelayError::Broken),
            Err(_) => return Err(RelayError::Busy),
        };
    let request = relay_request(&destination, purpose.byte());
    let answer = tokio::time::timeout(JOIN_TIMEOUT, async {
        send.write_all(&request).await.ok()?;
        read_byte(&mut recv).await
    })
    .await;
    match answer {
        Ok(Some(0)) => Ok((send, recv)),
        Ok(Some(1)) => Err(RelayError::UnknownDestination),
        Ok(Some(2)) => Err(RelayError::Busy),
        Ok(Some(_)) => Err(RelayError::Refused),
        Ok(None) | Err(_) => Err(RelayError::Broken),
    }
}

/// Runs TLS as the destination of a relayed stream, presenting the node's
/// key and requiring the client's, which must be `claimed`'s.
async fn accept_tls(
    keys: &Arc<Keys>,
    purpose: Purpose,
    claimed: NodeId,
    (send, recv): (SendStream, RecvStream),
) -> Option<ServedStream> {
    let config = server_tls(keys, purpose).ok()?;
    let accepting = TlsAcceptor::from(config).accept(tokio::io::join(recv, send));
    let stream = match tokio::time::timeout(HANDSHAKE_TIMEOUT, accepting).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            tracing::debug!(target: target::PEER, client = %claimed, %error, "relayed TLS handshake failed");
            return None;
        }
        Err(_) => return None,
    };
    let proven = match stream.get_ref().1.peer_certificates() {
        Some([leaf]) => keys::node_key_of(leaf).map(|key| key.node_id()),
        _ => None,
    };
    if proven != Some(claimed) {
        tracing::debug!(target: target::PEER, client = %claimed, ?proven, "relayed client is not the node the relay named");
        return None;
    }
    Some(stream)
}

/// Copies bytes both ways between two streams until both ends finish, one
/// breaks, nothing moves for [`IDLE_TIMEOUT`], the stream outlives
/// [`LIFETIME`] or `shutdown`. Short of a clean end, both are reset.
async fn pump(
    a: (RecvStream, SendStream),
    b: (RecvStream, SendStream),
    tap: Option<Tap>,
    shutdown: &CancellationToken,
) {
    let ((mut a_recv, mut a_send), (mut b_recv, mut b_send)) = (a, b);
    let started = tokio::time::Instant::now();
    let moved_ms = AtomicU64::new(0);
    let ended = {
        let both = async {
            tokio::try_join!(
                copy(&mut a_recv, &mut b_send, (&moved_ms, started), tap.as_ref()),
                copy(&mut b_recv, &mut a_send, (&moved_ms, started), tap.as_ref()),
            )
        };
        tokio::select! {
            copied = both => copied.is_ok(),
            () = idle(&moved_ms, started) => false,
            () = tokio::time::sleep(LIFETIME) => false,
            () = shutdown.cancelled() => false,
        }
    };
    if !ended {
        let _ = a_send.reset(0u32.into());
        let _ = b_send.reset(0u32.into());
        let _ = a_recv.stop(0u32.into());
        let _ = b_recv.stop(0u32.into());
    }
}

/// One direction of a relayed stream broke.
struct Broke;

/// Copies `from` to `to` until `from` ends, then ends `to`; notes in
/// `moved_ms` when bytes last moved, in milliseconds since `started`.
async fn copy(
    from: &mut RecvStream,
    to: &mut SendStream,
    (moved_ms, started): (&AtomicU64, tokio::time::Instant),
    tap: Option<&Tap>,
) -> Result<(), Broke> {
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    loop {
        match from.read(&mut buffer).await {
            Ok(Some(read)) => {
                let chunk = buffer.get(..read).ok_or(Broke)?;
                if let Some(tap) = tap {
                    tap(chunk);
                }
                to.write_all(chunk).await.map_err(|_| Broke)?;
                let since = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                moved_ms.store(since, Ordering::Relaxed);
            }
            Ok(None) => return to.finish().map_err(|_| Broke),
            Err(_) => return Err(Broke),
        }
    }
}

/// Returns once nothing moved for [`IDLE_TIMEOUT`].
async fn idle(moved_ms: &AtomicU64, started: tokio::time::Instant) {
    loop {
        let last = moved_ms.load(Ordering::Relaxed);
        tokio::time::sleep_until(started + Duration::from_millis(last) + IDLE_TIMEOUT).await;
        if moved_ms.load(Ordering::Relaxed) == last {
            return;
        }
    }
}

/// A relay request: kind, destination, purpose.
fn relay_request(destination: &NodeId, purpose: u8) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 + NODE_ID_BYTES);
    bytes.push(StreamKind::Relay.byte());
    write_node_id(&mut bytes, destination);
    bytes.push(purpose);
    bytes
}

/// A relayed stream's header: kind, purpose, client.
fn relayed_header(purpose: u8, client: &NodeId) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 + NODE_ID_BYTES);
    bytes.push(StreamKind::Relayed.byte());
    bytes.push(purpose);
    write_node_id(&mut bytes, client);
    bytes
}

/// Writes a node ID as the peer protocol's OER does: its 24 bytes
/// (spec/areas/identity.md#node-id).
fn write_node_id(bytes: &mut Vec<u8>, id: &NodeId) {
    bytes.extend_from_slice(&id.to_bytes());
}

/// The rest of a relay request, after its kind: destination and purpose.
async fn read_relay_request(recv: &mut RecvStream) -> Option<(NodeId, u8)> {
    let destination = read_node_id(recv).await?;
    let purpose = read_byte(recv).await?;
    Some((destination, purpose))
}

/// The rest of a relayed stream's header, after its kind: purpose and
/// client.
async fn read_relayed_header(recv: &mut RecvStream) -> Option<(u8, NodeId)> {
    let purpose = read_byte(recv).await?;
    let client = read_node_id(recv).await?;
    Some((purpose, client))
}

async fn read_byte(recv: &mut RecvStream) -> Option<u8> {
    let mut byte = [0u8; 1];
    recv.read_exact(&mut byte).await.ok()?;
    let [byte] = byte;
    Some(byte)
}

/// Reads a node ID written by [`write_node_id`].
async fn read_node_id(recv: &mut RecvStream) -> Option<NodeId> {
    let mut bytes = [0u8; NODE_ID_BYTES];
    recv.read_exact(&mut bytes).await.ok()?;
    Some(NodeId::from_bytes(&bytes))
}

fn client_tls(
    keys: &Arc<Keys>,
    destination: NodeId,
    purpose: Purpose,
) -> Result<Arc<rustls::ClientConfig>, rustls::Error> {
    let mut config = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServer::new(destination)))
        .with_client_cert_resolver(Arc::new(Present(Arc::clone(keys))));
    config.alpn_protocols = vec![purpose.alpn().to_vec()];
    Ok(Arc::new(config))
}

fn server_tls(
    keys: &Arc<Keys>,
    purpose: Purpose,
) -> Result<Arc<rustls::ServerConfig>, rustls::Error> {
    let mut config = rustls::ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyNodeKey::new()))
        .with_cert_resolver(Arc::new(Present(Arc::clone(keys))));
    config.alpn_protocols = vec![purpose.alpn().to_vec()];
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dassie_identity::NodeKey;
    use dassie_oer::Encode;

    #[test]
    fn node_ids_are_written_as_the_peer_protocol_writes_them() {
        let id = NodeKey::from_bytes(&[1; 32]).node_id();
        let mut ours = Vec::new();
        write_node_id(&mut ours, &id);
        assert_eq!(ours, id.to_vec());
    }

    #[test]
    fn headers_lay_out_as_specified() {
        let id = NodeId::from_bytes(&[0xab; NODE_ID_BYTES]);
        let ab = [0xab; NODE_ID_BYTES];
        assert_eq!(relay_request(&id, 2), [&[1][..], &ab, &[2]].concat());
        assert_eq!(relayed_header(2, &id), [&[2, 2][..], &ab].concat());
    }

    #[test]
    fn purposes_and_outcomes_keep_their_bytes() {
        assert_eq!(Purpose::from_byte(1), Some(Purpose::Owner));
        assert_eq!(Purpose::from_byte(2), Some(Purpose::Payments));
        assert_eq!(Purpose::from_byte(3), None);
        assert_ne!(Purpose::Owner.alpn(), Purpose::Payments.alpn());
        assert_eq!(Outcome::from_destination(0), Outcome::Joined);
        assert_eq!(Outcome::from_destination(2), Outcome::Busy);
        assert_eq!(Outcome::from_destination(1), Outcome::Refused);
        assert_eq!(Outcome::from_destination(9), Outcome::Refused);
        assert_eq!(StreamKind::from_byte(3), Some(StreamKind::Hub));
        assert_eq!(StreamKind::from_byte(4), Some(StreamKind::HubState));
        assert_eq!(StreamKind::from_byte(5), Some(StreamKind::HubFounding));
        assert_eq!(StreamKind::from_byte(6), None);
    }

    // The waits a relay that refuses every dial gets, in turn: it reads as
    // connecting until the wait reaches its cap, by when its dials have
    // failed for at least that long.
    #[test]
    fn a_relay_reads_as_unreachable_once_the_wait_reaches_its_cap() {
        let mut backoff = RELAY_BACKOFF_MIN;
        let mut failing = Duration::ZERO;
        for _ in 0..64 {
            let wait = backoff;
            backoff = backoff.saturating_mul(2).min(RELAY_BACKOFF_MAX);
            if RelayLink::waiting(wait) == RelayLink::Unreachable {
                break;
            }
            failing = failing.saturating_add(wait);
        }
        assert_eq!(backoff, RELAY_BACKOFF_MAX);
        assert!(failing >= RELAY_BACKOFF_MAX, "{failing:?}");
        assert_eq!(RelayLink::waiting(RELAY_BACKOFF_MIN), RelayLink::Connecting);
        for link in [
            RelayLink::Connected,
            RelayLink::Connecting,
            RelayLink::Unreachable,
        ] {
            assert_eq!(RelayLink::from_byte(link.byte()), link);
        }
    }

    #[test]
    fn a_destination_has_a_bounded_share() {
        let relaying = Relaying::new();
        let node = NodeKey::from_bytes(&[2; 32]).node_id();
        let held: Vec<_> = (0..MAX_RELAYED_PER_DESTINATION)
            .map(|_| relaying.to_destination(node, None).unwrap())
            .collect();
        assert!(relaying.to_destination(node, None).is_none());
        drop(held);
        assert!(lock(&relaying.destinations).is_empty());
        assert!(relaying.to_destination(node, None).is_some());
    }

    #[test]
    fn one_client_address_takes_only_its_share_of_a_destination() {
        let relaying = Relaying::new();
        let (node, other) = (
            NodeKey::from_bytes(&[2; 32]).node_id(),
            NodeKey::from_bytes(&[3; 32]).node_id(),
        );
        let client = |last: u8| Some(ClientKey::from_ip(IpAddr::from([192, 0, 2, last])));
        let mut held: Vec<_> = (0..MAX_RELAYED_PER_CLIENT_TO_DESTINATION)
            .map(|_| relaying.to_destination(node, client(1)).unwrap())
            .collect();
        assert!(relaying.to_destination(node, client(1)).is_none());
        // Another destination, and another client, still have room.
        held.push(relaying.to_destination(other, client(1)).unwrap());
        held.push(relaying.to_destination(node, client(2)).unwrap());
        held.pop();
        held.pop();
        held.pop();
        assert!(relaying.to_destination(node, client(1)).is_some());
        drop(held);
        assert!(lock(&relaying.destinations).is_empty());
    }
}
