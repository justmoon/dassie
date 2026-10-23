//! What the engine persists, so that it never signs two conflicting votes,
//! even across a crash: the view it is in, every vote it signed above its
//! last finalized block, and whether it declared the hub stalled. It also
//! keeps the blocks it voted for and the certificates it holds above that
//! block, so that the hub can go on even if every validator restarts at
//! once: a later leader builds on a notarized block, or skips a view, only
//! with its certificate.
//!
//! The host persists each [`Batch`] before it sends the step's outputs, in
//! the same transaction as what the application did with the step's
//! finalized blocks (as `crates/dassie-node/src/engine/mod.rs` does for the
//! node's ledger), and restarts the engine from the [`Stored`] state the
//! batches built.

use alloc::vec::Vec;

use super::types::{Block, Certificate, Finalized, Vote};
use crate::digest::Digest;
use crate::validators::ValidatorSet;

/// A validator set and when it runs: from block `from_height`, and from
/// view `from_view` once the block before it is final.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epoch {
    /// The first height it certifies.
    pub from_height: u64,
    /// The first view it runs: the view after the one that finalized block
    /// `from_height − 1`, unknown until that block is final.
    pub from_view: Option<u64>,
    /// The set.
    pub set: ValidatorSet,
}

/// Everything the engine starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// The last finalized block: the genesis block for a new hub, or a
    /// certified block a new validator loaded the state after.
    pub tip: Finalized,
    /// The validator sets: the one running, and the one taking over.
    pub epochs: Vec<Epoch>,
    /// The view the engine is in: it never notarizes or nullifies below it.
    pub view: u64,
    /// Every vote we signed in a view above the tip's, and at or above
    /// `floor`.
    pub votes: Vec<Vote>,
    /// The views below which we no longer vote at all, even to finalize.
    pub floor: u64,
    /// For each height above the tip, the block we voted to finalize there.
    pub finalize_heights: Vec<(u64, Digest)>,
    /// The blocks we proposed or notarized above the tip, so that a
    /// notarized block survives even if every validator restarts.
    pub blocks: Vec<Block>,
    /// The certificates we hold of views above the tip.
    pub certificates: Vec<Certificate>,
    /// Whether we declared the hub stalled.
    pub stalled: bool,
}

/// What a step changed, to persist before its outputs are sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    /// The view we are now in.
    pub view: Option<u64>,
    /// Votes we signed.
    pub votes: Vec<Vote>,
    /// A new floor.
    pub floor: Option<u64>,
    /// Heights we voted to finalize a block at.
    pub finalize_heights: Vec<(u64, Digest)>,
    /// Blocks we proposed or notarized.
    pub blocks: Vec<Block>,
    /// Certificates we took.
    pub certificates: Vec<Certificate>,
    /// The new last finalized block.
    pub tip: Option<Finalized>,
    /// The validator sets, if they changed.
    pub epochs: Option<Vec<Epoch>>,
    /// We declared the hub stalled.
    pub stalled: bool,
}

impl Batch {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.view.is_none()
            && self.votes.is_empty()
            && self.floor.is_none()
            && self.finalize_heights.is_empty()
            && self.blocks.is_empty()
            && self.certificates.is_empty()
            && self.tip.is_none()
            && self.epochs.is_none()
            && !self.stalled
    }
}

impl Stored {
    /// A new hub's state: `genesis` final at view 0, and `set` running from
    /// height 1.
    #[must_use]
    pub fn genesis(genesis: Block, set: ValidatorSet) -> Self {
        let from_view = genesis.view.saturating_add(1);
        Self {
            epochs: alloc::vec![Epoch {
                from_height: genesis.height.saturating_add(1),
                from_view: Some(from_view),
                set,
            }],
            view: from_view,
            tip: Finalized {
                block: genesis,
                finalization: None,
            },
            votes: Vec::new(),
            floor: 0,
            finalize_heights: Vec::new(),
            blocks: Vec::new(),
            certificates: Vec::new(),
            stalled: false,
        }
    }

    /// Applies a batch, dropping what the new tip and floor make moot.
    pub fn apply(&mut self, batch: Batch) {
        if let Some(view) = batch.view {
            self.view = view;
        }
        self.votes.extend(batch.votes);
        self.finalize_heights.extend(batch.finalize_heights);
        self.blocks.extend(batch.blocks);
        self.certificates.extend(batch.certificates);
        if let Some(floor) = batch.floor {
            self.floor = floor;
        }
        if let Some(tip) = batch.tip {
            self.tip = tip;
        }
        if let Some(epochs) = batch.epochs {
            self.epochs = epochs;
        }
        self.stalled |= batch.stalled;
        let (view, height, floor) = (self.tip.block.view, self.tip.block.height, self.floor);
        self.votes
            .retain(|vote| vote.view > view && vote.view >= floor);
        self.finalize_heights.retain(|(at, _)| *at > height);
        self.blocks
            .retain(|block| block.height > height && block.view >= floor);
        self.certificates
            .retain(|certificate| certificate.view > view && certificate.view >= floor);
    }
}
