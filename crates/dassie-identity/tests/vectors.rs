//! Golden vectors from `testdata/identity/generate-vectors.mjs`, which
//! computes them with libraries of its own: the derivation chain from
//! entropy to every node secret, the key-level vectors, and every path
//! below the node key (spec/areas/identity.md#derivation-chain).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use dassie_identity::{
    HubKey, Mnemonic, NodeId, NodeKey, NodeSecret, PublicKey, QuicEndpoint, Segment, Signature,
    X25519Secret,
};
use serde_json::Value;
use support::{hex, hex_array, str_field, to_hex, vectors};

#[test]
fn derivation_chain() {
    let vectors = vectors("chain.json");
    let chain = vectors["chain"].as_array().unwrap();
    assert!(
        chain.len() >= 64,
        "the chain file should cover many phrases"
    );
    let mut lengths = std::collections::BTreeSet::new();
    for entry in chain {
        let phrase = str_field(entry, "mnemonic");
        lengths.insert(phrase.split(' ').count());

        let generated = Mnemonic::from_entropy(&hex(str_field(entry, "entropy"))).unwrap();
        assert_eq!(generated.phrase().as_str(), phrase);

        let mnemonic = Mnemonic::parse(phrase).unwrap();
        assert_eq!(to_hex(mnemonic.entropy()), str_field(entry, "entropy"));
        let seed = mnemonic.to_seed();
        assert_eq!(
            to_hex(seed.as_bytes()),
            str_field(entry, "masterSeed"),
            "{phrase}"
        );

        let node_key = seed.node_key();
        assert_eq!(to_hex(node_key.as_bytes()), str_field(entry, "nodeKey"));
        let public_key = node_key.public_key();
        assert_eq!(to_hex(public_key.as_bytes()), str_field(entry, "publicKey"));
        assert_eq!(node_key.node_id().as_str(), str_field(entry, "nodeId"));
        assert_eq!(public_key.node_id().as_str(), str_field(entry, "nodeId"));

        let login = node_key.login_secret();
        assert_eq!(to_hex(login.as_bytes()), str_field(entry, "loginSecret"));

        let dev = seed.dev_session_token();
        assert_eq!(to_hex(dev.as_bytes()), str_field(entry, "devSessionToken"));

        let testnet = node_key.xrpl_account_entropy(1, 0);
        assert_eq!(
            to_hex(testnet.as_bytes()),
            str_field(entry, "xrplTestnetEntropy")
        );
        let mainnet = node_key.xrpl_account_entropy(0, 0);
        assert_eq!(
            to_hex(mainnet.as_bytes()),
            str_field(entry, "xrplMainnetEntropy")
        );
    }
    assert_eq!(lengths.len(), 5, "phrases of every length");
}

/// The secret a vector names, by its function's name and arguments.
fn secret_of<'a>(entry: &'a Value, nonce: &'a mut [u8; 32]) -> NodeSecret<'a> {
    let quic = || match str_field(entry, "endpoint") {
        "udp" => QuicEndpoint::Udp,
        "tunnel" => QuicEndpoint::Tunnel,
        other => panic!("endpoint {other}"),
    };
    match str_field(entry, "name") {
        "login" => NodeSecret::Login,
        "openPaymentsClient" => NodeSecret::OpenPaymentsClient,
        "streamReceiver" => NodeSecret::StreamReceiver {
            ledger: Segment::parse(str_field(entry, "ledger")).unwrap(),
        },
        "quicStatelessReset" => NodeSecret::QuicStatelessReset(quic()),
        "quicConnectionIds" => NodeSecret::QuicConnectionIds(quic()),
        "hub" => {
            *nonce = hex_array(str_field(entry, "nonce"));
            NodeSecret::Hub {
                network: u32::try_from(entry["network"].as_u64().unwrap()).unwrap(),
                nonce,
                key: match str_field(entry, "key") {
                    "consensus" => HubKey::Consensus,
                    "escrow" => HubKey::Escrow,
                    "payout" => HubKey::Payout,
                    "escrow-master" => HubKey::EscrowMaster,
                    other => panic!("hub key {other}"),
                },
            }
        }
        "hubMember" => {
            *nonce = hex_array(str_field(entry, "hub"));
            NodeSecret::HubMember { hub: nonce }
        }
        "xrplAccount" => NodeSecret::XrplAccount {
            network: u32::try_from(entry["network"].as_u64().unwrap()).unwrap(),
            index: u16::try_from(entry["index"].as_u64().unwrap()).unwrap(),
        },
        other => panic!("no secret {other}"),
    }
}

