//! The fake ledger's proofs (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation),
//! checked as a node checks them: an honest server's answers prove its
//! validated ledger by a quorum of the UNL its publisher lists, its state
//! and the absence of what it does not hold by branches to that ledger's
//! state root, older ledgers by its skip list, and transactions by branches
//! to their ledgers' transaction roots; and every lie a server can tell
//! fails one of those checks, or, for history left out, is found by
//! walking what the proven state names.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_xrpl::binary::{FieldId, Fields, Type};
use dassie_xrpl::codec::{Amount, Payment, Signing, TokenValue, Transact, TrustSet};
use dassie_xrpl::fake::{
    FEE_BASE, FIRST_LEDGER, FakeLedger, KeyedSignatures, Lie, MAX_STATE_PROOF_KEYS, Token,
    UNL_FLOOR, VALIDATORS,
};
use dassie_xrpl::header::LedgerHeader;
use dassie_xrpl::keys::{AccountId, sha512_half};
use dassie_xrpl::manifest::Manifest;
use dassie_xrpl::shamap::{BranchError, Leaf, Tree, verify};
use dassie_xrpl::unl::{
    Anchor, AnchorError, Anchored, RIPPLE_EPOCH_UNIX_S, Relation, Secp256k1Signatures, Unl,
    ValidationSignatures,
};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::{Wallet, base64, from_hex, hex_upper};
use serde_json::{Value, json};

const HONEST: u64 = 1;
const LIAR: u64 = 2;

const LAST_LEDGER_SEQUENCE: FieldId = FieldId::new(Type::UInt32, 27);
const PREVIOUS_TXN_LGR_SEQ: FieldId = FieldId::new(Type::UInt32, 5);
const PREVIOUS_TXN_ID: FieldId = FieldId::new(Type::Hash256, 5);
const LEDGER_INDEX: FieldId = FieldId::new(Type::Hash256, 6);
const HASHES: FieldId = FieldId::new(Type::Vector256, 2);
const BALANCE: FieldId = FieldId::new(Type::Amount, 2);
const TRANSACTION_RESULT: FieldId = FieldId::new(Type::UInt8, 3);
const AFFECTED_NODES: FieldId = FieldId::new(Type::Array, 8);

/// What a server answered: its result, or the error it gave.
fn ask(ledger: &mut FakeLedger, connection: u64, request: &Value) -> Result<Value, String> {
    let reply = ledger.request(connection, &serde_json::to_vec(request).unwrap());
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    if reply["status"] == "success" {
        Ok(reply["result"].clone())
    } else {
        Err(reply["error"].as_str().unwrap().to_owned())
    }
}

fn bytes(value: &Value) -> Vec<u8> {
    from_hex(value.as_str().unwrap()).unwrap()
}

fn key(space: u8, parts: &[&[u8]]) -> [u8; 32] {
    let mut message = vec![0, space];
    for part in parts {
        message.extend_from_slice(part);
    }
    sha512_half(&message)
}

fn root_key(account: AccountId) -> [u8; 32] {
    key(b'a', &[&account.0])
}

fn skip_key() -> [u8; 32] {
    key(b's', &[])
}

fn fees_key() -> [u8; 32] {
    key(b'e', &[])
}

/// The top-level field `id` of a serialization.
fn field(bytes: &[u8], id: FieldId) -> Option<Vec<u8>> {
    Fields::new(bytes)
        .map(Result::unwrap)
        .find(|field| field.id == id)
        .map(|field| field.value.to_vec())
}

/// Now, as the node's clock has it: a few seconds after the last ledger
/// closed.
fn now_ms(ledger: &FakeLedger) -> u64 {
    let close = ledger.header(ledger.ledger()).unwrap().close_time;
    (u64::from(close) + RIPPLE_EPOCH_UNIX_S) * 1_000 + 3_000
}

/// The UNL a node that names the fake's publisher keeps, with the list
/// the publisher's site serves.
fn unl(ledger: &FakeLedger) -> Unl {
    let config = UnlConfig::for_network(ledger.network().id)
        .unwrap()
        .with_override(&json!({
            "publishers": [{
                "key": hex_upper(ledger.publisher_key().as_bytes()),
                "sites": ["https://localhost/vl"],
            }],
            "floor": UNL_FLOOR,
        }))
        .unwrap();
    let mut unl = Unl::new(config);
    unl.take_list(&ledger.validator_list(), now_ms(ledger))
        .unwrap();
    unl
}

/// Why an answer proves nothing.
#[derive(Debug)]
enum Caught {
    /// The server answered with an error.
    Refused(String),
    /// The ledger is no anchor.
    Anchor(AnchorError),
    /// A branch does not hold.
    Branch(BranchError),
    /// A ledger no anchor proves.
    Unproven,
}

/// What a state proof at the validated ledger proved: the anchor, and
/// each key's object, or its absence.
struct Proven {
    anchored: Anchored,
    header: LedgerHeader,
    objects: Vec<Option<Vec<u8>>>,
}

