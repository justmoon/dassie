//! Golden vectors from `testdata/identity/generate-vectors.mjs`, which
//! computes them apart from Dassie's code: the derivation chain from a
//! phrase's secret to every node secret, phrases as typed and damaged with
//! what reading them gives, the key-level vectors, and every path below the
//! node key (spec/areas/identity.md#derivation-chain).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use dassie_identity::{
    Created, Error, HubKey, NodeId, NodeKey, NodeSecret, Phrase, PhraseSecret, Positions,
    PublicKey, QuicEndpoint, Reading, Realm, Segment, Signature,
};
use serde_json::Value;
use support::{hex, hex_array, str_field, to_hex, vectors};

fn realm_of(entry: &Value) -> Realm {
    Realm::parse(str_field(entry, "realm")).unwrap()
}

const fn other(realm: Realm) -> Realm {
    match realm {
        Realm::Test => Realm::Live,
        Realm::Live => Realm::Test,
    }
}

#[test]
fn derivation_chain() {
    let vectors = vectors("chain.json");
    let chain = vectors["chain"].as_array().unwrap();
    assert_eq!(chain.len(), 64, "the chain file covers many phrases");
    for entry in chain {
        let phrase = str_field(entry, "phrase");
        let realm = realm_of(entry);
        let hours = u32::try_from(entry["created"].as_u64().unwrap()).unwrap();
        let created = Created::from_hours(hours).unwrap();
        let secret = PhraseSecret::from_bytes(hex_array(str_field(entry, "secret")));

        let made = Phrase::new(realm, created, &secret);
        assert_eq!(made.text().as_str(), phrase);

        let Ok(Reading::Exact(read)) = Phrase::read(phrase, Positions::NONE, realm) else {
            panic!("{phrase} reads as written");
        };
        assert_eq!(read.realm(), realm);
        assert_eq!(read.created(), created);
        assert_eq!(read.secret().as_bytes(), secret.as_bytes());
        assert_eq!(
            read.created().unix_ms(),
            Created::EPOCH_UNIX_MS + u64::from(hours) * 3_600_000
        );

        let seed = read.master_seed(realm).unwrap();
        assert_eq!(
            to_hex(seed.as_bytes()),
            str_field(entry, "masterSeed"),
            "{phrase}"
        );
        assert_eq!(
            read.master_seed(other(realm)).map(|_| ()),
            Err(Error::OtherRealm { realm })
        );

        let node_key = seed.node_key();
        assert_eq!(to_hex(node_key.as_bytes()), str_field(entry, "nodeKey"));
        let public_key = node_key.public_key();
        assert_eq!(to_hex(public_key.as_bytes()), str_field(entry, "publicKey"));
        assert_eq!(node_key.node_id().as_str(), str_field(entry, "nodeId"));
        assert_eq!(public_key.node_id().as_str(), str_field(entry, "nodeId"));

        let login = node_key.login_secret();
        assert_eq!(to_hex(login.as_bytes()), str_field(entry, "loginSecret"));

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
}

// Each secret is in the chain once per realm, and is a different node in
// each.
#[test]
fn one_secret_is_a_node_of_its_own_in_each_realm() {
    let vectors = vectors("chain.json");
    let chain = vectors["chain"].as_array().unwrap();
    for pair in chain.chunks(2) {
        let [test, live] = pair else { panic!("pairs") };
        assert_eq!(realm_of(test), Realm::Test);
        assert_eq!(realm_of(live), Realm::Live);
        assert_eq!(str_field(test, "secret"), str_field(live, "secret"));
        for field in ["phrase", "masterSeed", "nodeKey", "nodeId", "loginSecret"] {
            assert_ne!(str_field(test, field), str_field(live, field), "{field}");
        }
    }
}

/// The positions a reading's field lists.
fn positions(reading: &Value, field: &str) -> Positions {
    reading[field]
        .as_array()
        .unwrap()
        .iter()
        .fold(Positions::NONE, |positions, position| {
            positions.with(usize::try_from(position.as_u64().unwrap()).unwrap())
        })
}

#[test]
fn readings() {
    let vectors = vectors("chain.json");
    let readings = vectors["readings"].as_array().unwrap();
    let mut corrected = 0;
    for reading in readings {
        let input = str_field(reading, "input");
        let realm = realm_of(reading);
        let result = Phrase::read(input, positions(reading, "unreadable"), realm);
        let Some(error) = reading["error"].as_str() else {
            let expected = str_field(reading, "phrase");
            let changed = positions(reading, "changed");
            let phrase = match result {
                Ok(Reading::Exact(phrase)) => {
                    assert!(changed.is_empty(), "{input}");
                    phrase
                }
                Ok(Reading::Corrected(correction)) => {
                    assert_eq!(correction.changed(), changed, "{input}");
                    assert_eq!(correction.text().as_str(), expected);
                    corrected += 1;
                    correction.confirm()
                }
                Err(error) => panic!("{input}: {error}"),
            };
            assert_eq!(phrase.text().as_str(), expected);
            continue;
        };
        let result = result.map(|_| ());
        let matched = match error {
            "tooDamaged" => result == Err(Error::TooDamaged),
            "bip39" => matches!(result, Err(Error::Bip39Phrase { .. })),
            "wordCount" => matches!(result, Err(Error::WrongWordCount { .. })),
            "otherRealm" => {
                result
                    == Err(Error::OtherRealm {
                        realm: other(realm),
                    })
            }
            "invalidKind" => result == Err(Error::InvalidKind),
            "newerKind" => matches!(result, Err(Error::NewerKind { .. })),
            other => panic!("no error {other}"),
        };
        assert!(matched, "{input}: {result:?}, expected {error}");
    }
    assert!(corrected >= 20, "{corrected}");
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
        "routing" => NodeSecret::Routing,
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
    assert_eq!(names.len(), 9);
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

        let message = hex(str_field(entry, "message"));
        let signature = key.sign(&message);
        assert_eq!(to_hex(signature.as_bytes()), str_field(entry, "signature"));
        let expected = Signature::from_bytes(hex_array(str_field(entry, "signature")));
        assert_eq!(public_key.verify(&message, &expected), Ok(()));
        assert!(public_key.verify(b"another message", &expected).is_err());
    }
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
