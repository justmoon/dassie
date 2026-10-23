//! Taking messages: proposals, votes and certificates, each checked against
//! the validator set of its epoch before it counts, and copies of what is
//! held dropped before any signature is checked. Those of a later epoch
//! than ours say we are behind: we ask a validator we know for what we
//! lack.

use super::super::hooks::Application;
use super::super::io::{Event, Evidence, PeerId, Recipient};
use super::super::types::{Block, Certificate, Vote, VoteKind, verify_certificate};
use super::super::wire::{Message, Proposal};
use super::chain::Hold;
use super::round::Proposed;
use super::{Engine, MAX_VIEWS_AHEAD, MAX_VOTE_LOG};
use crate::validators::Signers;

impl Engine {
    pub(super) fn receive(&mut self, app: &mut dyn Application, from: PeerId, message: Message) {
        match message {
            Message::Proposal(proposal) => self.on_proposal(from, proposal),
            Message::Vote(vote) => self.on_vote(vote),
            Message::Certificate(certificate) => {
                self.on_certificate(certificate, None);
            }
            Message::Fetch(fetch) => self.on_fetch(from, fetch),
            Message::Delivery(delivery) => self.on_delivery(app, delivery),
        }
    }

    pub(super) fn refuse(&mut self) {
        self.stats.refused = self.stats.refused.saturating_add(1);
    }

    /// Whether votes and proposals of `view` are taken: above the tip and
    /// the floor, and not too far ahead.
    fn admits(&self, view: u64) -> bool {
        self.holds_view(view) && view <= self.view.saturating_add(MAX_VIEWS_AHEAD)
    }

    /// Whether a message of `epoch` is of ours; one of a later epoch makes
    /// us ask for what we lack.
    fn of_our_epoch(&mut self, epoch: u64) -> bool {
        let ours = self.epoch();
        if epoch > ours {
            let view = self.view;
            self.want_from_any(view);
        }
        epoch == ours
    }

    fn on_proposal(&mut self, from: PeerId, proposal: Proposal) {
        if !self.of_our_epoch(proposal.block.epoch) {
            return self.refuse();
        }
        let view = proposal.block.view;
        let Some(leader) = self.leader(view) else {
            return self.refuse();
        };
        if !self.admits(view) {
            return self.refuse();
        }
        let digest = proposal.block.digest(&self.hub);
        let vote = Vote {
            kind: VoteKind::Notarize,
            epoch: proposal.block.epoch,
            view,
            block: digest,
            signer: leader,
            signature: proposal.signature,
        };
        let held = self.rounds.get(&view);
        if held.is_some_and(|round| round.proposal.is_some_and(|p| p.block == digest)) {
            return;
        }
        if vote.verify(&self.hub, self.set()).is_err() {
            return self.refuse();
        }
        self.hold_block(proposal.block, Hold::Proposal);
        self.take_vote(vote);
        let now = self.now_ms;
        if let Some(round) = self.round_mut(view)
            && round.proposal.is_none()
        {
            round.proposal = Some(Proposed {
                block: digest,
                arrived_ms: now,
                from: Some(from),
            });
        }
    }

    fn on_vote(&mut self, vote: Vote) {
        if vote.epoch < self.epoch() {
            return self.log_late_vote(vote);
        }
        if !self.of_our_epoch(vote.epoch) {
            return self.refuse();
        }
        if !self.holds_view(vote.view) {
            return self.log_late_vote(vote);
        }
        if !self.admits(vote.view) {
            return self.refuse();
        }
        if self
            .rounds
            .get(&vote.view)
            .is_some_and(|round| round.holds(&vote))
        {
            return;
        }
        if vote.verify(&self.hub, self.set()).is_err() {
            return self.refuse();
        }
        self.take_vote(vote);
    }

    /// Keeps a late vote for a finalized block we still hold, so a block of
    /// ours can record it.
    fn log_late_vote(&mut self, vote: Vote) {
        let finalized = self.history.iter().any(|(digest, finalized)| {
            finalized.block.epoch == vote.epoch
                && finalized.block.view == vote.view
                && *digest == vote.block
        });
        let known = self.vote_log.iter().any(|held| {
            held.epoch == vote.epoch
                && held.view == vote.view
                && held.signer == vote.signer
                && held.kind == vote.kind
        });
        if !finalized || known || vote.kind == VoteKind::Nullify {
            return;
        }
        let Some(set) = self.epochs.for_epoch(vote.epoch) else {
            return;
        };
        if vote.verify(&self.hub, set).is_err() {
            return self.refuse();
        }
        if self.vote_log.len() >= MAX_VOTE_LOG {
            self.vote_log.pop_front();
        }
        self.vote_log.push_back(vote);
    }

    /// Takes a verified vote, and holds the certificate it completes.
    pub(super) fn take_vote(&mut self, vote: Vote) {
        if let Some(certificate) = self.count_vote(vote) {
            self.store_certificate(certificate, true);
        }
    }

