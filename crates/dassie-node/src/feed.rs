//! The node's UDP endpoint for its snapshot
//! (spec/areas/rate-feed.md#the-node-side).
//!
//! Anyone may ask a node for its snapshot over UDP, once: a request padded
//! to the largest snapshot gets the snapshot the engine made last, cached,
//! within a budget on answers per second (`dassie_feed::Answerer`). The
//! endpoint keeps nothing per address. A node with no snapshot, a periphery
//! node or one whose owner offers no swaps, answers nothing.
//!
//! Every datagram is answered or dropped, counted and never logged, so a
//! flood costs no more than its own bytes and a decode each.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dassie_feed::{
    Answer, Answerer, MAX_DATAGRAM_BYTES, SnapshotMessage, Unanswered, decode_snapshot_datagram,
};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::node::Node;

/// The UDP port a node takes requests for its snapshot on, unless told
/// another (spec/areas/rate-feed.md#the-socket).
pub const DEFAULT_FEED_PORT: u16 = 7768;

/// What the endpoint has done since the node started, counted rather than
/// logged. The owner reads them in `GetStatus`, each on its own and without
/// a lock (spec/areas/rate-feed.md#what-the-owner-sees).
#[derive(Debug, Default)]
pub struct FeedStats {
    /// Datagrams taken.
    pub received: AtomicU64,
    /// Requests answered with the snapshot.
    pub answered: AtomicU64,
    /// Requests not answered because the budget was spent.
    pub over_budget: AtomicU64,
    /// Datagrams that got no answer and cost no budget: no request, from an
    /// address an answer cannot go to, or any while the node had no
    /// snapshot.
    pub ignored: AtomicU64,
    /// Snapshots the socket refused to send.
    pub send_failed: AtomicU64,
}

fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Whether `node` answers requests for its snapshot now: it has the socket,
/// and a snapshot that counts, so it is a core node that offers swaps and
/// its engine runs.
#[must_use]
pub fn serving(node: &Node) -> bool {
    node.feed().is_some() && node.current_snapshot().is_some()
}

/// Until when the snapshot of `bytes` counts, if they are one.
#[must_use]
pub fn valid_until_ms(bytes: &[u8]) -> Option<u64> {
    match decode_snapshot_datagram(bytes) {
        Ok(SnapshotMessage::Snapshot(signed)) => Some(signed.snapshot.valid_until_ms),
        Ok(SnapshotMessage::Request) | Err(_) => None,
    }
}

/// The endpoint's socket, and what outlives a restart of its service.
#[derive(Debug)]
pub struct FeedEndpoint {
    socket: Arc<UdpSocket>,
    stats: FeedStats,
}

impl FeedEndpoint {
    /// The endpoint on `socket`. Inside the async runtime.
    ///
    /// # Errors
    ///
    /// The socket cannot be made non-blocking or taken by the runtime.
    pub fn new(socket: std::net::UdpSocket) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket: Arc::new(UdpSocket::from_std(socket)?),
            stats: FeedStats::default(),
        })
    }

    /// Where it takes datagrams.
    ///
    /// # Errors
    ///
    /// The socket's address cannot be read.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// What it has done.
    #[must_use]
    pub const fn stats(&self) -> &FeedStats {
        &self.stats
    }
}

/// The snapshot to answer with, and until when it counts.
#[derive(Clone, Debug)]
struct Cached {
    bytes: Arc<[u8]>,
    valid_until_ms: u64,
}

impl Cached {
    fn of(snapshot: Option<&Arc<[u8]>>) -> Option<Self> {
        let bytes = snapshot?;
        Some(Self {
            valid_until_ms: valid_until_ms(bytes)?,
            bytes: Arc::clone(bytes),
        })
    }
}

/// What to do about one datagram, without IO: whether to answer it with
/// the cached snapshot, at `now_ms`, within `answerer`'s budget. Without a
/// snapshot that counts, nothing is answered, and nothing costs budget.
fn decide(
    answerer: &mut Answerer,
    cached: Option<&Cached>,
    from: SocketAddr,
    datagram: &[u8],
    now_ms: u64,
) -> Answer {
    if cached.is_none_or(|cached| cached.valid_until_ms <= now_ms) {
        return Answer::Ignored(Unanswered::NotRequest);
    }
    answerer.handle(from, datagram, now_ms)
}

