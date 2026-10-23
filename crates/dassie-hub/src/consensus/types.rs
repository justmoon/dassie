//! Blocks, votes and certificates (spec/designs/settlement-hubs.md#10-consensus):
//! what validators sign, and how anyone checks it.
//!
//! A vote signs a [`Digest`] under [`Domain::Vote`] that binds the hub, the
//! epoch, the kind of vote, the view and the block, so a vote can never be
//! taken for one of another hub, epoch, kind or view. A certificate is q
//! votes of one kind for one view and block, with the bitmap of who signed.
//!
//! An epoch is the run of one validator set; each numbers its views from 1
//! (spec/decisions/epoch-handover.md).

use alloc::vec::Vec;

use dassie_identity::{NodeKey, Signature};
use dassie_oer::Encode;

use crate::digest::{Digest, Domain};
use crate::validators::{Signers, ValidatorIndex, ValidatorSet};

/// The largest payload a block carries. The application's operations fit
/// well within it; it bounds what a leader can make every validator hold.
pub const MAX_PAYLOAD_BYTES: usize = 512 * 1024;

/// A block of the hub's log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The epoch it was proposed in: which validator set runs its view.
    pub epoch: u64,
    /// The view it was proposed in, within its epoch.
    pub view: u64,
    /// Its place in the chain: its parent's height plus one. The anchor of
    /// a hub, its genesis, is height 0. A repeat block alone has its
    /// parent's height ([`Block::repeats`]).
    pub height: u64,
    /// Its parent's digest.
    pub parent: Digest,
    /// The view of its parent in this block's epoch: every view between
    /// the two must have been nullified. The first block of an epoch builds
    /// on the last block of the one before, at view 0.
    pub parent_view: u64,
    /// When its leader stamped it, by the leader's clock
    /// (spec/designs/settlement-hubs.md#3-the-ledger, "Time").
    pub timestamp_ms: u64,
    /// The application's content, opaque to consensus.
    pub payload: Vec<u8>,
}

impl Block {
    /// The block's digest in hub `hub`: SHA-256 under [`Domain::Block`]
    /// over the hub ID and the block's canonical encoding.
    #[must_use]
    pub fn digest(&self, hub: &Digest) -> Digest {
        Digest::of(Domain::Block, &[hub.as_bytes(), &self.to_vec()])
    }

    /// Whether this block repeats `parent`, the block it builds on: the
    /// same height and no payload. Only a repeat of an epoch's last block
    /// is valid, by that epoch's set, while that block is notarized and not
    /// final: finalizing the repeat finalizes the block, when the votes for
    /// the block itself split so that it can be neither finalized nor
    /// skipped (spec/decisions/epoch-handover.md). The application never
    /// sees a repeat.
    #[must_use]
    pub fn repeats(&self, parent: &Self) -> bool {
        self.height == parent.height && self.payload.is_empty()
    }
}

/// What a vote says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VoteKind {
    /// The block is a valid proposal of its view.
    Notarize,
    /// The block's notarization was seen in time: once q say so, the block
    /// is final.
    Finalize,
    /// The view should be skipped: no block of it is final.
    Nullify,
}

impl VoteKind {
    /// Its code on the wire and in the signed digest.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Notarize => 0,
            Self::Finalize => 1,
            Self::Nullify => 2,
        }
    }

    /// The kind with code `code`.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Notarize),
            1 => Some(Self::Finalize),
            2 => Some(Self::Nullify),
            _ => None,
        }
    }
}

/// The digest a vote signs: the hub, the epoch, the kind, the view and the
/// block (all zero for a nullify).
#[must_use]
pub fn vote_digest(hub: &Digest, epoch: u64, kind: VoteKind, view: u64, block: &Digest) -> Digest {
    Digest::of(
        Domain::Vote,
        &[
            hub.as_bytes(),
            &epoch.to_be_bytes(),
            &[kind.code()],
            &view.to_be_bytes(),
            block.as_bytes(),
        ],
    )
}

/// A validator's signed vote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vote {
    /// What it says.
    pub kind: VoteKind,
    /// The epoch: which validator set it is a vote of.
    pub epoch: u64,
    /// The view.
    pub view: u64,
    /// The block; [`Digest::default`] for a nullify.
    pub block: Digest,
    /// Who signed, in the validator set of the view.
    pub signer: ValidatorIndex,
    /// The signature over [`vote_digest`].
    pub signature: Signature,
}

/// Why a vote or certificate does not verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// The signer is not in the set.
    UnknownSigner,
    /// A signature does not verify.
    BadSignature(ValidatorIndex),
    /// A nullify names a block.
    NullifyWithBlock,
    /// Fewer signers than the quorum.
    TooFewSigners,
    /// A different number of signatures than signers.
    SignatureCount,
    /// The certificate is not a finalization of the block it comes with.
    Unproven,
    /// The block is not above the last finalized one.
    Stale,
}