/// Anchors on what `connection` says is the validated ledger, as a node
/// does: a state proof of `keys` there, its header's validations, a quorum
/// of `unl`, and each branch to the header's state root.
fn anchor(
    ledger: &mut FakeLedger,
    connection: u64,
    keys: &[[u8; 32]],
    (unl, kept, signatures): (&Unl, Option<&Anchor>, &dyn ValidationSignatures),
) -> Result<Proven, Caught> {
    let keys_hex: Vec<String> = keys.iter().map(|key| hex_upper(key)).collect();
    let request =
        json!({ "command": "state_proof", "keys": keys_hex, "ledger_index": "validated" });
    let proof = ask(ledger, connection, &request).map_err(Caught::Refused)?;
    let header = LedgerHeader::from_bytes(&bytes(&proof["ledger_header"])).unwrap();
    let request = json!({ "command": "validations", "ledger_hash": hex_upper(&header.hash()) });
    let validations = ask(ledger, connection, &request).map_err(Caught::Refused)?;
    let validations: Vec<Vec<u8>> = (validations["validations"].as_array().unwrap().iter())
        .map(bytes)
        .collect();
    let now = now_ms(ledger);
    let anchored = unl
        .anchor(
            &header,
            validations.iter().map(Vec::as_slice),
            now,
            kept,
            signatures,
        )
        .map_err(Caught::Anchor)?;
    let mut objects = Vec::new();
    for (key, entry) in keys.iter().zip(proof["entries"].as_array().unwrap()) {
        assert_eq!(entry["key"], hex_upper(key));
        let path: Vec<Vec<u8>> = entry["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(bytes)
            .collect();
        let object = entry.get("node_binary").map(bytes);
        let leaf = object
            .as_deref()
            .map_or(Leaf::Absent(Tree::State), Leaf::State);
        verify(&header.account_hash, key, leaf, &path).map_err(Caught::Branch)?;
        objects.push(object);
    }
    Ok(Proven {
        anchored,
        header,
        objects,
    })
}

/// The ledgers a proven skip list proves, by index, with the anchor.
fn proven_hashes(proven: &Proven, skip_list: &[u8]) -> BTreeMap<u32, [u8; 32]> {
    let last = u32::from_be_bytes(
        field(skip_list, LAST_LEDGER_SEQUENCE)
            .unwrap()
            .try_into()
            .unwrap(),
    );
    let hashes = field(skip_list, HASHES).unwrap();
    let hashes = hashes.as_chunks::<32>().0;
    let first = last + 1 - u32::try_from(hashes.len()).unwrap();
    let mut proven_hashes: BTreeMap<u32, [u8; 32]> =
        (first..=last).zip(hashes.iter().copied()).collect();
    proven_hashes.insert(proven.header.sequence, proven.header.hash());
    proven_hashes
}

/// The transaction of `tx_hash` and its metadata, if `connection` proves
/// them in a ledger of `proven`.
fn prove_tx(
    ledger: &mut FakeLedger,
    connection: u64,
    tx_hash: &[u8; 32],
    proven: &BTreeMap<u32, [u8; 32]>,
) -> Result<(Vec<u8>, Vec<u8>, u32), Caught> {
    let request = json!({ "command": "tx_proof", "tx_hash": hex_upper(tx_hash) });
    let proof = ask(ledger, connection, &request).map_err(Caught::Refused)?;
    let header = LedgerHeader::from_bytes(&bytes(&proof["ledger_header"])).unwrap();
    let index = u32::try_from(proof["ledger_index"].as_u64().unwrap()).unwrap();
    if index != header.sequence || proven.get(&index) != Some(&header.hash()) {
        return Err(Caught::Unproven);
    }
    let (transaction, metadata) = (bytes(&proof["tx_blob"]), bytes(&proof["meta_blob"]));
    let path: Vec<Vec<u8>> = proof["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(bytes)
        .collect();
    let leaf = Leaf::Transaction {
        transaction: &transaction,
        metadata: &metadata,
    };
    verify(&header.transaction_hash, tx_hash, leaf, &path).map_err(Caught::Branch)?;
    Ok((transaction, metadata, index))
}

/// Pays `drops` from `wallet` to `to`, and closes the ledger it applies in.
fn pay(ledger: &mut FakeLedger, wallet: &Wallet, to: AccountId, drops: u64) -> [u8; 32] {
    let sequence = ledger.sequence(wallet.account()).unwrap();
    let signed = Payment {
        account: wallet.account(),
        destination: to,
        destination_tag: None,
        amount: Amount::Xrp(drops),
        send_max: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(wallet);
    let submit = json!({ "command": "submit", "tx_blob": hex_upper(&signed.blob) });
    let result = ask(ledger, HONEST, &submit).unwrap();
    assert_eq!(result["engine_result"], "tesSUCCESS");
    ledger.close();
    signed.hash
}

/// A ledger with an account that was funded and paid three times, and
/// more than the old anchor's ledgers closed since.
fn scene() -> (FakeLedger, Wallet, Vec<[u8; 32]>) {
    let mut ledger = FakeLedger::new();
    let (wallet, other) = (Wallet::from_seed([3; 16]), Wallet::from_seed([4; 16]));
    ledger.deposit(wallet.account(), 100_000_000);
    ledger.deposit(other.account(), 100_000_000);
    ledger.close();
    let payments = (1..=3)
        .map(|drops| pay(&mut ledger, &wallet, other.account(), drops * 1_000_000))
        .collect();
    for _ in 0..70 {
        ledger.close();
    }
    (ledger, wallet, payments)
}

#[test]
fn an_honest_server_proves_its_validated_ledger_its_state_and_an_absence() {
    let (mut ledger, wallet, _) = scene();
    let unl = unl(&ledger);
    let stranger = Wallet::from_seed([9; 16]).account();
    let keys = [
        root_key(wallet.account()),
        root_key(stranger),
        skip_key(),
        fees_key(),
    ];
    let proven = anchor(
        &mut ledger,
        HONEST,
        &keys,
        (&unl, None, &Secp256k1Signatures),
    )
    .unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS);
    assert_eq!(proven.header.sequence, ledger.ledger());
    assert_eq!(
        Some(proven.header.hash()),
        ledger.ledger_hash(ledger.ledger())
    );
    // The account's root holds its XRP; the stranger has none.
    let root = proven.objects[0].as_ref().unwrap();
    let balance = u64::from_be_bytes(field(root, BALANCE).unwrap().try_into().unwrap());
    assert_eq!(
        balance & !0x4000_0000_0000_0000,
        ledger.balance(wallet.account()).unwrap()
    );
    assert!(proven.objects[1].is_none());
    // The skip list proves the hash of each ledger since the chain began,
    // fewer than 256.
    let hashes = proven_hashes(&proven, proven.objects[2].as_ref().unwrap());
    let since = ledger.ledger() - FIRST_LEDGER + 1;
    assert_eq!(hashes.len(), usize::try_from(since).unwrap());
    for (index, hash) in &hashes {
        assert_eq!(ledger.ledger_hash(*index), Some(*hash), "{index}");
    }
    assert!(proven.objects[3].is_some());
}

#[test]
fn each_ledger_names_its_parent() {
    let (ledger, _, _) = scene();
    for index in ledger.ledger() - 50..=ledger.ledger() {
        let header = ledger.header(index).unwrap();
        assert_eq!(Some(header.hash()), ledger.ledger_hash(index));
        assert_eq!(Some(header.parent_hash), ledger.ledger_hash(index - 1));
        let parent = ledger.header(index - 1).unwrap();
        assert_eq!(header.parent_close_time, parent.close_time);
        assert!(header.close_time > parent.close_time);
    }
}

// The fake mainnet has validators and a publisher of its own, as live: a
// node that names the testnet's publisher anchors on no mainnet ledger.
#[test]
fn the_fake_networks_share_no_validator_key() {
    let (mut testnet, mut mainnet) = (FakeLedger::new(), FakeLedger::mainnet());
    testnet.close();
    mainnet.close();
    assert_ne!(testnet.publisher_key(), mainnet.publisher_key());
    let testnet_keys = testnet.validator_keys();
    assert!(
        mainnet
            .validator_keys()
            .iter()
            .all(|key| !testnet_keys.contains(key))
    );
    let signatures = &Secp256k1Signatures;
    let wrong = unl(&testnet);
    let caught = anchor(
        &mut mainnet,
        HONEST,
        &[fees_key()],
        (&wrong, None, signatures),
    );
    assert!(matches!(caught, Err(Caught::Anchor(_))));
    let right = unl(&mainnet);
    let proven = anchor(
        &mut mainnet,
        HONEST,
        &[fees_key()],
        (&right, None, signatures),
    )
    .unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS);
}

#[test]
fn a_transaction_proves_in_its_ledger_through_the_skip_list() {
    let (mut ledger, wallet, payments) = scene();
    let unl = unl(&ledger);
    let keys = [root_key(wallet.account()), skip_key()];
    let proven = anchor(
        &mut ledger,
        HONEST,
        &keys,
        (&unl, None, &Secp256k1Signatures),
    )
    .unwrap();
    let hashes = proven_hashes(&proven, proven.objects[1].as_ref().unwrap());
    for payment in &payments {
        let (transaction, metadata, _) = prove_tx(&mut ledger, HONEST, payment, &hashes).unwrap();
        assert_eq!(dassie_xrpl::codec::hash(&transaction), *payment);
        assert_eq!(field(&metadata, TRANSACTION_RESULT), Some(vec![0]));
    }
    // The fake's own deposit, unsigned, is a transaction of its ledger as
    // much: the walk from the account's root reaches it.
    let walked = walk(&mut ledger, HONEST, &proven, wallet.account(), &hashes);
    assert_eq!(walked.len(), 4);
    assert_eq!(
        walked[..3],
        payments.iter().rev().copied().collect::<Vec<_>>()[..]
    );
}

#[test]
fn a_ledger_past_the_skip_list_is_two_state_proofs_from_the_anchor() {
    let mut ledger = FakeLedger::new();
    let (wallet, other) = (Wallet::from_seed([3; 16]), Wallet::from_seed([4; 16]));
    ledger.deposit(wallet.account(), 100_000_000);
    ledger.deposit(other.account(), 100_000_000);
    ledger.close();
    let old = pay(&mut ledger, &wallet, other.account(), 1_000_000);
    let paid_in = ledger.ledger();
    for _ in 0..600 {
        ledger.close();
    }
    let unl = unl(&ledger);
    // The flag ledger after the payment's, older than the recent list.
    let flag = (paid_in / 256 + 1) * 256;
    assert!(flag + 256 < ledger.ledger());
    let flags_key = key(b's', &[&(flag >> 16).to_be_bytes()]);
    let keys = [skip_key(), flags_key];
    let proven = anchor(
        &mut ledger,
        HONEST,
        &keys,
        (&unl, None, &Secp256k1Signatures),
    )
    .unwrap();
    let flags = proven.objects[1].as_ref().unwrap();
    let last = u32::from_be_bytes(
        field(flags, LAST_LEDGER_SEQUENCE)
            .unwrap()
            .try_into()
            .unwrap(),
    );
    let hashes = field(flags, HASHES).unwrap();
    let hashes = hashes.as_chunks::<32>().0;
    let at = |index: usize| last - 256 * u32::try_from(hashes.len() - 1 - index).unwrap();
    let flag_hashes: BTreeMap<u32, [u8; 32]> = (0..hashes.len())
        .map(|index| (at(index), hashes[index]))
        .collect();
    // The second proof, at the flag ledger: its own skip list.
    let request =
        json!({ "command": "state_proof", "keys": [hex_upper(&skip_key())], "ledger_index": flag });
    let proof = ask(&mut ledger, HONEST, &request).unwrap();
    let header = LedgerHeader::from_bytes(&bytes(&proof["ledger_header"])).unwrap();
    assert_eq!(flag_hashes.get(&flag), Some(&header.hash()));
    let entry = &proof["entries"][0];
    let skip_list = bytes(&entry["node_binary"]);
    let path: Vec<Vec<u8>> = entry["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(bytes)
        .collect();
    verify(
        &header.account_hash,
        &skip_key(),
        Leaf::State(&skip_list),
        &path,
    )
    .unwrap();
    let at_flag = Proven {
        anchored: proven.anchored.clone(),
        header,
        objects: Vec::new(),
    };
    let hashes = proven_hashes(&at_flag, &skip_list);
    let (_, _, index) = prove_tx(&mut ledger, HONEST, &old, &hashes).unwrap();
    assert_eq!(index, paid_in);
}

/// The transactions that changed `account`'s root, newest first, as the
/// root proven at the anchor names the newest and each one's metadata the
/// one before, each proven by `tx_proof`.
fn walk(
    ledger: &mut FakeLedger,
    connection: u64,
    proven: &Proven,
    account: AccountId,
    hashes: &BTreeMap<u32, [u8; 32]>,
) -> Vec<[u8; 32]> {
    let root = proven.objects[0].as_ref().unwrap();
    walk_object(ledger, connection, (&root_key(account), root), hashes)
}

/// The transactions that changed the object under `key`, newest first,
/// from `object` as proven.
fn walk_object(
    ledger: &mut FakeLedger,
    connection: u64,
    (key, object): (&[u8; 32], &[u8]),
    hashes: &BTreeMap<u32, [u8; 32]>,
) -> Vec<[u8; 32]> {
    let key = *key;
    let mut next = field(object, PREVIOUS_TXN_ID).map(|id| <[u8; 32]>::try_from(id).unwrap());
    let mut walked = Vec::new();
    while let Some(tx_hash) = next.filter(|id| *id != [0; 32]) {
        assert!(walked.len() < 64, "a bounded walk");
        let (_, metadata, _) = prove_tx(ledger, connection, &tx_hash, hashes).unwrap();
        walked.push(tx_hash);
        next = previous_of(&metadata, &key);
    }
    walked
}

/// The transaction that changed the object under `key` before the one
/// `metadata` is of, as its affected node names it.
fn previous_of(metadata: &[u8], key: &[u8; 32]) -> Option<[u8; 32]> {
    let nodes = Fields::new(metadata)
        .map(Result::unwrap)
        .find(|field| field.id == AFFECTED_NODES)?;
    for node in nodes.elements().unwrap() {
        let node = node.unwrap();
        let fields: Vec<_> = node.fields().unwrap().map(Result::unwrap).collect();
        let index = fields.iter().find(|f| f.id == LEDGER_INDEX)?;
        if index.value == key {
            let previous = fields.iter().find(|f| f.id == PREVIOUS_TXN_ID)?;
            return previous.value.try_into().ok();
        }
    }
    None
}

#[test]
fn a_simulation_s_keyed_validations_check_only_under_its_secret() {
    let (mut ledger, wallet, _) = scene();
    ledger.sign_validations_with([7; 32]);
    ledger.close();
    let unl = unl(&ledger);
    let keys = [root_key(wallet.account())];
    let keyed = KeyedSignatures::new([7; 32]);
    let proven = anchor(&mut ledger, HONEST, &keys, (&unl, None, &keyed)).unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS);
    for signatures in [
        &Secp256k1Signatures as &dyn ValidationSignatures,
        &KeyedSignatures::new([8; 32]),
    ] {
        let caught = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures));
        assert!(matches!(
            caught,
            Err(Caught::Anchor(AnchorError::NoQuorum(_)))
        ));
    }
}

