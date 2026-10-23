//! What consensus asks of the application, the hub's ledger: a payload when
//! it leads, a verdict on another leader's payload, and the finalized blocks
//! in order. The ledger implements it; tests and simulations implement it as
//! they like.

use alloc::vec::Vec;

use super::types::{Block, Vote};
use crate::digest::Digest;
use crate::validators::ValidatorIndex;

/// The application has no payload to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declined;

/// What a leader builds a block on.
#[derive(Clone, Copy, Debug)]
pub struct ProposeContext<'a> {
    /// The view.
    pub view: u64,
    /// Ourselves, the leader.
    pub proposer: ValidatorIndex,
    /// The parent block's digest.
    pub parent: Digest,
    /// The new block's height.
    pub height: u64,
    /// The new block's stamp.
    pub timestamp_ms: u64,
    /// The notarize and finalize votes for blocks the engine still holds,
    /// its own among them, by view, signer and kind, so that the block can
    /// record those no block recorded yet
    /// (spec/designs/settlement-hubs.md#7-fees, "Who gets fees").
    pub votes: &'a [Vote],
}

/// What another leader's block is built on.
#[derive(Clone, Copy, Debug)]
pub struct VerifyContext {
    /// The view.
    pub view: u64,
    /// The leader that proposed it.
    pub proposer: ValidatorIndex,
    /// The parent block's digest.
    pub parent: Digest,
    /// The block's height.
    pub height: u64,
    /// The block's stamp, already checked against our clock and the
    /// parent's.
    pub timestamp_ms: u64,
}

/// What consensus needs from the application.
pub trait Application {
    /// Builds the payload of the block we propose into `payload`. The
    /// parent may not be final yet: the application builds on the state
    /// after it.
    ///
    /// # Errors
    ///
    /// No payload can be built on that parent; the view is then nullified.
    fn propose(
        &mut self,
        context: &ProposeContext<'_>,
        payload: &mut Vec<u8>,
    ) -> Result<(), Declined>;

    /// Whether `payload` is valid on its parent. Every honest validator
    /// must give the same answer for the same block, so it may depend on
    /// nothing but the chain.
    fn verify(&mut self, context: &VerifyContext, payload: &[u8]) -> bool;

    /// A block is final. Called once per block, in height order; the host
    /// persists what the application did with it in the same batch as the
    /// engine's.
    fn finalized(&mut self, block: &Block, digest: &Digest);
}
