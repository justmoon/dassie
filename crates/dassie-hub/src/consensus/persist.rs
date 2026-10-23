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

use super::types::{Block, Certificate, Finalized, Vote, VoteKind};
use crate::digest::Digest;
use crate::validators::ValidatorSet;

/// A validator set and when it runs: from block `from_height`, as epoch
/// `number`, from its view 1 once the block before that height is final.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Epoch {
    /// Its number: one more than the epoch before it.
    pub number: u64,
    /// The first height it certifies.
    pub from_height: u64,
    /// Whether it runs: the block before `from_height` is final.
    pub started: bool,
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
    /// The view the engine is in, within the running epoch.
    pub view: u64,
    /// Every vote we signed in the running epoch, in a view above the
    /// tip's, and at or above `floor`.
    pub votes: Vec<Vote>,
    /// The views of the running epoch below which we no longer vote at all,
    /// even to finalize.
    pub floor: u64,
    /// For each height above the tip, the block we voted to finalize there.
    pub finalize_heights: Vec<(u64, Digest)>,
    /// The blocks we notarized or saw certified above the tip, with their
    /// digests, while the engine holds them: the chain from the tip to the
    /// highest notarized view among them, so that a notarized block, and
    /// what a later one needs to be final, survive even if every validator
    /// restarts. At most `MAX_BLOCKS`.
    pub blocks: Vec<(Digest, Block)>,
    /// The certificates we hold of views above the tip.
    pub certificates: Vec<Certificate>,
    /// The last block of each recent epoch that ended, with its
    /// finalization, oldest first.
    pub boundaries: Vec<Finalized>,
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
    /// Blocks we notarized or saw certified, with their digests. A step
    /// writes only these and the digests of the blocks it let go of, never
    /// what it already wrote: at most the blocks it took in, each at most
    /// `MAX_PAYLOAD_BYTES`.
    pub blocks: Vec<(Digest, Block)>,
    /// The blocks we let go of.
    pub blocks_let_go: Vec<Digest>,
    /// Certificates we took.
    pub certificates: Vec<Certificate>,
    /// The last blocks of epochs that ended.
    pub boundaries: Vec<Finalized>,
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
            && self.blocks_let_go.is_empty()
            && self.certificates.is_empty()
            && self.boundaries.is_empty()
            && self.tip.is_none()
            && self.epochs.is_none()
            && !self.stalled
    }
}

impl Stored {
    /// A new hub's state: `genesis` final, and `set` running from the
    /// height after it as epoch 0, from view 1.
    #[must_use]
    pub fn genesis(genesis: Block, set: ValidatorSet) -> Self {
        Self {
            epochs: alloc::vec![Epoch {
                number: genesis.epoch,
                from_height: genesis.height.saturating_add(1),
                started: true,
                set,
            }],
            view: 1,
            tip: Finalized::new(genesis, None),
            votes: Vec::new(),
            floor: 0,
            finalize_heights: Vec::new(),
            blocks: Vec::new(),
            certificates: Vec::new(),
            boundaries: Vec::new(),
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
        let let_go = batch.blocks_let_go;
        self.blocks.retain(|(digest, _)| !let_go.contains(digest));
        for (digest, block) in batch.blocks {
            if !self.blocks.iter().any(|(held, _)| *held == digest) {
                self.blocks.push((digest, block));
            }
        }
        self.certificates.extend(batch.certificates);
        self.boundaries.extend(batch.boundaries);
        let excess = self
            .boundaries
            .len()
            .saturating_sub(super::engine::MAX_BOUNDARIES);
        self.boundaries.drain(..excess);
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
        let epoch = self.running_epoch();
        let view = if self.tip.block.epoch == epoch {
            self.tip.block.view
        } else {
            0
        };
        let (height, floor) = (self.tip.block.height, self.floor);
        let live = |at: u64, of: u64| of == epoch && at > view && at >= floor;
        self.votes.retain(|vote| live(vote.view, vote.epoch));
        self.finalize_heights.retain(|(at, _)| *at > height);
        // Blocks below the floor stay while the engine holds them: the chain
        // to the highest notarized view. Repeats of the tip may stay as well.
        self.blocks.retain(|(_, block)| {
            block.epoch == epoch && block.view > view && block.height >= height
        });
        // Below the floor, the certificates of the chain's blocks stay with
        // them, for validators behind on it.
        let blocks = &self.blocks;
        self.certificates.retain(|certificate| {
            live(certificate.view, certificate.epoch)
                || (certificate.epoch == epoch
                    && certificate.view > view
                    && certificate.kind != VoteKind::Nullify
                    && blocks
                        .iter()
                        .any(|(digest, _)| *digest == certificate.block))
        });
    }

    /// The number of the epoch running: the last that started.
    #[must_use]
    pub fn running_epoch(&self) -> u64 {
        self.epochs
            .iter()
            .rev()
            .find(|epoch| epoch.started)
            .map_or(0, |epoch| epoch.number)
    }
}
