//! What the peer node persists (spec/areas/network.md#persistence): the
//! table's nodes with their link states, registrations and peerings, our
//! own sequence, the settlement channels, the bans and the replay floors
//! (NET-1, NET-2, NET-7, ACC-18), a registrar's log and names, and the
//! node's view of who holds each name (spec/areas/network.md#names).
//! Changes come out as snapshots of what changed, written in the same
//! transaction as the ledger.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::channel::{Channel, Closed};
use crate::names::{Commitment, Granted, HeldName, LogHead, Logged, Name};
use crate::table::{Peering, Registration, Sources, Termination};

/// A node, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredNode {
    /// Its ID.
    pub id: NodeId,
    /// Its link state's `SignedPeerNodeInfo` bytes, verified when received.
    pub link_state: Option<Vec<u8>>,
    /// Why it is in the table. A downlink's is stored as a peer's: its
    /// link state, a periphery node's, tells it apart.
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

/// What stays of a closed settlement channel, as stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredClosed {
    /// The node at the other end.
    pub node: NodeId,
    /// What stays of the channel.
    pub closed: Closed,
}

/// What the peer node starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// The nodes.
    pub nodes: Vec<StoredNode>,
    /// The highest sequence of our own link state issued.
    pub own_sequence: u64,
    /// The settlement channels open: closed ones stay on disk, read
    /// through [`PeeringHooks::closed_channel`](crate::PeeringHooks::closed_channel).
    pub channels: Vec<StoredChannel>,
    /// The banned nodes.
    pub bans: Vec<NodeId>,
    /// The replay floors of the senders whose replay state is kept for
    /// good, and of those let go whose floors are not stale yet.
    pub floors: Vec<StoredFloor>,
    /// Where a registrar's log stands, which only rises.
    pub name_log: LogHead,
    /// The commitments in a registrar's log.
    pub commitments: Vec<Logged>,
    /// The names a registrar granted, held or freed a short while ago.
    pub names: Vec<Granted>,
    /// Who holds each name, in the node's view: kept while the view is
    /// frozen, across restarts too.
    pub holders: Vec<HeldName>,
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
    /// Channels opened, opened again or changed, as they are now.
    pub channels: Vec<StoredChannel>,
    /// Channels closed, or closed ones whose sequence moved: what stays of
    /// each. A channel is never removed, only closed
    /// (spec/decisions/closing-channels.md).
    pub closed: Vec<StoredClosed>,
    /// Nodes banned.
    pub banned: Vec<NodeId>,
    /// Nodes no longer banned.
    pub unbanned: Vec<NodeId>,
    /// Replay floors that moved, as they are now.
    pub floors: Vec<StoredFloor>,
    /// Senders whose floors are stale, and protect nothing any more.
    pub floors_removed: Vec<NodeId>,
    /// Where a registrar's log stands, if it moved.
    pub name_log: Option<LogHead>,
    /// Commitments logged or unsealed, as they are now.
    pub commitments: Vec<Logged>,
    /// Commitments that left the log.
    pub commitments_removed: Vec<Commitment>,
    /// Names granted, renewed or freed, as they are now.
    pub names: Vec<Granted>,
    /// Names forgotten.
    pub names_removed: Vec<Name>,
    /// Names whose holder, or its expiry, changed in the node's view, as
    /// they are now.
    pub holders: Vec<HeldName>,
    /// Names without a holder any more in the node's view.
    pub holders_removed: Vec<Name>,
}

impl PeerBatch {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
            && self.removed.is_empty()
            && self.own_sequence.is_none()
            && self.channels.is_empty()
            && self.closed.is_empty()
            && self.banned.is_empty()
            && self.unbanned.is_empty()
            && self.floors.is_empty()
            && self.floors_removed.is_empty()
            && self.name_log.is_none()
            && self.commitments.is_empty()
            && self.commitments_removed.is_empty()
            && self.names.is_empty()
            && self.names_removed.is_empty()
            && self.holders.is_empty()
            && self.holders_removed.is_empty()
    }
}
