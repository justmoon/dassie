//! The front door's connections to the nodes it serves: a few relayed
//! streams per node, each a keep-alive HTTP/1.1 connection, opened on the
//! QUIC connection the node keeps to the front door
//! ([`Quic::open_held`]).
//!
//! A node gets at most [`STREAMS_PER_NODE`] at once, the share any one
//! client of a relay has of a destination, and the front door holds at
//! most [`MAX_STREAMS`] in all. A stream left idle is closed before the
//! node would close it, so a request rarely meets a connection that is
//! going away; one that did is found before anything is sent.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeId;
use http_body_util::{BodyExt, Limited};
use hyper::client::conn::http1::{self, SendRequest};
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::clock::{Clock, interval_passed};
use crate::http::response::Body;
use crate::log::target;
use crate::node::lock;
use crate::quic::Quic;
use crate::quic::relay::{MAX_RELAYED, MAX_RELAYED_PER_CLIENT_TO_DESTINATION, Purpose, RelayError};

/// Most streams to one node at once: what a relay lets one client address
/// hold to one destination.
pub const STREAMS_PER_NODE: usize = MAX_RELAYED_PER_CLIENT_TO_DESTINATION;

/// Most streams to all nodes at once: what a relay carries in all.
pub const MAX_STREAMS: usize = MAX_RELAYED;

/// A stream unused this long is closed: well before a node's HTTP server
/// closes an idle connection, after its time to send a request's head.
const IDLE_FOR: Duration = Duration::from_secs(5);

/// Time to open a stream and run its TLS.
const OPEN_TIMEOUT: Duration = Duration::from_secs(15);

/// Most time a request waits for one of its node's streams to come free,
/// when all are in use, before it is answered 503, in ms of the front
/// door's clock: of real time on the system clock, and of the clock on one
/// moved by hand, whose moves wake it.
const WAIT_FOR_STREAM_MS: u64 = 2_000;

/// Time for a node to answer a request in full.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest answer passed back, in bytes: lists of payments are paged.
pub const MAX_ANSWER_BYTES: usize = 256 * 1024;

const _: () = assert!(IDLE_FOR.as_secs() < crate::http::HEADER_TIMEOUT.as_secs());

/// Why a node gave no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missed {
    /// The node holds no connection to the front door.
    NotConnected,
    /// The node, or the front door, has no stream to spare.
    Busy,
    /// The stream, its TLS or the node's answer broke, or the answer was
    /// too long.
    Broken,
    /// The node did not answer in time.
    TimedOut,
}

/// An answer read in full.
pub type Answer = (hyper::http::response::Parts, Bytes);

/// The streams to every node.
pub struct Nodes {
    quic: Arc<Quic>,
    /// The front door's clock, on which a request waits for a stream.
    clock: Arc<dyn Clock>,
    table: Mutex<Table>,
    /// Woken whenever a lease ends, for requests that wait for a stream.
    freed: tokio::sync::Notify,
    shutdown: CancellationToken,
}

#[derive(Default)]
struct Table {
    pools: BTreeMap<NodeId, Pool>,
    /// Streams in all pools, idle and leased.
    streams: usize,
}

/// One node's streams: those waiting for a request, and how many are
/// leased.
#[derive(Default)]
struct Pool {
    idle: Vec<(SendRequest<Body>, Instant)>,
    leased: usize,
}

impl Table {
    /// Drops the idle streams that closed, or waited too long, and the
    /// pools left empty.
    fn prune(&mut self, now: Instant) {
        let mut dropped = 0;
        for pool in self.pools.values_mut() {
            let before = pool.idle.len();
            pool.idle
                .retain(|(sender, since)| !sender.is_closed() && now - *since < IDLE_FOR);
            dropped += before - pool.idle.len();
        }
        self.pools
            .retain(|_, pool| pool.leased > 0 || !pool.idle.is_empty());
        self.streams -= dropped;
        self.check();
    }

    /// Closes the idle stream that waited longest, of any node, to make
    /// room for another node's: true if there was one.
    fn evict_oldest_idle(&mut self) -> bool {
        let oldest = self
            .pools
            .iter()
            .flat_map(|(node, pool)| {
                pool.idle
                    .iter()
                    .enumerate()
                    .map(move |(index, (_, since))| (*since, *node, index))
            })
            .min();
        let Some((_, node, index)) = oldest else {
            return false;
        };
        if let Some(pool) = self.pools.get_mut(&node)
            && index < pool.idle.len()
        {
            drop(pool.idle.remove(index));
            self.streams -= 1;
        }
        self.pools
            .retain(|_, pool| pool.leased > 0 || !pool.idle.is_empty());
        self.check();
        true
    }

