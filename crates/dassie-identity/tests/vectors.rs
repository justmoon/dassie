//! Golden vectors from `testdata/identity/generate-vectors.mjs`, which
//! computes them apart from Dassie's code: the phrase format, written from
//! its decision alone (`testdata/identity/phrase.mjs`), with phrases as
//! made, clock readings, and phrases as typed and damaged with what reading
//! them gives; the derivation chain from a phrase to every node secret; the
//! key-level vectors; and every path below the node key
//! (spec/areas/identity.md#derivation-chain).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::collections::BTreeMap;

use dassie_identity::{
    Created, Error, Fips205Keygen, HubKey, NodeId, NodeIdentity, NodeKey, NodeSecret, Phrase,
    PhraseSecret, Positions, PublicKey, QuicEndpoint, Reading, Realm, Segment, Signature,
    SlhDsaPublicKey, SlhDsaSeed,
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

fn number(entry: &Value, field: &str) -> u64 {
    entry[field]
        .as_u64()
        .unwrap_or_else(|| panic!("missing number {field}"))
}

fn created_of(entry: &Value) -> Created {
    Created::from_hours(u32::try_from(number(entry, "created")).unwrap()).unwrap()
}

/// A reader's clock past every phrase's creation time.
const AFTER_EVERY_PHRASE: u64 = u64::MAX;

#[test]
fn derivation_chain() {
    let vectors = vectors("chain.json");
    let chain = vectors["chain"].as_array().unwrap();
    assert_eq!(chain.len(), 64, "the chain file covers many phrases");
    let mut realms = BTreeMap::new();
    for entry in chain {
        let phrase = str_field(entry, "phrase");
        let realm = realm_of(entry);
        *realms.entry(realm.as_str()).or_insert(0) += 1;
        let created = created_of(entry);
        let secret = PhraseSecret::from_bytes(hex_array(str_field(entry, "secret")));
        assert_eq!(
            number(entry, "kind"),
            if realm == Realm::Test { 1 } else { 2 }
        );

        let made = Phrase::new(realm, created, &secret);
        assert_eq!(made.text().as_str(), phrase);

        let Ok(Reading::Exact(read)) =
            Phrase::read(phrase, Positions::NONE, realm, AFTER_EVERY_PHRASE)
        else {
            panic!("{phrase} reads as written");
        };
        assert_eq!(read.realm(), realm);
        assert_eq!(read.created(), created);
        assert_eq!(read.secret().as_bytes(), secret.as_bytes());

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
        let identity = node_key.identity(&Fips205Keygen);
        assert_eq!(
            to_hex(identity.slh_dsa().as_bytes()),
            str_field(entry, "slhDsaPublicKey")
        );
        assert_eq!(identity.node_id().as_str(), str_field(entry, "nodeId"));
        let received = NodeIdentity::new(
            public_key,
            SlhDsaPublicKey::from_bytes(hex_array(str_field(entry, "slhDsaPublicKey"))),
        );
        assert_eq!(received.node_id().as_str(), str_field(entry, "nodeId"));

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
    assert_eq!(realms.get("test"), Some(&32), "both realms");
    assert_eq!(realms.get("live"), Some(&32), "both realms");
}

// Phrases made from a kind, a creation hour and a secret: their words, the
// date they show, and the master seed, which is another for each realm.
#[test]
fn made_phrases() {
    let vectors = vectors("phrase.json");
    let made = vectors["made"].as_array().unwrap();
    let mut seeds: BTreeMap<(&str, u64), BTreeMap<&str, &str>> = BTreeMap::new();
    for entry in made {
        let realm = realm_of(entry);
        let created = created_of(entry);
        let secret = PhraseSecret::from_bytes(hex_array(str_field(entry, "secret")));
        let phrase = Phrase::new(realm, created, &secret);
        let words: Vec<&str> = entry["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap())
            .collect();
        assert_eq!(phrase.words().collect::<Vec<_>>(), words);
        assert_eq!(phrase.text().as_str(), str_field(entry, "phrase"));
        assert_eq!(phrase.created(), created);
        assert_eq!(created.to_string(), str_field(entry, "date"));
        let seed = phrase.master_seed(realm).unwrap();
        assert_eq!(to_hex(seed.as_bytes()), str_field(entry, "masterSeed"));
        seeds
            .entry((str_field(entry, "secret"), number(entry, "created")))
            .or_default()
            .insert(realm.as_str(), str_field(entry, "masterSeed"));
    }
    // Every secret is made in both realms, and is a node of its own in
    // each.
    for both in seeds.values() {
        assert_eq!(both.len(), 2);
        assert_ne!(both.get("test"), both.get("live"));
    }
}

// The hour a clock reading lies in, and no hour outside the field, where
// no phrase is made.
#[test]
fn clock_readings() {
    let vectors = vectors("phrase.json");
    for clock in vectors["clocks"].as_array().unwrap() {
        let now_ms = number(clock, "unixMillis");
        let hour = clock["hour"]
            .as_u64()
            .map(|hour| Created::from_hours(u32::try_from(hour).unwrap()).unwrap());
        assert_eq!(
            Created::at_unix_ms(now_ms),
            hour,
            "{}",
            str_field(clock, "time")
        );
        let generated = Phrase::generate(&mut Sevens, Realm::Test, now_ms);
        match hour {
            Some(hour) => assert_eq!(generated.unwrap().created(), hour),
            None => assert_eq!(generated.map(|_| ()), Err(Error::ClockOutOfRange)),
        }
    }
}

/// An RNG that always gives sevens.
struct Sevens;

impl rand_core::TryRng for Sevens {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(0x0707_0707)
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(0x0707_0707_0707_0707)
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), Self::Error> {
        destination.fill(7);
        Ok(())
    }
}

impl rand_core::TryCryptoRng for Sevens {}

/// A reading's input as text, with the positions the owner marks as
/// unreadable. A marked word is typed as the list's first word, which reads
/// fine, so only the mark makes it an erasure.
fn typed(input: &[Value]) -> (String, Positions) {
    let mut unreadable = Positions::NONE;
    let words: Vec<&str> = input
        .iter()
        .enumerate()
        .map(|(position, word)| {
            word.as_str().unwrap_or_else(|| {
                unreadable = unreadable.with(position);
                "abandon"
            })
        })
        .collect();
    (words.join(" "), unreadable)
}

fn positions(list: &Value) -> Positions {
    list.as_array()
        .unwrap()
        .iter()
        .fold(Positions::NONE, |positions, position| {
            positions.with(usize::try_from(position.as_u64().unwrap()).unwrap())
        })
}

// Phrases as a reader reads them: typed sloppily, with wrong, unknown and
// unreadable words, of other lengths, kinds and realms, and made more than
// a day after the reader's clock, each with what the decision's steps make
// of it, in their order.
#[test]
fn readings() {
    let vectors = vectors("phrase.json");
    let readings = vectors["reading"].as_array().unwrap();
    let mut outcomes = BTreeMap::new();
    for reading in readings {
        let name = str_field(reading, "name");
        let input = reading["input"].as_array().unwrap();
        let (text, unreadable) = typed(input);
        let realm = realm_of(reading);
        let now_ms = number(reading, "clockUnixMs");
        let result = &reading["result"];
        let read = Phrase::read(&text, unreadable, realm, now_ms);
        let Some(refused) = result["refused"].as_str() else {
            let expected = str_field(result, "phrase");
            let changed = positions(&result["changed"]);
            let phrase = match read {
                Ok(Reading::Exact(phrase)) => {
                    assert!(changed.is_empty(), "{name}");
                    assert_eq!(result["corrected"], false, "{name}");
                    *outcomes.entry("exact").or_insert(0) += 1;
                    phrase
                }
                Ok(Reading::Corrected(correction)) => {
                    assert_eq!(result["corrected"], true, "{name}");
                    assert_eq!(correction.changed(), changed, "{name}");
                    assert_eq!(correction.text().as_str(), expected, "{name}");
                    assert_eq!(correction.created(), created_of(result), "{name}");
                    assert_eq!(correction.realm(), realm_of(result), "{name}");
                    *outcomes.entry("corrected").or_insert(0) += 1;
                    correction.confirm()
                }
                Err(error) => panic!("{name}: {error}"),
            };
            assert_eq!(phrase.text().as_str(), expected, "{name}");
            assert_eq!(phrase.created(), created_of(result), "{name}");
            assert_eq!(phrase.created().to_string(), str_field(result, "date"));
            assert_eq!(
                to_hex(phrase.secret().as_bytes()),
                str_field(result, "secret")
            );
            let seed = phrase.master_seed(realm).unwrap();
            assert_eq!(to_hex(seed.as_bytes()), str_field(result, "masterSeed"));
            continue;
        };
        let expected = match refused {
            "bip39" => Error::Bip39Phrase { words: input.len() },
            "length" => Error::WrongWordCount { words: input.len() },
            "tooDamaged" => Error::TooDamaged,
            "kindZero" => Error::InvalidKind,
            "unassignedKind" => Error::NewerKind {
                kind: u8::try_from(number(result, "kind")).unwrap(),
            },
            "otherRealm" => Error::OtherRealm {
                realm: realm_of(result),
            },
            "createdInFuture" => {
                let created = created_of(result);
                assert_eq!(created.to_string(), str_field(result, "date"));
                Error::CreatedInFuture { created }
            }
            other => panic!("no refusal {other}"),
        };
        assert_eq!(read.map(|_| ()), Err(expected), "{name}");
        *outcomes.entry(refused).or_insert(0) += 1;
    }
    // Every outcome is covered, and corrections most.
    assert_eq!(outcomes.len(), 9, "{outcomes:?}");
    assert!(outcomes["corrected"] >= 20, "{outcomes:?}");
}

// The refusal of a phrase from the future names its date, and says how to
// read it anyway.
#[test]
fn a_phrase_from_the_future_names_its_date() {
    let vectors = vectors("phrase.json");
    let reading = vectors["reading"]
        .as_array()
        .unwrap()
        .iter()
        .find(|reading| reading["result"]["refused"] == "createdInFuture")
        .unwrap();
    let (text, unreadable) = typed(reading["input"].as_array().unwrap());
    let error = Phrase::read(
        &text,
        unreadable,
        realm_of(reading),
        number(reading, "clockUnixMs"),
    )
    .unwrap_err()
    .to_string();
    let date = str_field(&reading["result"], "date");
    assert!(error.contains(&format!("made on {date}")), "{error}");
    assert!(
        error.contains(&format!("setting this clock past {date}")),
        "{error}"
    );
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
        "slhDsa" => NodeSecret::SlhDsa(match str_field(entry, "seed") {
            "sk-seed" => SlhDsaSeed::SkSeed,
            "sk-prf" => SlhDsaSeed::SkPrf,
            "pk-seed" => SlhDsaSeed::PkSeed,
            other => panic!("SLH-DSA seed {other}"),
        }),
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
    assert_eq!(names.len(), 10);
    let login = entries.iter().find(|entry| entry["name"] == "login");
    assert_eq!(
        to_hex(key.login_secret().as_bytes()),
        str_field(login.unwrap(), "secret")
    );
    // The SLH-DSA key of the seeds at its paths.
    assert_eq!(
        to_hex(key.slh_dsa_public_key(&Fips205Keygen).as_bytes()),
        str_field(&vectors, "slhDsaPublicKey")
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
        let slh_dsa: [u8; 32] = hex_array(str_field(entry, "slhDsaPublicKey"));
        assert_eq!(key.slh_dsa_public_key(&Fips205Keygen).as_bytes(), &slh_dsa);
        assert_eq!(
            key.identity(&Fips205Keygen).node_id().as_str(),
            str_field(entry, "nodeId")
        );
        let id = NodeId::from_keys(public_key.as_bytes(), &slh_dsa);
        assert_eq!(NodeId::parse(str_field(entry, "nodeId")), Ok(id));
        assert_eq!(
            NodeIdentity::new(
                PublicKey::from_bytes(public_key.as_bytes()).unwrap(),
                SlhDsaPublicKey::from_bytes(slh_dsa)
            )
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
    assert_eq!(str_field(input, "oid"), "1.3.6.1.4.1.54392.5.1399.2.2");
    let key: [u8; 32] = hex_array(str_field(input, "publicKey"));
    let slh_dsa: [u8; 32] = hex_array(str_field(input, "slhDsaPublicKey"));
    let expected = str_field(input, "encoded");
    assert_eq!(to_hex(&NodeId::hash_input(&key, &slh_dsa)), expected);
    assert_eq!(
        expected,
        format!(
            "0d2b0601040183a878058a77020220{}20{}",
            to_hex(&key),
            to_hex(&slh_dsa)
        )
    );
}
