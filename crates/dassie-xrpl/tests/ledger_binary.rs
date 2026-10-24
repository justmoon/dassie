//! Golden data from the live ledgers (spec/designs/xrpl-validation.md#13-what-pins-it):
//! mainnet and testnet ledger headers, every validation of one ledger on
//! each network, the publishers' and the validators' manifests, and ledger
//! objects, transactions and metadata as rippled serializes them; and the
//! property that the binary format is read in its canonical form only, so
//! what reads, from whatever bytes, writes back to exactly those bytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;
use std::sync::OnceLock;

use dassie_xrpl::binary::{self, ARRAY_END, FieldId, Fields, OBJECT_END, Type};
use dassie_xrpl::codec::DecodeError;
use dassie_xrpl::header::{HEADER_BYTES, LedgerHeader};
use dassie_xrpl::keys::{PublicKey, sha512_half};
use dassie_xrpl::manifest::Manifest;
use dassie_xrpl::validation::{FULL, FULLY_CANONICAL, Validation};
use dassie_xrpl::{from_hex, hex_upper};
use proptest::prelude::*;
use serde_json::Value;

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

fn number(value: &Value, key: &str) -> u64 {
    value[key]
        .as_u64()
        .or_else(|| value[key].as_str().and_then(|text| text.parse().ok()))
        .unwrap_or_else(|| panic!("{key}"))
}

fn hash(value: &Value, key: &str) -> [u8; 32] {
    from_hex(text(value, key)).unwrap().try_into().unwrap()
}

/// A header from the `ledger` method's JSON, field by field.
fn header_from_json(json: &Value) -> LedgerHeader {
    let small = |key| u32::try_from(number(json, key)).unwrap();
    let byte = |key| u8::try_from(number(json, key)).unwrap();
    LedgerHeader {
        sequence: small("ledger_index"),
        total_drops: number(json, "total_coins"),
        parent_hash: hash(json, "parent_hash"),
        transaction_hash: hash(json, "transaction_hash"),
        account_hash: hash(json, "account_hash"),
        parent_close_time: small("parent_close_time"),
        close_time: small("close_time"),
        close_time_resolution: byte("close_time_resolution"),
        close_flags: byte("close_flags"),
    }
}

