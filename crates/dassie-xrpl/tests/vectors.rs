//! Golden vectors: the node's wallets on both networks, from
//! `dassie-identity`'s chain, and Payments, token amounts and TrustSets as
//! xrpl.js 3.1.0 writes and signs them, with Payments that name a
//! `SendMax` (testdata/xrpl).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_identity::NodeKey;
use dassie_xrpl::codec::{
    Amount, Currency, Memo, Payment, SendMaxTerms, Signed, Signing, TokenAmount, TokenValue,
    Transact, Transaction, TrustSet, hash, sign_send_max,
};
use dassie_xrpl::{AccountId, Wallet, from_hex, hex_upper, network};
use serde_json::Value;

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

// The wallet of the node's first account on each network, as xrpl.js
// derives it (spec/areas/xrpl.md#the-wallet): the same passphrase must
// find the same XRPL account. Every entry of the chain: node key → the
// account's entropy at its network's ID → wallet, the two networks' apart.
#[test]
fn each_networks_wallet_derives_from_the_node_key() {
    let chain = read("../dassie-identity/tests/data/chain.json");
    let entries = chain["chain"].as_array().unwrap();
    assert_eq!(entries.len(), 64);
    for entry in entries {
        let key: [u8; 32] = from_hex(text(entry, "nodeKey"))
            .unwrap()
            .try_into()
            .unwrap();
        let node_key = NodeKey::from_bytes(&key);
        let mut addresses = Vec::new();
        for (network, wallet) in [
            (&network::TESTNET, "xrplTestnetWallet"),
            (&network::MAINNET, "xrplMainnetWallet"),
        ] {
            let entropy = node_key.xrpl_account_entropy(network.id, 0);
            let derived = Wallet::from_entropy(entropy.as_bytes());
            let expected = &entry[wallet];
            assert_eq!(derived.family_seed(), text(expected, "familySeed"));
            assert_eq!(derived.account().to_address(), text(expected, "address"));
            assert_eq!(
                hex_upper(derived.public_key().as_bytes()),
                text(expected, "publicKey")
            );
            assert_eq!(
                AccountId::parse(text(expected, "address")),
                Some(derived.account())
            );
            addresses.push(derived.account());
        }
        assert_ne!(addresses[0], addresses[1], "keys of their own");
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
        destination_tag: None,
        amount: Amount::Xrp(drops("Amount")),
        send_max: None,
        fee: drops("Fee"),
        sequence: number("Sequence"),
        last_ledger_sequence: Some(number("LastLedgerSequence")),
        flags: number("Flags"),
        network_id: tx.get("NetworkID").map(|_| number("NetworkID")),
        memos: vec![Memo {
            memo_type: from_hex(text(memo, "MemoType")).unwrap(),
            memo_data: from_hex(text(memo, "MemoData")).unwrap(),
        }],
        signing: Signing::by(wallet.public_key()),
    };
    (wallet, payment)
}

// Payments (spec/areas/xrpl.md#the-payment-it-signs), field for
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
        assert_eq!(Payment::decode(&blob).unwrap(), signed.tx);
        assert_eq!(
            signed.tx.memos[0].memo_data,
            text(vector, "nodeId").as_bytes()
        );
    }
}

/// A token amount as the vectors write it, read by the node's parser.
fn token(value: &Value) -> TokenAmount {
    TokenAmount {
        value: TokenValue::parse(text(value, "value")).unwrap(),
        currency: Currency::parse(text(value, "currency")).unwrap(),
        issuer: AccountId::parse(text(value, "issuer")).unwrap(),
    }
}

// Token amounts byte for byte as xrpl.js writes them, at the edges of what
// the ledger expresses (spec/areas/xrpl.md#tokens); and the value xrpl.js
// reads back is the same value to the node.
#[test]
fn token_amounts_encode_as_xrpl_js_does() {
    let vectors = read("tests/data/vectors.json");
    let amounts = vectors["amounts"].as_array().unwrap();
    assert_eq!(amounts.len(), 15);
    for vector in amounts {
        let amount = token(&vector["amount"]);
        let hex = text(vector, "hex");
        assert_eq!(hex_upper(&amount.to_bytes()), hex, "{amount:?}");
        let bytes: [u8; 48] = from_hex(hex).unwrap().try_into().unwrap();
        assert_eq!(TokenAmount::from_bytes(&bytes), Some(amount));
        assert_eq!(token(&vector["decoded"]), amount);
    }
}

fn common(vector: &Value) -> (Wallet, &Value) {
    let wallet = Wallet::from_family_seed(text(vector, "familySeed")).unwrap();
    (wallet, &vector["tx"])
}

fn number(tx: &Value, key: &str) -> u32 {
    u32::try_from(tx[key].as_u64().unwrap()).unwrap()
}

/// Checks a signed transaction against its vector: what was signed, the
/// blob, the hash, and the blob read back.
fn check<T>(vector: &Value, payload: &[u8], signed: &Signed<T>, read: Transaction) {
    assert_eq!(hex_upper(payload), text(vector, "signingPayload"));
    assert_eq!(hex_upper(&signed.blob), text(vector, "txBlob"));
    assert_eq!(hex_upper(&signed.hash), text(vector, "hash"));
    let blob = from_hex(text(vector, "txBlob")).unwrap();
    assert_eq!(Transaction::decode(&blob).unwrap(), read);
}

