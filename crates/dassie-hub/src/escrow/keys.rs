//! The keys a node makes for a hub, all derived from its node key under a
//! path that names the hub's network and nonce
//! (spec/designs/settlement-hubs.md#2-founding-a-hub,
//! spec/areas/identity.md#seed-paths). The hub ID cannot name them: it is
//! the hash of rules that contain them. The sponsor picks the nonce, and a
//! transaction names no network on the testnet or mainnet, so keys that
//! followed the nonce alone would make a mainnet hub's escrow account and
//! escrow keys those of a testnet hub under the same nonce, and what was
//! signed for one could apply on the other
//! (spec/decisions/hub-keys-by-network.md).

use alloc::string::String;

use dassie_identity::{HubKey, NodeKey, NodeSecret, Signature};
use dassie_xrpl::Wallet;

use super::bip32::{ExtendedPrivate, ExtendedPublic};
use crate::rules::{Acceptance, Terms};

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
    /// The keys `node` makes for the hub of `nonce` on `network`, if the
    /// payout key is a valid BIP 32 master key, which fails for one secret
    /// in about 2^127.
    #[must_use]
    pub fn derive(node: &NodeKey, network: u32, nonce: &[u8; 32]) -> Option<Self> {
        let derived = |key| {
            node.secret(NodeSecret::Hub {
                network,
                nonce,
                key,
            })
        };
        let consensus = NodeKey::from_bytes(derived(HubKey::Consensus).as_bytes());
        let escrow = Wallet::from_entropy(derived(HubKey::Escrow).as_bytes());
        let payout = ExtendedPrivate::master(derived(HubKey::Payout).as_bytes())?;
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

/// The wallet whose account is the escrow of the hub of `nonce` on
/// `network`, made by its sponsor. The same key makes the same account
/// again after a reset of the test network
/// (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
#[must_use]
pub fn escrow_master(sponsor: &NodeKey, network: u32, nonce: &[u8; 32]) -> Wallet {
    let secret = sponsor.secret(NodeSecret::Hub {
        network,
        nonce,
        key: HubKey::EscrowMaster,
    });
    Wallet::from_entropy(secret.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::TESTNET_NETWORK;
    use crate::rules::tests::terms;

    /// The XRP Ledger mainnet's network ID.
    const MAINNET: u32 = 0;

    #[test]
    fn keys_follow_the_node_and_the_nonce() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let a = ValidatorKeys::derive(&node, TESTNET_NETWORK, &[2; 32]).unwrap();
        let b = ValidatorKeys::derive(&node, TESTNET_NETWORK, &[2; 32]).unwrap();
        let other = ValidatorKeys::derive(&node, TESTNET_NETWORK, &[3; 32]).unwrap();
        assert_eq!(a.consensus.public_key(), b.consensus.public_key());
        assert_ne!(a.consensus.public_key(), other.consensus.public_key());
        assert_ne!(a.escrow.account(), other.escrow.account());
        assert_ne!(a.consensus.public_key(), node.public_key());
    }

    // A hub on another network never shares a key with a hub of the same
    // nonce on the testnet, nor with one on a third network.
    #[test]
    fn keys_follow_the_network() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let networks = [TESTNET_NETWORK, MAINNET, 2, 21_337];
        for (index, &network) in networks.iter().enumerate() {
            let keys = ValidatorKeys::derive(&node, network, &[2; 32]).unwrap();
            let master = escrow_master(&node, network, &[2; 32]);
            for &other in networks.iter().skip(index + 1) {
                let theirs = ValidatorKeys::derive(&node, other, &[2; 32]).unwrap();
                assert_ne!(keys.consensus.public_key(), theirs.consensus.public_key());
                assert_ne!(keys.escrow.account(), theirs.escrow.account());
                assert_ne!(
                    keys.payout.public().unwrap().key,
                    theirs.payout.public().unwrap().key
                );
                assert_ne!(
                    master.account(),
                    escrow_master(&node, other, &[2; 32]).account()
                );
            }
        }
    }

    #[test]
    fn an_acceptance_verifies() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let terms = terms(&NodeKey::from_bytes(&[9; 32]));
        let keys = ValidatorKeys::derive(&node, terms.network, &terms.nonce).unwrap();
        let acceptance = keys.accept(&node, &terms, "v1.example:443".into()).unwrap();
        assert!(acceptance.verifies(&terms.digest(), &terms.nonce));
    }

    #[test]
    fn the_escrow_is_the_sponsors_to_make_again() {
        let sponsor = NodeKey::from_bytes(&[4; 32]);
        let first = escrow_master(&sponsor, TESTNET_NETWORK, &[5; 32]);
        let again = escrow_master(&sponsor, TESTNET_NETWORK, &[5; 32]);
        assert_eq!(first.account(), again.account());
    }
}
