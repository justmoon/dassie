//! Entering views, proposing, and our own votes: what Simplex has an honest
//! validator sign, and nothing else.
//!
//! - **Notarize** at most one block per view, the leader's first valid
//!   proposal, and only while we have not nullified the view.
//! - **Nullify** only the view we are in, when its proposal or notarization
//!   is late, its proposal invalid, or its leader nullified it.
//! - **Finalize** a notarized block only if we did not nullify its view,
//!   did not notarize another block in it, and did not vote to finalize
//!   another block of its height: so that two certificates no honest quorum
//!   signs name only faulty validators.

use alloc::vec::Vec;

use super::super::hooks::{Application, ProposeContext, VerifyContext};
use super::super::io::{Event, Recipient};
use super::super::types::{Block, MAX_PAYLOAD_BYTES, Vote, VoteKind};
use super::super::wire::{Message, Proposal};
use super::chain::{Ancestry, Hold};
use super::round::Proposed;
use super::{Engine, MAX_ROUNDS, Timers};
use crate::digest::Digest;

/// What a proposal turned out to be.
enum Check {
    /// Valid: notarize it.
    Valid,
    /// Not decidable yet: a block or certificate it builds on is missing.
    Missing(u64),
    /// Invalid: nullify the view.
    Invalid,
}

impl Engine {
    /// Enters `view`, and arms its timers.
    pub(super) fn enter(&mut self, view: u64) {
        self.view = view;
        self.batch.view = Some(view);
        let now = self.now_ms;
        self.timers = Timers {
            entered_ms: now,
            ..Timers::default()
        };
        if self.voter(view).is_some() {
            let (leader_ms, advance_ms) = if self.skips(view) {
                (now, now)
            } else {
                (
                    now.saturating_add(self.config.leader_timeout_ms),
                    now.saturating_add(self.config.advance_timeout_ms),
                )
            };
            self.timers.leader_ms = Some(leader_ms);
            self.timers.advance_ms = Some(advance_ms);
            // A view we nullified before a restart or a new set: say so
            // again at once.
            if self
                .rounds
                .get(&view)
                .is_some_and(|round| round.voted_nullify())
            {
                self.timers.retry = Some((now, self.config.advance_timeout_ms));
            }
        }
        if let Some(leader) = self.leader(view) {
            self.event(Event::EnteredView { view, leader });
        }
    }

    /// Whether `view`'s leader has been gone for a while that a quorum was
    /// not: its view is nullified at once (Commonware's skip timeout).
    fn skips(&self, view: u64) -> bool {
        let Some(leader) = self.leader(view) else {
            return false;
        };
        if Some(leader) == self.member() || view <= self.config.skip_views {
            return false;
        }
        let since = view.saturating_sub(self.config.skip_views);
        let active = |index: usize| {
            self.last_voted
                .get(index)
                .is_some_and(|last| *last >= since)
        };
        let set = self.set();
        let quorum_active = (0..set.len()).filter(|index| active(*index)).count() >= set.quorum();
        quorum_active && !active(leader.get())
    }

    /// Enters the view after the highest one at or above ours that is
    /// certified.
    pub(super) fn enter_certified(&mut self) -> bool {
        let highest = self
            .rounds
            .range(self.view..)
            .rev()
            .find(|(_, round)| round.is_certified())
            .map(|(view, _)| *view);
        match highest {
            Some(view) => {
                self.enter(view.saturating_add(1));
                true
            }
            None => false,
        }
    }

    /// Signs and records a vote of ours, to persist before it is sent.
    fn sign(&mut self, kind: VoteKind, view: u64, block: Digest) -> Option<Vote> {
        let me = self.voter(view)?;
        let vote = Vote::sign(&self.hub, &self.key, me, kind, (self.epoch(), view), block);
        let round = self.round_mut(view)?;
        if round.own(kind).is_some() {
            return None;
        }
        round.sign(vote);
        self.batch.votes.push(vote);
        self.take_vote(vote);
        Some(vote)
    }

