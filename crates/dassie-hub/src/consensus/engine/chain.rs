//! The chain: blocks above the tip, finalizing them in order, the handover
//! from one epoch to the next, and finding finalized blocks that conflict.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::super::hooks::Application;
use super::super::io::{Event, Evidence, Recipient};
use super::super::types::{Block, Certificate, Finalized, VoteKind};
use super::{
    BLOCKS_PER_VIEW, Engine, MAX_BLOCKS, MAX_BOUNDARIES, MAX_VIEWS_AHEAD, MAX_VOTE_LOG,
    RETAINED_BLOCKS,
};
use crate::digest::Digest;
use crate::validators::Signers;

/// How a block relates to the tip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ancestry {
    /// It is the tip, or its ancestors lead to it.
    Extends,
    /// An ancestor is not held; the view to ask from.
    Missing(u64),
    /// Its ancestors pass the tip's height elsewhere.
    Forks,
}

/// Why a block is held. Room is kept per view, so that no validator can
/// crowd out another view's blocks: a faulty leader fills only its own
/// views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hold {
    /// The first proposal of its view: ours, the leader's first, or one we
    /// voted for before a restart.
    Proposal,
    /// A block that a verified certificate names.
    Certified,
    /// A block persisted before a restart: ours, or one held then, and
    /// trusted as such. It may be below the floor, on the chain to the
    /// highest notarized view.
    Restored,
}

impl Engine {
    /// Holds a block of the running epoch above the tip and returns its
    /// digest: the first proposal of its view, or a block a certificate
    /// names, within [`BLOCKS_PER_VIEW`] for its view. A block that could
    /// not extend the tip, or of a view we hold no round for, is not held.
    pub(super) fn hold_block(&mut self, block: Block, why: Hold) -> Digest {
        let digest = block.digest(&self.hub);
        if self.blocks.contains_key(&digest) || self.blocks.len() >= MAX_BLOCKS {
            return digest;
        }
        if why == Hold::Restored {
            let above = block.height > self.tip.block.height || self.repeats_tip(&block);
            if block.epoch == self.epoch() && above {
                self.blocks.insert(digest, block);
            }
            return digest;
        }
        if !self.plausible(&block) {
            return digest;
        }
        let view = block.view;
        let Some(round) = self.round_mut(view) else {
            return digest;
        };
        let first = round.proposal.is_none();
        let held = self
            .blocks
            .values()
            .filter(|held| held.view == view)
            .count();
        let admitted = match why {
            Hold::Proposal => first,
            Hold::Certified | Hold::Restored => true,
        };
        if admitted && held < BLOCKS_PER_VIEW {
            self.blocks.insert(digest, block);
            // A certified block is kept across a restart: the chain to the
            // highest notarized view must survive one, even below the
            // floor. A proposal is, once we notarize it.
            if why == Hold::Certified {
                self.persist_block(digest);
            }
        }
        digest
    }

    /// Writes block `digest`, held, in the batch: one we notarize, or one a
    /// certificate names.
    pub(super) fn persist_block(&mut self, digest: Digest) {
        if let Some(block) = self.blocks.get(&digest) {
            self.batch.blocks.push((digest, block.clone()));
        }
    }

    /// Whether `block` could extend the tip: of the running epoch, in a
    /// view we may hold, above the tip, built on an earlier view, and no
    /// higher than one block per view since the tip allows.
    fn plausible(&self, block: &Block) -> bool {
        let tip_view = self.tip_view();
        let views = block.view.saturating_sub(tip_view);
        let heights = block.height.saturating_sub(self.tip.block.height);
        block.epoch == self.epoch()
            && self.holds_view(block.view)
            && block.view <= self.view.saturating_add(MAX_VIEWS_AHEAD)
            && block.parent_view < block.view
            && (heights > 0 || self.repeats_tip(block))
            && heights <= views
    }

    /// The block `digest` names: the tip, or one held.
    pub(super) fn known_block(&self, digest: &Digest) -> Option<&Block> {
        if *digest == self.tip_digest {
            return Some(&self.tip.block);
        }
        self.blocks.get(digest)
    }

    /// Whether `block`, if it has its parent's height, has a repeat's shape:
    /// no payload, in the running epoch. Whether it repeats the epoch's last
    /// block is a matter of the chain it builds on ([`Self::boundary_on`]).
    /// A block of another height, or whose parent is not known, passes.
    pub(super) fn repeat_shaped(&self, block: &Block) -> bool {
        let Some(parent) = self.known_block(&block.parent) else {
            return true;
        };
        block.height != parent.height || (block.repeats(parent) && block.epoch == self.epoch())
    }

