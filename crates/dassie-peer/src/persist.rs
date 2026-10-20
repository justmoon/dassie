//! What the peer node persists (spec/areas/network.md#persistence): the
//! table's nodes with their link states, registrations and peerings, our
//! own sequence, the settlement channels, the bans and the replay floors
//! (NET-1, NET-2, NET-7, ACC-18). Changes come out as snapshots of what changed, written in the
//! same transaction as the ledger.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::channel::Channel;
use crate::table::{Peering, Registration, Sources, Termination};

/// A node, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNode {
    /// Its ID.
    pub id: NodeId,
    /// Its link state's `SignedPeerNodeInfo` bytes, verified when received.
    pub link_state: Option<Vec<u8>>,
    /// Why it is in the table.
    pub sources: Sources,
    /// When it came in.
    pub first_seen_ms: u64,
    /// Its registration with us.
    pub registration: Option<Registration>,
    /// Our peering with it.
    pub peering: Option<Peering>,
    /// The peering's termination, if it is being ended.
    pub termination: Option<Termination>,
}

/// A sender's replay floor, as stored: the newest timestamp accepted from
/// it, so that after a restart nothing at or below it is accepted again
/// (spec/areas/peer-protocol.md#replay-protection). Kept apart from the
/// node table, since a node we hold a channel with may leave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredFloor {
    /// The sender.
    pub node: NodeId,
    /// Its floor.
    pub floor_ms: u64,
}

/// A settlement channel, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredChannel {
    /// The node at the other end.
    pub node: NodeId,
    /// The channel.
    pub channel: Channel,
}

/// What the peer node starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// The nodes.
    pub nodes: Vec<StoredNode>,
    /// The highest sequence of our own link state issued.
    pub own_sequence: u64,
    /// The settlement channels.
    pub channels: Vec<StoredChannel>,
    /// The banned nodes.
    pub bans: Vec<NodeId>,
    /// The replay floors of the senders whose replay state is kept for
    /// good, and of those let go whose floors are not stale yet.
    pub floors: Vec<StoredFloor>,
}

/// What changed since the last batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerBatch {
    /// Nodes added or changed, as they are now.
    pub nodes: Vec<StoredNode>,
    /// Nodes removed.
    pub removed: Vec<NodeId>,
    /// Our own new sequence, if one was issued.
    pub own_sequence: Option<u64>,
    /// Channels added or changed, as they are now. Channels are never
    /// removed.
    pub channels: Vec<StoredChannel>,
    /// Nodes banned.
    pub banned: Vec<NodeId>,
    /// Nodes no longer banned.
    pub unbanned: Vec<NodeId>,
    /// Replay floors that moved, as they are now.
    pub floors: Vec<StoredFloor>,
    /// Senders whose floors are stale, and protect nothing any more.
    pub floors_removed: Vec<NodeId>,
}

impl PeerBatch {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.removed.is_empty()
            && self.own_sequence.is_none()
            && self.channels.is_empty()
            && self.banned.is_empty()
            && self.unbanned.is_empty()
            && self.floors.is_empty()
            && self.floors_removed.is_empty()
    }
}
