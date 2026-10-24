//! The UNL from the live ledgers' data
//! (spec/designs/xrpl-validation.md#13-what-pins-it): the publishers'
//! signed lists as their sites served them verify under the built-in keys,
//! and the validations captured of one ledger on each network reach its
//! quorum, with the validators' live manifests and, by the margin they
//! leave, with the lists' own.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;

use dassie_xrpl::header::LedgerHeader;
use dassie_xrpl::keys::PublicKey;
use dassie_xrpl::unl::{
    AnchorError, FRESH_FOR_MS, RIPPLE_EPOCH_UNIX_S, Secp256k1Signatures, Taken, Unl,
    ValidationSignatures, quorum,
};
use dassie_xrpl::validation::Validation;
use dassie_xrpl::validator_list::{
    ListError, MAINNET_FLOOR, Message, RIPPLE, RIPPLE_SITE, TESTNET, TESTNET_FLOOR, UnlConfig,
    XRPLF,
};
use dassie_xrpl::{from_hex, hex_upper};
use proptest::prelude::*;
use serde_json::{Value, json};

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

/// The list bodies of `network`, in the order its config names their
/// publishers.
fn bodies(network: &str) -> Vec<Vec<u8>> {
    let data = read("tests/data/validator-lists.json");
    data["lists"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|list| text(list, "network") == network)
        .map(|list| text(list, "body").as_bytes().to_vec())
        .collect()
}