#[test]
fn a_rotated_key_counts_once_its_manifest_is_taken() {
    let (mut ledger, wallet, _) = scene();
    let mut unl = unl(&ledger);
    ledger.rotate_validator(0);
    ledger.close();
    let keys = [root_key(wallet.account())];
    let signatures = &Secp256k1Signatures;
    let proven = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS - 1);
    let missing = proven.anchored.tally.missing[0];
    let request = json!({ "command": "manifest", "public_key": missing.to_node_public() });
    let answer = ask(&mut ledger, HONEST, &request).unwrap();
    assert_eq!(answer["details"]["seq"], 2);
    let manifest = base64::decode(answer["manifest"].as_str().unwrap()).unwrap();
    assert!(Manifest::from_bytes(&manifest).unwrap().verify());
    assert_eq!(unl.take_manifest(&manifest), Ok(true));
    let proven = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS);
    // The next list carries it.
    ledger.publish_validator_list(u32::MAX, None);
    let fresh = self::unl(&ledger);
    let proven = anchor(&mut ledger, HONEST, &keys, (&fresh, None, signatures)).unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS);
    // A key the fake does not know is its own master and signing key.
    let stranger = Wallet::from_seed([9; 16]).public_key().to_node_public();
    let request = json!({ "command": "manifest", "public_key": stranger });
    let answer = ask(&mut ledger, HONEST, &request).unwrap();
    assert_eq!(answer["details"]["master_key"], stranger);
    assert!(answer.get("manifest").is_none());
}

