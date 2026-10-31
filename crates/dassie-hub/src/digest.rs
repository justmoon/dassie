//! The hashes a hub names things by: its ID, blocks, operations, state.
//!
//! Every hash is SHA-256 over a domain tag and the canonical encoding of
//! what it names, so that a hash of one kind can never be taken for a hash
//! of another, and a signature over one never verifies as a signature over
//! the other. Signatures sign digests, never raw bytes of their own choosing.

use core::fmt;

use sha2::{Digest as _, Sha256};

/// A SHA-256 digest under one of the [domains](Domain).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Digest(pub [u8; 32]);

/// What a digest names. Each tag is fixed: changing one changes every hub
/// ID and every signature made under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    /// A hub's rules; their digest is the hub ID.
    Rules,
    /// A hub's terms, which a validator's acceptance names.
    Terms,
    /// A validator's acceptance of the terms.
    Acceptance,
    /// A block of the hub's log.
    Block,
    /// A consensus vote.
    Vote,
    /// What a member or a validator signs for an operation.
    Operation,
    /// An operation as a block holds it, signature included: what its
    /// receipt names.
    OperationId,
    /// A leaf of the state's Merkle tree.
    StateLeaf,
    /// An inner node of the state's Merkle tree.
    StateNode,
    /// A payer's commitment to a payee, sent with a Prepare.
    Commitment,
    /// A payer's claim, sent with its reply to a Fulfill.
    Claim,
    /// A payee's release of a commitment, sent with its Reject.
    Release,
    /// The outcome of an operation in a block.
    Result,
    /// What a validator tells a member the log holds.
    Statement,
    /// A validator's signature of a hub's genesis, a share of its genesis
    /// certificate.
    Genesis,
    /// A member's account key saying it is the key of a node, by its node
    /// ID, which the node's link state carries.
    KeyProof,
}

impl Domain {
    const fn tag(self) -> &'static [u8] {
        match self {
            Self::Rules => b"dassie-hub/rules",
            Self::Terms => b"dassie-hub/terms",
            Self::Acceptance => b"dassie-hub/acceptance",
            Self::Block => b"dassie-hub/block",
            Self::Vote => b"dassie-hub/vote",
            Self::Operation => b"dassie-hub/operation",
            Self::OperationId => b"dassie-hub/operation-id",
            Self::StateLeaf => b"dassie-hub/state-leaf",
            Self::StateNode => b"dassie-hub/state-node",
            Self::Commitment => b"dassie-hub/commitment",
            Self::Claim => b"dassie-hub/claim",
            Self::Release => b"dassie-hub/release",
            Self::Result => b"dassie-hub/result",
            Self::Statement => b"dassie-hub/statement",
            Self::Genesis => b"dassie-hub/genesis",
            Self::KeyProof => b"dassie-hub/key-proof",
        }
    }
}

impl Digest {
    /// The digest of `parts`, concatenated, under `domain`. The tag is
    /// length-prefixed, so no tag is a prefix of another's input.
    #[must_use]
    pub fn of(domain: Domain, parts: &[&[u8]]) -> Self {
        let tag = domain.tag();
        let mut hasher = Sha256::new();
        // Tags are short constants.
        hasher.update([u8::try_from(tag.len()).unwrap_or(u8::MAX)]);
        hasher.update(tag);
        for part in parts {
            hasher.update(part);
        }
        Self(hasher.finalize().into())
    }

    /// The bytes of the digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0.iter().take(6) {
            write!(f, "{byte:02x}")?;
        }
        f.write_str("…")
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_separate() {
        let a = Digest::of(Domain::Block, &[b"x"]);
        let b = Digest::of(Domain::Vote, &[b"x"]);
        assert_ne!(a, b);
    }

    #[test]
    fn parts_concatenate() {
        assert_eq!(
            Digest::of(Domain::Block, &[b"ab", b"c"]),
            Digest::of(Domain::Block, &[b"abc"])
        );
    }
}
