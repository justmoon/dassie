//! The peer protocol over QUIC (spec/areas/peer-protocol.md#transport).
//!
//! One UDP socket, on the HTTPS listener's port number, both takes and makes
//! connections. Each request is a bidirectional stream: the envelope one
//! way, then a two-byte status and the body back, with the same bytes and
//! statuses as over HTTPS. Each side proves its node key in the TLS
//! handshake ([`keys`]), so a dialler knows it reached the node it meant.
//!
//! A pair of nodes keeps one connection, which both use: the one the lower
//! node ID opened, when both dialled. A node whose QUIC cannot be reached
//! (UDP filtered, say) is asked over HTTPS for a while instead, until every
//! node takes QUIC and `POST /peer` goes.

pub mod keys;

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use dassie_identity::{NodeId, NodeKey};
use dassie_node_core::address::is_public;
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use dassie_peer::wire::MAX_MESSAGE_BYTES;
use dassie_peer::{Failure, Request, Response};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{Connection, Endpoint, EndpointConfig, ReadToEndError, TransportConfig};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use self::keys::{AnyNodeKey, Keys, PinnedServer, Present};
use crate::http::peer::{Answer, BodyError};
use crate::log::target;
use crate::peer_client::PeerClient;
use crate::slots::ClientSlots;
use crate::tls;

/// The protocol a connection speaks, by ALPN.
pub const ALPN: &[u8] = b"dassie/1";

/// Time to set up a connection. Past it, the node is asked over HTTPS.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection nothing was sent on for this long closes. Peers send
/// heartbeats more often.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a node whose QUIC could not be reached is asked over HTTPS.
const UNREACHABLE_FOR: Duration = Duration::from_secs(10 * 60);

/// Longest response read: a node list of 4096 IDs fits, as over HTTPS.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Most requests open at once on one connection, as many as the node takes
/// at once from everyone.
const MAX_STREAMS: u32 = 256;

/// Most nodes with a connection, or one being made, at once.
const MAX_LINKS: usize = 4096;

/// Time for open connections to finish their requests at shutdown.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// The status ahead of a response's body.
const STATUS_BYTES: usize = 2;

/// Why a request over QUIC was not answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// No connection could be made: nothing was sent, and HTTPS may try.
    Unreachable,
    /// The request was sent, or lost on a connection that was up.
    Failed(Failure),
}

/// A request's body, being read.
pub type Body<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, BodyError>> + Send + 'a>>;

/// What answers the requests the endpoint takes.
pub trait Handler: Send + Sync + 'static {
    /// The share of connections `address` counts in; `None` for the total
    /// only.
    fn client(&self, address: IpAddr) -> Option<ClientKey>;

    /// How many connections the endpoint holds, in all and per client.
    fn limits(&self) -> ConnectionLimits;

    /// Answers a request from `client` whose body `body` reads.
    fn answer<'a>(
        &'a self,
        client: IpAddr,
        body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>>;
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
}

/// The node's QUIC endpoint.
pub struct Quic {
    endpoint: Endpoint,
    keys: Arc<Keys>,
    links: Mutex<BTreeMap<NodeId, Arc<tokio::sync::Mutex<Option<Link>>>>>,
    unreachable: Mutex<BTreeMap<NodeId, Instant>>,
    serving: Mutex<Option<Serving>>,
    allow_private: bool,
    stats: Stats,
}

