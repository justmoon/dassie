//! The live proof servers' answers, as the node reads them
//! (spec/designs/xrpl-validation.md#13-what-pins-it): captured from
//! `mainnet.xrpl.dassieland.net` and `testnet.xrpl.dassieland.net`, which
//! run the patch in `resources/proof-server/patches`, they read in the
//! proof API's shapes (spec/designs/xrpl-validation.md#3-the-proof-api),
//! and prove end to end: each header hashes to its ledger, a quorum of the
//! network's real UNL validated it, branches lead to its state and
//! transaction roots, from objects read from their leaves and from a
//! transaction's leaf the node builds from `tx` in binary, and keys it does
//! not hold are proven absent both ways. An account anchors on them as it
//! would on the live server.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_settlement::{AssetEffect, ExternalAsset, LedgerInput};
use dassie_xrpl::account::{Doubt, HISTORY_BOUND, XrplAccount};
use dassie_xrpl::anchor::{ProofError, prove_state, prove_transaction};
use dassie_xrpl::api::{state_proof_result, tx_binary_result, tx_proof_result, validations_result};
use dassie_xrpl::header::LedgerHeader;
use dassie_xrpl::network::{MAINNET, Network, TESTNET};
use dassie_xrpl::records::{self, account_root, fee_settings, ripple_state};
use dassie_xrpl::shamap::{BranchError, Tree, held_object, leaf_node, transaction_item};
use dassie_xrpl::skip_list::{SkipList, recent_key};
use dassie_xrpl::unl::{RIPPLE_EPOCH_UNIX_S, Secp256k1Signatures, Unl};
use dassie_xrpl::validation::Validation;
use dassie_xrpl::validator_list::{Message, UnlConfig};
use dassie_xrpl::{AccountId, Wallet, base64, from_hex};
use serde_json::{Value, json};

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

fn hash(value: &Value, key: &str) -> [u8; 32] {
    from_hex(text(value, key)).unwrap().try_into().unwrap()
}

/// The captured answers of `network`.
fn captured(network: &str) -> Value {
    let data = read("tests/data/proof-api.json");
    (data["ledgers"].as_array().unwrap().iter())
        .find(|ledger| text(ledger, "network") == network)
        .unwrap()
        .clone()
}

fn network(name: &str) -> &'static Network {
    if name == "mainnet" {
        &MAINNET
    } else {
        &TESTNET
    }
}

const NETWORKS: [&str; 2] = ["mainnet", "testnet"];

/// The captured ledger's header: the same bytes in every answer, hashing
/// to the ledger's hash.
fn header(ledger: &Value) -> LedgerHeader {
    let bytes = text(&ledger["ledger"]["ledger"], "ledger_data");
    for answer in ["anchor", "state_proof"] {
        assert_eq!(text(&ledger[answer], "ledger_header"), bytes, "{answer}");
    }
    let header = LedgerHeader::from_bytes(&from_hex(bytes).unwrap()).unwrap();
    assert_eq!(header.hash(), hash(&ledger["anchor"], "ledger_hash"));
    assert_eq!(header.hash(), hash(&ledger["ledger"], "ledger_hash"));
    header
}

/// The signed lists of `network`'s publishers, as their sites served them.
fn list_bodies(network: &str) -> Vec<Vec<u8>> {
    let data = read("tests/data/validator-lists.json");
    (data["lists"].as_array().unwrap().iter())
        .filter(|list| text(list, "network") == network)
        .map(|list| text(list, "body").as_bytes().to_vec())
        .collect()
}

/// The live manifests of `network`'s validators, by their masters, from
/// the `manifest` method.
fn live_manifests(network: &str) -> Vec<(String, Vec<u8>)> {
    let data = read("tests/data/manifests.json");
    let kind = format!("{network} validator");
    (data["manifests"].as_array().unwrap().iter())
        .filter(|entry| text(entry, "kind") == kind)
        .map(|entry| {
            let manifest = from_hex(text(entry, "manifest")).unwrap();
            (text(entry, "master_b58").to_owned(), manifest)
        })
        .collect()
}

/// A few seconds after the last of `validations` was signed.
fn soon_after(validations: &[Vec<u8>]) -> u64 {
    let signed = (validations.iter())
        .map(|bytes| Validation::from_bytes(bytes).unwrap().signing_time)
        .max()
        .unwrap();
    (u64::from(signed) + RIPPLE_EPOCH_UNIX_S) * 1_000 + 5_000
}

