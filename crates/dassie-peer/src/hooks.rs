//! What the peer node asks of the rest of the node when peering: the
//! settlement scheme's say, and the accounts a peering needs
//! (spec/areas/network.md#asking-and-answering). The node implements it over its
//! schemes and ledger; tests and simulations implement it as they like.

use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};

use crate::channel::Closed;
use crate::table::Peering;
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

    /// Whether methods on `scheme` are over a settlement hub: each Prepare
    /// on them comes with the payer's commitment, in its `methodPacket`,
    /// and the reply to a Fulfill may carry the payer's claim
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). A Prepare
    /// with a commitment is refused on any other method. On one over a hub
    /// a Prepare may come without one, while a side of the peering has no
    /// account in the hub (spec/designs/settlement-hubs.md#5-joining): the
    /// node, not the peer protocol, decides what it takes without one.
    fn uses_commitments(&self, scheme: SchemeId) -> bool;

    /// Whether peers on `scheme` trust each other without limit: they never
    /// ask each other to settle and never settle, as on the test stub with
    /// unlimited trust (spec/areas/settlement.md#the-stub-scheme). Beside
    /// the hubs two nodes share, only such a scheme becomes a method of
    /// their peering: no method settles directly
    /// (spec/decisions/hubs-only.md).
    fn trusts_unlimited(&self, scheme: SchemeId) -> bool;

    /// Whether a method on `scheme` would settle directly: it is neither
    /// over a hub nor of unlimited trust. No peering is made on such a
    /// scheme, and a method on one, as a peering stored from when a scheme
    /// settled directly may have, is dropped at once
    /// (spec/decisions/hubs-only.md). So is one on a scheme the node does
    /// not run.
    fn settles_directly(&self, scheme: SchemeId) -> bool {
        !self.uses_commitments(scheme) && !self.trusts_unlimited(scheme)
    }

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

    /// Whether a peer's settlement on `scheme` can reach us now: not once
    /// its hub is shutting down, stalled or ended
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

    /// Whether the node offers swaps to its peers, as its owner's setting
    /// says (spec/decisions/peer-swaps.md): only then does it take a
    /// subscription to its snapshots. A periphery node offers none,
    /// whatever this says. None by default.
    fn offers_swaps(&self) -> bool {
        false
    }

    /// Whether the node subscribes to the snapshots of `peer`, an active
    /// peer that is not our downlink, over `peering`: while its owner lets
    /// it swap, and the peering has methods on two ledgers or more to swap
    /// between (spec/decisions/peer-swaps.md). None by default.
    fn wants_snapshots(&self, peer: NodeId, peering: &Peering) -> bool {
        let _ = (peer, peering);
        false
    }

    /// Whether the node keeps `snapshot`, pushed by `peer`, whose
    /// connection proved `key`, in place of `held`, the one it kept last:
    /// where it checks the snapshot's signature, and that it is newer than
    /// the one held, since pushes may arrive out of order and the newest
    /// wins. The peer protocol reads a snapshot as opaque bytes. Every one
    /// by default.
    fn takes_snapshot(
        &self,
        peer: NodeId,
        key: &PublicKey,
        snapshot: &[u8],
        held: Option<&[u8]>,
    ) -> bool {
        let _ = (peer, key, snapshot, held);
        true
    }
}