impl std::fmt::Debug for Quic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        let keys = Arc::new(Keys::new(identity));
        let server = server_config(&keys).map_err(io::Error::other)?;
        let runtime = quinn::default_runtime().ok_or_else(|| io::Error::other("no runtime"))?;
        let endpoint = Endpoint::new(EndpointConfig::default(), Some(server), socket, runtime)?;
        Ok(Self {
            endpoint,
            keys,
            links: Mutex::new(BTreeMap::new()),
            unreachable: Mutex::new(BTreeMap::new()),
            serving: Mutex::new(None),
            allow_private,
            stats: Stats::default(),
        })
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

    /// Sends `request` and reads its response. The caller bounds the time
    /// the whole exchange takes.
    ///
    /// # Errors
    ///
    /// [`SendError::Unreachable`] when no connection could be made, and
    /// nothing was sent; otherwise the failure, as over HTTPS.
    pub async fn send(
        self: &Arc<Self>,
        request: &Request,
        resolver: &PeerClient,
    ) -> Result<Response, SendError> {
        // Without its key the node cannot prove who it is. TLS 1.3 would
        // only say so after the handshake, on the first stream.
        if self.is_unreachable(&request.to) || self.keys.current().is_none() {
            return Err(SendError::Unreachable);
        }
        let Some(link) = self.link(request.to) else {
            return Err(SendError::Unreachable);
        };
        // A connection kept from before may have died since: then the request
        // was not sent, and a fresh one is made once.
        for attempt in 0..2 {
            let Some(connection) = self.connection(&link, request, resolver).await else {
                self.mark_unreachable(request.to);
                return Err(SendError::Unreachable);
            };
            let result = exchange(&connection, &request.body).await;
            if result == Err(Failure::NotDelivered) && attempt == 0 {
                let mut held = link.lock().await;
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

    /// The open connection in `link`, or a new one. Only making one holds
    /// the place, so requests to a node wait for its connection, never for
    /// each other.
    async fn connection(
        self: &Arc<Self>,
        link: &tokio::sync::Mutex<Option<Link>>,
        request: &Request,
        resolver: &PeerClient,
    ) -> Option<Connection> {
        let mut held = link.lock().await;
        if let Some(kept) = held.as_ref().filter(|kept| kept.is_open()) {
            return Some(kept.connection.clone());
        }
        *held = None;
        let connection = self.dial(request, resolver).await?;
        self.answer_on(&connection);
        *held = Some(Link {
            connection: connection.clone(),
            ours: true,
        });
        Some(connection)
    }

    /// The place for `node`'s connection, if the table has room.
    fn link(&self, node: NodeId) -> Option<Arc<tokio::sync::Mutex<Option<Link>>>> {
        let mut links = self.links.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(link) = links.get(&node) {
            return Some(Arc::clone(link));
        }
        if links.len() >= MAX_LINKS {
            // Places nobody holds and whose connection is gone are free.
            links.retain(|_, link| {
                Arc::strong_count(link) > 1
                    || link
                        .try_lock()
                        .map_or(true, |held| held.as_ref().is_some_and(Link::is_open))
            });
            if links.len() >= MAX_LINKS {
                return None;
            }
        }
        let link = Arc::new(tokio::sync::Mutex::new(None));
        links.insert(node, Arc::clone(&link));
        Some(link)
    }

    /// Makes a connection to the node `request` goes to, at its URL's host
    /// and port, checking that the server holds that node's key.
    async fn dial(&self, request: &Request, resolver: &PeerClient) -> Option<Connection> {
        let (host, port) = authority(&request.url)?;
        if let Ok(ip) = host.parse::<IpAddr>()
            && !self.allow_private
            && !is_public(ip)
        {
            return None;
        }
        let addresses = resolver.resolve(host, port).await.ok()?;
        // An endpoint on IPv4 alone cannot reach an IPv6 address.
        let ipv6 = self
            .endpoint
            .local_addr()
            .is_ok_and(|address| address.is_ipv6());
        let address = addresses
            .into_iter()
            .find(|address| ipv6 || address.is_ipv4())?;
        let config = client_config(&self.keys, request.to).ok()?;
        let connecting = self.endpoint.connect_with(config, address, host).ok()?;
        match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
            Ok(Ok(connection)) => Some(connection),
            Ok(Err(error)) => {
                tracing::debug!(target: target::PEER, to = %request.to, %error, "QUIC connection failed");
                None
            }
            Err(_) => {
                tracing::debug!(target: target::PEER, to = %request.to, "QUIC connection timed out");
                None
            }
        }
    }

    /// Keeps a connection a node opened to us, for our requests to it too:
    /// unless we hold one already that the lower node ID opened.
    fn keep_inbound(&self, peer: NodeId, connection: &Connection) {
        let Some((own, _)) = self.keys.current() else {
            return;
        };
        let Some(link) = self.link(peer) else {
            return;
        };
        // Held while a request dials or sends: that connection stays.
        let Ok(mut held) = link.try_lock() else {
            return;
        };
        let preferred = |link: &Link| link.ours == (own < peer);
        match held.as_ref() {
            Some(kept) if kept.is_open() && preferred(kept) => {}
            _ => {
                *held = Some(Link {
                    connection: connection.clone(),
                    ours: false,
                });
            }
        }
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
    /// `shutdown`; and answers on the connections the node makes.
    pub async fn serve(self: Arc<Self>, handler: Arc<dyn Handler>, shutdown: CancellationToken) {
        let slots = ClientSlots::new(handler.limits());
        let tasks = TaskTracker::new();
        *self.serving.lock().unwrap_or_else(PoisonError::into_inner) = Some(Serving {
            handler: Arc::clone(&handler),
            shutdown: shutdown.clone(),
            tasks: tasks.clone(),
        });
        loop {
            let incoming = tokio::select! {
                incoming = self.endpoint.accept() => incoming,
                () = shutdown.cancelled() => break,
            };
            let Some(incoming) = incoming else {
                break;
            };
            let client = incoming.remote_address().ip().to_canonical();
            let slot = match slots.take(handler.client(client)) {
                Ok(slot) => slot,
                Err(refused) => {
                    tracing::debug!(target: target::PEER, %client, ?refused, "too many QUIC connections; refusing");
                    incoming.refuse();
                    continue;
                }
            };
            let quic = Arc::clone(&self);
            let handler = Arc::clone(&handler);
            let shutdown = shutdown.clone();
            tasks.spawn(async move {
                let _slot = slot;
                let Ok(connecting) = incoming.accept() else {
                    return;
                };
                let Ok(Ok(connection)) = tokio::time::timeout(CONNECT_TIMEOUT, connecting).await
                else {
                    return;
                };
                if let Some(peer) = keys::peer_node(&connection) {
                    quic.keep_inbound(peer, &connection);
                }
                quic.requests(&connection, client, &handler, &shutdown)
                    .await;
            });
        }
        *self.serving.lock().unwrap_or_else(PoisonError::into_inner) = None;
        tasks.close();
        if shutdown.is_cancelled() {
            self.endpoint.close(0u32.into(), b"shutting down");
        }
        if tokio::time::timeout(DRAIN_TIMEOUT, tasks.wait())
            .await
            .is_err()
        {
            tracing::warn!(target: target::PEER, "QUIC connections still open after the drain timeout");
        }
    }

    /// Answers the requests on `connection`, each on a task of its own, until
    /// it closes or `shutdown`. The connection's stream limit bounds how
    /// many run at once.
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
            let Ok((mut send, mut recv)) = stream else {
                break;
            };
            let quic = Arc::clone(self);
            let handler = Arc::clone(handler);
            streams.spawn(async move {
                let body: Body<'_> = Box::pin(async move {
                    recv.read_to_end(MAX_MESSAGE_BYTES)
                        .await
                        .map_err(|error| match error {
                            ReadToEndError::TooLong => BodyError::TooLarge,
                            ReadToEndError::Read(_) => BodyError::Broken,
                        })
                });
                let answer = handler.answer(client, body).await;
                let mut bytes = Vec::with_capacity(STATUS_BYTES + answer.body.len());
                bytes.extend_from_slice(&answer.status.to_be_bytes());
                bytes.extend_from_slice(&answer.body);
                if send.write_all(&bytes).await.is_ok() && send.finish().is_ok() {
                    quic.stats.answered.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        streams.close();
        streams.wait().await;
    }
}

/// Sends `body` on a new stream of `connection` and reads the answer.
async fn exchange(connection: &Connection, body: &[u8]) -> Result<Response, Failure> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|_| Failure::NotDelivered)?;
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

fn transport() -> Arc<TransportConfig> {
    let mut transport = TransportConfig::default();
    transport.max_idle_timeout(IDLE_TIMEOUT.try_into().ok());
    transport.max_concurrent_bidi_streams(MAX_STREAMS.into());
    transport.max_concurrent_uni_streams(0u8.into());
    Arc::new(transport)
}

fn server_config(keys: &Arc<Keys>) -> Result<quinn::ServerConfig, String> {
    let mut config = rustls::ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_client_cert_verifier(Arc::new(AnyNodeKey::new()))
        .with_cert_resolver(Arc::new(Present(Arc::clone(keys))));
    config.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicServerConfig::try_from(config).map_err(|e| e.to_string())?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(transport());
    Ok(config)
}

fn client_config(keys: &Arc<Keys>, expected: NodeId) -> Result<quinn::ClientConfig, String> {
    let mut config = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServer::new(expected)))
        .with_client_cert_resolver(Arc::new(Present(Arc::clone(keys))));
    config.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = QuicClientConfig::try_from(config).map_err(|e| e.to_string())?;
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(transport());
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
}