/// The keys of a `state_proof` answer's entries.
fn keys_of(answer: &Value) -> Vec<[u8; 32]> {
    (answer["entries"].as_array().unwrap().iter())
        .map(|entry| hash(entry, "key"))
        .collect()
}

// The answers have the proof API's final shapes: no bytes beside a branch,
// a transaction's branch without its leaf, and validations not cut.
#[test]
fn the_live_answers_have_the_proof_api_s_shapes() {
    let names = |value: &Value| {
        let mut names: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        names.retain(|name| *name != "status");
        names.join(",")
    };
    for network in NETWORKS {
        let ledger = captured(network);
        assert_eq!(
            names(&ledger["tx_proof"]),
            "ledger_hash,ledger_header,ledger_index,path,validated"
        );
        for answer in ["anchor", "state_proof"] {
            assert_eq!(
                names(&ledger[answer]),
                "entries,ledger_hash,ledger_header,ledger_index,validated"
            );
            for entry in ledger[answer]["entries"].as_array().unwrap() {
                assert_eq!(names(entry), "key,path");
            }
        }
        assert_eq!(names(&ledger["validations"]), "ledger_hash,validations");
        // `tx` in binary at API version 1: the transaction under `tx`, its
        // metadata under `meta`.
        assert!(ledger["tx"]["tx"].is_string() && ledger["tx"]["meta"].is_string());
        // The proof's branch starts at the inner node above the leaf.
        for node in ledger["tx_proof"]["path"].as_array().unwrap() {
            assert!(node.as_str().unwrap().starts_with("4D494E00"), "MIN\\0");
        }
    }
}

// spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers: the
// anchor's validations reach the quorum of each network's real UNL, from
// the lists its publishers serve and the validators' live manifests.
#[test]
fn a_quorum_of_the_real_unl_validated_each_anchor() {
    for network in NETWORKS {
        let ledger = captured(network);
        let header = header(&ledger);
        let answer = validations_result(&ledger["validations"]).unwrap();
        assert!(!answer.truncated);
        assert_eq!(hash(&ledger["validations"], "ledger_hash"), header.hash());
        let now_ms = soon_after(&answer.validations);
        let mut unl = Unl::new(UnlConfig::for_network(self::network(network).id).unwrap());
        for body in list_bodies(network) {
            unl.take_list(&body, now_ms).unwrap();
        }
        for (_, manifest) in live_manifests(network) {
            let _ = unl.take_manifest(&manifest);
        }
        let anchored = unl
            .anchor(
                &header,
                answer.validations.iter().map(Vec::as_slice),
                now_ms,
                None,
                &Secp256k1Signatures,
            )
            .unwrap_or_else(|error| panic!("{network}: {error:?}"));
        assert_eq!(anchored.anchor.hash, header.hash());
        assert_eq!(anchored.anchor.index, header.sequence);
        let tally = anchored.tally;
        // Mainnet's 35 validators, of whom 34 signed the anchor in time; the
        // testnet's 6, all of them. Every other validation is by a key no
        // listed manifest names.
        let expected = if network == "mainnet" {
            (34, 28, 35)
        } else {
            (6, 5, 6)
        };
        assert_eq!(
            (tally.counted, tally.quorum, tally.validators),
            expected,
            "{network}"
        );
        assert_eq!(tally.unknown, answer.validations.len() - tally.counted);
        assert_eq!(
            (tally.stale, tally.refused, tally.conflicting),
            (0, 0, 0),
            "{network}"
        );
    }
}

