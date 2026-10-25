//! Settlement channels (spec/areas/settlement.md#settlement-channels): what
//! makes a settlement message arrive exactly once. A channel belongs to a
//! pair of nodes, not to one peering: it is made at the first peering, kept
//! when the peering ends, and used again by the next. So a settlement that
//! arrives after the peering ended is still credited, and a copy from an
//! old peering is never applied twice. A channel left idle is closed: what
//! stays of it ([`Closed`]) is on disk only, and the next peering opens it
//! again from there (spec/decisions/closing-channels.md).

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
    /// Whether we once ended a peering with the node for want of room: a
    /// later peering with it as our senior raises no credit to the default
    /// (spec/areas/settlement.md#credit).
    pub ended_for_room: bool,
    /// Whether the node ever paid us: our books took a settlement of at
    /// least one largest packet from it. A downlink that never did is a
    /// free receiver, and free receivers take at most half of the
    /// downlinks' places (spec/decisions/free-receivers-half.md).
    pub paid_us: bool,
}

impl Channel {
    /// A new channel with `key` at `url`.
    #[must_use]
    pub const fn new(key: PublicKey, url: String) -> Self {
        Self::reopen(key, url, Closed::NEW)
    }

    /// The channel with `key` at `url`, opened again from what stayed of
    /// it when it was closed.
    #[must_use]
    pub const fn reopen(key: PublicKey, url: String, closed: Closed) -> Self {
        Self {
            key,
            url,
            settled_in: closed.settled_in,
            settled_out: closed.settled_out,
            outbox: None,
            ended_for_room: closed.ended_for_room,
            paid_us: closed.paid_us,
        }
    }

    /// What stays of the channel once it is closed. Only a channel with
    /// nothing on its way is closed.
    #[must_use]
    pub fn closed(&self) -> Closed {
        assert!(
            self.outbox.is_none(),
            "a channel closes with nothing on its way"
        );
        Closed {
            settled_in: self.settled_in,
            settled_out: self.settled_out,
            ended_for_room: self.ended_for_room,
            paid_us: self.paid_us,
        }
    }
}

/// What stays of a closed channel, on disk only: its sequences, so that a
/// settlement is applied once however late it comes and the next one we
/// send follows the last, and whether we ended a peering with the node for
/// want of room, which is for good (spec/decisions/ended-for-room.md), and
/// whether it ever paid us, also for good
/// (spec/decisions/free-receivers-half.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed {
    /// The highest sequence applied from the node.
    pub settled_in: u64,
    /// The last sequence we used.
    pub settled_out: u64,
    /// Whether we once ended a peering with the node for want of room.
    pub ended_for_room: bool,
    /// Whether the node ever paid us.
    pub paid_us: bool,
}

impl Closed {
    /// A channel that never carried anything.
    pub const NEW: Self = Self {
        settled_in: 0,
        settled_out: 0,
        ended_for_room: false,
        paid_us: false,
    };
}

/// When a channel's message is due again, besides its persisted state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Retry {
    /// When to send it next.
    pub at_ms: u64,
    /// Whether a send is in flight.
    pub in_flight: bool,
    /// Whether it may go again as soon as its node answers a request of
    /// ours, before its backoff ends: once after each try in its backoff
    /// that got no answer, never after an answer refusing it.
    pub early: bool,
    /// Whether the next try, or the one in flight, is one that went early.
    pub went_early: bool,
}