#[test]
fn a_withheld_quorum_validates_nothing_and_a_revoked_key_signs_nothing() {
    let (mut ledger, wallet, _) = scene();
    let unl = unl(&ledger);
    let keys = [root_key(wallet.account())];
    let signatures = &Secp256k1Signatures;
    ledger.withhold_validator(1, true);
    ledger.close();
    let proven = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS - 1);
    ledger.withhold_validator(2, true);
    ledger.close();
    let caught = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures));
    assert!(matches!(
        caught,
        Err(Caught::Anchor(AnchorError::NoQuorum(_)))
    ));
    ledger.withhold_validator(1, false);
    ledger.withhold_validator(2, false);
    ledger.revoke_validator(3);
    ledger.close();
    let master = ledger.validator_keys()[3];
    let request = json!({ "command": "manifest", "public_key": master.to_node_public() });
    let answer = ask(&mut ledger, HONEST, &request).unwrap();
    let manifest = base64::decode(answer["manifest"].as_str().unwrap()).unwrap();
    assert!(Manifest::from_bytes(&manifest).unwrap().is_revocation());
    assert!(answer["details"].get("ephemeral_key").is_none());
    let proven = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS - 1);
}

#[test]
fn a_reset_is_a_new_chain_the_same_validators_validate() {
    let (mut ledger, wallet, _) = scene();
    let unl = unl(&ledger);
    let keys = [root_key(wallet.account())];
    let signatures = &Secp256k1Signatures;
    let before = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
    ledger.reset();
    ledger.close();
    let after = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
    assert!(after.header.sequence < before.header.sequence);
    assert!(after.header.close_time > before.header.close_time);
    assert_eq!(
        after.anchored.anchor.relation(&before.anchored.anchor),
        Relation::Diverged
    );
    // The account is gone with the chain, and proven so.
    assert!(after.objects[0].is_none());
    assert_eq!(
        ledger.header(ledger.ledger() - 1).unwrap().parent_hash,
        [0; 32]
    );
    // A reset that keys new validators validates nothing the old list
    // names, until the new list is taken.
    ledger.rekey_validators(u32::MAX);
    ledger.close();
    let caught = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures));
    assert!(matches!(
        caught,
        Err(Caught::Anchor(AnchorError::NoQuorum(ref tally))) if tally.unknown == VALIDATORS
    ));
    let fresh = self::unl(&ledger);
    let proven = anchor(&mut ledger, HONEST, &keys, (&fresh, None, signatures)).unwrap();
    assert_eq!(proven.anchored.tally.counted, VALIDATORS);
}