// spec/designs/xrpl-validation.md#4-what-is-proven-and-how: the state
// proofs prove, at the anchor, each object from the leaf its branch starts
// at, and each key the state does not hold, by an empty slot or by another
// key's leaf.
#[test]
fn state_proofs_prove_objects_from_their_leaves_and_absences_both_ways() {
    for network in NETWORKS {
        let ledger = captured(network);
        let header = header(&ledger);
        let at = header.hash();
        // The anchoring round's: the skip list, the test account's root,
        // which no ledger holds, and the fee settings.
        let anchor = state_proof_result(&ledger["anchor"]).unwrap();
        let keys = keys_of(&ledger["anchor"]);
        let proven = prove_state(&anchor, &keys, None).unwrap();
        assert_eq!(proven.hash, at);
        let test_account = Wallet::from_entropy(&[7; 32]).account();
        assert_eq!(
            keys,
            [
                recent_key(),
                records::account_key(test_account),
                records::fees_key()
            ]
        );
        assert_eq!(proven.object(&keys[1]), Some(None), "{network}");
        let fees = fee_settings(proven.object(&keys[2]).unwrap().unwrap()).unwrap();
        assert!(fees.base_fee > 0 && fees.reserve_base > 0, "{network}");
        // The skip list holds the parent's hash.
        let recent = SkipList::parse(proven.object(&recent_key()).unwrap().unwrap()).unwrap();
        assert_eq!(
            recent.hash_of(header.sequence - 1, 1),
            Some(header.parent_hash)
        );
        // At the anchor by its hash: an account root, a trust line, and
        // three keys no ledger holds.
        let answer = state_proof_result(&ledger["state_proof"]).unwrap();
        let keys = keys_of(&ledger["state_proof"]);
        let proven = prove_state(&answer, &keys, Some(&at)).unwrap();
        let root = account_root(proven.object(&keys[1]).unwrap().unwrap()).unwrap();
        assert_eq!(
            root.account,
            AccountId::parse(text(&ledger, "tx_account")).unwrap()
        );
        let (line, _) = ripple_state(proven.object(&keys[2]).unwrap().unwrap()).unwrap();
        let holder = AccountId::parse(text(&ledger["line"], "holder")).unwrap();
        let issuer = AccountId::parse(text(&ledger["line"], "issuer")).unwrap();
        assert!(line.is_between(holder, issuer, line.currency), "{network}");
        assert_eq!(records::line_key(holder, issuer, line.currency), keys[2]);
        assert_eq!(proven.absences(), 3);
        let (mut slots, mut others) = (0, 0);
        for (entry, key) in answer.entries.iter().zip(&keys).skip(3) {
            assert_eq!(proven.object(key), Some(None));
            assert_eq!(held_object(key, &entry.path), None);
            if entry.path[0].starts_with(b"MLN\0") {
                others += 1;
            } else {
                slots += 1;
            }
        }
        assert!(slots > 0 && others > 0, "{network}: {slots} {others}");
        // Another anchor's hash, or an absence claimed for a key held,
        // proves nothing.
        assert_eq!(
            prove_state(&answer, &keys, Some(&[0; 32])),
            Err(ProofError::Ledger)
        );
        let mut false_absence = answer.clone();
        false_absence.entries[1].path.remove(0);
        assert_eq!(
            prove_state(&false_absence, &keys, Some(&at)),
            Err(ProofError::Branch(BranchError::Length))
        );
        let mut altered = answer.clone();
        let leaf = &mut altered.entries[2].path[0];
        let at_balance = leaf.len() / 2;
        leaf[at_balance] ^= 1;
        assert_eq!(
            prove_state(&altered, &keys, Some(&at)),
            Err(ProofError::Branch(BranchError::Hash))
        );
    }
}

// spec/designs/xrpl-validation.md#3-the-proof-api: a transaction proves by
// the leaf the node builds from its bytes and metadata, from `tx` in
// binary, with `tx_proof`'s branch above it to its ledger's transaction
// root; other bytes, or a leaf the server sent, prove nothing.
#[test]
fn a_transaction_proves_from_the_leaf_the_node_builds() {
    for network in NETWORKS {
        let ledger = captured(network);
        let header = header(&ledger);
        let tx_hash = hash(&ledger["tx"], "hash");
        let blobs = tx_binary_result(&ledger["tx"]).unwrap();
        let proof = tx_proof_result(&ledger["tx_proof"]).unwrap();
        let proven = prove_transaction(&proof, &blobs, &tx_hash, &[]).unwrap();
        // The capture took it from the anchor's ledger.
        assert_eq!(proven.ledger_hash, header.hash(), "{network}");
        assert_eq!(proven.header.transaction_hash, header.transaction_hash);
        assert_eq!(proven.decoded.record.hash, tx_hash);
        assert_eq!(
            proven.decoded.record.account,
            AccountId::parse(text(&ledger, "tx_account")).unwrap()
        );
        assert_eq!(proven.decoded.record.ledger_index, header.sequence);
        // Changed metadata fails the branch; changed bytes the hash.
        let mut other = blobs.clone();
        let last = other.meta.len() - 2;
        other.meta[last] ^= 1;
        assert_eq!(
            prove_transaction(&proof, &other, &tx_hash, &[]),
            Err(ProofError::Branch(BranchError::Hash))
        );
        let mut other = blobs.clone();
        other.tx[10] ^= 1;
        assert_eq!(
            prove_transaction(&proof, &other, &tx_hash, &[]),
            Err(ProofError::Transaction)
        );
        // A leaf in the branch is a node too many, even the right one.
        let mut with_leaf = proof.clone();
        let leaf = leaf_node(
            Tree::Transactions,
            &tx_hash,
            &transaction_item(&blobs.tx, &blobs.meta),
        );
        with_leaf.path.insert(0, leaf);
        assert_eq!(
            prove_transaction(&with_leaf, &blobs, &tx_hash, &[]),
            Err(ProofError::Branch(BranchError::Length))
        );
    }
}

