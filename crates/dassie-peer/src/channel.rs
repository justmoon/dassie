//! Settlement channels (spec/areas/settlement.md#settlement-channels): what
//! makes a settlement message arrive exactly once. A channel belongs to a
//! pair of nodes, not to one peering: it is made at the first peering, kept
//! when the peering ends, and used again by the next. So a final settlement
//! that arrives after the peering ended is still credited, and a copy from
//! an old peering is never applied twice.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::PublicKey;

use crate::wire::SchemeId;

/// A settlement message not yet acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    /// Its sequence.
    pub sequence: u64,
    /// The scheme.
    pub scheme: SchemeId,
    /// The amount, in the scheme ledger's units.
    pub amount: u64,
    /// The scheme's data.
    pub data: Vec<u8>,
    /// Tries that failed.
    pub attempts: u32,
}

/// A settlement of everything owed, due because a peering ended while the
/// other side knew (spec/areas/settlement.md#settling-at-a-peerings-end).
/// Kept until it is made, across restarts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalDue {
    /// The scheme the peering settled with.
    pub scheme: SchemeId,
    /// The scheme's state for the peer.
    pub state: Vec<u8>,
}

/// Settlement with one node, both ways.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    /// The node's key, so the channel works after the node left the table.
    pub key: PublicKey,
    /// The node's URL, likewise; its link state's, when there is one, is
    /// preferred.
    pub url: String,
    /// The highest sequence applied from the node.
    pub settled_in: u64,
    /// The last sequence we used.
    pub settled_out: u64,
    /// Our settlement message on its way, if any: one at a time (ACC-17).
    pub outbox: Option<Outgoing>,
    /// A final settlement still to be made, if any.
    pub final_due: Option<FinalDue>,
}

impl Channel {
    /// A new channel with `key` at `url`.
    #[must_use]
    pub const fn new(key: PublicKey, url: String) -> Self {
        Self {
            key,
            url,
            settled_in: 0,
            settled_out: 0,
            outbox: None,
            final_due: None,
        }
    }
}

/// When a channel's message is due again, besides its persisted state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Retry {
    /// When to send it next.
    pub at_ms: u64,
    /// Whether a send is in flight.
    pub in_flight: bool,
    /// Whether it may go again as soon as its node answers a request of
    /// ours, before its backoff ends: once after each failure to deliver
    /// it, never after an answer refusing it.
    pub early: bool,
}
