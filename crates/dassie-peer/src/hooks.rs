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
    /// Whether the node runs `scheme`: it is active. The peerings on a
    /// scheme it does not run are ended
    /// (spec/areas/settlement.md#schemes-and-ledgers).
    fn speaks(&self, scheme: SchemeId) -> bool;

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

    /// Accepts a request from `peer`: opens its accounts. `downlink` says
    /// it peers as our downlink, whose accounts take the downlinks' room,
    /// never the core peers' (spec/decisions/account-budgets.md). Called
    /// again for a peer already peered (spec/decisions/peering-lifecycle.md),
    /// which must change nothing.
    ///
    /// # Errors
    ///
    /// Why not.
    fn accept(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        downlink: bool,
        scheme: SchemeId,
        data: &[u8],
    ) -> Result<Agreed, RefusalReason>;

    /// Completes our request to `peer`, which accepted: opens its
    /// accounts.
    ///
    /// # Errors
    ///
    /// The scheme does not agree after all.
    fn finalize(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        scheme: SchemeId,
        info: &[u8],
    ) -> Result<Agreed, Declined>;

    /// Whether `scheme` holds an entry payment from `node`, which asks to
    /// peer with the scheme's `data`: a payment of at least one largest
    /// packet that reached us on the ledger before any peering, bound to
    /// the node and to its request (spec/decisions/pay-to-enter.md). A
    /// downlink that holds one gets in past a full free half; one on a
    /// scheme that takes them is told to pay, rather than refused for
    /// capacity.
    fn entry(&self, node: NodeId, scheme: SchemeId, data: &[u8]) -> EntryPayment;

    /// Whether methods on `scheme` are over a settlement hub: each Prepare
    /// on them comes with the payer's commitment, in its `methodPacket`,
    /// and the reply to a Fulfill may carry the payer's claim
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). A Prepare
    /// with a commitment is refused on any other method. On one over a hub
    /// a Prepare may come without one, while a side of the peering has no
    /// account in the hub (spec/designs/settlement-hubs.md#5-joining): the
    /// node, not the peer protocol, decides what it takes without one.
    fn uses_commitments(&self, scheme: SchemeId) -> bool;

    /// Whether our Prepare in flight to `peer` with `request_id` went on a
    /// method over a settlement hub, whether or not the peering still has
    /// that method: a dropped method's packets resolve on it
    /// (spec/decisions/settlement-methods.md), and the reply to the peer's
    /// Fulfill of one carries our claim even once the peering has no
    /// method over a hub left.
    fn forwarded_over_hub(&self, peer: NodeId, request_id: u32) -> bool;

    /// Whether the node takes a further settlement method on `scheme` with
    /// `peer`, beside those their peering has
    /// (spec/decisions/settlement-methods.md): whether it asks for one, as
    /// the node that made the peering, and whether it takes the peer's
    /// request for one. A method it takes opens its accounts as a
    /// peering's first does, through [`accept`](Self::accept) or
    /// [`finalize`](Self::finalize). `downlink` says the peer is our
    /// downlink, whose accounts take the downlinks' room, never the core
    /// peers' (spec/decisions/account-budgets.md).
    fn wants_method(&self, peer: NodeId, downlink: bool, scheme: SchemeId) -> bool;

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

    /// Whether a settlement of ours to `peer` on `scheme` is under way in
    /// the node, as [`settling_to`](Self::settling_to) says of any scheme:
    /// a node winding down its method on `scheme` waits for it before it
    /// asks the peer to settle on it (spec/decisions/settlement-methods.md).
    fn settling_on(&self, peer: NodeId, scheme: SchemeId) -> bool;

    /// Whether a peer's settlement on `scheme` can reach us now: not while
    /// our line on its ledger cannot receive
    /// (spec/areas/settlement.md#when-peers-cannot-pay-us). A method on a
    /// scheme that cannot for as long as a silent peer is kept is dropped,
    /// without collecting (spec/decisions/settlement-methods.md).
    fn receives(&self, scheme: SchemeId) -> bool;

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