impl Vote {
    /// Signs a vote with `key` as validator `signer` of epoch `epoch`.
    #[must_use]
    pub fn sign(
        hub: &Digest,
        key: &NodeKey,
        signer: ValidatorIndex,
        kind: VoteKind,
        (epoch, view): (u64, u64),
        block: Digest,
    ) -> Self {
        let block = if kind == VoteKind::Nullify {
            Digest::default()
        } else {
            block
        };
        let digest = vote_digest(hub, epoch, kind, view, &block);
        Self {
            kind,
            epoch,
            view,
            block,
            signer,
            signature: key.sign(digest.as_bytes()),
        }
    }

    /// Checks the vote against the validator set of its epoch.
    ///
    /// # Errors
    ///
    /// Why it does not verify.
    pub fn verify(&self, hub: &Digest, set: &ValidatorSet) -> Result<(), Invalid> {
        if self.kind == VoteKind::Nullify && self.block != Digest::default() {
            return Err(Invalid::NullifyWithBlock);
        }
        let key = set.key(self.signer).ok_or(Invalid::UnknownSigner)?;
        let digest = vote_digest(hub, self.epoch, self.kind, self.view, &self.block);
        key.verify(digest.as_bytes(), &self.signature)
            .map_err(|_| Invalid::BadSignature(self.signer))
    }

    /// Whether one validator signing both `self` and `other` proves it
    /// faulty: no honest validator signs two different blocks in one view
    /// with the same kind, nullifies and finalizes one view, or finalizes a
    /// block other than the one it notarized
    /// (spec/designs/settlement-hubs.md#10-consensus, "Evidence").
    #[must_use]
    pub fn conflicts_with(&self, other: &Self) -> bool {
        if self.signer != other.signer || self.epoch != other.epoch || self.view != other.view {
            return false;
        }
        match (self.kind, other.kind) {
            (VoteKind::Nullify, VoteKind::Nullify)
            | (VoteKind::Nullify, VoteKind::Notarize)
            | (VoteKind::Notarize, VoteKind::Nullify) => false,
            (VoteKind::Nullify, VoteKind::Finalize) | (VoteKind::Finalize, VoteKind::Nullify) => {
                true
            }
            _ => self.block != other.block,
        }
    }
}

/// q votes of one kind for one view and block: a notarization, a
/// finalization or a nullification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// The kind of the votes.
    pub kind: VoteKind,
    /// The epoch: which validator set signed it.
    pub epoch: u64,
    /// The view.
    pub view: u64,
    /// The block; [`Digest::default`] for a nullification.
    pub block: Digest,
    /// Who signed, in the validator set of the view.
    pub signers: Signers,
    /// Their signatures, in the order of their indexes.
    pub signatures: Vec<Signature>,
}

impl Certificate {
    /// The votes it is made of, in the order of their signers.
    pub fn votes(&self) -> impl Iterator<Item = Vote> + '_ {
        self.signers
            .iter()
            .zip(self.signatures.iter())
            .map(|(signer, signature)| Vote {
                kind: self.kind,
                epoch: self.epoch,
                view: self.view,
                block: self.block,
                signer,
                signature: *signature,
            })
    }

    /// Whether the same quorum could not have signed both `self` and
    /// `other` without some of them being faulty: two blocks notarized or
    /// finalized in one view, or one view both finalized and nullified.
    #[must_use]
    pub fn conflicts_with(&self, other: &Self) -> bool {
        if self.epoch != other.epoch || self.view != other.view {
            return false;
        }
        match (self.kind, other.kind) {
            (VoteKind::Nullify, VoteKind::Finalize) | (VoteKind::Finalize, VoteKind::Nullify) => {
                true
            }
            (VoteKind::Nullify, _) | (_, VoteKind::Nullify) => false,
            _ => self.block != other.block,
        }
    }
}

/// Checks a certificate against the validator set of its epoch: at least q
/// distinct signers of the set, each signature valid. Members check the
/// certificates in receipts with it too
/// (spec/designs/settlement-hubs.md#3-the-ledger, "Receipts").
///
/// # Errors
///
/// Why it does not verify.
pub fn verify_certificate(
    hub: &Digest,
    set: &ValidatorSet,
    certificate: &Certificate,
) -> Result<(), Invalid> {
    if certificate.kind == VoteKind::Nullify && certificate.block != Digest::default() {
        return Err(Invalid::NullifyWithBlock);
    }
    if !certificate.signers.fits(set.len()) {
        return Err(Invalid::UnknownSigner);
    }
    if certificate.signers.count() < set.quorum() {
        return Err(Invalid::TooFewSigners);
    }
    if certificate.signatures.len() != certificate.signers.count() {
        return Err(Invalid::SignatureCount);
    }
    let digest = vote_digest(
        hub,
        certificate.epoch,
        certificate.kind,
        certificate.view,
        &certificate.block,
    );
    for (signer, signature) in certificate.signers.iter().zip(&certificate.signatures) {
        let key = set.key(signer).ok_or(Invalid::UnknownSigner)?;
        key.verify(digest.as_bytes(), signature)
            .map_err(|_| Invalid::BadSignature(signer))?;
    }
    Ok(())
}

