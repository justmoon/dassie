//! Golden vectors: the original's wallets, from `dassie-identity`'s chain,
//! and Payments as xrpl.js 3.1.0 signs them (testdata/xrpl).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_identity::NodeKey;
use dassie_xrpl::codec::{Memo, Payment, hash};
use dassie_xrpl::{AccountId, Wallet, from_hex, hex_upper};
use serde_json::Value;

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

// The wallet is the original's (spec/areas/xrpl.md#the-wallet): the same
// passphrase must find the same XRPL account. Every entry of the chain:
// node key → settlement entropy → wallet.
#[test]
fn the_original_wallets_derive_from_the_node_key() {
    let chain = read("../dassie-identity/tests/data/chain.json");
    let entries = chain["chain"].as_array().unwrap();
    assert_eq!(entries.len(), 64);
    for entry in entries {
        let key: [u8; 32] = from_hex(text(entry, "nodeKey"))
            .unwrap()
            .try_into()
            .unwrap();
        let node_key = NodeKey::from_bytes(&key);
        let entropy = node_key
            .settlement_entropy("xrpl-testnet", "seed-testnet")
            .unwrap();
        let wallet = Wallet::from_entropy(entropy.as_bytes());
        let expected = &entry["xrplTestnetWallet"];
        assert_eq!(wallet.family_seed(), text(expected, "familySeed"));
        assert_eq!(wallet.account().to_address(), text(expected, "address"));
        assert_eq!(hex_upper(wallet.public_key()), text(expected, "publicKey"));
        assert_eq!(
            AccountId::parse(text(expected, "address")),
            Some(wallet.account())
        );
    }
}

fn payment(vector: &Value) -> (Wallet, Payment) {
    let wallet = Wallet::from_family_seed(text(vector, "familySeed")).unwrap();
    let tx = &vector["tx"];
    let drops = |key: &str| text(tx, key).parse::<u64>().unwrap();
    let number = |key: &str| u32::try_from(tx[key].as_u64().unwrap()).unwrap();
    let memo = &tx["Memos"][0]["Memo"];
    let payment = Payment {
        account: AccountId::parse(text(tx, "Account")).unwrap(),
        destination: AccountId::parse(text(tx, "Destination")).unwrap(),
        amount: drops("Amount"),
        fee: drops("Fee"),
        sequence: number("Sequence"),
        last_ledger_sequence: number("LastLedgerSequence"),
        flags: number("Flags"),
        network_id: tx.get("NetworkID").map(|_| number("NetworkID")),
        memos: vec![Memo {
            memo_type: from_hex(text(memo, "MemoType")).unwrap(),
            memo_data: from_hex(text(memo, "MemoData")).unwrap(),
        }],
        signing_pub_key: *wallet.public_key(),
        signature: None,
    };
    (wallet, payment)
}

// Settlement Payments (spec/areas/xrpl.md#the-payment-it-signs), field for
// field and byte for byte as xrpl.js signs them, with the original's memo.
#[test]
fn payments_sign_and_hash_as_xrpl_js_does() {
    let vectors = read("tests/data/vectors.json");
    let memo_type = from_hex(text(&vectors, "memoType")).unwrap();
    assert_eq!(memo_type, b"https://dassie.land/settlement/xrp-testnet/v1");
    let payments = vectors["payments"].as_array().unwrap();
    assert_eq!(payments.len(), 4);
    for vector in payments {
        let (wallet, payment) = payment(vector);
        assert_eq!(
            hex_upper(&payment.signing_payload()),
            text(vector, "signingPayload")
        );
        let signed = payment.sign(&wallet);
        assert_eq!(hex_upper(&signed.blob), text(vector, "txBlob"));
        assert_eq!(signed.hash_hex(), text(vector, "hash"));
        let blob = from_hex(text(vector, "txBlob")).unwrap();
        assert_eq!(hex_upper(&hash(&blob)), text(vector, "hash"));
        assert_eq!(Payment::decode(&blob).unwrap(), signed.payment);
        assert_eq!(
            signed.payment.memos[0].memo_data,
            text(vector, "nodeId").as_bytes()
        );
    }
}
