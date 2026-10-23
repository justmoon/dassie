//! What the engine keeps while nothing becomes final
//! (spec/areas/hubs.md#consensus, spec/decisions/finality-after-any-outage.md):
//! the views from the highest one that notarized a block that may extend
//! the tip, the blocks of those views, the chain from the tip to that block,
//! and the views below that certified a block of the chain. Nothing else is
//! needed to build on it, and nothing of that is let go of while it is not
//! final, so however long finality is lost, the hub finalizes again once the
//! votes go through. A validator behind on the chain asks for what it lacks,
//! by the view that certified it, and takes it below its floor; one that
//! learns the chain was finalized takes the finalization below its floor
//! too.
//!
//! What stays bounded: the chain, because no block is notarized more than
//! [`MAX_UNFINALIZED_HEIGHTS`] above the tip (past it, leaders repeat the
//! last); and the views after the highest notarized one, because each view
//! nullified in a row past a few waits twice as long as the one before.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::super::types::{Block, Certificate, VoteKind, verify_certificate};
use super::chain::Ancestry;
use super::round::Round;
use super::{Engine, MAX_BLOCKS, MAX_ROUNDS, MAX_UNFINALIZED_HEIGHTS};
use crate::digest::Digest;

/// Whether `round` notarized or finalized one of `blocks`.
pub(super) fn certifies(round: &Round, blocks: &BTreeSet<Digest>) -> bool {
    [&round.notarization, &round.finalization]
        .into_iter()
        .flatten()
        .any(|certificate| blocks.contains(&certificate.block))
}

impl Engine {
    /// The highest view that notarized (or finalized) a block held that may
    /// extend the tip, and the block it stands for (itself, or the block a
    /// repeat repeats). Its chain to the tip may lack blocks: a validator
    /// behind on it fetches them, below the floor if need be.
    pub(super) fn carrier(&self) -> Option<(u64, Digest)> {
        self.rounds.iter().rev().find_map(|(view, round)| {
            let certificate = round
                .notarization
                .as_ref()
                .or(round.finalization.as_ref())?;
            let (content, _) = self.content_of(&certificate.block)?;
            (self.ancestry(&content, *view) != Ancestry::Forks).then_some((*view, content))
        })
    }

    /// Whether `digest` is the block the chain to the highest notarized
    /// view lacks: the parent of its lowest block held.
    pub(super) fn gap_of_chain(&self, digest: &Digest) -> bool {
        if *digest == self.tip_digest || self.blocks.contains_key(digest) {
            return false;
        }
        let Some((_, content)) = self.carrier() else {
            return false;
        };
        self.chain_of(content)
            .iter()
            .filter_map(|held| self.blocks.get(held))
            .any(|block| block.parent == *digest)
    }

    /// Takes a certificate of a view below the floor (and above the tip)
    /// that the chain to the highest notarized view needs: one that names,
    /// with it, the block the chain lacks, or that finalizes a block of the
    /// chain. Returns whether it took it.
    pub(super) fn take_for_chain(
        &mut self,
        certificate: &Certificate,
        block: Option<&Block>,
    ) -> bool {
        let gap = block.is_some_and(|block| {
            certificate.kind != VoteKind::Nullify
                && block.view == certificate.view
                && block.epoch == self.epoch()
                && block.height > self.tip.block.height
                && block.digest(&self.hub) == certificate.block
                && self.gap_of_chain(&certificate.block)
                && self.blocks.len() < MAX_BLOCKS
        });
        let finalizes = certificate.kind == VoteKind::Finalize
            && self
                .carrier()
                .is_some_and(|(_, content)| self.chain_of(content).contains(&certificate.block));
        let view = certificate.view;
        let room = self.rounds.contains_key(&view) || self.rounds.len() < MAX_ROUNDS;
        if !(gap || finalizes)
            || !room
            || verify_certificate(&self.hub, self.set(), certificate).is_err()
        {
            return false;
        }
        let Some(leader) = self.leader(view) else {
            return false;
        };
        if let Some(block) = block.filter(|_| gap) {
            self.blocks.insert(certificate.block, block.clone());
            self.persist_block(certificate.block);
        }
        self.batch.certificates.push(certificate.clone());
        self.rounds
            .entry(view)
            .or_insert_with(|| Round::new(leader))
            .hold(certificate.clone());
        true
    }

    /// The blocks from the tip's child to `from`, which extends the tip.
    pub(super) fn chain_of(&self, from: Digest) -> BTreeSet<Digest> {
        let mut chain = BTreeSet::new();
        let mut at = from;
        for _ in 0..=MAX_BLOCKS {
            let Some(block) = self.blocks.get(&at) else {
                break;
            };
            chain.insert(at);
            at = block.parent;
        }
        chain
    }

    /// Whether `block` repeats the tip: a repeat of the block that became
    /// final, which later blocks may still build on through its view.
    pub(super) fn repeats_tip(&self, block: &Block) -> bool {
        block.parent == self.tip_digest && block.repeats(&self.tip.block)
    }

    /// The highest height a block of ours may have, or one we notarize: the
    /// tip's, and as many more as may wait to be final.
    pub(super) fn height_cap(&self) -> u64 {
        self.tip
            .block
            .height
            .saturating_add(MAX_UNFINALIZED_HEIGHTS)
    }

    /// Lets go of the views below the highest one that notarized a block
    /// extending the tip: no block is built on anything below it.
    pub(super) fn compact(&mut self) {
        if let Some((view, _)) = self.carrier()
            && view > self.floor
        {
            self.raise_floor(view);
        }
    }

    /// Keeps the views from the floor, and below it those that certified a
    /// block of the chain from the tip to the block the highest notarized
    /// view stands for: a validator behind on that chain asks for them. Keeps
    /// the blocks of the views held from the floor, the chain, and repeats of
    /// the tip; lets go of the rest.
    pub(super) fn retain_blocks(&mut self) {
        let chain = self
            .carrier()
            .map(|(_, content)| self.chain_of(content))
            .unwrap_or_default();
        let floor = self.floor;
        self.rounds
            .retain(|view, round| *view >= floor || certifies(round, &chain));
        let keep: BTreeSet<Digest> = self
            .blocks
            .iter()
            .filter(|(digest, block)| {
                (self.rounds.contains_key(&block.view) && block.view >= self.floor)
                    || chain.contains(*digest)
                    || self.repeats_tip(block)
            })
            .map(|(digest, _)| *digest)
            .collect();
        if keep.len() < self.blocks.len() {
            let let_go: Vec<Digest> = self
                .blocks
                .keys()
                .filter(|digest| !keep.contains(*digest))
                .copied()
                .collect();
            self.blocks.retain(|digest, _| keep.contains(digest));
            self.batch
                .blocks
                .retain(|(digest, _)| keep.contains(digest));
            self.batch.blocks_let_go.extend(let_go);
        }
        let blocks = &self.blocks;
        self.executed.retain(|digest| blocks.contains_key(digest));
    }

    /// Holds, after a restart, `certificate` of a view below the floor that
    /// notarized a block held: one of the chain, kept for those behind on it.
    pub(super) fn restore_chain_certificate(&mut self, certificate: Certificate) {
        let view = certificate.view;
        if certificate.kind == VoteKind::Nullify
            || view <= self.tip_view()
            || !self.blocks.contains_key(&certificate.block)
            || (!self.rounds.contains_key(&view) && self.rounds.len() >= MAX_ROUNDS)
        {
            return;
        }
        let Some(leader) = self.leader(view) else {
            return;
        };
        self.rounds
            .entry(view)
            .or_insert_with(|| Round::new(leader))
            .hold(certificate);
    }
}