    /// Nullifies `view`, and starts rebroadcasting it.
    pub(super) fn nullify(&mut self, view: u64) {
        if let Some(vote) = self.sign(VoteKind::Nullify, view, Digest::default()) {
            self.send(Recipient::All, &Message::Vote(vote));
            self.stats.nullifies = self.stats.nullifies.saturating_add(1);
            let interval = self.config.advance_timeout_ms;
            self.timers.retry = Some((self.now_ms.saturating_add(interval), interval));
        }
    }

    /// Proposes a block, if we lead the view and its time has come.
    pub(super) fn propose(&mut self, app: &mut dyn Application) -> bool {
        let view = self.view;
        let Some(me) = self.voter(view) else {
            return false;
        };
        let idle = self
            .rounds
            .get(&view)
            .is_none_or(|round| round.proposal.is_none() && round.own_votes().next().is_none());
        if self.leader(view) != Some(me) || !idle {
            return false;
        }
        // Armed again below only while we wait for the interval: a parent
        // that went missing must not leave a timer that is due for ever.
        self.timers.propose_ms = None;
        let Some((parent_digest, parent)) = self.select_parent(view) else {
            return false;
        };
        // At the end of the epoch, the next set builds the next block; we
        // repeat the last one, until a finalization proves it.
        let repeat = self
            .epochs
            .boundary(self.epoch())
            .is_some_and(|boundary| parent.height.saturating_add(1) >= boundary);
        // An interval after the parent's stamp, but never more than an
        // interval after we entered: our clock may lag the parent's leader.
        let interval = self.config.block_interval_ms;
        let due = parent
            .timestamp_ms
            .saturating_add(interval)
            .min(self.timers.entered_ms.saturating_add(interval));
        if self.now_ms < due {
            self.timers.propose_ms = Some(due);
            return false;
        }
        self.timers.propose_ms = None;
        let timestamp_ms = self.now_ms.max(parent.timestamp_ms.saturating_add(1));
        let block = if repeat {
            Some(self.repeat_of(view, parent_digest, &parent, timestamp_ms))
        } else {
            self.build(app, me, (view, parent_digest, &parent), timestamp_ms)
        };
        match block {
            Some(block) => self.broadcast_proposal(block),
            None => self.nullify(view),
        }
        true
    }

    /// An empty repeat of `parent`, the last block of the epoch, which is
    /// notarized and not final.
    fn repeat_of(
        &self,
        view: u64,
        parent_digest: Digest,
        parent: &Block,
        timestamp_ms: u64,
    ) -> Block {
        let block = Block {
            epoch: self.epoch(),
            view,
            height: parent.height,
            parent: parent_digest,
            parent_view: self.view_in_epoch(parent),
            timestamp_ms,
            payload: Vec::new(),
        };
        assert!(
            self.repeat_fits(&block),
            "a repeat only of an epoch's last block"
        );
        block
    }

    /// A block on `parent` with the application's payload, if it gives one.
    fn build(
        &self,
        app: &mut dyn Application,
        me: crate::validators::ValidatorIndex,
        (view, parent_digest, parent): (u64, Digest, &Block),
        timestamp_ms: u64,
    ) -> Option<Block> {
        let height = parent.height.saturating_add(1);
        let votes = self.votes();
        let context = ProposeContext {
            view,
            proposer: me,
            parent: parent_digest,
            height,
            timestamp_ms,
            votes: &votes,
        };
        let mut payload = Vec::new();
        if app.propose(&context, &mut payload).is_err() || payload.len() > MAX_PAYLOAD_BYTES {
            return None;
        }
        Some(Block {
            epoch: self.epoch(),
            view,
            height,
            parent: parent_digest,
            parent_view: self.view_in_epoch(parent),
            timestamp_ms,
            payload,
        })
    }