/// What the live server of `network` answered `request` with, as
/// captured: `None` for what an anchor does not need.
fn live_answer(network: &str, request: &Value) -> Option<Value> {
    let ledger = captured(network);
    let header = header(&ledger);
    let answer = match text(request, "command") {
        "server_state" => json!({ "state": {
            "network_id": self::network(network).id,
            "validated_ledger": {
                "seq": header.sequence,
                "close_time": header.close_time,
                "base_fee": 10, "reserve_base": 1_000_000, "reserve_inc": 200_000,
            },
        }}),
        "subscribe" => json!({}),
        "state_proof" => {
            // The account asks what the capture asked, at the validated
            // ledger.
            assert_eq!(request["ledger_index"], "validated");
            let keys: Vec<Value> = (ledger["anchor"]["entries"].as_array().unwrap().iter())
                .map(|entry| entry["key"].clone())
                .collect();
            assert_eq!(request["keys"], Value::from(keys));
            ledger["anchor"].clone()
        }
        "validations" => {
            assert_eq!(request["ledger_hash"], ledger["anchor"]["ledger_hash"]);
            ledger["validations"].clone()
        }
        "manifest" => {
            let asked = text(request, "public_key");
            let manifests = live_manifests(network);
            match manifests.iter().find(|(master, _)| master == asked) {
                Some((_, manifest)) => {
                    json!({ "requested": asked, "manifest": base64::encode(manifest) })
                }
                None => json!({ "requested": asked }),
            }
        }
        _ => return None,
    };
    Some(answer)
}

/// The list a site of `network`'s publishers served.
fn served_list(network: &str, url: &str) -> Vec<u8> {
    let config = UnlConfig::for_network(self::network(network).id).unwrap();
    let publisher = (config.publishers.iter())
        .find(|publisher| publisher.sites.iter().any(|site| site == url))
        .unwrap();
    list_bodies(network)
        .into_iter()
        .find(|body| Message::parse(body).unwrap().publisher == publisher.master_key)
        .unwrap()
}

/// A number xrpld's config `cfg` sets: the value of `key = …`, or the
/// line after the section `[key]`.
fn cfg_number(cfg: &str, key: &str) -> u32 {
    let lines: Vec<&str> = cfg.lines().map(str::trim).collect();
    let section = format!("[{key}]");
    let value = (lines.iter().enumerate())
        .find_map(|(at, line)| {
            if *line == section {
                return lines.get(at + 1).copied();
            }
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim())
        })
        .unwrap_or_else(|| panic!("{key}"));
    value.parse().unwrap_or_else(|_| panic!("{key}: {value}"))
}

// spec/areas/proof-servers.md#the-servers: the proof servers keep more
// history than the account's bound, even right after xrpld deleted its
// oldest ledgers and holds `online_delete` of them, with a margin for the
// ledgers that close while an account walks. So within the bound, a
// default server's `lgrNotFound` is never its retention, and code and
// configs cannot drift apart.
#[test]
fn the_proof_servers_keep_more_than_the_history_bound() {
    let margin = HISTORY_BOUND / 10;
    for network in NETWORKS {
        let path = format!(
            "{}/../../resources/proof-server/xrpld-{network}.cfg",
            manifest_dir()
        );
        let cfg = std::fs::read_to_string(&path).unwrap();
        for key in ["online_delete", "ledger_history"] {
            let kept = cfg_number(&cfg, key);
            assert!(
                kept >= HISTORY_BOUND + margin,
                "{network}: {key} {kept} keeps no more than the bound {HISTORY_BOUND} and {margin}"
            );
        }
    }
}

