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
use super::boundary::OnChain;
use super::chain::{Ancestry, Hold};
use super::round::Proposed;
use super::{Engine, MAX_ROUNDS, Timers};
use crate::digest::Digest;

/// What a block builds on: a block that is not a repeat, through the view
/// that last notarized it (or a repeat of it), or the tip.
#[derive(Clone, Debug)]
pub(super) struct Parent {
    /// The view it is built on through.
    pub(super) view: u64,
    /// Its digest.
    pub(super) digest: Digest,
    /// The block.
    pub(super) block: Block,
    /// The stamp of the block notarized in `view`: the block's, or its
    /// repeat's. The next block comes an interval after it.
    pub(super) stamp: u64,
}

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
                let factor = self.backoff_at(view);
                (
                    now.saturating_add(self.config.leader_timeout_ms.saturating_mul(factor)),
                    now.saturating_add(self.config.advance_timeout_ms.saturating_mul(factor)),
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

    /// How many times longer than usual `view` waits: twice for each view
    /// we hold nullified since the last notarized one, past
    /// `backoff_after_views`. Only views held count: they are what the
    /// engine must keep, and a validator that skipped views by a
    /// certificate, as one that catches up, holds none of them and waits
    /// as usual.
    pub(super) fn backoff_at(&self, view: u64) -> u64 {
        let notarized = self
            .rounds
            .range(..view)
            .rev()
            .find(|(_, round)| round.notarization.is_some() || round.finalization.is_some())
            .map_or(self.tip_view(), |(at, _)| *at);
        let failed = if notarized < view {
            self.rounds
                .range(notarized.saturating_add(1)..view)
                .filter(|(_, round)| round.nullification.is_some())
                .count() as u64
        } else {
            0
        };
        let doublings = failed
            .saturating_sub(self.config.backoff_after_views)
            .min(40);
        1u64 << doublings
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

    /// Nullifies `view`, and starts rebroadcasting it. Returns whether it
    /// signed: not if it did before, or holds no round for the view.
    pub(super) fn nullify(&mut self, view: u64) -> bool {
        let Some(vote) = self.sign(VoteKind::Nullify, view, Digest::default()) else {
            return false;
        };
        self.send(Recipient::All, &Message::Vote(vote));
        self.stats.nullifies = self.stats.nullifies.saturating_add(1);
        let interval = self.config.advance_timeout_ms;
        self.timers.retry = Some((self.now_ms.saturating_add(interval), interval));
        true
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
        // Without room for the view's round, nothing we propose is held.
        let room = self.rounds.contains_key(&view) || self.rounds.len() < MAX_ROUNDS;
        if self.leader(view) != Some(me) || !idle || !room {
            return false;
        }
        // Armed again below only while we wait for the interval: a parent
        // that went missing must not leave a timer that is due for ever.
        self.timers.propose_ms = None;
        let Some(parent) = self.select_parent(view) else {
            return false;
        };
        // At the end of the epoch on this chain, the next set builds the
        // next block; and past the blocks that may wait to be final, the
        // next one waits for them. Either way we repeat the last one, until
        // a finalization proves it.
        let boundary = match self.boundary_on(app, &parent.digest, view) {
            OnChain::Boundary(boundary) => boundary,
            OnChain::Missing(missing) => {
                self.want_from_any(missing);
                return false;
            }
            OnChain::Invalid => return false,
        };
        let repeat = boundary
            .is_some_and(|boundary| parent.block.height.saturating_add(1) >= boundary)
            || parent.block.height >= self.height_cap();
        // An interval after the parent's stamp, but never more than an
        // interval after we entered: our clock may lag the parent's leader.
        let interval = self.config.block_interval_ms;
        let due = parent
            .stamp
            .saturating_add(interval)
            .min(self.timers.entered_ms.saturating_add(interval));
        if self.now_ms < due {
            self.timers.propose_ms = Some(due);
            return false;
        }
        self.timers.propose_ms = None;
        let timestamp_ms = self.now_ms.max(parent.stamp.saturating_add(1));
        let block = if repeat {
            Some(self.repeat_of(view, &parent, timestamp_ms))
        } else {
            self.build(app, me, (view, &parent), timestamp_ms)
        };
        match block {
            Some(block) => {
                self.broadcast_proposal(block);
                true
            }
            None => self.nullify(view),
        }
    }

    /// An empty repeat of `parent`, notarized and not final: the last block
    /// of the epoch, or the last that may wait to be final. It builds on the
    /// block itself, through the view that last notarized it.
    fn repeat_of(&self, view: u64, parent: &Parent, timestamp_ms: u64) -> Block {
        let block = Block {
            epoch: self.epoch(),
            view,
            height: parent.block.height,
            parent: parent.digest,
            parent_view: parent.view,
            timestamp_ms,
            payload: Vec::new(),
        };
        assert!(
            self.repeat_shaped(&block),
            "a repeat has its parent's height and no payload"
        );
        block
    }

    /// A block on `parent` with the application's payload, if it gives one.
    fn build(
        &self,
        app: &mut dyn Application,
        me: crate::validators::ValidatorIndex,
        (view, parent): (u64, &Parent),
        timestamp_ms: u64,
    ) -> Option<Block> {
        let height = parent.block.height.saturating_add(1);
        let votes = self.votes();
        let context = ProposeContext {
            view,
            proposer: me,
            parent: parent.digest,
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
            parent: parent.digest,
            parent_view: parent.view,
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

    /// The block to build on in `view`: the block the highest notarized
    /// view below it stands for (itself, or the block a repeat repeats),
    /// whose later views are all nullified, or the tip. Notes what is
    /// missing if neither is known.
    fn select_parent(&mut self, view: u64) -> Option<Parent> {
        let mut at = view.saturating_sub(1);
        for _ in 0..=MAX_ROUNDS {
            if at <= self.tip_view() {
                return Some(Parent {
                    view: self.tip_view(),
                    digest: self.tip_digest,
                    block: self.tip.block.clone(),
                    stamp: self.tip.block.timestamp_ms,
                });
            }
            let round = self.rounds.get(&at);
            let certified = round
                .and_then(|round| round.notarization.as_ref().or(round.finalization.as_ref()))
                .map(|certificate| certificate.block);
            if let Some(digest) = certified {
                let content = self.content_of(&digest);
                let stamp = self.known_block(&digest).map(|block| block.timestamp_ms);
                if let Some((content, _)) = content
                    && self.ancestry(&content, at) == Ancestry::Extends
                    && let (Some(block), Some(stamp)) = (self.known_block(&content), stamp)
                {
                    return Some(Parent {
                        view: at,
                        digest: content,
                        block: block.clone(),
                        stamp,
                    });
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
        let (Some(proposed), leader) = (round.proposal, round.leader) else {
            return false;
        };
        if round.own(VoteKind::Notarize).is_some() || round.voted_nullify() {
            return false;
        }
        match self.check_proposal(app, &proposed, leader) {
            Check::Valid => {
                if let Some(block) = self.blocks.get(&proposed.block) {
                    self.batch.blocks.push(block.clone());
                }
                assert!(
                    self.blocks
                        .get(&proposed.block)
                        .is_some_and(|block| self.repeat_shaped(block)),
                    "a repeat we notarize has its parent's height and no payload"
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
            Check::Invalid => self.nullify(view),
        }
    }

    /// Checks a proposal: what it builds on, its height, its stamp, and its
    /// payload.
    fn check_proposal(
        &mut self,
        app: &mut dyn Application,
        proposed: &Proposed,
        proposer: crate::validators::ValidatorIndex,
    ) -> Check {
        let Some(block) = self.blocks.get(&proposed.block).cloned() else {
            // Not held: it could not extend the tip.
            return Check::Invalid;
        };
        let parent = match self.parent_of(&block) {
            Ok(parent) => parent,
            Err(check) => return check,
        };
        // Where the epoch ends on the chain it builds on, as the state after
        // its parent says, final or not.
        let boundary = match self.boundary_on(app, &block.parent, block.parent_view) {
            OnChain::Boundary(boundary) => boundary,
            OnChain::Missing(missing) => return Check::Missing(missing),
            OnChain::Invalid => return Check::Invalid,
        };
        let precision = self.config.precision_ms;
        let timely = block.timestamp_ms <= proposed.arrived_ms.saturating_add(precision)
            && proposed.arrived_ms
                <= block
                    .timestamp_ms
                    .saturating_add(precision)
                    .saturating_add(self.config.max_delay_ms);
        if block.height > self.height_cap() {
            // Above what may wait to be final here: others finalized more
            // than we did. We ask for it.
            return Check::Missing(block.view);
        }
        let fits = boundary.is_none_or(|boundary| block.height < boundary);
        // A repeat of a notarized block that is not final (by `parent_of`):
        // the epoch's last on this chain, or the last that may wait to be
        // final; else one height more than its parent.
        let repeat = block.height == parent.height;
        let height = if repeat {
            let last = boundary.is_some_and(|b| parent.height.saturating_add(1) == b)
                || parent.height >= self.height_cap();
            self.repeat_shaped(&block) && last
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
            self.executed.insert(proposed.block);
            Check::Valid
        } else {
            Check::Invalid
        }
    }

    /// The parent of a proposed block, if it may build on it: the tip, or
    /// a notarized block that extends the tip, never a repeat. It is built
    /// on through `parent_view`: the parent's own view, or a view that
    /// notarized a repeat of it. Every view after that is nullified.
    fn parent_of(&self, block: &Block) -> Result<Block, Check> {
        // Skipping views below the floor, it skips the highest notarized
        // view held, and the views it would need are let go of: we never
        // notarize it.
        let below = block.parent_view.saturating_add(1) < self.floor;
        if block.parent_view >= block.view || block.parent_view < self.tip_view() || below {
            return Err(Check::Invalid);
        }
        let Some(parent) = self.known_block(&block.parent).cloned() else {
            return Err(Check::Missing(self.ask_below(block.parent_view)));
        };
        if self
            .content_of(&block.parent)
            .is_none_or(|(content, _)| content != block.parent)
        {
            // A repeat is never a parent: blocks build on what it repeats.
            return Err(Check::Invalid);
        }
        if !self.carries(block.parent_view, &block.parent, &parent)? {
            return Err(Check::Invalid);
        }
        match self.ancestry(&block.parent, block.parent_view) {
            Ancestry::Extends => {}
            Ancestry::Missing(view) => return Err(Check::Missing(view)),
            Ancestry::Forks => return Err(Check::Invalid),
        }
        let skipped = block.parent_view.saturating_add(1)..block.view;
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

    /// Whether view `view` stands for `parent` (`digest`): the tip in its
    /// own view, or a view that notarized it or a repeat of it.
    fn carries(&self, view: u64, digest: &Digest, parent: &Block) -> Result<bool, Check> {
        if *digest == self.tip_digest && view == self.tip_view() {
            return Ok(true);
        }
        let Some(round) = self.rounds.get(&view) else {
            return Err(Check::Missing(view));
        };
        let certified = [&round.notarization, &round.finalization]
            .into_iter()
            .flatten()
            .map(|certificate| certificate.block)
            .next();
        let Some(certified) = certified else {
            return Err(Check::Missing(view));
        };
        if certified == *digest {
            return Ok(true);
        }
        match self.blocks.get(&certified) {
            Some(repeat) => Ok(repeat.parent == *digest && repeat.repeats(parent)),
            None => Err(Check::Missing(view)),
        }
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
