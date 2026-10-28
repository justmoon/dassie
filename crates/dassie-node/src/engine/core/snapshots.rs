//! Snapshots over the peer protocol (spec/decisions/peer-swaps.md,
//! spec/areas/network.md#snapshot-subscriptions): the core pushes the
//! node's snapshot to the peers subscribed to it, on the rate feed's
//! schedule, and keeps the latest each peer it subscribes to pushed, for
//! the node's swaps to read.
//!
//! The peer protocol carries a snapshot as opaque bytes, and keeps the
//! subscriptions; the core decides when to push, which of a peer's to take
//! ([`Core::takes_snapshot`]), and where its own comes from
//! ([`SnapshotSource`]): the engine's is [`OwnSnapshots`](super::OwnSnapshots).

use std::collections::BTreeSet;
use std::sync::Arc;

use dassie_connector::PriceTable;
use dassie_feed::{
    FEED_HEARTBEAT_MS, MAX_DATAGRAM_BYTES, Poll, Schedule, Snapshot, SnapshotMessage,
    decode_snapshot_datagram, method_tag,
};
use dassie_identity::{NodeId, PublicKey};
use dassie_ledger::{LedgerId, Name};
use dassie_peer::wire::{MAX_SNAPSHOT_BYTES, SchemeId};

use super::Core;
use super::ledgers::ActiveScheme;
use super::swapping::MAX_SWAP_PEERS;

// A snapshot fits one of the feed's datagrams, as the peer protocol bounds
// it: the two bounds are one, and the feed's snapshot fits either.
const _: () = assert!(MAX_SNAPSHOT_BYTES == MAX_DATAGRAM_BYTES);
const _: () = assert!(dassie_feed::MAX_SNAPSHOT_BYTES <= MAX_SNAPSHOT_BYTES);
// A subscriber waits three of a publisher's heartbeats before it subscribes
// again, so that one or two lost do not count.
const _: () = assert!(dassie_peer::SUBSCRIPTION_SILENCE_MS >= 3 * FEED_HEARTBEAT_MS);

/// A peer's snapshot as the owner sees it ([`Core::peer_snapshot_views`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSnapshotView {
    /// The peer that pushed it.
    pub peer: NodeId,
    /// The snapshot, verified under the peer's key when the node took it.
    pub snapshot: Snapshot,
    /// For each of its methods, in its order, the scheme the node runs
    /// whose tag it is, if any.
    pub schemes: Vec<Option<String>>,
}

/// What the node's snapshot is made from, as of the core's step: the price
/// table the connector converts at, and the methods the node offers.
#[derive(Clone, Copy, Debug)]
pub struct Quoting<'a> {
    /// The price table, with what the owner held when it was built.
    pub table: &'a PriceTable,
    /// The settlement methods the node offers its peers, each once with
    /// its scheme's ledger ([`Core::offered_methods`]).
    pub methods: &'a [(SchemeId, LedgerId)],
    /// How old a rate the connector converts at may be.
    pub max_rate_age_ms: u64,
}

/// Where the core takes the node's snapshot from: its quotes at one
/// moment, signed by its node key (spec/decisions/peer-swaps.md).
pub trait SnapshotSource: Send {
    /// The node's current snapshot, made from `quoting`, or none while it
    /// has none to give. The core asks at the end of every step while the
    /// node offers swaps, so it answers from what it made last, and gives
    /// the same bytes until what they quote changes or they are due again.
    /// A snapshot is never empty and at most [`MAX_SNAPSHOT_BYTES`]: one of
    /// the feed's datagrams.
    fn current(&mut self, now_ms: u64, quoting: &Quoting<'_>) -> Option<Arc<[u8]>>;

    /// When to be asked again at the latest, even if nothing changes: to
    /// make the snapshot of a change it held back, or the heartbeat's.
    fn next_ms(&self) -> Option<u64> {
        None
    }
}

/// No snapshot: what the core starts with, until one is plugged in
/// ([`Core::set_snapshot_source`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSnapshots;

impl SnapshotSource for NoSnapshots {
    fn current(&mut self, _: u64, _: &Quoting<'_>) -> Option<Arc<[u8]>> {
        None
    }
}

/// Publishing the node's snapshots to its subscribers.
pub(super) struct Publishing {
    source: Box<dyn SnapshotSource>,
    /// The snapshot the source gave last, which the node's UDP endpoint
    /// answers requests with.
    current: Option<Arc<[u8]>>,
    /// The feed's schedule: at most once per interval while the snapshot
    /// changes, a heartbeat otherwise (spec/areas/rate-feed.md#publishing).
    schedule: Schedule,
    /// When the schedule is next due, while the node has subscribers.
    due_ms: Option<u64>,
}

impl Publishing {
    /// From no source, nothing published yet.
    pub(super) fn new() -> Self {
        Self {
            source: Box::new(NoSnapshots),
            current: None,
            schedule: Schedule::new(),
            due_ms: None,
        }
    }
}

impl Core {
    /// Plugs in where the node's snapshot comes from.
    pub fn set_snapshot_source(&mut self, source: Box<dyn SnapshotSource>) {
        self.publishing.source = source;
    }

    /// The node's current snapshot, signed, as the step left it: none on a
    /// node that offers no swaps.
    #[must_use]
    pub const fn own_snapshot(&self) -> Option<&Arc<[u8]>> {
        self.publishing.current.as_ref()
    }

    /// The latest snapshot `peer` pushed us, while we subscribe to it and it
    /// has not expired: what the node's swaps price a swap with it by. It
    /// was taken only verified under the peer's key and newer than the one
    /// before ([`Self::takes_snapshot`]), so it is only decoded again here.
    #[must_use]
    pub fn peer_snapshot(&self, peer: &NodeId) -> Option<Snapshot> {
        self.peer
            .snapshot_of(peer)
            .and_then(|bytes| self.unexpired(bytes))
    }

