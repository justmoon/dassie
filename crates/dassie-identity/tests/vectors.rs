//! Golden vectors produced by the original at e962ab78: the derivation
//! chain from entropy to every node secret, and the key-level vectors.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use dassie_identity::{
    LOGIN_PATH, Mnemonic, NodeId, NodeKey, PublicKey, SeedPath, Signature, X25519Secret,
};
use support::{hex, hex_array, str_field, to_hex, vectors};

#[test]
fn derivation_chain() {
    let vectors = vectors("chain.json");
    let chain = vectors["chain"].as_array().unwrap();
    assert!(
        chain.len() >= 64,
        "the chain file should cover many phrases"
    );
    for entry in chain {
        let phrase = str_field(entry, "mnemonic");

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
        assert_eq!(
            public_key.node_id().as_str(),
            str_field(entry, "expectedNodeId")
        );

        let login = node_key.login_secret();
        assert_eq!(to_hex(login.as_bytes()), str_field(entry, "loginSecret"));
        // The browser derives the same secret from the master seed.
        let from_seed = seed.derive(&SeedPath::parse(LOGIN_PATH).unwrap());
        assert_eq!(from_seed.as_bytes(), login.as_bytes());

        let dev = seed.dev_session_token();
        assert_eq!(to_hex(dev.as_bytes()), str_field(entry, "devSessionToken"));

        let testnet = node_key
            .settlement_entropy("xrpl-testnet", "seed-testnet")
            .unwrap();
        assert_eq!(
            to_hex(testnet.as_bytes()),
            str_field(entry, "xrplTestnetEntropy")
        );
        let mainnet = node_key.settlement_entropy("xrpl", "seed-mainnet").unwrap();
        assert_eq!(
            to_hex(mainnet.as_bytes()),
            str_field(entry, "xrplMainnetEntropy")
        );
        let path = SeedPath::parse("node/settlement-module/xrpl/seed-mainnet").unwrap();
        assert_eq!(seed.derive(&path).as_bytes(), mainnet.as_bytes());
        assert_eq!(
            node_key.derive(&path).unwrap().as_bytes(),
            mainnet.as_bytes()
        );
    }
}

#[test]
fn bootstrap_node_ids() {
    let vectors = vectors("keys.json");
    for node in vectors["bootstrap"].as_array().unwrap() {
        let key: [u8; 32] = hex_array(str_field(node, "publicKey"));
        let id = NodeId::from_public_key(&key);
        assert_eq!(id.as_str(), str_field(node, "nodeId"));
        assert_eq!(NodeId::parse(str_field(node, "nodeId")), Ok(id));
        assert_eq!(PublicKey::from_bytes(&key).unwrap().node_id(), id);
    }
}

#[test]
fn keys() {
    let vectors = vectors("keys.json");
    for entry in vectors["keys"].as_array().unwrap() {
        let key = NodeKey::from_bytes(&hex_array(str_field(entry, "seed")));
        let public_key = key.public_key();
        assert_eq!(to_hex(public_key.as_bytes()), str_field(entry, "publicKey"));
        assert_eq!(key.node_id().as_str(), str_field(entry, "nodeId"));

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
