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
//! batches built. [`Stored`] encodes in canonical OER, so a host can keep
//! it as bytes:
//!
//! ```text
//! Stored ::= SEQUENCE {
//!   tip Finalized,
//!   epochs SEQUENCE (SIZE(1..3)) OF SEQUENCE {
//!     number UInt64, fromHeight UInt64, started BOOLEAN,
//!     set SEQUENCE (SIZE(4..16)) OF OCTET STRING (SIZE(32)) },
//!   view UInt64, votes SEQUENCE (SIZE(0..1024)) OF Vote, floor UInt64,
//!   finalizeHeights SEQUENCE (SIZE(0..1024)) OF SEQUENCE {
//!     height UInt64, block OCTET STRING (SIZE(32)) },
//!   blocks SEQUENCE (SIZE(0..768)) OF SEQUENCE {
//!     digest OCTET STRING (SIZE(32)), block Block },
//!   certificates SEQUENCE (SIZE(0..1024)) OF Certificate,
//!   boundaries SEQUENCE (SIZE(0..8)) OF Finalized,
//!   stalled BOOLEAN }
//! Finalized ::= SEQUENCE { block Block,
//!   repeats SEQUENCE (SIZE(0..32)) OF Block,
//!   finalization CHOICE { none [0] NULL, some [1] Certificate } }
//! ```
//!
//! The layout is this file's alone: a change of what the engine persists
//! changes it here, with nothing to migrate but a host's stored bytes.
//! ```

use alloc::vec::Vec;

use dassie_identity::PublicKey;
use dassie_oer::{Decode, DecodeError, Encode, Reader, Writer, size};

use super::engine::MAX_BOUNDARIES;
use super::epochs::MAX_EPOCHS;
use super::types::{Block, Certificate, Finalized, Vote, VoteKind};
use super::wire::MAX_DELIVERED_BLOCKS;
use super::wire::{optional_len, read_digest, read_optional, unexpected, write_optional};
use crate::digest::Digest;
use crate::validators::{MAX_VALIDATORS, MIN_VALIDATORS, ValidatorSet};

/// The most votes, heights voted to finalize, and certificates [`Stored`]
/// holds: each is of a view above the tip that the engine holds, at most
/// [`MAX_ROUNDS`](super::MAX_ROUNDS), and a view has a few of each.
pub const MAX_STORED_ITEMS: usize = 4 * super::MAX_ROUNDS;

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

impl Encode for Epoch {
    fn encoded_len(&self) -> usize {
        8 + 8 + 1 + size::quantity(self.set.len()) + 32 * self.set.len()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u64(self.number);
        writer.write_u64(self.from_height);
        writer.write_bool(self.started);
        writer.write_quantity(self.set.len());
        for key in self.set.keys() {
            writer.write_bytes(key.as_bytes());
        }
    }
}

impl<'a> Decode<'a> for Epoch {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let number = reader.read_u64()?;
        let from_height = reader.read_u64()?;
        let started = reader.read_bool()?;
        let count = reader.read_quantity(MAX_VALIDATORS)?;
        if count < MIN_VALIDATORS {
            return Err(unexpected(reader));
        }
        let mut keys = Vec::with_capacity(count);
        for _ in 0..count {
            let key = PublicKey::from_bytes(reader.read_array::<32>()?)
                .ok()
                .filter(|key| !key.is_weak())
                .ok_or_else(|| unexpected(reader))?;
            keys.push(key);
        }
        let set = ValidatorSet::new(keys).ok_or_else(|| unexpected(reader))?;
        Ok(Self {
            number,
            from_height,
            started,
            set,
        })
    }
}

impl Encode for Finalized {
    fn encoded_len(&self) -> usize {
        self.block.encoded_len()
            + items_len(&self.repeats)
            + optional_len(self.finalization.as_ref())
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.block.encode_to(writer);
        write_items(writer, &self.repeats);
        write_optional(writer, self.finalization.as_ref());
    }
}

impl<'a> Decode<'a> for Finalized {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        Ok(Self {
            block: Block::decode(reader)?,
            repeats: read_items(reader, MAX_DELIVERED_BLOCKS)?,
            finalization: read_optional(reader)?,
        })
    }
}