    /// Every peer's latest snapshot, as [`Self::peer_snapshot`] gives each.
    pub fn peer_snapshots(&self) -> impl Iterator<Item = (&NodeId, Snapshot)> {
        self.peer
            .snapshots()
            .filter_map(|(peer, bytes)| Some((peer, self.unexpired(bytes)?)))
    }

    /// The snapshots the swap service reads, as the owner sees them
    /// (`ListPeerSnapshots`): each peer's latest while it counts, the first
    /// [`MAX_SWAP_PEERS`] by node ID, with the scheme of each method whose
    /// tag is that of a scheme the node runs. A reader such as
    /// `dassie market-maker` prices its swaps by them
    /// (spec/areas/rate-feed.md#market-maker).
    #[must_use]
    pub fn peer_snapshot_views(&self) -> Vec<PeerSnapshotView> {
        let views: Vec<PeerSnapshotView> = self
            .peer_snapshots()
            .take(MAX_SWAP_PEERS)
            .map(|(peer, snapshot)| {
                let schemes = snapshot
                    .methods
                    .iter()
                    .map(|method| {
                        self.schemes
                            .keys()
                            .find(|name| method_tag(name.as_str()) == method.tag)
                            .map(|name| name.as_str().to_owned())
                    })
                    .collect();
                PeerSnapshotView {
                    peer: *peer,
                    snapshot,
                    schemes,
                }
            })
            .collect();
        assert!(views.len() <= MAX_SWAP_PEERS);
        views
    }

    /// The snapshot of `bytes`, one the node took, if it still counts.
    fn unexpired(&self, bytes: &[u8]) -> Option<Snapshot> {
        let snapshot = decoded(bytes)?;
        (self.now_ms < snapshot.valid_until_ms).then_some(snapshot)
    }

    /// Whether the node offers swaps to its peers: its owner lets it, and
    /// it is a core node, since a periphery node forwards nothing
    /// (spec/decisions/peer-swaps.md).
    pub(super) const fn offers_swaps(&self) -> bool {
        self.peer_settings.offer_swaps && !self.peer.is_periphery()
    }

    /// Takes the node's current snapshot, which a new subscriber is sent
    /// at once, and pushes it to every subscriber when the feed's schedule
    /// says: at most once per interval while it changes, a heartbeat
    /// otherwise. A node that offers no swaps has none. With no subscriber
    /// nothing is due: one that comes is sent the snapshot as it
    /// subscribes.
    pub(super) fn publish_snapshot(&mut self) {
        let now = self.now_ms;
        let current = if self.offers_swaps() {
            let methods = self.offered_methods();
            let quoting = Quoting {
                table: &self.prices.table,
                methods: &methods,
                max_rate_age_ms: self.connector.config().max_rate_age_ms,
            };
            self.publishing.source.current(now, &quoting)
        } else {
            None
        };
        self.publishing.current.clone_from(&current);
        let changed = self.peer.set_snapshot(current);
        if !self.peer.has_subscribers() {
            self.publishing.due_ms = None;
            return;
        }
        let next_ms = match self.publishing.schedule.poll(now, changed) {
            Poll::Publish { next_ms, .. } => {
                self.peer.push_snapshot(now);
                self.after_peer();
                next_ms
            }
            Poll::Wait { next_ms } => next_ms,
        };
        assert!(next_ms > now, "the schedule is due later");
        self.publishing.due_ms = Some(next_ms);
    }

    /// When the snapshot is next due: to be pushed, while the node has
    /// subscribers, or to be made, while the node offers swaps.
    pub(super) fn snapshot_deadline(&self) -> Option<u64> {
        let making = self
            .offers_swaps()
            .then(|| self.publishing.source.next_ms())
            .flatten();
        [self.publishing.due_ms, making].into_iter().flatten().min()
    }

    /// Whether to keep `snapshot`, pushed by a peer whose connection proved
    /// `key`, in place of `held`, the one kept last: only if it decodes,
    /// verifies under the key, is newer than the one held and has not
    /// expired by `now_ms`, as a reader takes one
    /// (`dassie_feed::take_snapshot`).
    pub(super) fn takes_snapshot(
        key: &PublicKey,
        snapshot: &[u8],
        held: Option<&[u8]>,
        now_ms: u64,
    ) -> bool {
        let last = held.and_then(decoded).map(|held| held.sequence_ms);
        dassie_feed::take_snapshot(key, snapshot, last, now_ms).is_ok()
    }
}

/// The snapshot of `bytes`, decoded but not verified: one the node took,
/// verified then.
fn decoded(bytes: &[u8]) -> Option<Snapshot> {
    match decode_snapshot_datagram(bytes) {
        Ok(SnapshotMessage::Snapshot(signed)) => Some(signed.snapshot),
        Ok(SnapshotMessage::Request) | Err(_) => None,
    }
}

/// Whether a peering whose methods are on `schemes` has methods on two
/// ledgers or more, among the schemes that run: only then is there
/// anything to swap between (spec/decisions/peer-swaps.md).
pub(super) fn spans_two_ledgers(
    schemes: impl Iterator<Item = dassie_peer::wire::SchemeId>,
    active: &std::collections::BTreeMap<Name, ActiveScheme>,
) -> bool {
    let ledgers: BTreeSet<LedgerId> = schemes
        .filter_map(|scheme| {
            let running = active.get(&Name::new(scheme.as_str()).ok()?)?;
            running.failed.is_none().then(|| running.scheme.ledger())
        })
        .collect();
    ledgers.len() >= 2
}
