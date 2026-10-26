//! The hub's golden vectors (spec/areas/hubs.md#golden-vectors): the code
//! makes exactly the bytes `testdata/hub/vectors.json` holds, and reads
//! them as the vectors say.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[path = "support/vectors.rs"]
mod vectors;

use dassie_hub::escrow::{OPEN_MEMO_TYPE, deposit_of};
use dassie_hub::founding::starting_terms;
use dassie_xrpl::api::TxRecord;
use dassie_xrpl::codec::{Amount, Payment, Transact};
use dassie_xrpl::from_hex;
use serde_json::Value;

fn committed() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/hub/vectors.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    from_hex(value.as_str().unwrap()).unwrap()
}

// Running the generator again changes nothing: a vector that moved is a
// change to a frozen surface.
#[test]
fn the_code_makes_the_committed_vectors() {
    assert_eq!(
        vectors::all(),
        committed(),
        "regenerate with `cargo run -p dassie-hub --example hub_vectors`, and only for a new version"
    );
}

// spec/areas/hubs.md#the-escrow: the opening deposit, as a wallet outside
// Dassie writes it, opens an account under the key of its memo.
#[test]
fn the_opening_deposit_names_its_key() {
    let open = &committed()["open"];
    assert_eq!(bytes(&open["memoType"]), OPEN_MEMO_TYPE);
    assert_eq!(
        open["memoTypeText"].as_str().unwrap(),
        "https://dassie.land/hub/open/v1"
    );
    let payment = Payment::decode(&bytes(&open["txBlob"])).unwrap();
    let Amount::Xrp(drops) = payment.amount else {
        panic!("XRP");
    };
    let memo = &payment.memos[0];
    let record = TxRecord {
        hash: bytes(&open["hash"]).try_into().unwrap(),
        ledger_index: 10,
        index: 0,
        validated: true,
        account: payment.account,
        is_payment: true,
        destination: Some(payment.destination),
        destination_tag: payment.destination_tag,
        sequence: payment.sequence,
        fee: payment.fee,
        result: "tesSUCCESS".into(),
        delivered: Some(drops),
        delivered_token: None,
        memo: Some((memo.memo_type.clone(), memo.memo_data.clone())),
        changes: Vec::new(),
        lines: Vec::new(),
    };
    let sponsor = vectors::member_key().public_key();
    let terms = starting_terms([0; 32], sponsor, 1, payment.destination.0);
    let deposit = deposit_of(&record, &terms).unwrap();
    assert_eq!(deposit.tag, 0);
    assert_eq!(deposit.amount, drops);
    assert_eq!(
        deposit.key.map(|key| key.as_bytes().to_vec()),
        Some(bytes(&open["key"]))
    );
}
