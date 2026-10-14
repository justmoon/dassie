//! What the peer node asks of the rest of the node when peering: the
//! settlement scheme's say, and the accounts a peering needs (spec 3
//! §12.5). The node implements it over its schemes and ledger; tests and
//! simulations implement it as they like.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::table::Termination;
use crate::wire::{RefusalReason, SchemeId};

/// The scheme would not go on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declined;

/// A peering the scheme agreed to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agreed {
    /// The scheme's state for the peer (ACC-15).
    pub state: Vec<u8>,
    /// The credit we extend to the peer.
    pub credit_out: u64,
}

/// What peering needs from the node.
pub trait PeeringHooks {
    /// The versions of `scheme` the node speaks, or none if it is not
    /// active.
    fn versions(&self, scheme: SchemeId) -> &'static [u16];

    /// The credit the node extends to `peer` on `scheme`, in its ledger's
    /// units: what it posted before, if they were peered, else what it
    /// extends now.
    fn credit(&self, peer: NodeId, scheme: SchemeId) -> u64;

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

    /// Accepts a request from `peer`, which extends `credit_in`: opens its
    /// accounts and writes the scheme's response into `response`. Called
    /// again for a peer already peered (D12), which must change nothing.
    ///
    /// # Errors
    ///
    /// Why not.
    #[allow(clippy::too_many_arguments)]
    fn accept(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        scheme: SchemeId,
        version: u16,
        credit_in: u64,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Agreed, RefusalReason>;

    /// Completes our request to `peer`, which accepted with `version` and
    /// `credit_in`: opens its accounts.
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
        credit_in: u64,
        info: &[u8],
        data: &[u8],
    ) -> Result<Agreed, Declined>;

    /// Whether nothing is in flight with `peer` any more, as of `now_ms`:
    /// no packet waits for its response, in either direction, and no
    /// Fulfill for its verdict (D17). The node decides from `termination`
    /// how long verdicts are waited for: one that stopped answering may
    /// never give them. A terminating peering ends only then, so both
    /// sides' books agree.
    fn drained(&self, peer: NodeId, termination: &Termination, now_ms: u64) -> bool;
}