/// An account of `network`, of entropy [7; 32], driven by the captured
/// answers as a node drives it, its clock a few seconds after the last
/// validation was signed: the answers it asks for, and the publishers'
/// lists from their sites. With `short`, a `validations` request without
/// `untrusted` gets one validation alone, as from a server whose lists lag
/// the account's. Also each `validations` request it sent.
fn anchor_on_captured(network: &str, short: bool) -> (XrplAccount, Vec<Value>) {
    let on = self::network(network);
    let ledger = captured(network);
    let validations = validations_result(&ledger["validations"]).unwrap();
    let now = soon_after(&validations.validations);
    let mut account = XrplAccount::new(on.first(), &[on.xrp_scheme], "", &[7; 32], &[]).unwrap();
    let mut asked = Vec::new();
    let mut validations_asked = Vec::new();
    account.on_ledger(now, LedgerInput::Connected);
    for _ in 0..64 {
        account.told();
        account.check_invariants();
        let mut effects = Vec::new();
        account.take_effects(&mut effects);
        if effects.is_empty() {
            break;
        }
        for effect in effects {
            match effect {
                AssetEffect::Send { message } => {
                    let request: Value = serde_json::from_slice(&message).unwrap();
                    asked.push(text(&request, "command").to_owned());
                    let Some(mut result) = live_answer(network, &request) else {
                        continue;
                    };
                    if text(&request, "command") == "validations" {
                        if short && request.get("untrusted").is_none() {
                            result["validations"].as_array_mut().unwrap().truncate(1);
                        }
                        validations_asked.push(request.clone());
                    }
                    let reply = json!({
                        "type": "response", "id": request["id"], "status": "success",
                        "result": result,
                    });
                    let reply = serde_json::to_vec(&reply).unwrap();
                    account.on_ledger(now, LedgerInput::Message(&reply));
                }
                AssetEffect::Fetch { url, .. } => {
                    let body = served_list(network, &url);
                    let fetched = LedgerInput::Fetched {
                        url: &url,
                        body: Some(&body),
                    };
                    account.on_ledger(now, fetched);
                }
                _ => {}
            }
        }
    }
    assert_eq!(
        asked[..4],
        ["server_state", "subscribe", "state_proof", "validations"],
        "{network}"
    );
    (account, validations_asked)
}

// spec/designs/xrpl-validation.md#6-the-account-on-proofs: an account
// anchors on the live servers' answers end to end: the network checked,
// the lists taken, the round's state proof read from its leaves and its
// ledger validated by a quorum of the real UNL. Its root, which no ledger
// holds, is proven absent there, and it lies about nothing.
#[test]
fn an_account_anchors_on_the_live_answers() {
    for network in NETWORKS {
        let ledger = captured(network);
        let header = header(&ledger);
        let (account, validations_asked) = anchor_on_captured(network, false);
        // With the network's publishers, the server's own validators are
        // the account's: it asks for no others
        // (spec/designs/xrpl-validation.md#3-the-proof-api).
        assert_eq!(validations_asked.len(), 1, "{network}");
        assert_eq!(validations_asked[0].get("untrusted"), None, "{network}");
        let anchor = account
            .anchor()
            .unwrap_or_else(|| panic!("{network}: no anchor, waiting: {:?}", account.waiting()));
        assert_eq!(anchor.hash, header.hash(), "{network}");
        assert_eq!(anchor.index, header.sequence);
        assert_eq!(account.validated(), header.sequence);
        for doubt in Doubt::ALL {
            assert_eq!(account.doubts(doubt), 0, "{network}: {doubt:?}");
        }
    }
}

// spec/designs/xrpl-validation.md#3-the-proof-api: the server's own
// validators may lag the account's, as when its lists are older. An answer
// of them short of a quorum is asked again with `untrusted` before it is
// taken as a lie, and the account anchors on the whole answer.
#[test]
fn an_answer_of_the_servers_validators_short_of_a_quorum_is_asked_again() {
    for network in NETWORKS {
        let ledger = captured(network);
        let header = header(&ledger);
        let (account, validations_asked) = anchor_on_captured(network, true);
        assert_eq!(validations_asked.len(), 2, "{network}");
        assert_eq!(validations_asked[0].get("untrusted"), None, "{network}");
        assert_eq!(validations_asked[1]["untrusted"], true, "{network}");
        let anchor = account
            .anchor()
            .unwrap_or_else(|| panic!("{network}: no anchor, waiting: {:?}", account.waiting()));
        assert_eq!(anchor.hash, header.hash(), "{network}");
        for doubt in Doubt::ALL {
            assert_eq!(account.doubts(doubt), 0, "{network}: {doubt:?}");
        }
    }
}

/// The package's directory, as cargo and nextest name it to a test they
/// run, never `env!`'s from when the test compiled: a build is reused in
/// other checkouts of the repository, whose files are their own.
#[allow(clippy::disallowed_methods, clippy::expect_used)] // What the test runner says, not configuration.
fn manifest_dir() -> String {
    std::env::var("CARGO_MANIFEST_DIR").expect("cargo and nextest name the package's directory")
}