    fn broadcast_proposal(&mut self, block: Block) {
        let view = block.view;
        let height = block.height;
        let digest = self.hold_block(block.clone(), Hold::Proposal);
        let now = self.now_ms;
        if let Some(round) = self.round_mut(view) {
            round.proposal = Some(Proposed {
                block: digest,
                arrived_ms: now,
                from: None,
            });
        }
        self.batch.blocks.push(block.clone());
        if let Some(vote) = self.sign(VoteKind::Notarize, view, digest) {
            let proposal = Proposal {
                block,
                signature: vote.signature,
            };
            self.send(Recipient::All, &Message::Proposal(proposal));
            self.stats.proposals = self.stats.proposals.saturating_add(1);
            self.event(Event::Proposed {
                view,
                height,
                block: digest,
            });
        }
    }

    /// The block to build on in `view`: the highest notarized block below
    /// it whose later views are all nullified, or the tip. Notes what is
    /// missing if neither is known.
    fn select_parent(&mut self, view: u64) -> Option<(Digest, Block)> {
        let mut at = view.saturating_sub(1);
        for _ in 0..=MAX_ROUNDS {
            if at <= self.tip_view() {
                return Some((self.tip_digest, self.tip.block.clone()));
            }
            let round = self.rounds.get(&at);
            let certified = round
                .and_then(|round| round.notarization.as_ref().or(round.finalization.as_ref()))
                .map(|certificate| certificate.block);
            if let Some(digest) = certified {
                if let (Some(block), Ancestry::Extends) =
                    (self.blocks.get(&digest), self.ancestry(&digest, at))
                {
                    return Some((digest, block.clone()));
                }
                self.want_from_any(at);
                return None;
            }
            if round.is_none_or(|round| round.nullification.is_none()) {
                self.want_from_any(at);
                return None;
            }
            at = at.saturating_sub(1);
        }
        None
    }

    /// Notarizes the leader's proposal of our view, if it is valid; nullifies
    /// the view if it is not.
    pub(super) fn vote_notarize(&mut self, app: &mut dyn Application) -> bool {
        let view = self.view;
        if self.voter(view).is_none() {
            return false;
        }
        let Some(round) = self.rounds.get(&view) else {
            return false;
        };
        let Some(proposed) = round.proposal else {
            return false;
        };
        if round.own(VoteKind::Notarize).is_some() || round.voted_nullify() {
            return false;
        }
        match self.check_proposal(app, &proposed, round.leader) {
            Check::Valid => {
                if let Some(block) = self.blocks.get(&proposed.block) {
                    self.batch.blocks.push(block.clone());
                }
                assert!(
                    self.blocks
                        .get(&proposed.block)
                        .is_some_and(|block| self.repeat_fits(block)),
                    "we notarize a repeat only of an epoch's last block"
                );
                if let Some(vote) = self.sign(VoteKind::Notarize, view, proposed.block) {
                    self.send(Recipient::All, &Message::Vote(vote));
                }
                true
            }
            Check::Missing(missing) => {
                match proposed.from {
                    Some(from) => self.want(missing, Recipient::Peer(from)),
                    None => self.want_from_any(missing),
                }
                false
            }
            Check::Invalid => {
                self.nullify(view);
                true
            }
        }
    }

