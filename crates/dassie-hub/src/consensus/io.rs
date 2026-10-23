//! What comes out of the engine: messages to send, events, evidence and
//! counters.

use alloc::boxed::Box;
use alloc::vec::Vec;

use dassie_identity::PublicKey;

use super::types::{Certificate, Finalized, Vote};
use crate::digest::Digest;
use crate::validators::{Signers, ValidatorIndex};

/// The host's handle for whoever sent a message, so an answer finds its way
/// back: a validator or a member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(pub u64);

/// Where a message goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recipient {
    /// Every validator of the sets the host knows, but ourselves.
    All,
    /// One validator, by its consensus key.
    Validator(Box<PublicKey>),
    /// Whoever sent the message this answers.
    Peer(PeerId),
}

/// A message for the host to send, encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// Where it goes.
    pub to: Recipient,
    /// The encoded [`Message`](super::wire::Message).
    pub message: Vec<u8>,
}

/// A proof that validators broke the rules no honest validator breaks
/// (spec/designs/settlement-hubs.md#10-consensus, "Evidence"). It proves
/// itself to anyone with the validator set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// One validator signed two votes that conflict
    /// ([`Vote::conflicts_with`]).
    Votes {
        /// The vote seen first.
        first: Vote,
        /// The one that conflicts with it.
        second: Vote,
    },
    /// Two certificates of one view that conflict
    /// ([`Certificate::conflicts_with`]): every validator that signed both
    /// signed conflicting votes.
    Certificates {
        /// The certificate seen first.
        first: Certificate,
        /// The one that conflicts with it.
        second: Certificate,
        /// Who signed both: at least f + 1.
        signers: Signers,
    },
    /// Two different blocks finalized at one height. No honest validator
    /// votes to finalize two blocks of one height, so every validator that
    /// signed both finalizations is faulty.
    Finalizations {
        /// The block finalized first, with its finalization.
        first: Finalized,
        /// The other, with its finalization.
        second: Finalized,
        /// Who signed both: at least f + 1.
        signers: Signers,
    },
}

impl Evidence {
    /// The validators it proves faulty.
    #[must_use]
    pub fn culprits(&self) -> Signers {
        match self {
            Self::Votes { first, .. } => Signers::NONE.with(first.signer),
            Self::Certificates { signers, .. } | Self::Finalizations { signers, .. } => *signers,
        }
    }
}

/// What happened, for the host, the logs and the simulations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// We entered a view.
    EnteredView {
        /// The view.
        view: u64,
        /// Its leader.
        leader: ValidatorIndex,
    },
    /// We proposed a block.
    Proposed {
        /// The view.
        view: u64,
        /// Its height.
        height: u64,
        /// Its digest.
        block: Digest,
    },
    /// A view was notarized.
    Notarized {
        /// The view.
        view: u64,
        /// The block.
        block: Digest,
    },
    /// A view was nullified.
    Nullified {
        /// The view.
        view: u64,
    },
    /// A block became final, and the application learned it.
    Finalized {
        /// Its view.
        view: u64,
        /// Its height.
        height: u64,
        /// Its digest.
        block: Digest,
    },
    /// Validators broke the rules.
    Evidence(Box<Evidence>),
    /// A finalized block above our chain that we cannot connect to it from
    /// what peers keep: the host loads the state after it from a validator
    /// or member, checks it against the block's state root
    /// (spec/designs/settlement-hubs.md#9-shutdown, "The state"), and then
    /// calls [`Engine::jump`](super::Engine::jump) with it.
    Behind(Box<Finalized>),
    /// We declared the hub stalled, and never vote in it again.
    Stalled,
    /// A new validator set took over.
    Reconfigured {
        /// The first height it certifies.
        from_height: u64,
        /// The first view it runs.
        from_view: u64,
    },
}

/// What the engine counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Messages that did not decode, did not verify or were out of place.
    pub refused: u64,
    /// Valid votes taken, ours included.
    pub votes: u64,
    /// Certificates formed or taken.
    pub certificates: u64,
    /// Blocks we proposed.
    pub proposals: u64,
    /// Views we nullified.
    pub nullifies: u64,
    /// Catch-up requests sent.
    pub fetches: u64,
    /// Blocks finalized.
    pub finalized: u64,
}