/// The captured ledger of `network`: its header, its validations, and
/// rippled's quorum for it.
fn ledger(network: &str) -> (LedgerHeader, Vec<Vec<u8>>, Value) {
    let data = read("tests/data/validations.json");
    let ledger = data["ledgers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|ledger| text(ledger, "network") == network)
        .unwrap()
        .clone();
    let json = &ledger["header"];
    let small = |key| u32::try_from(number(json, key)).unwrap();
    let byte = |key| u8::try_from(number(json, key)).unwrap();
    let header = LedgerHeader {
        sequence: u32::try_from(number(&ledger, "ledger_index")).unwrap(),
        total_drops: number(json, "total_coins"),
        parent_hash: hash(json, "parent_hash"),
        transaction_hash: hash(json, "transaction_hash"),
        account_hash: hash(json, "account_hash"),
        parent_close_time: small("parent_close_time"),
        close_time: small("close_time"),
        close_time_resolution: byte("close_time_resolution"),
        close_flags: byte("close_flags"),
    };
    assert_eq!(header.hash(), hash(&ledger, "ledger_hash"));
    let validations = (ledger["validations"].as_array().unwrap().iter())
        .map(|entry| from_hex(text(entry, "data")).unwrap())
        .collect();
    (header, validations, ledger)
}

/// A few seconds after the last validation of `ledger` was signed.
fn soon_after(ledger: &Value) -> u64 {
    let signed = (ledger["validations"].as_array().unwrap().iter())
        .map(|entry| number(entry, "signing_time"))
        .max()
        .unwrap();
    (signed + RIPPLE_EPOCH_UNIX_S) * 1_000 + 5_000
}

/// The UNL of `config` with the lists of `network` taken at `now_ms`.
fn with_lists(config: UnlConfig, network: &str, now_ms: u64) -> Unl {
    let mut unl = Unl::new(config);
    for body in bodies(network) {
        let publisher = Message::parse(&body).unwrap().publisher;
        let taken = unl.take_list(&body, now_ms);
        let trusted = unl
            .config()
            .publishers
            .iter()
            .any(|p| p.master_key == publisher);
        assert_eq!(taken.is_ok(), trusted, "{taken:?}");
        unl.check_invariants();
    }
    unl
}

/// Mainnet's built-in config with Ripple's publisher alone, as an owner
/// may set it.
fn ripple_only() -> UnlConfig {
    UnlConfig::mainnet()
        .with_override(&json!({
            "publishers": [{"key": hex_upper(RIPPLE.as_bytes()), "sites": [RIPPLE_SITE]}],
        }))
        .unwrap()
}

/// The live manifests of `network`'s validators, from the `manifest`
/// method.
fn live_manifests(network: &str) -> Vec<Vec<u8>> {
    let data = read("tests/data/manifests.json");
    let kind = format!("{network} validator");
    (data["manifests"].as_array().unwrap().iter())
        .filter(|entry| text(entry, "kind") == kind)
        .map(|entry| from_hex(text(entry, "manifest")).unwrap())
        .collect()
}

// The lists the publishers serve verify under the keys built in, with the
// sequences and expiries the sites served; mainnet's two name the same 35
// validators, the testnet's 6, and each UNL meets its floor.
#[test]
fn the_publishers_lists_verify_under_the_built_in_keys() {
    let (_, _, mainnet_ledger) = ledger("mainnet");
    let now_ms = soon_after(&mainnet_ledger);
    let mut unl = Unl::new(UnlConfig::mainnet());
    for (body, publisher, sequence) in [
        (&bodies("mainnet")[0], RIPPLE, 85),
        (&bodies("mainnet")[1], XRPLF, 2_026_070_302),
    ] {
        assert_eq!(Message::parse(body).unwrap().publisher, publisher);
        assert_eq!(unl.take_list(body, now_ms), Ok(Taken::Current));
        assert_eq!(unl.take_list(body, now_ms), Ok(Taken::Same));
        unl.check_invariants();
        let index = usize::from(publisher == XRPLF);
        let (current, pending) = unl.lists(index);
        assert_eq!((current.unwrap().sequence, pending), (sequence, None));
        assert_eq!(current.unwrap().validators.len(), 35);
    }
    let (ripple, xrplf) = (unl.lists(0).0.unwrap(), unl.lists(1).0.unwrap());
    assert!(ripple.validators.keys().eq(xrplf.validators.keys()));
    // 2027-04-06T17:51:34Z and 2027-07-03T00:00:00Z.
    assert_eq!(
        (ripple.expiration, xrplf.expiration),
        (860_349_094, 867_888_000)
    );
    let validators = unl.unl(now_ms).unwrap();
    assert_eq!((validators.len(), quorum(validators.len())), (35, 28));
    assert!(validators.len() >= MAINNET_FLOOR);

    let mut testnet = Unl::new(UnlConfig::testnet());
    let body = &bodies("testnet")[0];
    assert_eq!(Message::parse(body).unwrap().publisher, TESTNET);
    assert_eq!(testnet.take_list(body, now_ms), Ok(Taken::Current));
    testnet.check_invariants();
    assert_eq!(testnet.lists(0).0.unwrap().sequence, 59);
    let validators = testnet.unl(now_ms).unwrap();
    assert_eq!((validators.len(), quorum(validators.len())), (6, 5));
    assert!(validators.len() >= TESTNET_FLOOR);
    // The networks' UNLs share no key: each binds its network.
    let mainnet: BTreeSet<PublicKey> = unl.unl(now_ms).unwrap().into_iter().collect();
    assert!(validators.iter().all(|key| !mainnet.contains(key)));
    // A network's lists are not another's.
    assert!(
        Unl::new(UnlConfig::testnet())
            .take_list(&bodies("mainnet")[0], now_ms)
            .is_err()
    );
}

// What the lists carry, kept with the account's state, verifies again at
// a restart and comes back whole.
#[test]
fn the_real_lists_are_kept_and_verify_again() {
    let (_, _, ledger) = ledger("mainnet");
    let now_ms = soon_after(&ledger);
    let mut unl = with_lists(UnlConfig::mainnet(), "mainnet", now_ms);
    for manifest in live_manifests("mainnet") {
        unl.take_manifest(&manifest).unwrap();
    }
    unl.check_invariants();
    let state = unl.state();
    let restored = Unl::restore(UnlConfig::mainnet(), &state, now_ms).unwrap();
    restored.check_invariants();
    assert_eq!(restored, unl);
    assert_eq!(restored.state(), state);
    // Every validator's newest manifest is kept, also one the lists carry.
    assert_eq!(state["manifests"].as_array().unwrap().len(), 35);
    // Kept under Ripple's list alone, which lags behind for five of them,
    // the newer manifests come back all the same.
    let ripple_only = ripple_only();
    let alone = Unl::restore(ripple_only.clone(), &state, now_ms).unwrap();
    alone.check_invariants();
    let fresh = with_lists(ripple_only, "mainnet", now_ms);
    let rotated = (alone.unl(now_ms).unwrap().iter())
        .filter(|master| alone.signing_key(master) != fresh.signing_key(master))
        .count();
    assert_eq!(rotated, 5);
    for master in alone.unl(now_ms).unwrap() {
        assert_eq!(alone.signing_key(&master), unl.signing_key(&master));
    }
}

// spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: five of
// mainnet's validators had rotated their keys past the manifests in
// Ripple's list. With its manifests alone, 30 of the 35 count, two above
// the quorum; with XRPLF's list too, whose manifest of one is newer, 31;
// with the manifests the server's `manifest` method gives, all 35.
#[test]
fn the_captured_mainnet_validations_reach_quorum() {
    let (header, validations, ledger) = ledger("mainnet");
    let now_ms = soon_after(&ledger);
    let anchor = |unl: &Unl| {
        unl.anchor(
            &header,
            validations.iter().map(Vec::as_slice),
            now_ms,
            None,
            &Secp256k1Signatures,
        )
    };
    let alone = with_lists(ripple_only(), "mainnet", now_ms);
    let tally = anchor(&alone).unwrap().tally;
    assert_eq!((tally.counted, tally.quorum), (30, 28));
    assert_eq!(tally.missing.len(), 5);

    let mut unl = with_lists(UnlConfig::mainnet(), "mainnet", now_ms);
    let anchored = anchor(&unl).unwrap();
    assert_eq!(anchored.anchor.hash, hash(&ledger, "ledger_hash"));
    let tally = anchored.tally;
    assert_eq!(
        (tally.counted, tally.quorum, tally.validators),
        (31, 28, 35)
    );
    assert_eq!(
        usize::try_from(number(&ledger, "quorum")).unwrap(),
        tally.quorum
    );
    assert_eq!((tally.stale, tally.refused, tally.conflicting), (0, 0, 0));
    // Every other validation is of a key no listed manifest names: the
    // rotated four, and validators not on the UNL.
    assert_eq!(tally.unknown, validations.len() - 31);
    let missing = tally.missing;
    assert_eq!(missing.len(), 4);
    for manifest in live_manifests("mainnet") {
        unl.take_manifest(&manifest).unwrap();
    }
    unl.check_invariants();
    let tally = anchor(&unl).unwrap().tally;
    assert_eq!((tally.counted, tally.missing.len()), (35, 0));
    assert_eq!(tally.unknown, validations.len() - 35);
    for master in &missing {
        assert!(unl.signing_key(master).is_some());
    }
    // Three minutes on, the same signatures are no longer fresh.
    let later = unl.anchor(
        &header,
        validations.iter().map(Vec::as_slice),
        now_ms - 5_000 + FRESH_FOR_MS,
        None,
        &Secp256k1Signatures,
    );
    let Err(AnchorError::NoQuorum(stale)) = later else {
        panic!("{later:?}");
    };
    assert_eq!((stale.counted, stale.stale), (0, 35));
    // Under the testnet's UNL, mainnet's validations count for nothing.
    let testnet = with_lists(UnlConfig::testnet(), "testnet", now_ms);
    let Err(AnchorError::NoQuorum(none)) = anchor(&testnet) else {
        panic!("no quorum");
    };
    assert_eq!(none.counted, 0);
}

#[test]
fn the_captured_testnet_validations_reach_quorum() {
    let (header, validations, ledger) = ledger("testnet");
    let now_ms = soon_after(&ledger);
    let mut unl = with_lists(UnlConfig::testnet(), "testnet", now_ms);
    let anchor = |unl: &Unl| {
        unl.anchor(
            &header,
            validations.iter().map(Vec::as_slice),
            now_ms,
            None,
            &Secp256k1Signatures,
        )
    };
    let tally = anchor(&unl).unwrap().tally;
    assert_eq!((tally.counted, tally.quorum, tally.validators), (6, 5, 6));
    assert_eq!(
        usize::try_from(number(&ledger, "quorum")).unwrap(),
        tally.quorum
    );
    for manifest in live_manifests("testnet") {
        unl.take_manifest(&manifest).unwrap();
    }
    assert_eq!(anchor(&unl).unwrap().tally.counted, 6);
}

/// Counts the signatures it checks.
#[derive(Default)]
struct Counting(std::cell::Cell<usize>);

impl ValidationSignatures for Counting {
    fn signed(&self, validation: &Validation) -> bool {
        self.0.set(self.0.get() + 1);
        Secp256k1Signatures.signed(validation)
    }
}

// What a lying server or list site can make of the real data: a list
// under another publisher's name, the real validations under a header of
// its own, and a validation of the ledger forged in the name of a
// validator of the UNL, which then only withholds that validator.
#[test]
fn lies_from_real_data_count_for_nothing() {
    let (header, validations, ledger) = ledger("mainnet");
    let now_ms = soon_after(&ledger);
    // Ripple's list as XRPLF's: XRPLF's manifest, Ripple's blob.
    let mut swapped = Message::parse(&bodies("mainnet")[1]).unwrap();
    let ripple = Message::parse(&bodies("mainnet")[0]).unwrap();
    swapped.blobs = ripple.blobs.clone();
    let mut unl = Unl::new(UnlConfig::mainnet());
    assert_eq!(
        unl.take_list(&swapped.to_json(), now_ms),
        Err(ListError::BadSignature)
    );
    // And under Ripple's key, with XRPLF's manifest.
    let mut renamed = Message::parse(&bodies("mainnet")[1]).unwrap();
    renamed.publisher = RIPPLE;
    assert_eq!(
        unl.take_list(&renamed.to_json(), now_ms),
        Err(ListError::BadManifest)
    );
    unl.check_invariants();
    assert_eq!((unl.lists(0), unl.lists(1)), ((None, None), (None, None)));

    let mut unl = with_lists(UnlConfig::mainnet(), "mainnet", now_ms);
    for manifest in live_manifests("mainnet") {
        unl.take_manifest(&manifest).unwrap();
    }
    // The real validations under another header: no quorum, and not one
    // signature checked.
    let mut made_up = header;
    made_up.account_hash[0] ^= 1;
    let checks = Counting::default();
    let anchored = unl.anchor(
        &made_up,
        validations.iter().map(Vec::as_slice),
        now_ms,
        None,
        &checks,
    );
    let Err(AnchorError::NoQuorum(tally)) = anchored else {
        panic!("{anchored:?}");
    };
    assert_eq!((tally.counted, checks.0.get()), (0, 0));
    // Each validation of the UNL's validators changed in its signature,
    // and sent before the real one: those validators are withheld, at one
    // signature check each.
    let unl_validations: Vec<Vec<u8>> = (validations.iter())
        .filter(|bytes| {
            let validation = Validation::from_bytes(bytes).unwrap();
            unl.master_of(&validation.signing_key).is_some()
        })
        .cloned()
        .collect();
    assert_eq!(unl_validations.len(), 35);
    let forged: Vec<Vec<u8>> = (unl_validations.iter().take(8))
        .map(|bytes| {
            let mut validation = Validation::from_bytes(bytes).unwrap();
            validation.signing_time += 1;
            validation.to_bytes()
        })
        .collect();
    let checks = Counting::default();
    let sent = forged.iter().chain(&validations).map(Vec::as_slice);
    let anchored = unl.anchor(&header, sent, now_ms, None, &checks);
    let Err(AnchorError::NoQuorum(tally)) = anchored else {
        panic!("{anchored:?}");
    };
    assert_eq!((tally.counted, tally.refused, tally.quorum), (27, 8, 28));
    assert_eq!(checks.0.get(), 35);
}

/// A byte of a list body changed, or the body cut short.
fn mutate(mut body: Vec<u8>, at: usize, byte: u8, cut: bool) -> Vec<u8> {
    let at = at % body.len();
    if cut {
        body.truncate(at);
    } else {
        body[at] = byte;
    }
    body
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // What a site answers, however changed, never panics the UNL, and
    // what it takes leaves its invariants holding.
    #[test]
    fn changed_lists_never_break_the_unl(
        list in 0usize..3,
        at in any::<usize>(),
        byte in any::<u8>(),
        cut in any::<bool>(),
    ) {
        let (_, _, ledger) = ledger("mainnet");
        let now_ms = soon_after(&ledger);
        let (network, config) = match list {
            2 => ("testnet", UnlConfig::testnet()),
            _ => ("mainnet", UnlConfig::mainnet()),
        };
        let mut unl = Unl::new(config);
        let body = mutate(bodies(network)[list % 2].clone(), at, byte, cut);
        let _ = unl.take_list(&body, now_ms);
        unl.check_invariants();
        let _ = unl.take_manifest(&body);
        let restored = Unl::restore(unl.config().clone(), &unl.state(), now_ms);
        prop_assert_eq!(restored.as_ref(), Some(&unl));
    }
}
