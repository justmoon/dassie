//! One view: its leader's proposal, the votes taken, the certificates held,
//! and our own votes in it.

use alloc::vec::Vec;

use super::super::types::{Certificate, Vote, VoteKind};
use crate::digest::Digest;
use crate::validators::{MAX_VALIDATORS, Signers, ValidatorIndex};

/// The proposal a view's leader made, as first seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proposed {
    /// The block's digest.
    pub block: Digest,
    /// When it arrived, by our clock: its stamp is judged against this.
    pub arrived_ms: u64,
}

/// What taking a vote did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Taken {
    /// Whether it was new, and counts.
    pub new: bool,
    /// A vote of the same signer that it conflicts with.
    pub conflict: Option<Vote>,
}

/// A view.
#[derive(Clone, Debug)]
pub struct Round {
    /// Its leader.
    pub leader: ValidatorIndex,
    /// The leader's proposal, the first seen.
    pub proposal: Option<Proposed>,
    /// Votes by signer, of each kind: the first seen of each.
    notarize: [Option<Vote>; MAX_VALIDATORS],
    finalize: [Option<Vote>; MAX_VALIDATORS],
    nullify: [Option<Vote>; MAX_VALIDATORS],
    /// Its notarization, the first held.
    pub notarization: Option<Certificate>,
    /// Its finalization, the first held.
    pub finalization: Option<Certificate>,
    /// Its nullification.
    pub nullification: Option<Certificate>,
    /// Our own votes in it, by kind: we sign at most one of each.
    own: [Option<Vote>; 3],
    /// Validators whose conflicting votes in it were reported, so each is
    /// reported once.
    pub reported: Signers,
    /// Whether its finalization was found on a fork of our chain.
    pub forked: bool,
}

impl Round {
    /// A view led by `leader`, with nothing in it yet.
    #[must_use]
    pub const fn new(leader: ValidatorIndex) -> Self {
        Self {
            leader,
            proposal: None,
            notarize: [None; MAX_VALIDATORS],
            finalize: [None; MAX_VALIDATORS],
            nullify: [None; MAX_VALIDATORS],
            notarization: None,
            finalization: None,
            nullification: None,
            own: [None; 3],
            reported: Signers::NONE,
            forked: false,
        }
    }

    const fn slots(&self, kind: VoteKind) -> &[Option<Vote>; MAX_VALIDATORS] {
        match kind {
            VoteKind::Notarize => &self.notarize,
            VoteKind::Finalize => &self.finalize,
            VoteKind::Nullify => &self.nullify,
        }
    }

    const fn slots_mut(&mut self, kind: VoteKind) -> &mut [Option<Vote>; MAX_VALIDATORS] {
        match kind {
            VoteKind::Notarize => &mut self.notarize,
            VoteKind::Finalize => &mut self.finalize,
            VoteKind::Nullify => &mut self.nullify,
        }
    }

    /// The vote of `signer` of `kind`, if one was taken.
    #[must_use]
    pub fn vote(&self, kind: VoteKind, signer: ValidatorIndex) -> Option<&Vote> {
        self.slots(kind).get(signer.get()).and_then(Option::as_ref)
    }

    /// Whether exactly this vote is held already: a copy needs no check.
    #[must_use]
    pub fn holds(&self, vote: &Vote) -> bool {
        self.vote(vote.kind, vote.signer) == Some(vote)
    }

    /// Takes a verified vote: the first of each kind per signer counts. A
    /// second of the same kind for another block does not; a vote of
    /// another kind that conflicts still counts for its own kind.
    pub fn take(&mut self, vote: Vote) -> Taken {
        let kinds = [VoteKind::Notarize, VoteKind::Finalize, VoteKind::Nullify];
        let conflict = kinds
            .iter()
            .filter_map(|kind| self.vote(*kind, vote.signer))
            .find(|held| held.conflicts_with(&vote))
            .copied();
        let Some(slot) = self.slots_mut(vote.kind).get_mut(vote.signer.get()) else {
            return Taken {
                new: false,
                conflict: None,
            };
        };
        let new = slot.is_none();
        if new {
            *slot = Some(vote);
        }
        Taken { new, conflict }
    }

    /// The certificate of `kind` for `block`, if a quorum `quorum` of votes
    /// is held for it.
    #[must_use]
    pub fn assemble(
        &self,
        kind: VoteKind,
        (epoch, view): (u64, u64),
        block: &Digest,
        quorum: usize,
    ) -> Option<Certificate> {
        let votes: Vec<&Vote> = self
            .slots(kind)
            .iter()
            .flatten()
            .filter(|vote| vote.block == *block)
            .collect();
        if votes.len() < quorum {
            return None;
        }
        Some(Certificate {
            kind,
            epoch,
            view,
            block: *block,
            signers: votes
                .iter()
                .fold(Signers::NONE, |signers, vote| signers.with(vote.signer)),
            signatures: votes.iter().map(|vote| vote.signature).collect(),
        })
    }

    /// The certificate of `kind` held, if any.
    #[must_use]
    pub const fn certificate(&self, kind: VoteKind) -> Option<&Certificate> {
        match kind {
            VoteKind::Notarize => self.notarization.as_ref(),
            VoteKind::Finalize => self.finalization.as_ref(),
            VoteKind::Nullify => self.nullification.as_ref(),
        }
    }

    /// Holds `certificate`, unless one of its kind is held. Returns whether
    /// it was taken.
    pub fn hold(&mut self, certificate: Certificate) -> bool {
        let slot = match certificate.kind {
            VoteKind::Notarize => &mut self.notarization,
            VoteKind::Finalize => &mut self.finalization,
            VoteKind::Nullify => &mut self.nullification,
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(certificate);
        true
    }

    /// Whether the view is over for us: notarized, finalized or nullified.
    #[must_use]
    pub const fn is_certified(&self) -> bool {
        self.notarization.is_some() || self.finalization.is_some() || self.nullification.is_some()
    }

    /// The votes for blocks taken in it, by signer and kind.
    pub fn block_votes(&self) -> impl Iterator<Item = &Vote> {
        self.notarize.iter().chain(self.finalize.iter()).flatten()
    }

    /// Our own vote of `kind`, if we signed one.
    #[must_use]
    pub fn own(&self, kind: VoteKind) -> Option<&Vote> {
        self.own
            .get(usize::from(kind.code()))
            .and_then(Option::as_ref)
    }

    /// Records a vote we signed.
    pub fn sign(&mut self, vote: Vote) {
        assert!(
            self.own(vote.kind).is_none(),
            "one vote of each kind per view"
        );
        if let Some(slot) = self.own.get_mut(usize::from(vote.kind.code())) {
            *slot = Some(vote);
        }
    }

    /// The block we voted to notarize, if any.
    #[must_use]
    pub fn voted_notarize(&self) -> Option<Digest> {
        self.own(VoteKind::Notarize).map(|vote| vote.block)
    }

    /// The block we voted to finalize, if any.
    #[must_use]
    pub fn voted_finalize(&self) -> Option<Digest> {
        self.own(VoteKind::Finalize).map(|vote| vote.block)
    }

    /// Whether we voted to nullify.
    #[must_use]
    pub fn voted_nullify(&self) -> bool {
        self.own(VoteKind::Nullify).is_some()
    }

    /// Our own votes.
    pub fn own_votes(&self) -> impl Iterator<Item = &Vote> {
        self.own.iter().flatten()
    }
}