    /// Counts a verified vote, and reports a conflict with another of its
    /// signer's. Returns the certificate it completes, if any.
    fn count_vote(&mut self, vote: Vote) -> Option<Certificate> {
        assert!(vote.epoch == self.epoch(), "votes counted are of our epoch");
        let quorum = self.set().quorum();
        let leader = self.leader(vote.view);
        let me = self.member();
        let round = self.round_mut(vote.view)?;
        let taken = round.take(vote);
        let mut evidence = None;
        if let Some(first) = taken.conflict
            && !round.reported.contains(vote.signer)
        {
            round.reported = round.reported.with(vote.signer);
            evidence = Some(Evidence::Votes {
                first,
                second: vote,
            });
        }
        let formed = if taken.new && round.certificate(vote.kind).is_none() {
            round.assemble(vote.kind, (vote.epoch, vote.view), &vote.block, quorum)
        } else {
            None
        };
        if let Some(evidence) = evidence {
            self.evidence(evidence);
        }
        if !taken.new {
            return None;
        }
        self.stats.votes = self.stats.votes.saturating_add(1);
        self.note_voter(vote.signer, vote.view);
        // The leader gave up its own view: so do we, at once.
        let gave_up = vote.kind == VoteKind::Nullify && Some(vote.signer) == leader;
        if gave_up && vote.view == self.view && me != leader {
            self.timers.leader_ms = Some(self.now_ms);
            self.timers.advance_ms = Some(self.now_ms);
        }
        formed
    }

    /// Takes a certificate, with its block if one came with it.
    pub(super) fn on_certificate(&mut self, certificate: Certificate, block: Option<Block>) {
        let view = certificate.view;
        if certificate.epoch < self.epoch() {
            return self.check_settled(&certificate, block.as_ref());
        }
        if !self.of_our_epoch(certificate.epoch) {
            return self.refuse();
        }
        if !self.holds_view(view) {
            if view > self.tip_view() && self.take_for_chain(&certificate, block.as_ref()) {
                return;
            }
            return self.check_settled(&certificate, block.as_ref());
        }
        let held = self
            .rounds
            .get(&view)
            .and_then(|round| round.certificate(certificate.kind))
            .is_some_and(|held| held.block == certificate.block);
        if !held && verify_certificate(&self.hub, self.set(), &certificate).is_err() {
            return self.refuse();
        }
        if let Some(block) = block
            && block.view == view
            && block.epoch == certificate.epoch
            && block.digest(&self.hub) == certificate.block
        {
            self.hold_block(block, Hold::Certified);
        }
        if !held {
            self.store_certificate(certificate, false);
        }
    }

    /// Holds a verified certificate: reports what it conflicts with, passes
    /// it on once, and notes the block it needs.
    pub(super) fn store_certificate(&mut self, certificate: Certificate, formed: bool) {
        let view = certificate.view;
        let Some(round) = self.round_mut(view) else {
            return;
        };
        let conflicts: alloc::vec::Vec<Evidence> = [
            &round.notarization,
            &round.finalization,
            &round.nullification,
        ]
        .into_iter()
        .flatten()
        .filter(|held| held.conflicts_with(&certificate))
        .map(|held| Evidence::Certificates {
            first: held.clone(),
            second: certificate.clone(),
            signers: Signers::from_bits(held.signers.bits() & certificate.signers.bits()),
        })
        .collect();
        if !round.hold(certificate.clone()) {
            for evidence in conflicts {
                self.evidence(evidence);
            }
            return;
        }
        if !formed {
            // Its votes count as votes: a signer's conflicting vote shows,
            // and the certificate of their kind is held already.
            for vote in certificate.votes() {
                let _ = self.count_vote(vote);
            }
        }
        self.stats.certificates = self.stats.certificates.saturating_add(1);
        self.batch.certificates.push(certificate.clone());
        self.send(Recipient::All, &Message::Certificate(certificate.clone()));
        match certificate.kind {
            VoteKind::Notarize => self.event(Event::Notarized {
                view,
                block: certificate.block,
            }),
            VoteKind::Nullify => self.event(Event::Nullified { view }),
            VoteKind::Finalize => {}
        }
        if certificate.kind != VoteKind::Nullify && !self.blocks.contains_key(&certificate.block) {
            self.want_from_any(view);
        }
        for evidence in conflicts {
            self.evidence(evidence);
        }
    }

    /// A certificate of a view at or below the tip: it conflicts with the
    /// finalization of a block we finalized, if it is of the same view and
    /// names another block or nullifies the view; or, with its block, if it
    /// finalizes another block at a height we finalized (a repeat of ours
    /// is ours).
    fn check_settled(&mut self, certificate: &Certificate, block: Option<&Block>) {
        let Some(set) = self.epochs.for_epoch(certificate.epoch).cloned() else {
            return;
        };
        let ours_at_height = |block: &Block| {
            self.history
                .iter()
                .find(|(_, finalized)| finalized.block.height == block.height)
                .map(|(digest, _)| *digest)
        };
        if let (Some(block), VoteKind::Finalize) = (block, certificate.kind)
            && ours_at_height(block)
                .is_some_and(|ours| ours != certificate.block && ours != block.parent)
            && block.digest(&self.hub) == certificate.block
            && block.epoch == certificate.epoch
            && verify_certificate(&self.hub, &set, certificate).is_ok()
        {
            self.check_finalized(block, certificate);
        }
        let ours = self
            .history
            .iter()
            .filter_map(|(_, finalized)| finalized.finalization.clone())
            .find(|ours| ours.epoch == certificate.epoch && ours.view == certificate.view);
        let Some(ours) = ours else {
            return;
        };
        let at = (certificate.epoch, certificate.view);
        if !ours.conflicts_with(certificate) || self.settled_conflicts.contains(&at) {
            return;
        }
        if verify_certificate(&self.hub, &set, certificate).is_err() {
            return self.refuse();
        }
        self.settled_conflicts.insert(at);
        let signers = Signers::from_bits(ours.signers.bits() & certificate.signers.bits());
        self.evidence(Evidence::Certificates {
            first: ours,
            second: certificate.clone(),
            signers,
        });
    }
}
