//! A mainnet `AccountDelete` (testdata/xrpl/capture-account-delete.mjs):
//! what it did to accounts' XRP, as both decoders read it. The ledger
//! empties a root before it deletes it, so the deleted root's final balance
//! is zero and its balance before is in its previous fields.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_xrpl::api::{self, TxRecord};
use dassie_xrpl::{AccountId, from_hex, hex_upper, records};
use serde_json::Value;

const SENDER: &str = "rDcqo5xKvMf1SwnEmqDkTV1WRPmuzHJTPc";
const DESTINATION: &str = "rPEoau9PDuEb14GgsPy8HJM9zsbT6JKpng";

fn data() -> Value {
    serde_json::from_str(include_str!("data/account-delete.json")).unwrap()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

/// The record from the binary, and from the JSON.
fn records() -> (TxRecord, TxRecord) {
    let data = data();
    let tx = from_hex(text(&data, "tx_blob")).unwrap();
    let meta = from_hex(text(&data, "meta")).unwrap();
    let ledger = u32::try_from(data["ledger_index"].as_u64().unwrap()).unwrap();
    let decoded = records::transaction(&tx, &meta, ledger, &[]).unwrap();
    let read = api::tx_result(&data["json"]).unwrap();
    (decoded.record, read)
}

fn change(record: &TxRecord, address: &str) -> (u64, u64) {
    let change = record
        .change_of(AccountId::parse(address).unwrap())
        .unwrap();
    (change.before, change.after)
}

// The transaction itself, and what it paid the destination, read alike
// from the binary and the JSON.
#[test]
fn an_account_delete_reads_alike_from_binary_and_json() {
    let (decoded, read) = records();
    assert_eq!(decoded, read);
    assert_eq!(hex_upper(&decoded.hash), text(&data(), "hash"));
    assert_eq!(decoded.ledger_index, 87_242_346);
    assert_eq!(decoded.index, 37);
    assert_eq!(decoded.result, "tesSUCCESS");
    assert!(!decoded.is_payment);
    assert_eq!(decoded.fee, 2_000_000);
    assert_eq!(
        change(&decoded, DESTINATION),
        (211_721_772_946, 211_729_772_946)
    );
}

// The deleted root held 10 XRP before, not the zero it was left with: the
// fee and what the destination got.
#[test]
fn a_deleted_root_s_balance_before_is_its_previous_one() {
    let (decoded, read) = records();
    assert_eq!(change(&decoded, SENDER), (10_000_000, 0));
    assert_eq!(change(&read, SENDER), (10_000_000, 0));
}