/// Answers requests on the node's snapshot socket, if it has one, until
/// `shutdown`.
pub async fn run(node: Arc<Node>, shutdown: CancellationToken) {
    let Some(endpoint) = node.feed().cloned() else {
        return shutdown.cancelled().await;
    };
    let mut answerer = Answerer::new(node.policies().feed);
    let mut snapshots = node.snapshots();
    let mut cached = Cached::of(snapshots.borrow_and_update().as_ref());
    let socket = Arc::clone(&endpoint.socket);
    let stats = &endpoint.stats;
    // One more byte than a datagram may have, so a longer one is seen as
    // too long rather than cut to size.
    let mut buffer = vec![0u8; MAX_DATAGRAM_BYTES + 1];
    loop {
        // The socket comes last: under a flood it is always ready, and must
        // not keep a new snapshot from being taken.
        tokio::select! {
            biased;
            () = shutdown.cancelled() => return,
            changed = snapshots.changed() => {
                if changed.is_err() {
                    return;
                }
                cached = Cached::of(snapshots.borrow_and_update().as_ref());
            }
            received = socket.recv_from(&mut buffer) => {
                let (length, from) = match received {
                    Ok(received) => received,
                    // An ICMP error for an earlier datagram, on some
                    // systems.
                    Err(error) if is_icmp(&error) => continue,
                    Err(_) => return,
                };
                count(&stats.received);
                let datagram = buffer.get(..length).unwrap_or_default();
                let now_ms = node.now_ms();
                match decide(&mut answerer, cached.as_ref(), from, datagram, now_ms) {
                    Answer::Snapshot => {
                        if let Some(cached) = &cached {
                            count(&stats.answered);
                            if socket.try_send_to(&cached.bytes, from).is_err() {
                                count(&stats.send_failed);
                            }
                        }
                    }
                    Answer::Ignored(Unanswered::OverBudget) => count(&stats.over_budget),
                    Answer::Ignored(_) => count(&stats.ignored),
                }
            }
        }
    }
}

/// Whether `error` is an ICMP error for an earlier datagram, which some
/// systems hand to the next receive.
fn is_icmp(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use dassie_feed::{AnswerLimits, Snapshot, encode_request, sign_snapshot};
    use dassie_identity::NodeKey;

    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    fn from() -> SocketAddr {
        SocketAddr::from(([203, 0, 113, 7], 4000))
    }

    fn cached(valid_until_ms: u64) -> Cached {
        let snapshot = Snapshot {
            sequence_ms: NOW - 1,
            valid_until_ms,
            grid_bp: [1, 3, 10, 30, 100, 300, 1_000, 3_000],
            methods: Vec::new(),
        };
        let bytes = sign_snapshot(&NodeKey::from_bytes(&[7; 32]), &snapshot).unwrap();
        Cached::of(Some(&Arc::from(bytes))).unwrap()
    }

    // spec/areas/rate-feed.md#requests: a request gets the snapshot while
    // the node has one that counts, within the budget; anything else, or
    // any request while it has none, gets nothing, and costs no budget.
    #[test]
    fn requests_get_the_snapshot_while_it_counts_within_the_budget() {
        let limits = AnswerLimits {
            per_second: 1,
            burst: 1,
        };
        let mut answerer = Answerer::new(limits);
        let request = encode_request();
        let snapshot = cached(NOW + 10_000);
        assert_eq!(snapshot.valid_until_ms, NOW + 10_000);
        let none = Answer::Ignored(Unanswered::NotRequest);
        assert_eq!(decide(&mut answerer, None, from(), &request, NOW), none);
        let expired = cached(NOW);
        assert_eq!(
            decide(&mut answerer, Some(&expired), from(), &request, NOW),
            none
        );
        assert!(matches!(
            decide(&mut answerer, Some(&snapshot), from(), &request[..10], NOW),
            Answer::Ignored(Unanswered::Malformed(_))
        ));
        assert_eq!(
            decide(&mut answerer, Some(&snapshot), from(), &request, NOW),
            Answer::Snapshot
        );
        assert_eq!(
            decide(&mut answerer, Some(&snapshot), from(), &request, NOW),
            Answer::Ignored(Unanswered::OverBudget)
        );
        assert_eq!(
            decide(&mut answerer, Some(&snapshot), from(), &request, NOW + 999),
            Answer::Ignored(Unanswered::OverBudget)
        );
        // A second fills the budget by one answer again.
        assert_eq!(
            decide(
                &mut answerer,
                Some(&snapshot),
                from(),
                &request,
                NOW + 1_000
            ),
            Answer::Snapshot
        );
    }

    // What is cached is a snapshot, with its validity; anything else is
    // none.
    #[test]
    fn only_a_snapshot_is_cached() {
        assert!(Cached::of(None).is_none());
        let request: Arc<[u8]> = Arc::from(encode_request().to_vec());
        assert!(Cached::of(Some(&request)).is_none());
        assert_eq!(valid_until_ms(&request), None);
        assert_eq!(cached(NOW + 5).valid_until_ms, NOW + 5);
    }
}
