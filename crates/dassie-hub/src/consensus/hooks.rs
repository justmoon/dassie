//! What consensus asks of the application, the hub's ledger: a payload when
//! it leads, a verdict on another leader's payload, and the finalized blocks
//! in order. The ledger implements it; tests and simulations implement it as
//! they like.

use alloc::vec::Vec;

use super::types::{Block, Vote};
use crate::digest::Digest;
use crate::validators::{ValidatorIndex, ValidatorSet};

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
    /// The block's digest, by which the application keeps what it made of
    /// it until it is final.
    pub block: Digest,
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
    /// nothing but the chain. The application keeps the state after a block
    /// it found valid, by the block's digest, for as long as it likes: at
    /// least until its next call, and it says whether it still does
    /// ([`holds`](Self::holds)). [`scheduled_change`](Self::scheduled_change)
    /// reads that state, and later blocks build on it. The state after
    /// `context.parent` is held when the engine asks: the engine verifies a
    /// block's ancestors again, in order from the nearest whose state is
    /// held, when the application let go of theirs. So it may ask again
    /// about a block it asked about before (as after a restart, or after
    /// the application let go of its state), and about a block a quorum
    /// notarized that it never saw proposed, before it builds on it.
    fn verify(&mut self, context: &VerifyContext, payload: &[u8]) -> bool;

    /// Whether the application still holds the state after `block`, a block
    /// it found valid ([`verify`](Self::verify)). The state after the last
    /// finalized block is always held. An application that keeps the state
    /// after every block it verified until that block is final or abandoned
    /// keeps the default.
    fn holds(&mut self, block: &Digest) -> bool {
        let _ = block;
        true
    }

    /// A block is final. Called once per block, in height order; the host
    /// persists what the application did with it in the same batch as the
    /// engine's.
    fn finalized(&mut self, block: &Block, digest: &Digest);

    /// The change of validators the block just [`finalized`](Self::finalized)
    /// scheduled, if any: the height the new set takes over at, and the set
    /// (spec/designs/settlement-hubs.md#8-changing-the-validators). The
    /// engine asks after every finalized block and takes the change in
    /// before it finalizes the next. The height must be at least two above
    /// the block; a larger notice only makes it less likely that blocks
    /// near the boundary are notarized before the change is final, which
    /// [`scheduled_change`](Self::scheduled_change) covers.
    /// [`Engine::reconfigure`](super::Engine::reconfigure) does the same for
    /// a host that learns of a change otherwise.
    fn reconfiguration(&mut self) -> Option<(u64, ValidatorSet)> {
        None
    }

    /// The change of validators scheduled and not yet made in the state
    /// after block `after`, if any: the height its set takes over at, and
    /// the set. `after` is the last finalized block, or a block the
    /// application found valid ([`verify`](Self::verify)) and still holds
    /// the state after ([`holds`](Self::holds)): the engine asks about no
    /// other, so `None` always means no change is pending, never that the
    /// state is unknown. `after` need not be final: the boundary is a property of the chain being
    /// extended, so that no block of the old set at or above it, and only
    /// repeats of the block before it, are notarized on that chain, whatever
    /// a validator has finalized (spec/decisions/epoch-handover.md). It must
    /// agree with [`reconfiguration`](Self::reconfiguration) once the
    /// scheduling block is final. An application that never changes
    /// validators keeps the default.
    fn scheduled_change(&mut self, after: &Digest) -> Option<(u64, ValidatorSet)> {
        let _ = after;
        None
    }
}
