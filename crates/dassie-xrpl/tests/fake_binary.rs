//! What the fake ledger proves is what it says
//! (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation):
//! for every kind of transaction and ledger object it writes, the binary
//! form a proof carries reads, field by field, as the JSON the fake's other
//! methods give. Each is read here with names and forms of its own, apart
//! from the fake's, so neither can drift from the other unseen.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;

use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::binary::{Field, FieldId, Fields, Type};
use dassie_xrpl::codec::{
    AccountSet, Amount, Currency, Payment, SetRegularKey, SignerEntry, SignerListSet, Signing,
    TF_SET_NO_RIPPLE, TokenAmount, TokenValue, Transact, Transaction, TrustSet,
};
use dassie_xrpl::fake::{FEE_BASE, FakeLedger};
use dassie_xrpl::header::LedgerHeader;
use dassie_xrpl::keys::{AccountId, sha512_half};
use dassie_xrpl::shamap::{held_object, verify_transaction};
use dassie_xrpl::{Wallet, from_hex, hex_upper};
use serde_json::{Value, json};

const CONNECTION: u64 = 1;

/// Each field the fake writes: its name, type code and field code, as
/// rippled's definitions have them.
const FIELDS: &[(&str, u8, u8)] = &[
    ("TransactionResult", 16, 3),
    ("LedgerEntryType", 1, 1),
    ("TransactionType", 1, 2),
    ("SignerWeight", 1, 3),
    ("TradingFee", 1, 5),
    ("Flags", 2, 2),
    ("Sequence", 2, 4),
    ("PreviousTxnLgrSeq", 2, 5),
    ("TransferRate", 2, 11),
    ("OwnerCount", 2, 13),
    ("DestinationTag", 2, 14),
    ("LastLedgerSequence", 2, 27),
    ("TransactionIndex", 2, 28),
    ("SetFlag", 2, 33),
    ("ClearFlag", 2, 34),
    ("SignerQuorum", 2, 35),
    ("SignerListID", 2, 38),
    ("OwnerNode", 3, 4),
    ("LowNode", 3, 7),
    ("HighNode", 3, 8),
    ("PreviousTxnID", 5, 5),
    ("LedgerIndex", 5, 6),
    ("AMMID", 5, 14),
    ("Amount", 6, 1),
    ("Balance", 6, 2),
    ("LimitAmount", 6, 3),
    ("LowLimit", 6, 6),
    ("HighLimit", 6, 7),
    ("Fee", 6, 8),
    ("SendMax", 6, 9),
    ("Amount2", 6, 11),
    ("BaseFeeDrops", 6, 22),
    ("ReserveBaseDrops", 6, 23),
    ("ReserveIncrementDrops", 6, 24),
    ("LPTokenBalance", 6, 31),
    ("SigningPubKey", 7, 3),
    ("TxnSignature", 7, 4),
    ("MemoType", 7, 12),
    ("MemoData", 7, 13),
    ("Account", 8, 1),
    ("Destination", 8, 3),
    ("RegularKey", 8, 8),
    ("Hashes", 19, 2),
    ("Asset", 24, 3),
    ("Asset2", 24, 4),
    ("CreatedNode", 14, 3),
    ("DeletedNode", 14, 4),
    ("ModifiedNode", 14, 5),
    ("PreviousFields", 14, 6),
    ("FinalFields", 14, 7),
    ("NewFields", 14, 8),
    ("Memo", 14, 10),
    ("SignerEntry", 14, 11),
    ("Signer", 14, 16),
    ("Signers", 15, 3),
    ("SignerEntries", 15, 4),
    ("AffectedNodes", 15, 8),
    ("Memos", 15, 9),
];