/// A finalized block and, when one is held, the finalization that proves
/// it: its own, or that of the last of the repeats of it, when it is the
/// last block of an epoch. A block finalized as the ancestor of another has
/// none of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finalized {
    /// The block.
    pub block: Block,
    /// The repeats of it the finalization proves it through, each building
    /// on the one before, the first on the block; usually none.
    pub repeats: Vec<Block>,
    /// The finalization of the block, or of the last repeat, if held.
    pub finalization: Option<Certificate>,
}

impl Finalized {
    /// A block with its own finalization, if any.
    #[must_use]
    pub const fn new(block: Block, finalization: Option<Certificate>) -> Self {
        Self {
            block,
            repeats: Vec::new(),
            finalization,
        }
    }

    /// The block, then its repeats: the chain the finalization proves.
    #[must_use]
    pub fn chain(&self) -> Vec<Block> {
        core::iter::once(&self.block)
            .chain(&self.repeats)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use dassie_identity::PublicKey;

    fn keys(n: u8) -> Vec<NodeKey> {
        (1..=n).map(|i| NodeKey::from_bytes(&[i; 32])).collect()
    }

    fn set(keys: &[NodeKey]) -> ValidatorSet {
        ValidatorSet::new(
            keys.iter()
                .map(NodeKey::public_key)
                .collect::<Vec<PublicKey>>(),
        )
        .unwrap()
    }

    fn index(i: usize) -> ValidatorIndex {
        ValidatorIndex::new(i).unwrap()
    }

    #[test]
    fn a_vote_binds_hub_epoch_kind_view_and_block() {
        let keys = keys(4);
        let set = set(&keys);
        let hub = Digest([1; 32]);
        let block = Digest([2; 32]);
        let vote = Vote::sign(&hub, &keys[0], index(0), VoteKind::Notarize, (0, 5), block);
        assert_eq!(vote.verify(&hub, &set), Ok(()));
        for changed in [
            Vote { view: 6, ..vote },
            Vote { epoch: 1, ..vote },
            Vote {
                kind: VoteKind::Finalize,
                ..vote
            },
            Vote {
                block: Digest([3; 32]),
                ..vote
            },
            Vote {
                signer: index(1),
                ..vote
            },
        ] {
            assert!(changed.verify(&hub, &set).is_err());
        }
        assert!(vote.verify(&Digest([9; 32]), &set).is_err());
    }

    #[test]
    fn certificates_need_a_quorum_of_valid_signatures() {
        let keys = keys(4);
        let set = set(&keys);
        let hub = Digest([1; 32]);
        let block = Digest([2; 32]);
        let votes: Vec<Vote> = (0..3)
            .map(|i| Vote::sign(&hub, &keys[i], index(i), VoteKind::Finalize, (0, 7), block))
            .collect();
        let mut certificate = Certificate {
            kind: VoteKind::Finalize,
            epoch: 0,
            view: 7,
            block,
            signers: votes.iter().fold(Signers::NONE, |s, v| s.with(v.signer)),
            signatures: votes.iter().map(|v| v.signature).collect(),
        };
        assert_eq!(verify_certificate(&hub, &set, &certificate), Ok(()));
        assert_eq!(certificate.votes().collect::<Vec<_>>(), votes);
        certificate.signatures.swap(0, 1);
        assert!(verify_certificate(&hub, &set, &certificate).is_err());
        certificate.signatures.pop();
        assert_eq!(
            verify_certificate(&hub, &set, &certificate),
            Err(Invalid::SignatureCount)
        );
        let two = Certificate {
            signers: Signers::NONE.with(index(0)).with(index(1)),
            signatures: vec![votes[0].signature, votes[1].signature],
            ..certificate.clone()
        };
        assert_eq!(
            verify_certificate(&hub, &set, &two),
            Err(Invalid::TooFewSigners)
        );
    }

    #[test]
    fn conflicts_are_what_no_honest_validator_signs() {
        let keys = keys(4);
        let hub = Digest([1; 32]);
        let (a, b) = (Digest([2; 32]), Digest([3; 32]));
        let vote = |kind, view, block| Vote::sign(&hub, &keys[0], index(0), kind, (0, view), block);
        let notarize_a = vote(VoteKind::Notarize, 1, a);
        assert!(notarize_a.conflicts_with(&vote(VoteKind::Notarize, 1, b)));
        assert!(notarize_a.conflicts_with(&vote(VoteKind::Finalize, 1, b)));
        assert!(!notarize_a.conflicts_with(&vote(VoteKind::Finalize, 1, a)));
        assert!(!notarize_a.conflicts_with(&vote(VoteKind::Nullify, 1, a)));
        assert!(!notarize_a.conflicts_with(&vote(VoteKind::Notarize, 2, b)));
        assert!(vote(VoteKind::Finalize, 1, a).conflicts_with(&vote(VoteKind::Nullify, 1, a)));
    }
}
