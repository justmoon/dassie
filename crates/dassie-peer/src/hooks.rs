//! What the peer node asks of the rest of the node when peering: the
//! settlement scheme's say, and the accounts a peering needs
//! (spec/areas/network.md#asking-and-answering). The node implements it over its
//! schemes and ledger; tests and simulations implement it as they like.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::channel::Closed;
use crate::wire::{RefusalReason, SchemeId};

/// The scheme would not go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declined;

/// A peering the scheme agreed to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agreed {
    /// The scheme's state for the peer.
    pub state: Vec<u8>,
    /// The credit we extend to the peer.
    pub credit_out: u64,
}

/// Whether a scheme takes entry payments, and holds one from a node that
/// asks to peer (spec/decisions/pay-to-enter.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryPayment {
    /// The scheme takes none: a node new to us waits for a free place.
    NotTaken,
    /// It takes them, and holds none bound to the node and its request.
    Missing,
    /// It holds one: the node paid us on the ledger before it peered.
    Held,
}

/// What peering needs from the node.
pub trait PeeringHooks {
    /// The versions of `scheme` the node speaks, or none if it is not
    /// active. A scheme that speaks none is one the node does not run: its
    /// peerings are ended (spec/areas/settlement.md#schemes-and-ledgers), so
    /// an active scheme speaks at least one.
    fn versions(&self, scheme: SchemeId) -> &'static [u16];

    /// The scheme's peering info, for a node about to ask.
    ///
    /// # Errors
    ///
    /// The scheme is not active.
    fn peering_info(&mut self, scheme: SchemeId, out: &mut Vec<u8>) -> Result<(), Declined>;

    /// The scheme's data for a request to `peer`.
    ///
    /// # Errors
    ///
    /// The scheme will not peer with it.
    fn create_request(
        &mut self,
        peer: NodeId,
        scheme: SchemeId,
        info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), Declined>;

    /// Accepts a request from `peer`: opens its accounts, and writes the
    /// scheme's response into `response`. `downlink` says it peers as our
    /// downlink, whose accounts take the downlinks' room, never the core
    /// peers' (spec/decisions/account-budgets.md). Called
    /// again for a peer already peered (spec/decisions/peering-lifecycle.md),
    /// which must change nothing.
    ///
    /// # Errors
    ///
    /// Why not.
    #[allow(clippy::too_many_arguments)]
    fn accept(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        downlink: bool,
        scheme: SchemeId,
        version: u16,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Agreed, RefusalReason>;

    /// Completes our request to `peer`, which accepted with `version`:
    /// opens its accounts. `ended_for_room` says we once ended a peering
    /// with it for want of room, so it earns no more credit for asking
    /// (spec/areas/settlement.md#credit).
    ///
    /// # Errors
    ///
    /// The scheme does not agree after all.
    #[allow(clippy::too_many_arguments)]
    fn finalize(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        scheme: SchemeId,
        version: u16,
        info: &[u8],
        data: &[u8],
        ended_for_room: bool,
    ) -> Result<Agreed, Declined>;

    /// Whether `scheme` holds an entry payment from `node`, which asks to
    /// peer with the scheme's `data`: a payment of at least one largest
    /// packet that reached us on the ledger before any peering, bound to
    /// the node and to its request (spec/decisions/pay-to-enter.md). A
    /// downlink that holds one gets in past a full free half; one on a
    /// scheme that takes them is told to pay, rather than refused for
    /// capacity.
    fn entry(&self, node: NodeId, scheme: SchemeId, data: &[u8]) -> EntryPayment;

    /// Whether peerings on `scheme` are over a settlement hub: each Prepare
    /// on them comes with the payer's commitment, in a `hubPacket`, and
    /// the reply to a Fulfill may carry the payer's claim
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). A Prepare
    /// without a commitment is refused on such a peering, and one with a
    /// commitment on any other.
    fn uses_commitments(&self, scheme: SchemeId) -> bool;

    /// Whether nothing is in flight with `peer` any more: no packet waits
    /// for its response, in either direction. A terminating peering ends
    /// only then, so no response is lost with it.
    fn drained(&self, peer: NodeId) -> bool;

    /// Whether a settlement of ours to `peer` is under way in the node:
    /// prepared, executing, or pending in its ledger. A node ending a
    /// peering waits for it before it asks the peer a last time
    /// (spec/decisions/ending-a-peering.md); the message announcing one is
    /// the peer node's own to know.
    fn settling_to(&self, peer: NodeId) -> bool;

    /// What stays of the closed settlement channel with `node`, as it was
    /// last written, or `None` if there never was one
    /// (spec/decisions/closing-channels.md). The node reads it from disk
    /// when the channel opens again and when a settlement comes over it
    /// closed.
    fn closed_channel(&self, node: NodeId) -> Option<Closed>;

    /// Whether the node's books let the channel with `node`, a node we no
    /// longer peer with, close: nothing of it is pending in the ledger, and
    /// every scheme it has accounts with runs and sees its ledger, so a
    /// settlement it made before the close is booked as its own.
    fn closable(&self, node: NodeId) -> bool;
}