/// Codes rippled names in JSON.
const ENTRY_TYPES: &[(u16, &str)] = &[
    (0x61, "AccountRoot"),
    (0x72, "RippleState"),
    (0x53, "SignerList"),
    (0x79, "AMM"),
    (0x68, "LedgerHashes"),
    (0x73, "FeeSettings"),
];
const TRANSACTION_TYPES: &[(u16, &str)] = &[
    (0, "Payment"),
    (3, "AccountSet"),
    (5, "SetRegularKey"),
    (12, "SignerListSet"),
    (20, "TrustSet"),
    (30, "Clawback"),
    (0xFFFF, "AMMSet"),
];
const RESULTS: &[(u8, &str)] = &[
    (0, "tesSUCCESS"),
    (101, "tecPATH_PARTIAL"),
    (104, "tecUNFUNDED_PAYMENT"),
    (124, "tecNO_DST"),
    (125, "tecNO_DST_INSUF_XRP"),
    (128, "tecPATH_DRY"),
];

fn name(id: FieldId) -> &'static str {
    let found = FIELDS
        .iter()
        .find(|(_, kind, code)| *kind == id.kind().code() && *code == id.code());
    found.unwrap_or_else(|| panic!("{id:?}")).0
}

fn coded<T: PartialEq + Copy + std::fmt::Debug>(table: &[(T, &'static str)], code: T) -> Value {
    let found = table.iter().find(|(known, _)| *known == code);
    Value::from(found.unwrap_or_else(|| panic!("{code:?}")).1)
}

fn token(amount: &TokenAmount) -> Value {
    json!({
        "currency": amount.currency.to_string(),
        "issuer": amount.issuer.to_address(),
        "value": amount.value.to_string(),
    })
}

/// A field that holds no others, as rippled's JSON writes it.
fn leaf(field: &Field<'_>) -> Value {
    let bytes = field.value;
    match (field.id.kind(), name(field.id)) {
        (Type::UInt8, "TransactionResult") => coded(RESULTS, bytes[0]),
        (Type::UInt16, "LedgerEntryType") => coded(ENTRY_TYPES, field.uint16().unwrap()),
        (Type::UInt16, "TransactionType") => coded(TRANSACTION_TYPES, field.uint16().unwrap()),
        (Type::UInt16, _) => field.uint16().unwrap().into(),
        (Type::UInt32, _) => field.uint32().unwrap().into(),
        (Type::UInt64, _) => format!("{:x}", field.uint64().unwrap()).into(),
        (Type::Amount, _) => match field.amount().unwrap() {
            Amount::Xrp(drops) => drops.to_string().into(),
            Amount::Token(amount) => token(&amount),
        },
        (Type::AccountId, _) => field.account().unwrap().unwrap().to_address().into(),
        (Type::Vector256, _) => (field.vector256().unwrap().iter())
            .map(|hash| Value::from(hex_upper(hash)))
            .collect::<Vec<_>>()
            .into(),
        (Type::Issue, _) if bytes.len() == 20 => json!({ "currency": "XRP" }),
        (Type::Issue, _) => json!({
            "currency": Currency(bytes[..20].try_into().unwrap()).to_string(),
            "issuer": AccountId(bytes[20..].try_into().unwrap()).to_address(),
        }),
        _ => hex_upper(bytes).into(),
    }
}

/// Checks that the fields of `bytes` are those of `json`, by name, each
/// read as rippled writes it, nested objects and arrays too; with
/// `subset`, only that `json`'s fields are among them.
fn reads_as(json: &Value, bytes: &[u8], subset: bool) {
    let mut stack = vec![(json.clone(), bytes.to_vec(), String::new())];
    while let Some((json, bytes, path)) = stack.pop() {
        let object = json.as_object().unwrap_or_else(|| panic!("{path}: {json}"));
        let fields: Vec<Field<'_>> = Fields::new(&bytes).map(Result::unwrap).collect();
        let names: BTreeSet<&str> = fields.iter().map(|field| name(field.id)).collect();
        let keys: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        if subset {
            assert!(keys.is_subset(&names), "{path}: {keys:?} in {names:?}");
        } else {
            assert_eq!(keys, names, "{path}");
        }
        for field in &fields {
            let name = name(field.id);
            let Some(value) = object.get(name) else {
                continue;
            };
            let path = format!("{path}.{name}");
            match field.id.kind() {
                Type::Object => stack.push((value.clone(), field.value.to_vec(), path)),
                Type::Array => {
                    let elements: Vec<Field<'_>> =
                        field.elements().unwrap().map(Result::unwrap).collect();
                    let values = value.as_array().unwrap();
                    assert_eq!(values.len(), elements.len(), "{path}");
                    for (element, value) in elements.iter().zip(values) {
                        let inner = &value[self::name(element.id)];
                        stack.push((inner.clone(), element.value.to_vec(), path.clone()));
                    }
                }
                _ => assert_eq!(&leaf(field), value, "{path}"),
            }
        }
    }
}

fn ask(ledger: &mut FakeLedger, request: &Value) -> Value {
    let reply = ledger.request(CONNECTION, &serde_json::to_vec(request).unwrap());
    let reply: Value = serde_json::from_slice(&reply).unwrap();
    assert_eq!(reply["status"], "success", "{request}: {reply}");
    reply["result"].clone()
}

/// The transaction `hash` and its metadata in binary, as `tx` gives them,
/// and the ledger of its proof: the leaf built from them proves with
/// `tx_proof`'s branch, which starts above it.
fn proven_blobs(ledger: &mut FakeLedger, hash: &str) -> (Vec<u8>, Vec<u8>, Value) {
    let binary = ask(
        ledger,
        &json!({ "command": "tx", "transaction": hash, "binary": true }),
    );
    let blobs = dassie_xrpl::api::tx_binary_result(&binary).unwrap();
    let proof = ask(ledger, &json!({ "command": "tx_proof", "tx_hash": hash }));
    let proof_read = dassie_xrpl::api::tx_proof_result(&proof).unwrap();
    let header = LedgerHeader::from_bytes(&proof_read.header).unwrap();
    verify_transaction(
        &header.transaction_hash,
        &blobs.tx,
        &blobs.meta,
        &proof_read.path,
    )
    .unwrap();
    assert_eq!(binary["ledger_index"], proof["ledger_index"]);
    (blobs.tx, blobs.meta, proof["ledger_index"].clone())
}

fn submit(ledger: &mut FakeLedger, blob: &[u8]) {
    let result = ask(
        ledger,
        &json!({ "command": "submit", "tx_blob": hex_upper(blob) }),
    );
    assert_eq!(result["engine_result"], "tesSUCCESS", "{result}");
}

fn sequence(ledger: &FakeLedger, wallet: &Wallet) -> u32 {
    ledger.sequence(wallet.account()).unwrap()
}

fn payment(ledger: &FakeLedger, wallet: &Wallet, to: AccountId, amount: Amount) -> Payment {
    Payment {
        account: wallet.account(),
        destination: to,
        destination_tag: None,
        amount,
        send_max: None,
        fee: FEE_BASE,
        sequence: sequence(ledger, wallet),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
}

fn trust_set(ledger: &FakeLedger, wallet: &Wallet, token: IssuedAsset) -> TrustSet {
    TrustSet {
        account: wallet.account(),
        limit: token.amount(TokenValue::MAX),
        fee: FEE_BASE,
        sequence: sequence(ledger, wallet),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: TF_SET_NO_RIPPLE,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
}

fn value(text: &str) -> TokenValue {
    TokenValue::parse(text).unwrap()
}

/// A mainnet ledger, where RLUSD is, in which every kind of transaction the
/// fake applies applied, and every kind of object it keeps exists: the
/// accounts that took part.
fn every_kind() -> (FakeLedger, Vec<AccountId>) {
    let mut ledger = FakeLedger::mainnet();
    let (alice, bob, carol) = (
        Wallet::from_seed([1; 16]),
        Wallet::from_seed([2; 16]),
        Wallet::from_seed([3; 16]),
    );
    for wallet in [&alice, &bob, &carol] {
        ledger.deposit(wallet.account(), 1_000_000_000);
    }
    ledger.close();
    for wallet in [&alice, &bob] {
        let signed = trust_set(&ledger, wallet, IssuedAsset::RLUSD).sign(wallet);
        submit(&mut ledger, &signed.blob);
    }
    ledger.close();
    ledger
        .issue(alice.account(), IssuedAsset::RLUSD.amount(value("100")))
        .unwrap();
    ledger
        .set_pool(IssuedAsset::RLUSD, 500_000_000, value("250"), 300)
        .unwrap();
    ledger.close();
    let rlusd = |text| Amount::Token(IssuedAsset::RLUSD.amount(value(text)));
    let token_paid = payment(&ledger, &alice, bob.account(), rlusd("12.5")).sign(&alice);
    submit(&mut ledger, &token_paid.blob);
    let paid = payment(&ledger, &carol, alice.account(), Amount::Xrp(5_000_000));
    submit(&mut ledger, &paid.sign(&carol).blob);
    ledger.close();
    let failed = payment(&ledger, &bob, AccountId([9; 20]), Amount::Xrp(1));
    submit(&mut ledger, &failed.sign(&bob).blob);
    let carol_trusts = trust_set(&ledger, &carol, IssuedAsset::RLUSD);
    submit(&mut ledger, &carol_trusts.sign(&carol).blob);
    ledger.close();
    let mut swap = payment(&ledger, &carol, carol.account(), rlusd("1"));
    swap.send_max = Some(Amount::Xrp(10_000_000));
    submit(&mut ledger, &swap.sign(&carol).blob);
    ledger
        .freeze(bob.account(), IssuedAsset::RLUSD, true)
        .unwrap();
    ledger
        .global_freeze(IssuedAsset::RLUSD.issuer, true)
        .unwrap();
    ledger
        .set_transfer_rate(IssuedAsset::RLUSD.issuer, 1_002_000_000)
        .unwrap();
    ledger.close();
    let clawed: TokenAmount = IssuedAsset::RLUSD.amount(value("2"));
    ledger.claw_back(alice.account(), clawed).unwrap();
    ledger
        .global_freeze(IssuedAsset::RLUSD.issuer, false)
        .unwrap();
    keys_and_signers(&mut ledger, [&alice, &bob, &carol]);
    let accounts = vec![
        alice.account(),
        bob.account(),
        carol.account(),
        IssuedAsset::RLUSD.issuer,
        dassie_xrpl::fake::pool_account(IssuedAsset::RLUSD),
    ];
    (ledger, accounts)
}

/// Settings, a regular key and a signer list, made and deleted again, and
/// a pool set again.
fn keys_and_signers(ledger: &mut FakeLedger, [alice, bob, carol]: [&Wallet; 3]) {
    let tag = AccountSet {
        account: alice.account(),
        set_flag: Some(dassie_xrpl::codec::ASF_REQUIRE_DEST),
        clear_flag: None,
        fee: FEE_BASE,
        sequence: sequence(ledger, alice),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(alice.public_key()),
    };
    submit(ledger, &tag.sign(alice).blob);
    let regular = SetRegularKey {
        account: bob.account(),
        regular_key: Some(carol.account()),
        fee: FEE_BASE,
        sequence: sequence(ledger, bob),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(bob.public_key()),
    };
    submit(ledger, &regular.sign(bob).blob);
    let mut entries = vec![
        SignerEntry {
            account: alice.account(),
            weight: 1,
        },
        SignerEntry {
            account: bob.account(),
            weight: 2,
        },
    ];
    entries.sort_by_key(|entry| entry.account);
    let signers = SignerListSet {
        account: carol.account(),
        signer_quorum: 2,
        signer_entries: entries,
        fee: FEE_BASE,
        sequence: sequence(ledger, carol),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(carol.public_key()),
    };
    submit(ledger, &signers.clone().sign(carol).blob);
    ledger
        .set_pool(IssuedAsset::RLUSD, 600_000_000, value("200"), 500)
        .unwrap();
    ledger.close();
    // The list deleted again.
    let unlisted = SignerListSet {
        signer_quorum: 0,
        signer_entries: Vec::new(),
        sequence: sequence(ledger, carol),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        ..signers
    };
    submit(ledger, &unlisted.sign(carol).blob);
    ledger.close();
    ledger.close();
}

/// Every transaction that touched `accounts`, by hash.
fn transactions(ledger: &mut FakeLedger, accounts: &[AccountId]) -> BTreeSet<String> {
    let mut hashes = BTreeSet::new();
    for account in accounts {
        let answer = ask(
            ledger,
            &json!({ "command": "account_tx", "account": account.to_address() }),
        );
        for entry in answer["transactions"].as_array().unwrap() {
            hashes.insert(entry["tx"]["hash"].as_str().unwrap().to_owned());
        }
    }
    hashes
}

#[test]
fn every_transaction_and_its_metadata_read_as_its_json() {
    let (mut ledger, accounts) = every_kind();
    let hashes = transactions(&mut ledger, &accounts);
    let mut kinds = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    for hash in &hashes {
        let json = ask(
            &mut ledger,
            &json!({ "command": "tx", "transaction": hash }),
        );
        let (blob, meta, proven_in) = proven_blobs(&mut ledger, hash);
        assert_eq!(hex_upper(&dassie_xrpl::codec::hash(&blob)), *hash);
        assert_eq!(json["ledger_index"], proven_in);
        let mut transaction = json.clone();
        let object = transaction.as_object_mut().unwrap();
        for key in ["hash", "ledger_index", "meta", "validated"] {
            object.remove(key);
        }
        reads_as(&transaction, &blob, false);
        let mut metadata = json["meta"].clone();
        metadata.as_object_mut().unwrap().remove("delivered_amount");
        reads_as(&metadata, &meta, false);
        // A kind the node signs reads with its codec, as it was signed.
        let kind = json["TransactionType"].as_str().unwrap().to_owned();
        if json["SigningPubKey"] != "" {
            assert_eq!(Transaction::decode(&blob).unwrap().blob(), blob);
        }
        kinds.insert(kind);
        for node in json["meta"]["AffectedNodes"].as_array().unwrap() {
            let (kind, inner) = node.as_object().unwrap().iter().next().unwrap();
            let entry = inner["LedgerEntryType"].as_str().unwrap();
            nodes.insert(format!("{kind} {entry}"));
            // An owner of a line made, which the transaction changed
            // otherwise not: named with the transaction before alone.
            if kind == "ModifiedNode" && inner.get("FinalFields").is_none() {
                assert!(inner["PreviousTxnID"].is_string(), "{inner}");
                nodes.insert(format!("threaded {entry}"));
            }
        }
    }
    for kind in [
        "Payment",
        "TrustSet",
        "AccountSet",
        "SetRegularKey",
        "SignerListSet",
        "Clawback",
        "AMMSet",
    ] {
        assert!(kinds.contains(kind), "{kind} in {kinds:?}");
    }
    for node in [
        "CreatedNode AccountRoot",
        "ModifiedNode AccountRoot",
        "CreatedNode RippleState",
        "ModifiedNode RippleState",
        "CreatedNode SignerList",
        "DeletedNode SignerList",
        "CreatedNode AMM",
        "ModifiedNode AMM",
        "threaded AccountRoot",
    ] {
        assert!(nodes.contains(node), "{node} in {nodes:?}");
    }
}

// spec/designs/xrpl-validation.md#8-the-verifier: a transaction and its
// metadata in binary decode into the record the schemes read, as the JSON
// of the stream and `tx` gives it, for every kind the fake writes: what it
// delivered too, from the metadata's `DeliveredAmount`, else a successful
// Payment's `Amount`.
#[test]
fn every_transaction_decodes_as_the_record_its_json_gives() {
    let (mut ledger, accounts) = every_kind();
    let hashes = transactions(&mut ledger, &accounts);
    let mut delivered = 0;
    for hash in &hashes {
        let json = ask(
            &mut ledger,
            &json!({ "command": "tx", "transaction": hash }),
        );
        let (blob, meta, proven_in) = proven_blobs(&mut ledger, hash);
        let index = u32::try_from(proven_in.as_u64().unwrap()).unwrap();
        let decoded = dassie_xrpl::records::transaction(&blob, &meta, index, &[]).unwrap();
        let read = dassie_xrpl::api::tx_result(&json).unwrap();
        assert_eq!(decoded.record, read, "{hash}");
        if read.delivered.is_some() || read.delivered_token.is_some() {
            delivered += 1;
        }
    }
    assert!(delivered > 0);
}

fn state(ledger: &mut FakeLedger, key: &[u8; 32]) -> Vec<u8> {
    let answer = ask(
        ledger,
        &json!({ "command": "state_proof", "keys": [hex_upper(key)] }),
    );
    // The object is the leaf the branch starts at.
    let entry = &answer["entries"][0];
    assert_eq!(entry.as_object().unwrap().len(), 2, "{entry}");
    let path: Vec<Vec<u8>> = (entry["path"].as_array().unwrap().iter())
        .map(|node| from_hex(node.as_str().unwrap()).unwrap())
        .collect();
    held_object(key, &path).unwrap().to_vec()
}

#[test]
fn every_object_reads_as_its_json() {
    let (mut ledger, accounts) = every_kind();
    for account in &accounts {
        let root = state(
            &mut ledger,
            &sha512_half(&[&[0, b'a'][..], &account.0].concat()),
        );
        let info = ask(
            &mut ledger,
            &json!({ "command": "account_info", "account": account.to_address() }),
        );
        reads_as(&info["account_data"], &root, true);
        let objects = ask(
            &mut ledger,
            &json!({ "command": "account_objects", "account": account.to_address() }),
        );
        for object in objects["account_objects"].as_array().unwrap() {
            let key: [u8; 32] = from_hex(object["index"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            let mut fields = object.clone();
            fields.as_object_mut().unwrap().remove("index");
            reads_as(&fields, &state(&mut ledger, &key), true);
        }
    }
    let amm = ask(
        &mut ledger,
        &json!({
            "command": "amm_info",
            "asset": { "currency": "XRP" },
            "asset2": { "currency": IssuedAsset::RLUSD.currency.to_string(), "issuer": IssuedAsset::RLUSD.issuer.to_address() },
        }),
    );
    let amm_key = sha512_half(
        &[
            &[0, b'A'][..],
            &[0; 40],
            &IssuedAsset::RLUSD.issuer.0,
            &IssuedAsset::RLUSD.currency.0,
        ]
        .concat(),
    );
    let object = state(&mut ledger, &amm_key);
    reads_as(
        &json!({
            "Account": amm["amm"]["account"],
            "TradingFee": amm["amm"]["trading_fee"],
            "LedgerEntryType": "AMM",
        }),
        &object,
        true,
    );
}

// A signer list, as a hub's validators read their escrow's from a proof:
// it reads as its JSON, and decodes as `account_objects` reads it, under
// the key of its owner.
#[test]
fn a_signer_list_reads_as_its_json() {
    let mut ledger = FakeLedger::new();
    let (owner, alice, bob) = (
        Wallet::from_seed([1; 16]),
        Wallet::from_seed([2; 16]),
        Wallet::from_seed([3; 16]),
    );
    ledger.fund(owner.account());
    ledger.close();
    let mut entries = vec![
        SignerEntry {
            account: alice.account(),
            weight: 1,
        },
        SignerEntry {
            account: bob.account(),
            weight: 2,
        },
    ];
    entries.sort_by_key(|entry| entry.account);
    let list = SignerListSet {
        account: owner.account(),
        signer_quorum: 2,
        signer_entries: entries,
        fee: FEE_BASE,
        sequence: sequence(&ledger, &owner),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(owner.public_key()),
    }
    .sign(&owner);
    submit(&mut ledger, &list.blob);
    ledger.close();
    let objects = ask(
        &mut ledger,
        &json!({ "command": "account_objects", "account": owner.account().to_address() }),
    );
    let object = &objects["account_objects"][0];
    assert_eq!(object["LedgerEntryType"], "SignerList");
    let key = dassie_xrpl::records::signer_list_key(owner.account());
    assert_eq!(object["index"], hex_upper(&key));
    let bytes = state(&mut ledger, &key);
    let mut fields = object.clone();
    fields.as_object_mut().unwrap().remove("index");
    reads_as(&fields, &bytes, true);
    let (decoded, thread) = dassie_xrpl::records::signer_list(&bytes).unwrap();
    let read = dassie_xrpl::api::account_objects_result(&objects).unwrap();
    assert_eq!(Some(decoded), read.signer_list);
    assert_eq!(thread.map(|(hash, _)| hash), Some(list.hash));
    // Another kind of object is none.
    let root = state(
        &mut ledger,
        &dassie_xrpl::records::account_key(owner.account()),
    );
    assert!(dassie_xrpl::records::signer_list(&root).is_err());
}
