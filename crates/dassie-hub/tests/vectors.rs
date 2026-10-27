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

// spec/decisions/hub-protocol-version.md: what the vectors hold reads back
// as the code reads it, within the version the hub ID commits to.
#[test]
fn the_vectors_read_as_they_say() {
    use dassie_hub::Rules;
    use dassie_hub::consensus::wire::Message;
    use dassie_hub::operation::Operation;
    use dassie_hub::promise::{Claim, Commitment, Signed};
    use dassie_hub::rules::HUB_VERSION;
    use dassie_hub::service::payload::Payload;
    use dassie_oer::{Encode, Reader};

    let vectors = committed();
    let read = |value: &Value| bytes(value);
    // The rules, and the ID that commits to their version.
    let rules_bytes = read(&vectors["rules"]["rules"]);
    let mut reader = Reader::new(&rules_bytes);
    let rules = Rules::decode(&mut reader).unwrap();
    reader.finish().unwrap();
    assert_eq!(rules.check(), Ok(()));
    assert_eq!(rules.terms.version, HUB_VERSION);
    assert_eq!(vectors["rules"]["version"], HUB_VERSION);
    assert_eq!(
        rules.id().0.as_bytes().to_vec(),
        read(&vectors["rules"]["hubId"])
    );
    let terms = read(&vectors["rules"]["terms"]);
    assert_eq!(terms[..2], HUB_VERSION.to_be_bytes(), "the version first");
    // Another version is refused before anything else is read.
    let mut later = rules_bytes;
    later[1] = later[1].wrapping_add(1);
    assert!(Rules::decode(&mut Reader::new(&later)).is_err());
    // Operations and promises.
    for kind in ["member", "validator"] {
        let encoded = read(&vectors["operations"][kind]["operation"]);
        let mut reader = Reader::new(&encoded);
        let operation = Operation::decode(&mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(operation.to_vec(), encoded);
        assert_eq!(
            operation.hash().as_bytes().to_vec(),
            read(&vectors["operations"][kind]["hash"])
        );
    }
    let commitment = read(&vectors["promises"]["commitment"]);
    let commitment = Signed::<Commitment>::decode(&mut Reader::new(&commitment)).unwrap();
    assert!(commitment.verifies(&rules.id(), &vectors::member_key().public_key()));
    let claim = read(&vectors["promises"]["claim"]);
    let claim = Signed::<Claim>::decode(&mut Reader::new(&claim)).unwrap();
    assert!(claim.verifies(&rules.id(), &vectors::member_key().public_key()));
    // The block's payload names the state the ledger leaves, and the vote
    // is for the block.
    let payload = Payload::decode_exact(&read(&vectors["block"]["payload"])).unwrap();
    assert_eq!(
        payload.state.as_bytes().to_vec(),
        read(&vectors["block"]["state"])
    );
    let Ok(Message::Vote(vote)) = Message::decode_message(&read(&vectors["block"]["vote"])) else {
        panic!("a vote");
    };
    assert_eq!(
        vote.block.as_bytes().to_vec(),
        read(&vectors["block"]["blockDigest"])
    );
    let set = rules.validator_set().unwrap();
    assert!(vote.verify(&rules.id().0, &set).is_ok());
    let statements =
        dassie_hub::client::SignedStatement::decode_list(&read(&vectors["block"]["statements"]))
            .unwrap();
    assert!(statements.iter().all(|s| s.verifies(&rules.id(), &set)));
}