#[test]
fn proofs_are_bounded_and_end_with_the_history() {
    let (mut ledger, _, payments) = scene();
    let too_many: Vec<String> = (0..=MAX_STATE_PROOF_KEYS)
        .map(|n| hex_upper(&[u8::try_from(n).unwrap(); 32]))
        .collect();
    let request = json!({ "command": "state_proof", "keys": too_many });
    assert_eq!(
        ask(&mut ledger, HONEST, &request),
        Err("invalidParams".into())
    );
    let request =
        json!({ "command": "state_proof", "keys": [hex_upper(&fees_key())], "ledger_index": 1 });
    assert_eq!(
        ask(&mut ledger, HONEST, &request),
        Err("lgrNotFound".into())
    );
    let request =
        json!({ "command": "tx_proof", "tx_hash": hex_upper(&payments[0]), "ledger_index": 1 });
    assert_eq!(
        ask(&mut ledger, HONEST, &request),
        Err("lgrNotFound".into())
    );
    ledger.forget_history();
    let request = json!({ "command": "tx_proof", "tx_hash": hex_upper(&payments[0]) });
    assert_eq!(
        ask(&mut ledger, HONEST, &request),
        Err("txnNotFound".into())
    );
    // The validated ledger is still served.
    let request = json!({ "command": "state_proof", "keys": [hex_upper(&fees_key())] });
    assert!(ask(&mut ledger, HONEST, &request).is_ok());
}