    /// How the block `digest`, of view `view`, relates to the tip.
    pub(super) fn ancestry(&self, digest: &Digest, view: u64) -> Ancestry {
        self.ancestry_known(digest, view, &mut BTreeMap::new())
    }

    /// [`Self::ancestry`], for many blocks while neither the tip nor the
    /// blocks held change: `known` keeps how each held block met on the way
    /// relates to the tip, and a later walk stops at the first it reaches.
    /// A held block's ancestry is the same whatever view it was asked with,
    /// since only a block not held answers with the view, so each answer is
    /// the one [`Self::ancestry`] gives; walks of many finalizations down
    /// one chain then cost its length once, not once each.
    fn ancestry_known(
        &self,
        digest: &Digest,
        view: u64,
        known: &mut BTreeMap<Digest, Ancestry>,
    ) -> Ancestry {
        let mut at = *digest;
        let mut view = view;
        let mut path = Vec::new();
        let mut found = Ancestry::Forks;
        for _ in 0..=MAX_BLOCKS {
            if at == self.tip_digest {
                found = Ancestry::Extends;
                break;
            }
            if let Some(seen) = known.get(&at) {
                found = *seen;
                break;
            }
            let Some(block) = self.blocks.get(&at) else {
                found = Ancestry::Missing(view);
                break;
            };
            if block.height <= self.tip.block.height {
                found = Ancestry::Forks;
                break;
            }
            path.push(at);
            view = block.parent_view;
            at = block.parent;
        }
        for held in path {
            known.insert(held, found);
        }
        assert!(known.len() <= MAX_BLOCKS, "more blocks known than held");
        found
    }

    /// The content block a block stands for: itself, or the block its
    /// repeats repeat. With its height.
    pub(super) fn content_of(&self, digest: &Digest) -> Option<(Digest, u64)> {
        let mut at = *digest;
        for _ in 0..=MAX_BLOCKS {
            let block = self.known_block(&at)?;
            match self.known_block(&block.parent) {
                Some(parent) if block.repeats(parent) => at = block.parent,
                _ => return Some((at, block.height)),
            }
        }
        None
    }

    /// The blocks from the tip's child to `digest`, which must extend the
    /// tip.
    fn chain_to(&self, digest: &Digest) -> Vec<(Digest, Block)> {
        let mut chain = Vec::new();
        let mut at = *digest;
        for _ in 0..=MAX_BLOCKS {
            if at == self.tip_digest {
                break;
            }
            let Some(block) = self.blocks.get(&at) else {
                break;
            };
            chain.push((at, block.clone()));
            at = block.parent;
        }
        chain.reverse();
        chain
    }

    /// Notes that we lack what view `view` needs, and that `from` has it.
    pub(super) fn want(&mut self, view: u64, from: Recipient) {
        let lower = self.want.as_ref().is_none_or(|(held, _)| view < *held);
        if lower {
            self.want = Some((view, from));
        }
    }

    /// Notes that we lack what view `view` needs, from any validator.
    pub(super) fn want_from_any(&mut self, view: u64) {
        let set = self.set();
        let others: Vec<_> = set
            .keys()
            .iter()
            .filter(|key| **key != self.public)
            .collect();
        let count = others.len().max(1);
        let pick = others
            .get(self.next_ask.checked_rem(count).unwrap_or(0))
            .map(|key| **key);
        if let Some(key) = pick {
            self.want(view, Recipient::Validator(alloc::boxed::Box::new(key)));
        }
    }

