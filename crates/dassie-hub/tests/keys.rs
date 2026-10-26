//! A hub's keys, from the golden vectors of every path below the node key
//! (`crates/dassie-identity/tests/data/paths.json`, from
//! `testdata/identity/`): a validator's consensus, escrow and payout keys
//! and a sponsor's escrow master on two networks, and a member's account
//! key. They hold funds, and a hub's rules name them, so a key that moves
//! fails here (spec/areas/identity.md#derivation-chain).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_hub::escrow::{ValidatorKeys, escrow_master};
use dassie_hub::member::member_key;
use dassie_hub::{Digest, HubId};
use dassie_identity::NodeKey;
use dassie_xrpl::hex_upper;
use serde_json::Value;

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

fn bytes32(hex: &str) -> [u8; 32] {
    dassie_xrpl::from_hex(hex).unwrap().try_into().unwrap()
}

fn lower(bytes: &[u8]) -> String {
    hex_upper(bytes).to_lowercase()
}

#[test]
fn a_hubs_keys_are_the_vectors() {
    let path = "../dassie-identity/tests/data/paths.json";
    let vectors: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let node = NodeKey::from_bytes(&bytes32(text(&vectors, "nodeKey")));
    let mut checked = 0;
    for entry in vectors["paths"].as_array().unwrap() {
        match text(entry, "name") {
            "hub" => {
                let network = u32::try_from(entry["network"].as_u64().unwrap()).unwrap();
                let nonce = bytes32(text(entry, "nonce"));
                let keys = ValidatorKeys::derive(&node, network, &nonce).unwrap();
                match text(entry, "key") {
                    "consensus" => assert_eq!(
                        lower(keys.consensus.public_key().as_bytes()),
                        text(entry, "publicKey")
                    ),
                    "escrow" => {
                        let wallet = &entry["wallet"];
                        assert_eq!(keys.escrow.family_seed(), text(wallet, "familySeed"));
                        assert_eq!(keys.escrow.account().to_address(), text(wallet, "address"));
                    }
                    "payout" => {
                        let public = keys.payout.public().unwrap();
                        assert_eq!(lower(&public.key), text(entry, "payoutPublicKey"));
                        assert_eq!(lower(&public.chain), text(entry, "payoutChainCode"));
                    }
                    "escrow-master" => {
                        let wallet = &entry["wallet"];
                        let master = escrow_master(&node, network, &nonce);
                        assert_eq!(master.family_seed(), text(wallet, "familySeed"));
                        assert_eq!(master.account().to_address(), text(wallet, "address"));
                    }
                    other => panic!("hub key {other}"),
                }
                checked += 1;
            }
            "hubMember" => {
                let hub = HubId(Digest(bytes32(text(entry, "hub"))));
                assert_eq!(
                    lower(member_key(&node, &hub).public_key().as_bytes()),
                    text(entry, "publicKey")
                );
                checked += 1;
            }
            _ => {}
        }
    }
    assert_eq!(checked, 9, "four keys on two networks, and a member's");
}