// Every path below the node key, each with its text and its secret, so a
// path that moves fails here and not in a user's wallet.
#[test]
fn every_path_below_the_node_key() {
    let vectors = vectors("paths.json");
    let key = NodeKey::from_bytes(&hex_array(str_field(&vectors, "nodeKey")));
    let entries = vectors["paths"].as_array().unwrap();
    let mut names = std::collections::BTreeSet::new();
    for entry in entries {
        let mut bytes = [0; 32];
        let which = secret_of(entry, &mut bytes);
        names.insert(str_field(entry, "name"));
        assert_eq!(which.to_string(), str_field(entry, "path"));
        assert_eq!(
            to_hex(key.secret(which).as_bytes()),
            str_field(entry, "secret"),
            "{which}"
        );
    }
    // One name per variant of `NodeSecret`.
    assert_eq!(names.len(), 8);
    let login = entries.iter().find(|entry| entry["name"] == "login");
    assert_eq!(
        to_hex(key.login_secret().as_bytes()),
        str_field(login.unwrap(), "secret")
    );
}

// The key of a hub's member account is the Ed25519 key of its secret, and
// a validator's consensus key too.
#[test]
fn hub_keys_name_these_public_keys() {
    let vectors = vectors("paths.json");
    let key = NodeKey::from_bytes(&hex_array(str_field(&vectors, "nodeKey")));
    let mut checked = 0;
    for entry in vectors["paths"].as_array().unwrap() {
        let Some(public_key) = entry["publicKey"].as_str() else {
            continue;
        };
        let mut bytes = [0; 32];
        let secret = key.secret(secret_of(entry, &mut bytes));
        let derived = NodeKey::from_bytes(secret.as_bytes()).public_key();
        assert_eq!(to_hex(derived.as_bytes()), public_key);
        checked += 1;
    }
    assert_eq!(checked, 3, "two consensus keys and a member's");
}

#[test]
fn keys() {
    let vectors = vectors("keys.json");
    for entry in vectors["keys"].as_array().unwrap() {
        let key = NodeKey::from_bytes(&hex_array(str_field(entry, "seed")));
        let public_key = key.public_key();
        assert_eq!(to_hex(public_key.as_bytes()), str_field(entry, "publicKey"));
        assert_eq!(key.node_id().as_str(), str_field(entry, "nodeId"));
        let id = NodeId::from_public_key(public_key.as_bytes());
        assert_eq!(NodeId::parse(str_field(entry, "nodeId")), Ok(id));
        assert_eq!(
            PublicKey::from_bytes(public_key.as_bytes())
                .unwrap()
                .node_id(),
            id
        );

        let x25519 = key.x25519_secret();
        assert_eq!(to_hex(x25519.as_bytes()), str_field(entry, "x25519Secret"));
        assert_eq!(
            to_hex(x25519.public_key().as_bytes()),
            str_field(entry, "x25519Public")
        );
        assert_eq!(public_key.to_x25519(), x25519.public_key());

        let message = hex(str_field(entry, "message"));
        let signature = key.sign(&message);
        assert_eq!(to_hex(signature.as_bytes()), str_field(entry, "signature"));
        let expected = Signature::from_bytes(hex_array(str_field(entry, "signature")));
        assert_eq!(public_key.verify(&message, &expected), Ok(()));
        assert!(public_key.verify(b"another message", &expected).is_err());
    }
}

#[test]
fn session_shared_secret() {
    let vectors = vectors("keys.json");
    let session = &vectors["session"];
    let secret = X25519Secret::from_bytes(hex_array(str_field(session, "privateKey")));
    assert_eq!(
        to_hex(secret.public_key().as_bytes()),
        str_field(session, "publicKey")
    );
    let peer = PublicKey::from_bytes(&hex_array(str_field(session, "peerPublicKey"))).unwrap();
    let shared = secret.diffie_hellman(&peer.to_x25519()).unwrap();
    assert_eq!(
        to_hex(shared.as_bytes()),
        str_field(session, "sharedSecret")
    );
}

#[test]
fn node_id_hash_input() {
    let vectors = vectors("keys.json");
    let input = &vectors["nodeIdHashInput"];
    assert_eq!(str_field(input, "oid"), "1.3.6.1.4.1.54392.5.1399.2.1");
    let key: [u8; 32] = hex_array(str_field(input, "publicKey"));
    let expected = format!("{}20{}", str_field(input, "encoded"), to_hex(&key));
    assert_eq!(to_hex(&NodeId::hash_input(&key)), expected);
    assert_eq!(
        expected,
        format!("0d2b0601040183a878058a77020120{}", to_hex(&key))
    );
}
