//! Snapshots over the peer protocol (spec/decisions/peer-swaps.md,
//! spec/areas/network.md#snapshot-subscriptions): the core pushes the
//! node's snapshot to the peers subscribed to it, on the rate feed's
//! schedule, and keeps the latest each peer it subscribes to pushed, for
//! the node's swaps to read.
//!
//! The peer protocol carries a snapshot as opaque bytes, and keeps the
//! subscriptions; the core decides when to push, and where its snapshot
//! comes from ([`SnapshotSource`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use dassie_feed::{FEED_HEARTBEAT_MS, MAX_DATAGRAM_BYTES, Poll, Schedule};
use dassie_identity::NodeId;
use dassie_ledger::{LedgerId, Name};
use dassie_peer::wire::MAX_SNAPSHOT_BYTES;

use super::Core;
use super::ledgers::ActiveScheme;

// A snapshot fits one of the feed's datagrams, as the peer protocol bounds
// it: the two bounds are one, and the feed's snapshot fits either.
const _: () = assert!(MAX_SNAPSHOT_BYTES == MAX_DATAGRAM_BYTES);
const _: () = assert!(dassie_feed::MAX_SNAPSHOT_BYTES <= MAX_SNAPSHOT_BYTES);
// A subscriber waits three of a publisher's heartbeats before it subscribes
// again, so that one or two lost do not count.
const _: () = assert!(dassie_peer::SUBSCRIPTION_SILENCE_MS >= 3 * FEED_HEARTBEAT_MS);

/// Where the core takes the node's snapshot from: its quotes at one
/// moment, signed by its node key (spec/decisions/peer-swaps.md).
pub trait SnapshotSource: Send {
    /// The node's current snapshot, or none while it has none to give. The
    /// core asks at the end of every step while the node offers swaps, so
    /// it answers from what it made last, and gives the same bytes until
    /// what they quote changes. A snapshot is never empty and at most
    /// [`MAX_SNAPSHOT_BYTES`]: one of the feed's datagrams.
    fn current(&mut self, now_ms: u64) -> Option<Arc<[u8]>>;
}

/// No snapshot: what the core starts with, until one is plugged in
/// ([`Core::set_snapshot_source`]).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSnapshots;

impl SnapshotSource for NoSnapshots {
    fn current(&mut self, _: u64) -> Option<Arc<[u8]>> {
        None
    }
}

/// Publishing the node's snapshots to its subscribers.
pub(super) struct Publishing {
    source: Box<dyn SnapshotSource>,
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

    /// The latest snapshot `peer` pushed us, while we subscribe to it: what
    /// the node's swaps price a swap with it by. Its signature and age are
    /// for the reader to check, as `dassie-feed` reads a snapshot.
    #[must_use]
    pub fn peer_snapshot(&self, peer: &NodeId) -> Option<&[u8]> {
        self.peer.snapshot_of(peer)
    }

    /// Every peer's latest snapshot, as [`Self::peer_snapshot`] gives each.
    pub fn peer_snapshots(&self) -> impl Iterator<Item = (&NodeId, &[u8])> {
        self.peer.snapshots()
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
            self.publishing.source.current(now)
        } else {
            None
        };
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

    /// When the snapshot is next due to be pushed, while the node has
    /// subscribers.
    pub(super) const fn snapshot_deadline(&self) -> Option<u64> {
        self.publishing.due_ms
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
