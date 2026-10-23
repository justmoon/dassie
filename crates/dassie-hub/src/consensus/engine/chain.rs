//! The chain: blocks above the tip, finalizing them in order, and finding
//! finalized blocks that conflict.

use alloc::vec::Vec;

use super::super::hooks::Application;
use super::super::io::{Event, Evidence, Recipient};
use super::super::types::{Block, Certificate, Finalized, VoteKind};
use super::{Engine, MAX_BLOCKS, MAX_VOTE_LOG, RETAINED_BLOCKS};
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

impl Engine {
    /// Holds a block above the tip, if there is room, and returns its
    /// digest.
    pub(super) fn hold_block(&mut self, block: Block) -> Digest {
        let digest = block.digest(&self.hub);
        let room = self.blocks.len() < MAX_BLOCKS || self.blocks.contains_key(&digest);
        if block.height > self.tip.block.height && block.view >= self.floor && room {
            self.blocks.insert(digest, block);
        }
        digest
    }

    /// How the block `digest`, of view `view`, relates to the tip.
    pub(super) fn ancestry(&self, digest: &Digest, view: u64) -> Ancestry {
        let mut at = *digest;
        let mut view = view;
        for _ in 0..=MAX_BLOCKS {
            if at == self.tip_digest {
                return Ancestry::Extends;
            }
            let Some(block) = self.blocks.get(&at) else {
                return Ancestry::Missing(view);
            };
            if block.height <= self.tip.block.height {
                return Ancestry::Forks;
            }
            view = block.parent_view;
            at = block.parent;
        }
        Ancestry::Forks
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
        let set = self.epochs.for_view(self.view);
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
        for (view, digest) in finalized {
            match self.ancestry(&digest, view) {
                Ancestry::Extends => {
                    let certificate = self
                        .rounds
                        .get(&view)
                        .and_then(|round| round.finalization.clone());
                    let chain = self.chain_to(&digest);
                    let last = chain.len().saturating_sub(1);
                    for (at, (digest, block)) in chain.into_iter().enumerate() {
                        let own = if at == last {
                            certificate.clone()
                        } else {
                            None
                        };
                        self.apply_finalized(app, digest, block, own);
                    }
                    return true;
                }
                Ancestry::Missing(missing) => self.want_from_any(missing),
                Ancestry::Forks => {
                    if let Some(round) = self.rounds.get_mut(&view) {
                        round.forked = true;
                    }
                    if let (Some(block), Some(certificate)) = (
                        self.blocks.get(&digest).cloned(),
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

    /// Makes `block` final: tells the application, moves the tip, and lets
    /// go of what is now moot.
    pub(super) fn apply_finalized(
        &mut self,
        app: &mut dyn Application,
        digest: Digest,
        block: Block,
        finalization: Option<Certificate>,
    ) {
        assert!(
            block.parent == self.tip_digest
                && block.height == self.tip.block.height.saturating_add(1)
                && block.view > self.tip.block.view,
            "a finalized block extends the tip"
        );
        app.finalized(&block, &digest);
        self.stats.finalized = self.stats.finalized.saturating_add(1);
        self.event(Event::Finalized {
            view: block.view,
            height: block.height,
            block: digest,
        });
        let (height, view) = (block.height, block.view);
        self.tip = Finalized {
            block,
            finalization,
        };
        self.tip_digest = digest;
        self.batch.tip = Some(self.tip.clone());
        self.history.push_back((digest, self.tip.clone()));
        while self.history.len() > RETAINED_BLOCKS {
            self.history.pop_front();
        }
        self.prune();
        if let Some((from_height, from_view)) = self.epochs.finalized(height, view) {
            self.take_over(from_height, from_view);
        }
        if self.view <= view {
            self.enter(view.saturating_add(1));
        }
    }

    /// Lets go of rounds, blocks and heights at or below the tip, keeping
    /// the votes for blocks it finalized.
    pub(super) fn prune(&mut self) {
        let (view, height) = (self.tip.block.view, self.tip.block.height);
        let done: Vec<u64> = self.rounds.range(..=view).map(|(view, _)| *view).collect();
        for at in done {
            let Some(round) = self.rounds.remove(&at) else {
                continue;
            };
            let final_block = self
                .history
                .iter()
                .find(|(_, finalized)| finalized.block.view == at)
                .map(|(digest, _)| *digest);
            if let Some(final_block) = final_block {
                let votes = round.block_votes().filter(|vote| vote.block == final_block);
                self.vote_log.extend(votes.copied());
            }
        }
        let oldest = self
            .history
            .front()
            .map_or(view, |(_, finalized)| finalized.block.view);
        while self.vote_log.len() > MAX_VOTE_LOG
            || self.vote_log.front().is_some_and(|vote| vote.view < oldest)
        {
            self.vote_log.pop_front();
        }
        self.settled_conflicts.retain(|at| *at >= oldest);
        self.blocks.retain(|_, block| block.height > height);
        self.finalize_heights.retain(|at, _| *at > height);
        if self.want.as_ref().is_some_and(|(want, _)| *want <= view) {
            self.want = None;
        }
    }

    /// A new set runs from `from_view`: views from there were read in the
    /// old set, so their votes are dropped, but for ours.
    pub(super) fn take_over(&mut self, from_height: u64, from_view: u64) {
        self.batch.epochs = Some(self.epochs.to_vec());
        self.event(Event::Reconfigured {
            from_height,
            from_view,
        });
        self.last_voted = [0; crate::validators::MAX_VALIDATORS];
        let views: Vec<u64> = self
            .rounds
            .range(from_view..)
            .map(|(view, _)| *view)
            .collect();
        for view in views {
            let leader = self.leader(view);
            let own = self.member(view);
            if let (Some(round), Some(leader)) = (self.rounds.get_mut(&view), leader) {
                round.reset(leader, own);
            }
        }
        if self.view > from_view {
            self.enter(from_view);
        }
    }

    /// Checks a finalized block against the one we finalized at its
    /// height: two different ones, each with its finalization, name the
    /// validators that signed both.
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
        if digest == certificate.block {
            return;
        }
        let signers =
            Signers::from_bits(our_certificate.signers.bits() & certificate.signers.bits());
        self.evidence(Evidence::Finalizations {
            first: ours,
            second: Finalized {
                block: block.clone(),
                finalization: Some(certificate.clone()),
            },
            signers,
        });
    }
}
