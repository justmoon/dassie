//! The keys a node makes for a hub, all derived from its node key under a
//! path that names the hub's nonce (spec/designs/settlement-hubs.md#2-founding-a-hub).
//! The hub ID cannot name them: it is the hash of rules that contain them.

use alloc::format;
use alloc::string::String;

use dassie_identity::{NodeKey, SeedPath, Signature};
use dassie_xrpl::Wallet;

use super::bip32::{ExtendedPrivate, ExtendedPublic};
use crate::rules::{Acceptance, Terms};

/// The path under the node key of a secret for the hub of `nonce`.
fn path(nonce: &[u8; 32], name: &str) -> String {
    let mut hex = String::with_capacity(64);
    for byte in nonce {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("node/hub/{hex}/{name}")
}

/// The secret at `name` for the hub of `nonce`.
fn secret(node: &NodeKey, nonce: &[u8; 32], name: &str) -> Option<[u8; 32]> {
    let text = path(nonce, name);
    let path = SeedPath::parse(&text).ok()?;
    node.derive(&path).ok().map(|secret| *secret.as_bytes())
}

/// What a validator makes for a hub: a consensus key, an escrow key on the
/// escrow's signer list, and a payout key whose children sign payout sets.
pub struct ValidatorKeys {
    /// Signs votes, certificates and receipts.
    pub consensus: NodeKey,
    /// Signs the escrow's transactions, as an entry of its signer list.
    pub escrow: Wallet,
    /// The parent of the keys that sign payout sets.
    pub payout: ExtendedPrivate,
}

impl ValidatorKeys {
    /// The keys `node` makes for the hub of `nonce`.
    #[must_use]
    pub fn derive(node: &NodeKey, nonce: &[u8; 32]) -> Option<Self> {
        let consensus = NodeKey::from_bytes(&secret(node, nonce, "consensus")?);
        let escrow = Wallet::from_entropy(&secret(node, nonce, "escrow")?);
        let payout = ExtendedPrivate::master(&secret(node, nonce, "payout")?)?;
        Some(Self {
            consensus,
            escrow,
            payout,
        })
    }

    /// The validator's acceptance of `terms`, reachable at `endpoint`,
    /// signed by `node`.
    #[must_use]
    pub fn accept(&self, node: &NodeKey, terms: &Terms, endpoint: String) -> Option<Acceptance> {
        let ExtendedPublic { key, chain } = self.payout.public()?;
        Some(
            Acceptance {
                node: node.public_key(),
                consensus: self.consensus.public_key(),
                escrow: *self.escrow.public_key().as_bytes(),
                payout: key,
                payout_chain: chain,
                endpoint,
                signature: Signature::from_bytes([0; 64]),
            }
            .sign(node, terms),
        )
    }
}

/// The wallet whose account is the escrow of the hub of `nonce`, made by
/// its sponsor. The same key makes the same account again after a reset
/// of the test network (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
#[must_use]
pub fn escrow_master(sponsor: &NodeKey, nonce: &[u8; 32]) -> Option<Wallet> {
    Some(Wallet::from_entropy(&secret(
        sponsor,
        nonce,
        "escrow-master",
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::tests::terms;

    #[test]
    fn keys_follow_the_node_and_the_nonce() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let a = ValidatorKeys::derive(&node, &[2; 32]).unwrap();
        let b = ValidatorKeys::derive(&node, &[2; 32]).unwrap();
        let other = ValidatorKeys::derive(&node, &[3; 32]).unwrap();
        assert_eq!(a.consensus.public_key(), b.consensus.public_key());
        assert_ne!(a.consensus.public_key(), other.consensus.public_key());
        assert_ne!(a.escrow.account(), other.escrow.account());
        assert_ne!(a.consensus.public_key(), node.public_key());
    }

    #[test]
    fn an_acceptance_verifies() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let terms = terms(&NodeKey::from_bytes(&[9; 32]));
        let keys = ValidatorKeys::derive(&node, &terms.nonce).unwrap();
        let acceptance = keys.accept(&node, &terms, "v1.example:443".into()).unwrap();
        assert!(acceptance.verifies(&terms.digest(), &terms.nonce));
    }

    #[test]
    fn the_escrow_is_the_sponsors_to_make_again() {
        let sponsor = NodeKey::from_bytes(&[4; 32]);
        let first = escrow_master(&sponsor, &[5; 32]).unwrap();
        let again = escrow_master(&sponsor, &[5; 32]).unwrap();
        assert_eq!(first.account(), again.account());
    }
}
