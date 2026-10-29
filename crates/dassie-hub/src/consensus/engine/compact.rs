//! What the engine keeps while nothing becomes final
//! (spec/areas/hubs.md#consensus, spec/decisions/finality-after-any-outage.md):
//! the views from the highest one that notarized a block that may extend
//! the tip (the floor), the blocks of those views, and below them every
//! block that the views from the floor notarized, finalized or that we
//! voted for builds on, back to the tip, with the views that certified it
//! ([`Engine::kept`]).
//!
//! Not only the chain to the highest notarized block: a block may skip a
//! notarized view that is also nullified, which Simplex allows, so a later
//! block may build on a notarized block below the floor, and one we voted
//! for may do so already. Letting go of its parent would leave such a block
//! held without it, which nothing can then finalize or build on, and which
//! no validator may hold any more to hand over. Nothing kept is let go of
//! while it is not final, so however long finality is lost, the hub
//! finalizes again once the votes go through. A validator behind asks for
//! what it lacks, by the view that certified it, and takes it below its
//! floor; one that learns a block kept was finalized takes the finalization
//! below its floor too.
//!
//! What stays bounded: each chain, because no block is notarized more than
//! [`MAX_UNFINALIZED_HEIGHTS`] above the tip (past it, leaders repeat the
//! last); the views after the highest notarized one, because each view
//! nullified in a row past a few waits twice as long as the one before;
//! and all of it by [`MAX_ROUNDS`] and [`MAX_BLOCKS`]. Chains branch below
//! the floor only at views both notarized and nullified, and while at most
//! f validators are faulty a view notarizes at most one block.

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

    /// The blocks kept whatever the floor ([`Self::retain_blocks`]): every
    /// block held that a view from the floor notarized or finalized, or
    /// that we voted to notarize there, and the blocks held it builds on,
    /// back to the tip. A later block may build on any of them, through
    /// views both notarized and nullified, and a block we voted for may
    /// build on one below the floor already.
    pub(super) fn kept(&self) -> BTreeSet<Digest> {
        let mut kept = BTreeSet::new();
        for round in self.rounds.range(self.floor..).map(|(_, round)| round) {
            let certified = [&round.notarization, &round.finalization]
                .into_iter()
                .flatten()
                .map(|certificate| certificate.block);
            for root in certified.chain(round.voted_notarize()) {
                let mut at = root;
                // Each step keeps a block held that was not kept before.
                for _ in 0..=MAX_BLOCKS {
                    if kept.contains(&at) {
                        break;
                    }
                    let Some(block) = self.blocks.get(&at) else {
                        break;
                    };
                    kept.insert(at);
                    at = block.parent;
                }
            }
        }
        assert!(kept.len() <= self.blocks.len(), "only blocks held are kept");
        kept
    }

    /// Whether `digest` is a block we lack that a block kept builds on.
    pub(super) fn gap_of_chain(&self, digest: &Digest) -> bool {
        if *digest == self.tip_digest || self.blocks.contains_key(digest) {
            return false;
        }
        self.kept()
            .iter()
            .filter_map(|held| self.blocks.get(held))
            .any(|block| block.parent == *digest)
    }

    /// Takes a certificate of a view below the floor (and above the tip)
    /// that the blocks kept need: one that names, with it, a block they
    /// lack ([`Self::gap_of_chain`]), or that finalizes one of them.
    /// Returns whether it took it.
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
        let finalizes =
            certificate.kind == VoteKind::Finalize && self.kept().contains(&certificate.block);
        let view = certificate.view;
        if !(gap || finalizes) || verify_certificate(&self.hub, self.set(), certificate).is_err() {
            return false;
        }
        if !self.rounds.contains_key(&view) && self.rounds.len() >= MAX_ROUNDS {
            self.stats.no_room = self.stats.no_room.saturating_add(1);
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
    /// extending the tip, but for what [`Self::kept`] needs: an honest
    /// leader that knows of that view builds on it or a later one; a block
    /// built lower skips views let go of, whose nullifications we no longer
    /// hold to check it by; and what a block held notarized, or voted for,
    /// builds on is kept.
    pub(super) fn compact(&mut self) {
        if let Some((view, _)) = self.carrier()
            && view > self.floor
        {
            self.raise_floor(view);
        }
    }

    /// Keeps the views from the floor, and below it those that certified a
    /// block kept ([`Self::kept`]): a validator behind on one asks for them.
    /// Keeps the blocks of the views held from the floor, the blocks kept,
    /// and repeats of the tip; lets go of the rest.
    pub(super) fn retain_blocks(&mut self) {
        let kept = self.kept();
        let floor = self.floor;
        self.rounds
            .retain(|view, round| *view >= floor || certifies(round, &kept));
        let keep: BTreeSet<Digest> = self
            .blocks
            .iter()
            .filter(|(digest, block)| {
                (self.rounds.contains_key(&block.view) && block.view >= self.floor)
                    || kept.contains(*digest)
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
            assert!(
                kept.iter()
                    .filter_map(|held| self.blocks.get(held))
                    .all(|block| {
                        keep.contains(&block.parent) || !self.blocks.contains_key(&block.parent)
                    }),
                "a block notarized or voted for keeps its parent"
            );
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
    /// notarized a block held: one kept, for those behind on it.
    pub(super) fn restore_chain_certificate(&mut self, certificate: Certificate) {
        let view = certificate.view;
        if certificate.kind == VoteKind::Nullify
            || view <= self.tip_view()
            || !self.blocks.contains_key(&certificate.block)
        {
            return;
        }
        if !self.rounds.contains_key(&view) && self.rounds.len() >= MAX_ROUNDS {
            self.stats.no_room = self.stats.no_room.saturating_add(1);
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