/// What `lie` is caught by, against what an honest connection proves.
fn catch(lie: Lie) {
    let (mut ledger, wallet, payments) = scene();
    let unl = unl(&ledger);
    let signatures = &Secp256k1Signatures;
    let keys = [root_key(wallet.account()), skip_key(), fees_key()];
    let honest = anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
    let hashes = proven_hashes(&honest, honest.objects[1].as_ref().unwrap());
    ledger.set_lie(LIAR, Some(lie));
    let account_tx = json!({ "command": "account_tx", "account": wallet.account().to_address() });
    let listed = |answer: &Value| -> Vec<[u8; 32]> {
        (answer["transactions"].as_array().unwrap().iter())
            .map(|entry| bytes(&entry["tx"]["hash"]).try_into().unwrap())
            .collect()
    };
    match lie {
        Lie::Transaction => catch_made_up_transactions(&mut ledger, &wallet, &hashes),
        Lie::HistoryNewestLeftOut | Lie::HistoryMiddleLeftOut => {
            let lied = listed(&ask(&mut ledger, LIAR, &account_tx).unwrap());
            let walked = walk(&mut ledger, LIAR, &honest, wallet.account(), &hashes);
            assert!(walked.iter().any(|hash| !lied.contains(hash)));
            assert_eq!(
                walked[..3],
                payments.iter().rev().copied().collect::<Vec<_>>()[..]
            );
        }
        Lie::OldLedgerHint => {
            let lied = ask(&mut ledger, LIAR, &account_tx).unwrap();
            let named = lied["transactions"][0]["tx"]["ledger_index"]
                .as_u64()
                .unwrap();
            let root = honest.objects[0].as_ref().unwrap();
            let proven = field(root, PREVIOUS_TXN_LGR_SEQ).unwrap();
            let proven = u32::from_be_bytes(proven.try_into().unwrap());
            assert!(named < u64::from(proven));
            let caught = prove_tx(&mut ledger, LIAR, &payments[2], &hashes);
            assert!(matches!(caught, Err(Caught::Refused(ref error)) if error == "lgrNotFound"));
        }
        Lie::WrongHeader => {
            // The header's bytes are not those of the hash it names.
            let request = json!({ "command": "state_proof", "keys": [hex_upper(&keys[0])] });
            let lied = ask(&mut ledger, LIAR, &request).unwrap();
            let header = LedgerHeader::from_bytes(&bytes(&lied["ledger_header"])).unwrap();
            assert_ne!(hex_upper(&header.hash()), lied["ledger_hash"]);
            let caught = anchor(&mut ledger, LIAR, &keys, (&unl, None, signatures));
            assert!(anchoring_catches(lie, &caught), "{:?}", caught.err());
        }
        Lie::OlderAnchor => {
            let kept = Some(&honest.anchored.anchor);
            let caught = anchor(&mut ledger, LIAR, &keys, (&unl, kept, signatures));
            assert!(
                matches!(caught, Err(Caught::Anchor(AnchorError::Older))),
                "{:?}",
                caught.err()
            );
        }
        _ => {
            let caught = anchor(&mut ledger, LIAR, &keys, (&unl, None, signatures));
            assert!(
                anchoring_catches(lie, &caught),
                "{lie:?}: {:?}",
                caught.err()
            );
        }
    }
    assert!(ledger.lies_told(lie) > 0, "{lie:?} told");
    // The liar's lies are its own: the honest connection still proves.
    anchor(&mut ledger, HONEST, &keys, (&unl, None, signatures)).unwrap();
}

/// A Payment no ledger holds, listed and proven against a header no anchor
/// proves; and a transaction of the account's that never applied, said to
/// have succeeded.
fn catch_made_up_transactions(
    ledger: &mut FakeLedger,
    wallet: &Wallet,
    hashes: &BTreeMap<u32, [u8; 32]>,
) {
    let account_tx = json!({ "command": "account_tx", "account": wallet.account().to_address() });
    let listed = |answer: Value| -> Vec<[u8; 32]> {
        (answer["transactions"].as_array().unwrap().iter())
            .map(|entry| bytes(&entry["tx"]["hash"]).try_into().unwrap())
            .collect()
    };
    let honest = listed(ask(ledger, HONEST, &account_tx).unwrap());
    let lied = listed(ask(ledger, LIAR, &account_tx).unwrap());
    let made_up = *lied.iter().find(|hash| !honest.contains(hash)).unwrap();
    let caught = prove_tx(ledger, LIAR, &made_up, hashes);
    assert!(matches!(caught, Err(Caught::Unproven)), "{caught:?}");
    let sequence = ledger.sequence(wallet.account()).unwrap();
    let never = submit_payment(ledger, wallet, sequence);
    ledger.set_sequence(wallet.account(), sequence + 1);
    ledger.close();
    let caught = prove_tx(ledger, LIAR, &never, hashes);
    assert!(matches!(caught, Err(Caught::Unproven)), "{caught:?}");
    let honest = prove_tx(ledger, HONEST, &never, hashes);
    assert!(matches!(honest, Err(Caught::Refused(_))));
}