fn items_len<T: Encode>(items: &[T]) -> usize {
    size::quantity(items.len()) + items.iter().map(Encode::encoded_len).sum::<usize>()
}

fn write_items<T: Encode>(writer: &mut Writer<'_>, items: &[T]) {
    writer.write_quantity(items.len());
    for item in items {
        item.encode_to(writer);
    }
}

fn read_items<'a, T: Decode<'a, Error = DecodeError>>(
    reader: &mut Reader<'a>,
    max: usize,
) -> Result<Vec<T>, DecodeError> {
    let count = reader.read_quantity(max)?;
    // Allocates only as it reads: a count is no promise of the items.
    let mut items = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        items.push(T::decode(reader)?);
    }
    Ok(items)
}

/// The blocks of a [`Stored`], each with its digest.
fn read_blocks(reader: &mut Reader<'_>) -> Result<Vec<(Digest, Block)>, DecodeError> {
    let count = reader.read_quantity(super::MAX_BLOCKS)?;
    let mut blocks = Vec::with_capacity(count.min(64));
    for _ in 0..count {
        blocks.push((read_digest(reader)?, Block::decode(reader)?));
    }
    Ok(blocks)
}

impl Stored {
    /// Whether every list fits the bounds its encoding is read with. What
    /// the engine holds always does; a host that finds otherwise has a
    /// bug, and does not write what it could not read back.
    #[must_use]
    pub fn fits(&self) -> bool {
        !self.epochs.is_empty()
            && self.epochs.len() <= MAX_EPOCHS
            && self.tip.repeats.len() <= MAX_DELIVERED_BLOCKS
            && self.votes.len() <= MAX_STORED_ITEMS
            && self.finalize_heights.len() <= MAX_STORED_ITEMS
            && self.blocks.len() <= super::MAX_BLOCKS
            && self.certificates.len() <= MAX_STORED_ITEMS
            && self.boundaries.len() <= MAX_BOUNDARIES
            && self
                .boundaries
                .iter()
                .all(|b| b.repeats.len() <= MAX_DELIVERED_BLOCKS)
    }
}

impl Encode for Stored {
    fn encoded_len(&self) -> usize {
        self.tip.encoded_len()
            + items_len(&self.epochs)
            + 8
            + items_len(&self.votes)
            + 8
            + size::quantity(self.finalize_heights.len())
            + 40 * self.finalize_heights.len()
            + size::quantity(self.blocks.len())
            + self
                .blocks
                .iter()
                .map(|(_, block)| 32 + block.encoded_len())
                .sum::<usize>()
            + items_len(&self.certificates)
            + items_len(&self.boundaries)
            + 1
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.tip.encode_to(writer);
        write_items(writer, &self.epochs);
        writer.write_u64(self.view);
        write_items(writer, &self.votes);
        writer.write_u64(self.floor);
        writer.write_quantity(self.finalize_heights.len());
        for (height, block) in &self.finalize_heights {
            writer.write_u64(*height);
            writer.write_bytes(block.as_bytes());
        }
        writer.write_quantity(self.blocks.len());
        for (digest, block) in &self.blocks {
            writer.write_bytes(digest.as_bytes());
            block.encode_to(writer);
        }
        write_items(writer, &self.certificates);
        write_items(writer, &self.boundaries);
        writer.write_bool(self.stalled);
    }
}

impl<'a> Decode<'a> for Stored {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let tip = Finalized::decode(reader)?;
        let epochs: Vec<Epoch> = read_items(reader, MAX_EPOCHS)?;
        if epochs.is_empty() {
            return Err(unexpected(reader));
        }
        let view = reader.read_u64()?;
        let votes = read_items(reader, MAX_STORED_ITEMS)?;
        let floor = reader.read_u64()?;
        let count = reader.read_quantity(MAX_STORED_ITEMS)?;
        let mut finalize_heights = Vec::with_capacity(count.min(64));
        for _ in 0..count {
            finalize_heights.push((reader.read_u64()?, read_digest(reader)?));
        }
        Ok(Self {
            tip,
            epochs,
            view,
            votes,
            floor,
            finalize_heights,
            blocks: read_blocks(reader)?,
            certificates: read_items(reader, MAX_STORED_ITEMS)?,
            boundaries: read_items(reader, MAX_BOUNDARIES)?,
            stalled: reader.read_bool()?,
        })
    }
}
