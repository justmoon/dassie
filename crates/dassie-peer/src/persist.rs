//! What the peer node persists (spec 7 §4.4): the table's nodes with their
//! link states, registrations and peerings, and our own sequence (NET-1,
//! NET-2, NET-7, ACC-18). Changes come out as snapshots of the entries
//! that changed, written in the same transaction as the ledger.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::table::{Peering, Registration, Sources};

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
    /// For a peer, the newest timestamp accepted from it: the replay floor
    /// after a restart.
    pub replay_floor_ms: u64,
}

/// What the peer node starts from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// The nodes.
    pub nodes: Vec<StoredNode>,
    /// The highest sequence of our own link state issued.
    pub own_sequence: u64,
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
}

impl PeerBatch {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.removed.is_empty() && self.own_sequence.is_none()
    }
}