/// Whether anchoring on a liar's validated ledger caught `lie` for what it
/// is.
fn anchoring_catches(lie: Lie, caught: &Result<Proven, Caught>) -> bool {
    let short = |counted: fn(&dassie_xrpl::unl::Tally) -> usize| matches!(caught, Err(Caught::Anchor(AnchorError::NoQuorum(tally))) if counted(tally) == VALIDATORS);
    match lie {
        // The branch ends at the leaf the tree holds, which is not what the
        // answer says.
        Lie::AlteredObject | Lie::OtherChainSkipList | Lie::FalseAbsence => {
            matches!(caught, Err(Caught::Branch(BranchError::Leaf)))
        }
        // Another key's path parts from this one's.
        Lie::OtherBranch => matches!(
            caught,
            Err(Caught::Branch(
                BranchError::Hash | BranchError::Leaf | BranchError::Shape
            ))
        ),
        Lie::Refusal | Lie::NoProofs => matches!(caught, Err(Caught::Refused(_))),
        // Each validation counts for nothing, for what it is.
        Lie::OldAnchor => short(|tally| tally.stale),
        Lie::ForgedValidations => short(|tally| tally.refused),
        Lie::ReplayedValidations | Lie::MadeUpReset => short(|tally| tally.elsewhere),
        _ => matches!(caught, Err(Caught::Anchor(AnchorError::NoQuorum(_)))),
    }
}