    /// Finalizes the chain up to the highest finalization held whose
    /// blocks we hold. Returns whether it did anything.
    pub(super) fn finalize_chain(&mut self, app: &mut dyn Application) -> bool {
        let finalized: Vec<(u64, Digest)> = self
            .rounds
            .iter()
            .rev()
            .filter(|(_, round)| !round.forked)
            .filter_map(|(view, round)| Some((*view, round.finalization.as_ref()?.block)))
            .collect();
        // Nothing below changes the tip or the blocks held but what returns.
        let mut known = BTreeMap::new();
        for (view, digest) in finalized {
            match self.ancestry_known(&digest, view, &mut known) {
                Ancestry::Extends => {
                    let certificate = self
                        .rounds
                        .get(&view)
                        .and_then(|round| round.finalization.clone());
                    let chain = self.chain_to(&digest);
                    if self.apply_chain(app, chain, certificate) {
                        return true;
                    }
                    // A finalization of nothing we can finalize: only more
                    // than f faulty validators, or an application that
                    // breaks its contract, make one. We stall on it, and
                    // never take it up again.
                    if let Some(round) = self.rounds.get_mut(&view) {
                        round.forked = true;
                    }
                }
                Ancestry::Missing(missing) => self.want_from_any(missing),
                Ancestry::Forks => {
                    if let Some(round) = self.rounds.get_mut(&view) {
                        round.forked = true;
                    }
                    let content = self
                        .content_of(&digest)
                        .and_then(|(content, _)| self.known_block(&content).cloned());
                    if let (Some(block), Some(certificate)) = (
                        content,
                        self.rounds.get(&view).and_then(|r| r.finalization.clone()),
                    ) {
                        self.check_finalized(&block, &certificate);
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Finalizes `chain`, which extends the tip, with `finalization` of its
    /// last block. Repeats come only at its end, of its last content block,
    /// which is then the last of its epoch: they prove it, and the
    /// application never sees them. Returns whether it finalized a block.
    pub(super) fn apply_chain(
        &mut self,
        app: &mut dyn Application,
        chain: Vec<(Digest, Block)>,
        finalization: Option<Certificate>,
    ) -> bool {
        let content = chain
            .iter()
            .zip(chain.iter().skip(1))
            .position(|((_, block), (_, next))| next.repeats(block))
            .map_or(chain.len(), |at| at.saturating_add(1));
        let mut chain = chain;
        let repeats: Vec<Block> = chain
            .split_off(content)
            .into_iter()
            .map(|(_, block)| block)
            .collect();
        let last = chain.len().saturating_sub(1);
        let mut proof = Some((repeats, finalization));
        // The content first, in order: a block in it may schedule the
        // change that ends the epoch. The chain stops where the epoch
        // ended, which only more than f faulty validators can make happen
        // before its last block.
        let mut applied = false;
        for (at, (digest, block)) in chain.into_iter().enumerate() {
            let follows = block.parent == self.tip_digest
                && block.height == self.tip.block.height.saturating_add(1)
                && block.epoch == self.epoch();
            if !follows {
                break;
            }
            let own = if at == last { proof.take() } else { None };
            self.apply_finalized(app, digest, block, own.unwrap_or_default());
            applied = true;
        }
        applied
    }

    /// Makes `block` final: tells the application, moves the tip, and lets
    /// go of what is now moot. `proof` is its repeats and the finalization
    /// that proves it, if it has its own.
    pub(super) fn apply_finalized(
        &mut self,
        app: &mut dyn Application,
        digest: Digest,
        block: Block,
        (repeats, finalization): (Vec<Block>, Option<Certificate>),
    ) {
        assert!(
            block.parent == self.tip_digest
                && block.height == self.tip.block.height.saturating_add(1)
                && block.view > self.view_in_epoch(&self.tip.block)
                && block.epoch == self.epoch(),
            "a finalized block extends the tip, in the running epoch"
        );
        app.finalized(&block, &digest);
        self.stats.finalized = self.stats.finalized.saturating_add(1);
        self.event(Event::Finalized {
            view: block.view,
            height: block.height,
            block: digest,
        });
        let height = block.height;
        self.tip = Finalized {
            block,
            repeats,
            finalization,
        };
        self.tip_digest = digest;
        self.batch.tip = Some(self.tip.clone());
        self.history.push_back((digest, self.tip.clone()));
        while self.history.len() > RETAINED_BLOCKS {
            self.history.pop_front();
        }
        self.prune();
        // The change the final state names, taken in before the next block
        // is final, in the same step, so a catch-up that finalizes the block
        // scheduling a change and the block before its boundary at once
        // cannot miss it. Every final block until the change is made names
        // it; taking it again changes nothing.
        if let Some((from_height, set)) = app.scheduled_change(&digest) {
            match self.epochs.take_in(from_height, set, height) {
                Ok(true) => self.batch.epochs = Some(self.epochs.to_vec()),
                Ok(false) => {}
                Err(error) => self.event(Event::ReconfigureRefused(error)),
            }
        }
        self.after_finalized();
    }

    /// What follows a new tip: the next epoch, if the tip was the last
    /// block of this one, and the view after the tip's.
    pub(super) fn after_finalized(&mut self) {
        if let Some(epoch) = self.epochs.finalized(self.tip.block.height) {
            self.start_epoch(epoch);
        } else if self.view <= self.tip_view() {
            self.enter(self.tip_view().saturating_add(1));
        }
    }

    /// Keeps the tip as the last block of an epoch, with the finalization
    /// that proves it (its own or a repeat's): what a validator coming
    /// back from that epoch can check, and catch up from.
    fn keep_boundary(&mut self) {
        if self.tip.finalization.is_none() {
            return;
        }
        if self.boundaries.len() >= MAX_BOUNDARIES {
            self.boundaries.pop_front();
        }
        self.boundaries.push_back(self.tip.clone());
        self.batch.boundaries.push(self.tip.clone());
    }

    /// Lets go of rounds, blocks and heights at or below the tip, keeping
    /// the votes for blocks it finalized.
    pub(super) fn prune(&mut self) {
        let (view, height) = (self.tip_view(), self.tip.block.height);
        let epoch = self.epoch();
        let done: Vec<u64> = self.rounds.range(..=view).map(|(view, _)| *view).collect();
        for at in done {
            let Some(round) = self.rounds.remove(&at) else {
                continue;
            };
            let final_block = self
                .history
                .iter()
                .find(|(_, finalized)| finalized.block.epoch == epoch && finalized.block.view == at)
                .map(|(digest, _)| *digest);
            if let Some(final_block) = final_block {
                let votes = round.block_votes().filter(|vote| vote.block == final_block);
                self.vote_log.extend(votes.copied());
            }
        }
        let oldest = self
            .history
            .front()
            .map_or((epoch, view), |(_, f)| (f.block.epoch, f.block.view));
        while self.vote_log.len() > MAX_VOTE_LOG
            || self
                .vote_log
                .front()
                .is_some_and(|vote| (vote.epoch, vote.view) < oldest)
        {
            self.vote_log.pop_front();
        }
        self.settled_conflicts.retain(|at| *at >= oldest);
        let (tip, tip_block) = (self.tip_digest, self.tip.block.clone());
        self.blocks.retain(|_, block| {
            let repeats_tip = block.parent == tip && block.repeats(&tip_block);
            (block.height > height || repeats_tip) && block.view > view
        });
        let blocks = &self.blocks;
        self.executed.retain(|digest| blocks.contains_key(digest));
        self.finalize_heights.retain(|at, _| *at > height);
        if self.want.as_ref().is_some_and(|(want, _)| *want <= view) {
            self.want = None;
        }
    }

    /// The tip is the last block of its epoch: the next set runs epoch
    /// `epoch` from view 1, building on it. Nothing of the epoch that ended
    /// is held any more, but its last block and the proof of it.
    pub(super) fn start_epoch(&mut self, epoch: u64) {
        assert!(epoch == self.epoch(), "the epoch that started runs");
        if self.tip.block.epoch < epoch {
            self.keep_boundary();
        }
        self.batch.epochs = Some(self.epochs.to_vec());
        self.event(Event::Reconfigured {
            from_height: self.tip.block.height.saturating_add(1),
            epoch,
        });
        self.last_voted = [0; crate::validators::MAX_VALIDATORS];
        self.rounds.clear();
        self.blocks.clear();
        self.executed.clear();
        self.settled_conflicts.clear();
        self.want = None;
        self.floor = 0;
        self.batch.floor = Some(0);
        // From view 1, or after the tip if a jump took us into the epoch.
        self.enter(self.tip_view().saturating_add(1));
    }

    /// Checks a finalized content block, proven by `certificate` (its own
    /// or a repeat's), against the one we finalized at its height: two
    /// different ones, each with its finalization, name the validators that
    /// signed both.
    pub(super) fn check_finalized(&mut self, block: &Block, certificate: &Certificate) {
        if certificate.kind != VoteKind::Finalize {
            return;
        }
        let ours = self
            .history
            .iter()
            .find(|(_, finalized)| finalized.block.height == block.height)
            .cloned();
        let Some((digest, ours)) = ours else {
            return;
        };
        let Some(our_certificate) = ours.finalization.as_ref() else {
            return;
        };
        if digest == block.digest(&self.hub) || our_certificate.epoch != certificate.epoch {
            return;
        }
        let signers =
            Signers::from_bits(our_certificate.signers.bits() & certificate.signers.bits());
        self.evidence(Evidence::Finalizations {
            first: ours,
            second: Finalized::new(block.clone(), Some(certificate.clone())),
            signers,
        });
    }
}
