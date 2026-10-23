//! Golden vectors for an account several keys control
//! (spec/areas/xrpl.md#the-transactions): secp256k1 keys and their
//! signatures, and AccountSet, SetRegularKey, SignerListSet and Payments
//! with a destination tag, single-signed and multi-signed, as xrpl.js 3.1.0
//! and its ripple-keypairs sign them (testdata/xrpl/generate-escrow-vectors.mjs).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use dassie_xrpl::codec::{
    AccountSet, Amount, Memo, Payment, SetRegularKey, SignerEntry, SignerListSet, Signing,
    Transact, Transaction, hash,
};
use dassie_xrpl::keys::{Key, KeyType, Secp256k1Key, Signature};
use dassie_xrpl::{AccountId, Wallet, from_hex, hex_upper};
use serde_json::Value;

fn vectors() -> Value {
    let text = std::fs::read_to_string("tests/data/escrow-vectors.json").unwrap();
    serde_json::from_str(&text).unwrap()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

fn number(tx: &Value, key: &str) -> Option<u32> {
    tx.get(key)
        .map(|value| u32::try_from(value.as_u64().unwrap()).unwrap())
}

fn account(tx: &Value, key: &str) -> AccountId {
    AccountId::parse(text(tx, key)).unwrap()
}

/// The key a vector's wallet describes: an Ed25519 family seed, or a
/// secp256k1 secret, which xrpl.js writes with a leading zero byte.
fn key(wallet: &Value) -> Box<dyn Key> {
    let key: Box<dyn Key> = match text(wallet, "algorithm") {
        "ed25519" => Box::new(Wallet::from_family_seed(text(wallet, "seed")).unwrap()),
        _ => {
            let secret = from_hex(text(wallet, "privateKey")).unwrap();
            assert_eq!(secret.len(), 33);
            Box::new(Secp256k1Key::from_secret(&secret[1..].try_into().unwrap()).unwrap())
        }
    };
    assert_eq!(
        hex_upper(key.public_key().as_bytes()),
        text(wallet, "publicKey")
    );
    assert_eq!(key.account().to_address(), text(wallet, "address"));
    key
}

/// The transaction a vector writes as JSON, signed by `signing`.
fn transaction(tx: &Value, signing: Signing) -> Transaction {
    let account = account(tx, "Account");
    let fee = text(tx, "Fee").parse().unwrap();
    let sequence = number(tx, "Sequence").unwrap();
    let last_ledger_sequence = number(tx, "LastLedgerSequence");
    let flags = number(tx, "Flags").unwrap();
    let network_id = number(tx, "NetworkID");
    let memos: Vec<Memo> = tx
        .get("Memos")
        .map(|memos| {
            memos
                .as_array()
                .unwrap()
                .iter()
                .map(|memo| Memo {
                    memo_type: from_hex(text(&memo["Memo"], "MemoType")).unwrap(),
                    memo_data: from_hex(text(&memo["Memo"], "MemoData")).unwrap(),
                })
                .collect()
        })
        .unwrap_or_default();
    match text(tx, "TransactionType") {
        "Payment" => Transaction::Payment(Payment {
            account,
            destination: self::account(tx, "Destination"),
            destination_tag: number(tx, "DestinationTag"),
            amount: Amount::Xrp(text(tx, "Amount").parse().unwrap()),
            send_max: None,
            fee,
            sequence,
            last_ledger_sequence,
            flags,
            network_id,
            memos,
            signing,
        }),
        "AccountSet" => Transaction::AccountSet(AccountSet {
            account,
            set_flag: number(tx, "SetFlag"),
            clear_flag: number(tx, "ClearFlag"),
            fee,
            sequence,
            last_ledger_sequence,
            flags,
            network_id,
            memos,
            signing,
        }),
        "SetRegularKey" => Transaction::SetRegularKey(SetRegularKey {
            account,
            regular_key: tx
                .get("RegularKey")
                .map(|_| self::account(tx, "RegularKey")),
            fee,
            sequence,
            last_ledger_sequence,
            flags,
            network_id,
            memos,
            signing,
        }),
        "SignerListSet" => Transaction::SignerListSet(SignerListSet {
            account,
            signer_quorum: number(tx, "SignerQuorum").unwrap(),
            signer_entries: tx
                .get("SignerEntries")
                .map(|entries| {
                    entries
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|entry| SignerEntry {
                            account: self::account(&entry["SignerEntry"], "Account"),
                            weight: u16::try_from(
                                entry["SignerEntry"]["SignerWeight"].as_u64().unwrap(),
                            )
                            .unwrap(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            fee,
            sequence,
            last_ledger_sequence,
            flags,
            network_id,
            memos,
            signing,
        }),
        other => panic!("{other}"),
    }
}

// secp256k1 as ripple-keypairs signs (spec/areas/xrpl.md#keys-and-signatures):
// the same key from the same secret, the same signature byte for byte
// (RFC 6979 and a low S), and a high S refused, as rippled refuses it.
#[test]
fn secp256k1_keys_sign_as_ripple_keypairs_does() {
    let vectors = vectors();
    let keys = vectors["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 5);
    for wallet in keys {
        let key = key(wallet);
        let public_key = key.public_key();
        assert_eq!(public_key.key_type(), KeyType::Secp256k1);
        for vector in wallet["signatures"].as_array().unwrap() {
            let message = from_hex(text(vector, "message")).unwrap();
            let expected = from_hex(text(vector, "signature")).unwrap();
            assert_eq!(key.sign(&message).as_bytes(), expected.as_slice());
            assert!(vector["verifies"].as_bool().unwrap());
            assert!(public_key.verify(&message, &Signature::from_slice(&expected).unwrap()));
            let high = Signature::from_slice(&from_hex(text(vector, "highS")).unwrap()).unwrap();
            assert!(!vector["highSVerifies"].as_bool().unwrap());
            assert!(!public_key.verify(&message, &high));
        }
    }
}

// Each kind, single-signed by an Ed25519 or a secp256k1 key: what was
// signed, the blob, the hash, and the blob read back.
#[test]
fn single_signed_settings_and_payments_are_as_xrpl_js_signs_them() {
    let vectors = vectors();
    let single = vectors["single"].as_array().unwrap();
    assert_eq!(single.len(), 8);
    for vector in single {
        let key = key(&vector["wallet"]);
        let tx = transaction(&vector["tx"], Signing::by(key.public_key()));
        assert_eq!(
            hex_upper(&tx.signing_payload()),
            text(vector, "signingPayload")
        );
        let signed = tx.sign(key.as_ref());
        assert_eq!(hex_upper(&signed.blob), text(vector, "txBlob"));
        assert_eq!(signed.hash_hex(), text(vector, "hash"));
        assert!(signed.tx.signatures_valid());
        let blob = from_hex(text(vector, "txBlob")).unwrap();
        assert_eq!(Transaction::decode(&blob), Ok(signed.tx));
    }
}

// Multi-signed: each signer signs alone what xrpl.js has it sign, and the
// signatures put together, sorted by account, are the blob xrpl.js's
// `multisign` makes, with its hash.
#[test]
fn multi_signed_transactions_are_as_xrpl_js_puts_them_together() {
    let vectors = vectors();
    let multi = vectors["multi"].as_array().unwrap();
    assert_eq!(multi.len(), 2);
    for vector in multi {
        let tx = transaction(&vector["tx"], Signing::multi());
        let mut signers = Vec::new();
        for signer in vector["signers"].as_array().unwrap() {
            let key = key(&signer["wallet"]);
            let account = key.account();
            assert_eq!(
                hex_upper(&tx.multi_signing_payload(account)),
                text(signer, "multiSigningPayload")
            );
            let signature = tx.multi_sign(account, key.as_ref());
            assert!(tx.verify_signer(&signature));
            // One signer's part, as xrpl.js writes it alone.
            let alone = tx.clone().with_signers(vec![signature]).unwrap();
            assert_eq!(hex_upper(&alone.blob), text(signer, "txBlob"));
            signers.push(signature);
        }
        let signed = tx.with_signers(signers).unwrap();
        assert_eq!(hex_upper(&signed.blob), text(vector, "txBlob"));
        assert_eq!(signed.hash_hex(), text(vector, "hash"));
        assert_eq!(hash(&signed.blob), signed.hash);
        assert!(signed.tx.signatures_valid());
        let blob = from_hex(text(vector, "txBlob")).unwrap();
        assert_eq!(Transaction::decode(&blob), Ok(signed.tx));
    }
}