    /// Checks a proposal: what it builds on, its height, its stamp, and its
    /// payload.
    fn check_proposal(
        &self,
        app: &mut dyn Application,
        proposed: &Proposed,
        proposer: crate::validators::ValidatorIndex,
    ) -> Check {
        let Some(block) = self.blocks.get(&proposed.block) else {
            // Not held: it could not extend the tip.
            return Check::Invalid;
        };
        let parent = match self.parent_of(block) {
            Ok(parent) => parent,
            Err(check) => return check,
        };
        let precision = self.config.precision_ms;
        let timely = block.timestamp_ms <= proposed.arrived_ms.saturating_add(precision)
            && proposed.arrived_ms
                <= block
                    .timestamp_ms
                    .saturating_add(precision)
                    .saturating_add(self.config.max_delay_ms);
        let boundary = self.epochs.boundary(block.epoch);
        let fits = boundary.is_none_or(|boundary| block.height < boundary);
        // A repeat of the epoch's last block, notarized (by `parent_of`)
        // and not final (or the epoch would have ended); else one height
        // more than its parent.
        let repeat = block.height == parent.height;
        let height = if repeat {
            self.repeat_fits(block)
        } else {
            block.height == parent.height.saturating_add(1)
        };
        if !height || block.timestamp_ms <= parent.timestamp_ms || !timely || !fits {
            return Check::Invalid;
        }
        if repeat {
            return Check::Valid;
        }
        let context = VerifyContext {
            view: block.view,
            proposer,
            parent: block.parent,
            height: block.height,
            timestamp_ms: block.timestamp_ms,
            block: proposed.block,
        };
        if app.verify(&context, &block.payload) {
            Check::Valid
        } else {
            Check::Invalid
        }
    }

    /// The parent of a proposed block, if it may build on it: the tip, or
    /// a notarized block that extends the tip, with every view between
    /// nullified.
    fn parent_of(&self, block: &Block) -> Result<Block, Check> {
        if block.parent_view >= block.view {
            return Err(Check::Invalid);
        }
        let parent = if block.parent == self.tip_digest {
            self.tip.block.clone()
        } else {
            if block.parent_view < self.tip_view() {
                return Err(Check::Invalid);
            }
            let notarized = self.rounds.get(&block.parent_view).is_some_and(|round| {
                [&round.notarization, &round.finalization]
                    .into_iter()
                    .flatten()
                    .any(|certificate| certificate.block == block.parent)
            });
            match (self.blocks.get(&block.parent), notarized) {
                (Some(parent), true) => parent.clone(),
                _ => return Err(Check::Missing(block.parent_view)),
            }
        };
        let parent_view = self.view_in_epoch(&parent);
        if parent_view != block.parent_view {
            return Err(Check::Invalid);
        }
        match self.ancestry(&block.parent, block.parent_view) {
            Ancestry::Extends => {}
            Ancestry::Missing(view) => return Err(Check::Missing(view)),
            Ancestry::Forks => return Err(Check::Invalid),
        }
        let skipped = parent_view.saturating_add(1)..block.view;
        for view in skipped {
            let nullified = self
                .rounds
                .get(&view)
                .is_some_and(|round| round.nullification.is_some());
            if !nullified {
                return Err(Check::Missing(view));
            }
        }
        Ok(parent)
    }

    /// Votes to finalize every notarized block we may.
    pub(super) fn vote_finalize(&mut self) -> bool {
        let candidates: Vec<(u64, Digest)> = self
            .rounds
            .iter()
            .filter(|(view, round)| {
                self.voter(**view).is_some()
                    && !round.voted_nullify()
                    && round.voted_finalize().is_none()
            })
            .filter_map(|(view, round)| {
                let certificate = round.notarization.as_ref()?;
                let notarized = round.voted_notarize();
                notarized
                    .is_none_or(|block| block == certificate.block)
                    .then_some((*view, certificate.block))
            })
            .collect();
        let mut voted = false;
        for (view, digest) in candidates {
            if !self.blocks.contains_key(&digest) {
                continue;
            }
            // A repeat stands for the block it repeats: voting to finalize
            // both is voting for one block at their height.
            let Some((content, height)) = self.content_of(&digest) else {
                continue;
            };
            let free = self
                .finalize_heights
                .get(&height)
                .is_none_or(|held| *held == content);
            if !free || self.ancestry(&digest, view) != Ancestry::Extends {
                continue;
            }
            if let Some(vote) = self.sign(VoteKind::Finalize, view, digest) {
                self.finalize_heights.insert(height, content);
                self.batch.finalize_heights.push((height, content));
                self.send(Recipient::All, &Message::Vote(vote));
                voted = true;
            }
        }
        voted
    }
}