    /// A place for one more request to `node`, with an idle stream of its
    /// own if one is left; none while the node has all its streams in use.
    fn take(&mut self, node: NodeId) -> Option<Option<SendRequest<Body>>> {
        self.prune(Instant::now());
        let (in_pool, has_idle) = self.pools.get(&node).map_or((0, false), |pool| {
            (pool.idle.len() + pool.leased, !pool.idle.is_empty())
        });
        if !has_idle {
            if in_pool >= STREAMS_PER_NODE {
                return None;
            }
            if self.streams >= MAX_STREAMS && !self.evict_oldest_idle() {
                return None;
            }
            self.streams += 1;
        }
        let pool = self.pools.entry(node).or_default();
        pool.leased += 1;
        let sender = pool.idle.pop().map(|(sender, _)| sender);
        self.check();
        Some(sender)
    }

    fn check(&self) {
        let counted: usize = self
            .pools
            .values()
            .map(|pool| pool.idle.len() + pool.leased)
            .sum();
        assert_eq!(counted, self.streams);
        assert!(self.streams <= MAX_STREAMS);
        assert!(
            self.pools
                .values()
                .all(|pool| pool.idle.len() + pool.leased <= STREAMS_PER_NODE)
        );
    }
}

impl Nodes {
    /// Streams over `quic`'s connections, closed at `shutdown`; a request
    /// waits for one on `clock`.
    #[must_use]
    pub fn new(quic: Arc<Quic>, clock: Arc<dyn Clock>, shutdown: CancellationToken) -> Self {
        Self {
            quic,
            clock,
            table: Mutex::new(Table::default()),
            freed: tokio::sync::Notify::new(),
            shutdown,
        }
    }

    /// Sends `request` to `node` and reads its answer, within the bounds.
    ///
    /// # Errors
    ///
    /// See [`Missed`].
    pub async fn send(
        self: &Arc<Self>,
        node: NodeId,
        request: Request<Body>,
    ) -> Result<Answer, Missed> {
        let mut lease = self.lease(node).await?;
        let reused = match lease.sender.take() {
            Some(mut sender) => match tokio::time::timeout(OPEN_TIMEOUT, sender.ready()).await {
                Ok(Ok(())) => Some(sender),
                _ => None,
            },
            None => None,
        };
        let mut sender = match reused {
            Some(sender) => sender,
            None => self.open(node).await?,
        };
        let answered = async {
            // hyper's HTTP/1 client cancels a request sent before the
            // connection is ready, so even a new one is awaited.
            sender.ready().await.map_err(|_| Missed::Broken)?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|_| Missed::Broken)?;
            read(response).await
        };
        let answer = tokio::time::timeout(ANSWER_TIMEOUT, answered)
            .await
            .map_err(|_| Missed::TimedOut)??;
        lease.sender = Some(sender);
        Ok(answer)
    }

    /// A place for one more request to `node`, with an idle stream if one
    /// is left, waiting at most [`WAIT_FOR_STREAM_MS`] for one to come free.
    async fn lease(self: &Arc<Self>, node: NodeId) -> Result<Lease, Missed> {
        let mut moves = self.clock.subscribe();
        // One wait across the loop's turns, so that on the system clock it
        // ends the interval's real time after the request came.
        let expired = interval_passed(&*self.clock, &mut moves, WAIT_FOR_STREAM_MS);
        let mut expired = std::pin::pin!(expired);
        loop {
            // Registered before the table is looked at, so that a lease
            // ending in between still wakes this request.
            let freed = self.freed.notified();
            let mut freed = std::pin::pin!(freed);
            freed.as_mut().enable();
            if let Some(sender) = lock(&self.table).take(node) {
                return Ok(Lease {
                    nodes: Arc::clone(self),
                    node,
                    sender,
                });
            }
            tokio::select! {
                () = expired.as_mut() => return Err(Missed::Busy),
                () = freed => {}
            }
        }
    }

    /// Opens a stream to `node` and runs HTTP/1.1 on it until it closes or
    /// the front door shuts down.
    async fn open(&self, node: NodeId) -> Result<SendRequest<Body>, Missed> {
        let opening = self.quic.open_held(node, Purpose::Payments);
        let stream = match tokio::time::timeout(OPEN_TIMEOUT, opening).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(RelayError::UnknownDestination | RelayError::Unreachable)) => {
                return Err(Missed::NotConnected);
            }
            Ok(Err(RelayError::Busy)) => return Err(Missed::Busy),
            Ok(Err(error)) => {
                tracing::debug!(target: target::FRONT_DOOR, %node, %error, "no stream to the node");
                return Err(Missed::Broken);
            }
            Err(_) => return Err(Missed::TimedOut),
        };
        let (sender, connection) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| Missed::Broken)?;
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = connection => {}
                () = shutdown.cancelled() => {}
            }
        });
        Ok(sender)
    }

    /// Whether `node` holds a connection to the front door now, so that a
    /// request to it could be passed on.
    pub async fn is_connected(&self, node: NodeId) -> bool {
        self.quic.holds_connection(node).await
    }

    /// Streams open now, idle and in use.
    #[must_use]
    pub fn streams(&self) -> usize {
        lock(&self.table).streams
    }
}