/// Submits a Payment of one drop from `wallet` with `sequence`; its hash.
fn submit_payment(ledger: &mut FakeLedger, wallet: &Wallet, sequence: u32) -> [u8; 32] {
    let signed = Payment {
        account: wallet.account(),
        destination: Wallet::from_seed([4; 16]).account(),
        destination_tag: None,
        amount: Amount::Xrp(1),
        send_max: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(wallet);
    let submit = json!({ "command": "submit", "tx_blob": hex_upper(&signed.blob) });
    assert_eq!(
        ask(ledger, HONEST, &submit).unwrap()["engine_result"],
        "tesSUCCESS"
    );
    signed.hash
}

#[test]
fn every_lie_is_caught() {
    for lie in Lie::ALL {
        catch(lie);
    }
}

#[test]
fn a_lie_names_itself() {
    for lie in Lie::ALL {
        assert_eq!(Lie::by_name(lie.name()), Some(lie));
    }
    assert_eq!(Lie::by_name("truth"), None);
}

#[test]
fn a_state_proof_has_the_proof_api_s_shape() {
    let (mut ledger, wallet, _) = scene();
    let latest = ledger.ledger();
    let header = ledger.header(latest).unwrap();
    let stranger = Wallet::from_seed([9; 16]).account();
    let keys = [
        hex_upper(&root_key(wallet.account())),
        hex_upper(&root_key(stranger)),
    ];
    // Naming no ledger is naming the newest validated one.
    let proof = ask(
        &mut ledger,
        HONEST,
        &json!({ "command": "state_proof", "keys": keys }),
    )
    .unwrap();
    assert_eq!(proof["ledger_index"], latest);
    assert_eq!(proof["ledger_hash"], hex_upper(&header.hash()));
    assert_eq!(proof["ledger_header"], hex_upper(&header.to_bytes()));
    assert_eq!(proof["validated"], true);
    let entries = proof["entries"].as_array().unwrap();
    assert_eq!(entries[0]["key"], keys[0]);
    assert!(entries[0]["node_binary"].is_string());
    // An absent key's entry has its branch and no object.
    assert_eq!(entries[1]["key"], keys[1]);
    let absent = entries[1].as_object().unwrap();
    assert_eq!(
        absent.keys().map(String::as_str).collect::<Vec<_>>(),
        ["key", "path"]
    );
    // Each node of a branch as rippled hashes it, deepest first: the root
    // is an inner node that hashes to the header's state root.
    let path = entries[0]["path"].as_array().unwrap();
    assert!(bytes(&path[0]).starts_with(b"MLN\0"));
    let root = bytes(path.last().unwrap());
    assert!(root.starts_with(b"MIN\0") && root.len() == 4 + 16 * 32);
    assert_eq!(sha512_half(&root), header.account_hash);
    // A ledger by its hash.
    let older = ledger.header(latest - 3).unwrap();
    let by_hash = json!({
        "command": "state_proof", "keys": keys, "ledger_hash": hex_upper(&older.hash()),
    });
    assert_eq!(
        ask(&mut ledger, HONEST, &by_hash).unwrap()["ledger_index"],
        latest - 3
    );
    // At most so many distinct keys, and a ledger named once.
    let twice = json!({ "command": "state_proof", "keys": [keys[0], keys[0]] });
    assert_eq!(
        ask(&mut ledger, HONEST, &twice),
        Err("invalidParams".into())
    );
    let both = json!({
        "command": "state_proof", "keys": keys, "ledger_index": latest,
        "ledger_hash": hex_upper(&header.hash()),
    });
    assert_eq!(ask(&mut ledger, HONEST, &both), Err("invalidParams".into()));
}

#[test]
fn a_transaction_s_proof_has_the_proof_api_s_shape() {
    let (mut ledger, _, payments) = scene();
    let header = ledger.header(ledger.ledger()).unwrap();
    // Found without its ledger, by its ledger's index, or by its ledger's
    // hash.
    let found = ask(
        &mut ledger,
        HONEST,
        &json!({ "command": "tx_proof", "tx_hash": hex_upper(&payments[0]) }),
    )
    .unwrap();
    let index = u32::try_from(found["ledger_index"].as_u64().unwrap()).unwrap();
    let in_ledger = ledger.header(index).unwrap();
    assert_eq!(found["ledger_hash"], hex_upper(&in_ledger.hash()));
    assert_eq!(found["ledger_header"], hex_upper(&in_ledger.to_bytes()));
    assert_eq!(found["validated"], true);
    let path = found["path"].as_array().unwrap();
    assert!(bytes(&path[0]).starts_with(b"SND\0"));
    for named in [
        json!({ "ledger_index": index }),
        json!({ "ledger_hash": hex_upper(&in_ledger.hash()) }),
    ] {
        let mut request = json!({ "command": "tx_proof", "tx_hash": hex_upper(&payments[0]) });
        request
            .as_object_mut()
            .unwrap()
            .extend(named.as_object().unwrap().clone());
        assert_eq!(ask(&mut ledger, HONEST, &request).unwrap(), found);
    }
    let elsewhere = json!({
        "command": "tx_proof", "tx_hash": hex_upper(&payments[0]),
        "ledger_hash": hex_upper(&header.hash()),
    });
    assert_eq!(
        ask(&mut ledger, HONEST, &elsewhere),
        Err("txnNotFound".into())
    );
    let unknown = json!({
        "command": "tx_proof", "tx_hash": hex_upper(&payments[0]),
        "ledger_hash": hex_upper(&[7; 32]),
    });
    assert_eq!(
        ask(&mut ledger, HONEST, &unknown),
        Err("lgrNotFound".into())
    );
}

#[test]
fn an_honest_server_holds_validations_for_ten_minutes() {
    let (mut ledger, _, _) = scene();
    let validations_of = |ledger: &mut FakeLedger, index: u32| {
        let hash = ledger.ledger_hash(index).unwrap();
        let request = json!({ "command": "validations", "ledger_hash": hex_upper(&hash) });
        let answer = ask(ledger, HONEST, &request).unwrap();
        assert_eq!(answer["ledger_hash"], hex_upper(&hash));
        answer["validations"].as_array().unwrap().len()
    };
    let first = ledger.ledger();
    assert_eq!(validations_of(&mut ledger, first), VALIDATORS);
    // Ten minutes of ledgers, and one more: the first one's are gone.
    for _ in 0..=600 / 4 {
        ledger.close();
    }
    assert_eq!(validations_of(&mut ledger, first), 0);
    assert_eq!(validations_of(&mut ledger, first + 1), VALIDATORS);
    // A ledger it does not know gives none.
    let request = json!({ "command": "validations", "ledger_hash": hex_upper(&[7; 32]) });
    let answer = ask(&mut ledger, HONEST, &request).unwrap();
    assert_eq!(answer["validations"], json!([]));
}

// History left out of a line: an issuer's payment changes the holder's
// line and not its root, so walking the root never meets it, and walking
// the line, proven at the anchor, does.
#[test]
fn history_left_out_of_a_line_is_found_by_walking_the_line() {
    let mut ledger = FakeLedger::new();
    let wallet = Wallet::from_seed([3; 16]);
    ledger.deposit(wallet.account(), 100_000_000);
    ledger.close();
    let token = Token::DSUSD;
    let trust = TrustSet {
        account: wallet.account(),
        limit: token.amount(TokenValue::new(false, 1_000, 0).unwrap()),
        fee: FEE_BASE,
        sequence: ledger.sequence(wallet.account()).unwrap(),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(&wallet);
    let submit = json!({ "command": "submit", "tx_blob": hex_upper(&trust.blob) });
    assert_eq!(
        ask(&mut ledger, HONEST, &submit).unwrap()["engine_result"],
        "tesSUCCESS"
    );
    ledger.close();
    ledger
        .issue(
            wallet.account(),
            token.amount(TokenValue::new(false, 5, 0).unwrap()),
        )
        .unwrap();
    ledger.close();
    ledger.close();
    let (low, high) = if wallet.account() < token.issuer {
        (wallet.account(), token.issuer)
    } else {
        (token.issuer, wallet.account())
    };
    let line_key = key(b'r', &[&low.0, &high.0, &token.currency.0]);
    let unl = unl(&ledger);
    let keys = [root_key(wallet.account()), skip_key(), line_key];
    let proven = anchor(
        &mut ledger,
        HONEST,
        &keys,
        (&unl, None, &Secp256k1Signatures),
    )
    .unwrap();
    let hashes = proven_hashes(&proven, proven.objects[1].as_ref().unwrap());
    ledger.set_lie(LIAR, Some(Lie::HistoryNewestLeftOut));
    let account_tx = json!({ "command": "account_tx", "account": wallet.account().to_address() });
    let lied: Vec<[u8; 32]> = ask(&mut ledger, LIAR, &account_tx).unwrap()["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| bytes(&entry["tx"]["hash"]).try_into().unwrap())
        .collect();
    let by_root = walk(&mut ledger, LIAR, &proven, wallet.account(), &hashes);
    let line = proven.objects[2].as_ref().unwrap();
    let by_line = walk_object(&mut ledger, LIAR, (&line_key, line), &hashes);
    // The newest change of the line is the issuer's payment: the liar left
    // it out, and only the line's history names it.
    let newest = by_line[0];
    assert!(!lied.contains(&newest));
    assert!(!by_root.contains(&newest));
    assert_eq!(by_line.len(), 2);
    assert_eq!(by_line[1], trust.hash);
    assert!(ledger.lies_told(Lie::HistoryNewestLeftOut) > 0);
}
