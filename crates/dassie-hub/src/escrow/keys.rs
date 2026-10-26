//! The keys a node makes for a hub, all derived from its node key under a
//! path that names the hub's nonce (spec/designs/settlement-hubs.md#2-founding-a-hub).
//! The hub ID cannot name them: it is the hash of rules that contain them.
//!
//! The path names the hub's network too, unless it is the XRP Ledger
//! testnet's, whose paths came first and stay as they are, so the hubs
//! running there keep their keys (spec/areas/identity.md#derivation-chain).
//! The sponsor picks the nonce, and a transaction names no network on the
//! testnet or mainnet, so keys that followed the nonce alone would make a
//! mainnet hub's escrow account and escrow keys those of a testnet hub
//! under the same nonce, and what was signed for one could apply on the
//! other.

use alloc::format;
use alloc::string::String;

use dassie_identity::{DerivedSecret, NodeKey, SeedPath, Signature};
use dassie_xrpl::Wallet;

use super::bip32::{ExtendedPrivate, ExtendedPublic};
use crate::ledger::TESTNET_NETWORK;
use crate::rules::{Acceptance, Terms};

/// 32 bytes as 64 lowercase hex digits.
fn hex(bytes: &[u8; 32]) -> String {
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The path under the node key of a secret for the hub of `nonce` on
/// `network`: `node/hub/<nonce>/<name>` on the testnet, and
/// `node/hub/network-<network>/<nonce>/<name>` on every other network.
/// The third segment is 64 hex digits on the testnet and never elsewhere,
/// so no two networks share a path.
fn path(network: u32, nonce: &[u8; 32], name: &str) -> String {
    let nonce = hex(nonce);
    if network == TESTNET_NETWORK {
        format!("node/hub/{nonce}/{name}")
    } else {
        format!("node/hub/network-{network}/{nonce}/{name}")
    }
}

/// The secret at `text`, wiped on drop.
fn secret(node: &NodeKey, text: &str) -> Option<DerivedSecret> {
    let path = SeedPath::parse(text).ok()?;
    node.derive(&path).ok()
}

/// The secret at `name` for the hub of `nonce` on `network`.
fn hub_secret(node: &NodeKey, network: u32, nonce: &[u8; 32], name: &str) -> Option<DerivedSecret> {
    secret(node, &path(network, nonce, name))
}

/// A member's account key in the hub `hub`, at `node/hub/<hub ID>/account`:
/// one key per hub, so accounts in two hubs are not linked by their key.
/// The path needs no network: the hub ID is the hash of rules whose terms
/// name it, so hubs on two networks never share an ID.
#[must_use]
pub fn member_key(node: &NodeKey, hub: &crate::rules::HubId) -> Option<NodeKey> {
    let text = format!("node/hub/{}/account", hex(&hub.0.0));
    Some(NodeKey::from_bytes(secret(node, &text)?.as_bytes()))
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
    /// The keys `node` makes for the hub of `nonce` on `network`.
    #[must_use]
    pub fn derive(node: &NodeKey, network: u32, nonce: &[u8; 32]) -> Option<Self> {
        let derived = |name| hub_secret(node, network, nonce, name);
        let consensus = NodeKey::from_bytes(derived("consensus")?.as_bytes());
        let escrow = Wallet::from_entropy(derived("escrow")?.as_bytes());
        let payout = ExtendedPrivate::master(derived("payout")?.as_bytes())?;
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
pub fn escrow_master(sponsor: &NodeKey, network: u32, nonce: &[u8; 32]) -> Option<Wallet> {
    Some(Wallet::from_entropy(
        hub_secret(sponsor, network, nonce, "escrow-master")?.as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;
    use crate::digest::Digest;
    use crate::rules::tests::terms;
    use crate::rules::{HubId, Rules};

    /// The XRP Ledger mainnet's network ID.
    const MAINNET: u32 = 0;

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|x| format!("{x:02x}")).collect()
    }

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

    // A hub on another network never shares a key with a testnet hub of
    // the same nonce, nor with a hub on a third network.
    #[test]
    fn keys_follow_the_network() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let networks = [TESTNET_NETWORK, MAINNET, 2, 21_337];
        for (index, &network) in networks.iter().enumerate() {
            let keys = ValidatorKeys::derive(&node, network, &[2; 32]).unwrap();
            let master = escrow_master(&node, network, &[2; 32]).unwrap();
            for &other in networks.iter().skip(index + 1) {
                let theirs = ValidatorKeys::derive(&node, other, &[2; 32]).unwrap();
                assert_ne!(keys.consensus.public_key(), theirs.consensus.public_key());
                assert_ne!(keys.escrow.account(), theirs.escrow.account());
                assert_ne!(
                    keys.payout.public().unwrap().key,
                    theirs.payout.public().unwrap().key
                );
                let their_master = escrow_master(&node, other, &[2; 32]).unwrap();
                assert_ne!(master.account(), their_master.account());
            }
        }
    }

    // The hub ID names the network, so a member's key follows it without
    // the path naming it.
    #[test]
    fn a_members_key_follows_the_hubs_network() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let testnet = terms(&NodeKey::from_bytes(&[9; 32]));
        let mut mainnet = testnet.clone();
        mainnet.network = MAINNET;
        let id = |terms| {
            Rules {
                terms,
                validators: Vec::new(),
            }
            .id()
        };
        let (testnet, mainnet) = (id(testnet), id(mainnet));
        assert_ne!(testnet, mainnet);
        assert_ne!(
            member_key(&node, &testnet).unwrap().public_key(),
            member_key(&node, &mainnet).unwrap().public_key()
        );
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
        let first = escrow_master(&sponsor, TESTNET_NETWORK, &[5; 32]).unwrap();
        let again = escrow_master(&sponsor, TESTNET_NETWORK, &[5; 32]).unwrap();
        assert_eq!(first.account(), again.account());
    }

    /// The paths are a contract (spec/areas/identity.md): a changed path
    /// changes the keys a hub's rules name, where a sponsor makes its
    /// escrow again, and a member's account. The testnet's are the paths
    /// without a network, which its hubs were founded and joined with.
    #[test]
    fn the_paths_give_these_keys() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let keys = ValidatorKeys::derive(&node, TESTNET_NETWORK, &[2; 32]).unwrap();
        assert_eq!(
            hex_of(keys.consensus.public_key().as_bytes()),
            "7f55e267ef2e17c23bf9fa1017b6333eaa3e3d1700c3df4072fb9b8e955cedd5"
        );
        assert_eq!(
            hex_of(keys.escrow.public_key().as_bytes()),
            "ed622c65311fe0a74847f8096749ca146611f6dfa093c781b6da9bcc29431b4e30"
        );
        assert_eq!(
            hex_of(&keys.payout.public().unwrap().key),
            "03f2899aaca40be88ee8b9fcf411329e120cb56b7aab1c19d6220ddc15fa350673"
        );
        assert_eq!(
            escrow_master(&node, TESTNET_NETWORK, &[2; 32])
                .unwrap()
                .account()
                .to_address(),
            "rJjEKAGxQud4c6YVbRJrDgmUQWjJ5QgJvw"
        );
        let hub = HubId(Digest([2; 32]));
        assert_eq!(
            hex_of(member_key(&node, &hub).unwrap().public_key().as_bytes()),
            "063d7a3d2d694c8a5e27213c30798081ee5ecbf2e0d41f443acdc96a3bce864d"
        );
    }

    /// The same on mainnet, whose paths name the network.
    #[test]
    fn the_mainnet_paths_give_these_keys() {
        let node = NodeKey::from_bytes(&[1; 32]);
        let keys = ValidatorKeys::derive(&node, MAINNET, &[2; 32]).unwrap();
        assert_eq!(
            hex_of(keys.consensus.public_key().as_bytes()),
            "3845b87da01db8d33a7a12138c08fcb4e7ee5d92b7666056bccdce0aaf261491"
        );
        assert_eq!(
            hex_of(keys.escrow.public_key().as_bytes()),
            "ed8dd6756847130369425d9995dc1f10028eb254004ec9b4139eb1b607b283a8b8"
        );
        assert_eq!(
            hex_of(&keys.payout.public().unwrap().key),
            "02295030c6a595b031de1ae90af2802ec55a674b31d2739d45da8dc0c42572f281"
        );
        assert_eq!(
            escrow_master(&node, MAINNET, &[2; 32])
                .unwrap()
                .account()
                .to_address(),
            "r9mjcj9Qs9yPJfy8HrPhCSm1PPvygXxbWE"
        );
    }

    /// The paths as text: the testnet's name no network, every other
    /// network's names it before the nonce.
    #[test]
    fn the_paths_read_so() {
        let nonce = "02".repeat(32);
        assert_eq!(
            path(TESTNET_NETWORK, &[2; 32], "consensus"),
            format!("node/hub/{nonce}/consensus")
        );
        assert_eq!(
            path(MAINNET, &[2; 32], "escrow-master"),
            format!("node/hub/network-0/{nonce}/escrow-master")
        );
        let node = NodeKey::from_bytes(&[1; 32]);
        let at = |text: &str| *secret(&node, text).unwrap().as_bytes();
        let derived = hub_secret(&node, MAINNET, &[2; 32], "consensus").unwrap();
        assert_eq!(
            at(&format!("node/hub/network-0/{nonce}/consensus")),
            *derived.as_bytes()
        );
    }
}