/// Reads an answer whole, up to [`MAX_ANSWER_BYTES`].
async fn read(response: Response<hyper::body::Incoming>) -> Result<Answer, Missed> {
    let (parts, body) = response.into_parts();
    let body = Limited::new(body, MAX_ANSWER_BYTES)
        .collect()
        .await
        .map_err(|_| Missed::Broken)?
        .to_bytes();
    Ok((parts, body))
}

/// A request's place among a node's streams. Its stream goes back to the
/// pool if the request was answered, and is closed otherwise, however the
/// request ended.
struct Lease {
    nodes: Arc<Nodes>,
    node: NodeId,
    sender: Option<SendRequest<Body>>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut table = lock(&self.nodes.table);
        let kept = self.sender.take().filter(|sender| !sender.is_closed());
        let Some(pool) = table.pools.get_mut(&self.node) else {
            return;
        };
        pool.leased -= 1;
        match kept {
            Some(sender) => pool.idle.push((sender, Instant::now())),
            None => table.streams -= 1,
        }
        table.prune(Instant::now());
        drop(table);
        self.nodes.freed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use dassie_identity::IdentityKey;

    fn node(index: usize) -> NodeId {
        let mut seed = [7u8; 32];
        seed[..8].copy_from_slice(&u64::try_from(index).unwrap().to_le_bytes());
        IdentityKey::from_bytes(&seed).node_id()
    }

    /// An open HTTP/1.1 connection's sender, and the other end, which
    /// keeps it open.
    async fn sender() -> (SendRequest<Body>, tokio::io::DuplexStream) {
        let (ours, theirs) = tokio::io::duplex(1024);
        let (sender, connection) = http1::handshake(TokioIo::new(ours)).await.unwrap();
        tokio::spawn(connection);
        (sender, theirs)
    }

    #[test]
    fn a_node_has_its_share_of_streams() {
        let mut table = Table::default();
        for _ in 0..STREAMS_PER_NODE {
            assert!(matches!(table.take(node(1)), Some(None)));
        }
        assert!(table.take(node(1)).is_none());
        assert!(matches!(table.take(node(2)), Some(None)));
        assert_eq!(table.streams, STREAMS_PER_NODE + 1);
    }

    #[tokio::test]
    async fn a_full_table_closes_the_oldest_idle_stream_for_another_node() {
        let mut table = Table::default();
        let nodes = MAX_STREAMS / STREAMS_PER_NODE;
        for index in 0..nodes {
            for _ in 0..STREAMS_PER_NODE {
                assert!(table.take(node(index)).is_some());
            }
        }
        assert_eq!(table.streams, MAX_STREAMS);
        // Every stream in use: another node waits.
        assert!(table.take(node(nodes)).is_none());
        // The last node's requests end, and its streams wait idle.
        let mut ends = Vec::new();
        let pool = table.pools.get_mut(&node(nodes - 1)).unwrap();
        for _ in 0..STREAMS_PER_NODE {
            let (sender, end) = sender().await;
            pool.leased -= 1;
            pool.idle.push((sender, Instant::now()));
            ends.push(end);
        }
        assert!(matches!(table.take(node(nodes)), Some(None)));
        assert_eq!(table.streams, MAX_STREAMS);
        let left = &table.pools.get(&node(nodes - 1)).unwrap().idle;
        assert_eq!(left.len(), STREAMS_PER_NODE - 1);
    }
}