/// The serialization read field by field, objects and arrays too, and
/// written again, with the types it holds. No recursion: a stack of the
/// levels being read, each with the marker that ends it.
fn rebuild(bytes: &[u8]) -> Result<(Vec<u8>, BTreeSet<u8>), DecodeError> {
    let mut out = Vec::new();
    let mut types = BTreeSet::new();
    let mut levels: Vec<(Fields<'_>, Option<FieldId>)> = vec![(Fields::new(bytes), None)];
    while let Some((fields, _)) = levels.last_mut() {
        let Some(field) = fields.next() else {
            if let Some((_, Some(end))) = levels.pop() {
                binary::write_header(&mut out, end);
            }
            continue;
        };
        let field = field?;
        types.insert(field.id.kind().code());
        match field.id.kind() {
            Type::Object => {
                binary::write_header(&mut out, field.id);
                levels.push((field.fields()?, Some(OBJECT_END)));
            }
            Type::Array => {
                binary::write_header(&mut out, field.id);
                levels.push((field.elements()?, Some(ARRAY_END)));
            }
            _ => binary::write_field(&mut out, field.id, field.value),
        }
    }
    Ok((out, types))
}

// The hash of a header is the ledger's, on both networks, and its fields
// are the JSON's.
#[test]
fn headers_hash_to_their_ledgers() {
    let data = read("tests/data/ledger-headers.json");
    let ledgers = data["ledgers"].as_array().unwrap();
    assert_eq!(ledgers.len(), 8);
    for ledger in ledgers {
        let bytes = from_hex(text(ledger, "ledger_data")).unwrap();
        assert_eq!(bytes.len(), HEADER_BYTES);
        let header = LedgerHeader::from_bytes(&bytes).unwrap();
        assert_eq!(hex_upper(&header.hash()), text(ledger, "ledger_hash"));
        assert_eq!(header.to_bytes().as_slice(), bytes.as_slice());
        let mut json = ledger["json"].clone();
        json["ledger_index"] = ledger["ledger_index"].clone();
        assert_eq!(header_from_json(&json), header);
    }
}

// Every validation of one ledger reads, writes back to its bytes and names
// the key the stream named; the UNL's validators, each with its current
// signing key, signed the ledger's hash, full and fully canonical, enough
// of them for the quorum.
#[test]
fn a_ledger_s_validations_verify_and_reach_quorum() {
    let data = read("tests/data/validations.json");
    let ledgers = data["ledgers"].as_array().unwrap();
    assert_eq!(ledgers.len(), 2);
    for ledger in ledgers {
        let mut header = ledger["header"].clone();
        header["ledger_index"] = ledger["ledger_index"].clone();
        let ledger_hash = hash(ledger, "ledger_hash");
        assert_eq!(header_from_json(&header).hash(), ledger_hash);
        let unl = ledger["unl"].as_array().unwrap();
        let signing_keys: Vec<PublicKey> = unl
            .iter()
            .map(|validator| {
                let key = PublicKey::from_node_public(text(validator, "signing_b58")).unwrap();
                assert_eq!(hex_upper(key.as_bytes()), text(validator, "signing"));
                key
            })
            .collect();
        let mut signed = BTreeSet::new();
        let validations = ledger["validations"].as_array().unwrap();
        for entry in validations {
            let bytes = from_hex(text(entry, "data")).unwrap();
            let validation = Validation::from_bytes(&bytes).unwrap();
            assert_eq!(validation.to_bytes(), bytes);
            assert_eq!(
                validation.signing_key.to_node_public(),
                text(entry, "validation_public_key")
            );
            assert_eq!(u64::from(validation.flags), number(entry, "flags"));
            assert_eq!(
                u64::from(validation.signing_time),
                number(entry, "signing_time")
            );
            assert_eq!(validation.ledger_hash, ledger_hash);
            assert_eq!(
                u64::from(validation.ledger_sequence),
                number(ledger, "ledger_index")
            );
            let full = validation.flags & (FULL | FULLY_CANONICAL) == FULL | FULLY_CANONICAL;
            assert_eq!(validation.verify(), full);
            if let Some(index) = signing_keys
                .iter()
                .position(|key| *key == validation.signing_key)
            {
                assert!(validation.verify());
                assert!(signed.insert(index), "one validation per validator");
            }
        }
        assert_eq!(signed.len(), unl.len());
        assert!(signed.len() >= usize::try_from(number(ledger, "quorum")).unwrap());
    }
}

// Each manifest reads and writes back, names the keys the lists and the
// `manifest` method named, signs the bytes rippled signs, and both its
// signatures verify; the UNL's signing keys are their manifests'.
#[test]
fn manifests_verify_under_both_keys() {
    let data = read("tests/data/manifests.json");
    let manifests = data["manifests"].as_array().unwrap();
    assert_eq!(manifests.len(), 3 + 35 + 6);
    let mut signing_keys = BTreeSet::new();
    for entry in manifests {
        let bytes = from_hex(text(entry, "manifest")).unwrap();
        let manifest = Manifest::from_bytes(&bytes).unwrap();
        assert_eq!(manifest.to_bytes(), bytes);
        assert_eq!(
            hex_upper(manifest.master_key.as_bytes()),
            text(entry, "master")
        );
        assert_eq!(
            manifest.master_key.to_node_public(),
            text(entry, "master_b58")
        );
        let (signing_key, _) = manifest.signing.unwrap();
        assert_eq!(hex_upper(signing_key.as_bytes()), text(entry, "signing"));
        assert_eq!(signing_key.to_node_public(), text(entry, "signing_b58"));
        assert_eq!(u64::from(manifest.sequence), number(entry, "sequence"));
        assert_eq!(manifest.domain.as_deref(), entry["domain"].as_str());
        assert_eq!(
            hex_upper(&manifest.signing_bytes()),
            text(entry, "signed_bytes")
        );
        assert!(manifest.verify());
        assert!(!manifest.is_revocation());
        // Its hash is of all of it, signatures and all.
        assert_eq!(
            manifest.hash(),
            sha512_half(&[b"MAN\0", &bytes[..]].concat())
        );
        // Another sequence, or another domain, is not what its keys signed.
        let later = Manifest {
            sequence: manifest.sequence + 1,
            ..manifest.clone()
        };
        assert!(!later.verify());
        let elsewhere = Manifest {
            domain: Some("example.com".into()),
            ..manifest.clone()
        };
        assert_ne!(elsewhere.domain, manifest.domain);
        assert!(!elsewhere.verify());
        // Nor is a signature by the master key alone: both must sign.
        let (key, _) = manifest.signing.unwrap();
        let one_key = Manifest {
            signing: Some((key, manifest.master_signature)),
            ..manifest.clone()
        };
        assert!(!one_key.verify());
        signing_keys.insert(text(entry, "signing_b58").to_owned());
    }
    let validations = read("tests/data/validations.json");
    for ledger in validations["ledgers"].as_array().unwrap() {
        for validator in ledger["unl"].as_array().unwrap() {
            assert!(signing_keys.contains(text(validator, "signing_b58")));
        }
    }
}

/// The ledger's own serializations: transactions and their metadata, and
/// ledger objects.
fn ledger_blobs() -> Vec<Vec<u8>> {
    let data = read("tests/data/ledger-binary.json");
    data["blobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blob| from_hex(text(blob, "hex")).unwrap())
        .collect()
}

// Every serialization rippled wrote reads in canonical form, nested
// objects and arrays too, and writes back to itself; between them they
// hold every type the captures reached.
#[test]
fn the_ledger_s_serializations_read_and_write_back() {
    let mut seen = BTreeSet::new();
    for blob in ledger_blobs() {
        binary::check(&blob).unwrap();
        let (again, types) = rebuild(&blob).unwrap();
        assert_eq!(again, blob);
        seen.extend(types);
    }
    let expected = [
        Type::UInt16,
        Type::UInt32,
        Type::UInt64,
        Type::Hash128,
        Type::Hash256,
        Type::Amount,
        Type::Blob,
        Type::AccountId,
        Type::Object,
        Type::Array,
        Type::UInt8,
        Type::Hash160,
        Type::PathSet,
        Type::Vector256,
        Type::Issue,
    ];
    for kind in expected {
        assert!(seen.contains(&kind.code()), "{kind:?}");
    }
}

/// Every serialization above, of every kind.
fn samples() -> &'static [Vec<u8>] {
    static SAMPLES: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    SAMPLES.get_or_init(|| {
        let mut samples = ledger_blobs();
        let validations = read("tests/data/validations.json");
        for ledger in validations["ledgers"].as_array().unwrap() {
            for entry in ledger["validations"].as_array().unwrap().iter().take(8) {
                samples.push(from_hex(text(entry, "data")).unwrap());
            }
        }
        let manifests = read("tests/data/manifests.json");
        for entry in manifests["manifests"].as_array().unwrap().iter().take(8) {
            samples.push(from_hex(text(entry, "manifest")).unwrap());
        }
        let headers = read("tests/data/ledger-headers.json");
        for ledger in headers["ledgers"].as_array().unwrap().iter().take(2) {
            samples.push(from_hex(text(ledger, "ledger_data")).unwrap());
        }
        samples
    })
}

/// A change to valid bytes.
#[derive(Clone, Debug)]
enum Mutation {
    /// A byte changed, by a non-zero XOR.
    Flip(usize, u8),
    /// Cut short.
    Cut(usize),
    /// Bytes put in.
    Insert(usize, Vec<u8>),
    /// Bytes taken out.
    Remove(usize, usize),
}

fn mutations() -> impl Strategy<Value = Vec<Mutation>> {
    let mutation = prop_oneof![
        (any::<usize>(), 1..=u8::MAX).prop_map(|(at, mask)| Mutation::Flip(at, mask)),
        any::<usize>().prop_map(Mutation::Cut),
        (any::<usize>(), prop::collection::vec(any::<u8>(), 1..8))
            .prop_map(|(at, bytes)| Mutation::Insert(at, bytes)),
        (any::<usize>(), 1usize..8).prop_map(|(at, count)| Mutation::Remove(at, count)),
    ];
    prop::collection::vec(mutation, 0..4)
}

fn mutate(mut bytes: Vec<u8>, mutations: &[Mutation]) -> Vec<u8> {
    for mutation in mutations {
        let length = bytes.len();
        match mutation {
            Mutation::Flip(at, mask) => {
                if length > 0 {
                    bytes[at % length] ^= mask;
                }
            }
            Mutation::Cut(at) => bytes.truncate(at % (length + 1)),
            Mutation::Insert(at, more) => {
                let at = at % (length + 1);
                bytes.splice(at..at, more.iter().copied());
            }
            Mutation::Remove(at, count) => {
                let at = at % (length + 1);
                bytes.drain(at..(at + count).min(length));
            }
        }
    }
    bytes
}

proptest! {
    // The form is canonical (spec/designs/xrpl-validation.md#8-the-verifier):
    // what reads, from whatever bytes, writes back to exactly those bytes,
    // so no record has a second form that the node would read as the same.
    #[test]
    fn what_reads_writes_back_to_itself(
        sample in any::<prop::sample::Index>(),
        mutations in mutations(),
    ) {
        let samples = samples();
        let bytes = mutate(sample.get(samples).clone(), &mutations);
        if let Ok((again, _)) = rebuild(&bytes) {
            prop_assert_eq!(&again, &bytes);
        }
        if let Ok(validation) = Validation::from_bytes(&bytes) {
            prop_assert_eq!(&validation.to_bytes(), &bytes);
        }
        if let Ok(manifest) = Manifest::from_bytes(&bytes) {
            prop_assert_eq!(&manifest.to_bytes(), &bytes);
        }
        if let Ok(header) = LedgerHeader::from_bytes(&bytes) {
            prop_assert_eq!(header.to_bytes().to_vec(), bytes);
        }
    }

    #[test]
    fn readers_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400), text in "\\PC{0,60}") {
        let _ = binary::check(&bytes);
        let _ = rebuild(&bytes);
        let _ = Validation::from_bytes(&bytes);
        let _ = Manifest::from_bytes(&bytes);
        let _ = LedgerHeader::from_bytes(&bytes);
        let _ = PublicKey::from_node_public(&text);
    }
}
