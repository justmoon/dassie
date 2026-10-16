//! Nothing that comes from the network makes the crate panic (spec 7 §7).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_ledger::Name;
use dassie_settlement::{LedgerInput, SettlementScheme};
use dassie_xrpl::codec::Payment;
use dassie_xrpl::{AccountId, XrplScheme, api, base58};
use proptest::prelude::*;

fn scheme() -> XrplScheme {
    XrplScheme::new("", &[5; 32], Name::new("node").unwrap(), Some(1_000), &[]).unwrap()
}

/// Messages that look like the ledger's, with any values.
fn ledger_message() -> impl Strategy<Value = Vec<u8>> {
    let value = prop_oneof![
        Just("null".to_string()),
        any::<u64>().prop_map(|n| n.to_string()),
        any::<i64>().prop_map(|n| n.to_string()),
        "[0-9A-Fa-f]{0,70}".prop_map(|s| format!("\"{s}\"")),
        "r[1-9A-Za-z]{20,40}".prop_map(|s| format!("\"{s}\"")),
    ];
    (
        prop_oneof![Just("response"), Just("transaction"), Just("ledgerClosed")],
        prop::collection::vec(value, 12),
        0u64..8,
    )
        .prop_map(|(kind, v, id)| {
            format!(
                r#"{{"type":"{kind}","id":{id},"status":"success","validated":true,"ledger_index":{0},
                "fee_base":{1},"reserve_base":{2},"reserve_inc":{3},
                "result":{{"state":{{"validated_ledger":{{"seq":{0},"base_fee":{1},"reserve_base":{2},"reserve_inc":{3}}}}},
                  "account_data":{{"Balance":{4},"Sequence":{5},"OwnerCount":{6}}},"ledger_index":{0},
                  "transactions":[{{"tx":{{"Account":{7},"Sequence":{5},"Fee":{4},"hash":{8}}},"meta":{{"TransactionIndex":{9},"TransactionResult":"tesSUCCESS"}}}}],
                  "ledger_index_max":{10},"engine_result":"tem"}},
                "transaction":{{"TransactionType":"Payment","Account":{7},"Destination":{7},"Fee":{4},"Sequence":{5},"hash":{8},
                  "Memos":[{{"Memo":{{"MemoType":{8},"MemoData":{11}}}}}]}},
                "meta":{{"TransactionIndex":{9},"TransactionResult":"tesSUCCESS","delivered_amount":{4},
                  "AffectedNodes":[{{"ModifiedNode":{{"LedgerEntryType":"AccountRoot","FinalFields":{{"Account":{7},"Balance":{4}}},"PreviousFields":{{"Balance":{6}}}}}}}]}}}}"#,
                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], v[8], v[9], v[10], v[11]
            )
            .into_bytes()
        })
}

proptest! {
    #[test]
    fn decoders_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..400), text in "\\PC{0,60}") {
        let _ = Payment::decode(&bytes);
        let _ = api::parse(&bytes);
        let _ = base58::decode_check(&text, &[0]);
        let _ = AccountId::parse(&text);
    }

    #[test]
    fn the_scheme_takes_anything_the_ledger_says(
        messages in prop::collection::vec(prop_oneof![ledger_message(), prop::collection::vec(any::<u8>(), 0..64)], 1..30),
    ) {
        let mut scheme = scheme();
        let mut effects = Vec::new();
        scheme.on_ledger(1, LedgerInput::Connected, &mut effects);
        for (at, message) in messages.iter().enumerate() {
            scheme.on_ledger(at as u64, LedgerInput::Message(message), &mut effects);
            let _ = scheme.state();
            let _ = scheme.balance();
        }
        scheme.on_timer(u64::MAX / 2, &mut effects);
        // The state it keeps is one it can start from again.
        let state = scheme.state();
        prop_assert!(XrplScheme::new("", &[5; 32], Name::new("node").unwrap(), Some(scheme.balance()), &state).is_ok());
    }
}