// Token Payments as the scheme sends them: a token Amount, no paths, no
// SendMax, not partial.
#[test]
fn token_payments_sign_and_hash_as_xrpl_js_does() {
    let vectors = read("tests/data/vectors.json");
    let payments = vectors["tokenPayments"].as_array().unwrap();
    assert_eq!(payments.len(), 4);
    for vector in payments {
        let (wallet, tx) = common(vector);
        let memo = &tx["Memos"][0]["Memo"];
        let payment = Payment {
            account: AccountId::parse(text(tx, "Account")).unwrap(),
            destination: AccountId::parse(text(tx, "Destination")).unwrap(),
            destination_tag: None,
            amount: Amount::Token(token(&tx["Amount"])),
            send_max: None,
            fee: text(tx, "Fee").parse().unwrap(),
            sequence: number(tx, "Sequence"),
            last_ledger_sequence: Some(number(tx, "LastLedgerSequence")),
            flags: number(tx, "Flags"),
            network_id: tx.get("NetworkID").map(|_| number(tx, "NetworkID")),
            memos: vec![Memo {
                memo_type: from_hex(text(memo, "MemoType")).unwrap(),
                memo_data: from_hex(text(memo, "MemoData")).unwrap(),
            }],
            signing: Signing::by(wallet.public_key()),
        };
        let payload = payment.signing_payload();
        let signed = payment.sign(&wallet);
        check(
            vector,
            &payload,
            &signed,
            Transaction::Payment(signed.tx.clone()),
        );
        assert_eq!(
            token(&vector["decoded"]["Amount"]),
            *signed.tx.amount.token().unwrap()
        );
    }
}

// TrustSets: the line to the issuer, with rippling disabled on our side,
// at the largest limit, at zero, and plain.
#[test]
fn trust_sets_sign_and_hash_as_xrpl_js_does() {
    let vectors = read("tests/data/vectors.json");
    let trust_sets = vectors["trustSets"].as_array().unwrap();
    assert_eq!(trust_sets.len(), 4);
    let limits: Vec<TokenValue> = trust_sets
        .iter()
        .map(|vector| token(&vector["tx"]["LimitAmount"]).value)
        .collect();
    assert!(limits.contains(&TokenValue::MAX) && limits.contains(&TokenValue::ZERO));
    for vector in trust_sets {
        let (wallet, tx) = common(vector);
        let trust_set = TrustSet {
            account: AccountId::parse(text(tx, "Account")).unwrap(),
            limit: token(&tx["LimitAmount"]),
            fee: text(tx, "Fee").parse().unwrap(),
            sequence: number(tx, "Sequence"),
            last_ledger_sequence: Some(number(tx, "LastLedgerSequence")),
            flags: number(tx, "Flags"),
            network_id: tx.get("NetworkID").map(|_| number(tx, "NetworkID")),
            memos: Vec::new(),
            signing: Signing::by(wallet.public_key()),
        };
        let payload = trust_set.signing_payload();
        let signed = trust_set.sign(&wallet);
        check(
            vector,
            &payload,
            &signed,
            Transaction::TrustSet(signed.tx.clone()),
        );
        assert_eq!(token(&vector["decoded"]["LimitAmount"]), signed.tx.limit);
    }
}

/// An amount as the vectors write it: drops in a string, or a token.
fn amount(value: &Value) -> Amount {
    match value.as_str() {
        Some(drops) => Amount::Xrp(drops.parse().unwrap()),
        None => Amount::Token(token(value)),
    }
}

// Payments with `SendMax` (spec/areas/xrpl.md#tokens): XRP for a token and
// a token for XRP, as a swap through an AMM pool is sent, with and without
// a memo, signed by the codec's own helper.
#[test]
fn send_max_payments_sign_and_hash_as_xrpl_js_does() {
    let vectors = read("tests/data/vectors.json");
    let payments = vectors["sendMaxPayments"].as_array().unwrap();
    assert_eq!(payments.len(), 4);
    let kinds: Vec<(bool, bool)> = payments
        .iter()
        .map(|vector| {
            let tx = &vector["tx"];
            (tx["Amount"].is_string(), tx["SendMax"].is_string())
        })
        .collect();
    assert!(kinds.contains(&(true, false)) && kinds.contains(&(false, true)));
    for vector in payments {
        let (wallet, tx) = common(vector);
        let memos = tx
            .get("Memos")
            .map(|memos| {
                let memo = &memos[0]["Memo"];
                vec![Memo {
                    memo_type: from_hex(text(memo, "MemoType")).unwrap(),
                    memo_data: from_hex(text(memo, "MemoData")).unwrap(),
                }]
            })
            .unwrap_or_default();
        assert_eq!(number(tx, "Flags"), 0);
        let terms = SendMaxTerms {
            destination: AccountId::parse(text(tx, "Destination")).unwrap(),
            amount: amount(&tx["Amount"]),
            send_max: amount(&tx["SendMax"]),
            fee: text(tx, "Fee").parse().unwrap(),
            sequence: number(tx, "Sequence"),
            last_ledger_sequence: number(tx, "LastLedgerSequence"),
            network_id: tx.get("NetworkID").map(|_| number(tx, "NetworkID")),
            memos,
        };
        let signed = sign_send_max(&wallet, terms).unwrap();
        assert_eq!(signed.tx.account.to_address(), text(tx, "Account"));
        let unsigned = Payment {
            signing: Signing::by(wallet.public_key()),
            ..signed.tx.clone()
        };
        check(
            vector,
            &unsigned.signing_payload(),
            &signed,
            Transaction::Payment(signed.tx.clone()),
        );
        assert_eq!(
            amount(&vector["decoded"]["SendMax"]),
            signed.tx.send_max.unwrap()
        );
    }
}
