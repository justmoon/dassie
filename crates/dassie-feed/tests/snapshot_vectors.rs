//! The snapshot's golden vectors, `testdata/feed/snapshots.json`
//! (spec/areas/rate-feed.md#test-vectors): the code reads every datagram as
//! the vector says, makes exactly its bytes again, and prices the swaps it
//! names alike. Regenerate them only for a new version of the snapshot, or
//! while no release carries this one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

mod support;

use dassie_feed::number::{STEP_FRACTIONS, amount16_floor};
use dassie_feed::snapshot::TAG_DOMAIN;
use dassie_feed::{
    InvalidSnapshot, Ladder, MAX_SNAPSHOT_BYTES, MethodQuotes, Quote, Ratio, Snapshot,
    SnapshotMessage, SnapshotWireError, decode_snapshot_datagram, encode_request, method_tag,
    sign_snapshot,
};
use dassie_identity::PublicKey;
use serde_json::Value;
use support::{hex, key, two_method_snapshot, unhex};

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../../testdata/feed/snapshots.json")).unwrap()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn array<const N: usize>(value: &Value) -> [u8; N] {
    unhex(text(value)).try_into().unwrap()
}

fn word(value: &Value) -> u32 {
    u32::from_str_radix(text(value), 16).unwrap()
}

#[test]
fn the_key_and_the_tags() {
    let vectors = vectors();
    assert_eq!(text(&vectors["seed"]), hex(&support::SEED));
    assert_eq!(
        text(&vectors["publicKey"]),
        hex(key().public_key().as_bytes())
    );
    assert_eq!(text(&vectors["tagDomain"]).as_bytes(), TAG_DOMAIN);
    for vector in vectors["tags"].as_array().unwrap() {
        let id = text(&vector["schemeId"]);
        assert_eq!(hex(&method_tag(id)), text(&vector["tag"]), "{id}");
    }
}

#[test]
fn numbers() {
    let vectors = vectors();
    for vector in vectors["quotes"].as_array().unwrap() {
        let numerator: u128 = text(&vector["numerator"]).parse().unwrap();
        let denominator: u128 = text(&vector["denominator"]).parse().unwrap();
        assert_eq!(
            Quote::floor(numerator, denominator).to_bits(),
            word(&vector["floor"])
        );
        assert_eq!(
            Quote::ceil(numerator, denominator).to_bits(),
            word(&vector["ceil"])
        );
    }
    for vector in vectors["firstAmounts"].as_array().unwrap() {
        let amount = vector["amount"].as_u64().unwrap();
        assert_eq!(
            format!("{:04x}", amount16_floor(amount)),
            text(&vector["bits"])
        );
    }
    let fractions: Vec<String> = STEP_FRACTIONS
        .iter()
        .map(|fraction| format!("{fraction:016x}"))
        .collect();
    let pinned: Vec<&str> = vectors["stepFractions"]
        .as_array()
        .unwrap()
        .iter()
        .map(text)
        .collect();
    assert_eq!(fractions, pinned);
    for vector in vectors["ladders"].as_array().unwrap() {
        let amounts: [u64; 8] = serde_json::from_value(vector["amounts"].clone()).unwrap();
        let decoded: [u64; 8] = serde_json::from_value(vector["decoded"].clone()).unwrap();
        let ladder = Ladder::floor(amounts);
        assert_eq!(hex(&ladder.to_bytes()), text(&vector["bytes"]));
        assert_eq!(Ladder::from_bytes(&array(&vector["bytes"])), Some(ladder));
        assert_eq!(ladder.amounts(), decoded);
    }
}

fn method(vector: &Value) -> MethodQuotes {
    MethodQuotes {
        tag: array(&vector["tag"]),
        bid: Quote::from_bits(word(&vector["bid"])).unwrap(),
        ask: Quote::from_bits(word(&vector["ask"])).unwrap(),
        bid_ladder: Ladder::from_bytes(&array(&vector["bidLadder"])).unwrap(),
        ask_ladder: Ladder::from_bytes(&array(&vector["askLadder"])).unwrap(),
    }
}

#[test]
fn requests_and_snapshots() {
    let vectors = vectors();
    let request = unhex(text(&vectors["request"]));
    assert_eq!(request, encode_request());
    assert_eq!(
        decode_snapshot_datagram(&request),
        Ok(SnapshotMessage::Request)
    );
    let key = key();
    let snapshots = vectors["snapshots"].as_array().unwrap();
    assert!(!snapshots.is_empty());
    for vector in snapshots {
        let name = text(&vector["name"]);
        let datagram = unhex(text(&vector["datagram"]));
        let Ok(SnapshotMessage::Snapshot(signed)) = decode_snapshot_datagram(&datagram) else {
            panic!("{name}: not a snapshot");
        };
        signed.verify(&key.public_key()).unwrap();
        let expected = Snapshot {
            sequence_ms: vector["sequenceMs"].as_u64().unwrap(),
            valid_until_ms: vector["validUntilMs"].as_u64().unwrap(),
            grid_bp: serde_json::from_value(vector["gridBp"].clone()).unwrap(),
            methods: vector["methods"]
                .as_array()
                .unwrap()
                .iter()
                .map(method)
                .collect(),
        };
        assert_eq!(signed.snapshot, expected, "{name}");
        assert_eq!(sign_snapshot(&key, &expected).unwrap(), datagram, "{name}");
    }
    let largest = snapshots
        .iter()
        .find(|vector| vector["name"] == "largest")
        .unwrap();
    assert_eq!(
        text(&largest["datagram"]).len() / 2,
        MAX_SNAPSHOT_BYTES,
        "as large as a snapshot gets"
    );
}

#[test]
fn swaps() {
    let vectors = vectors();
    let snapshot = two_method_snapshot();
    let swaps = vectors["swaps"].as_array().unwrap();
    assert!(!swaps.is_empty());
    for vector in swaps {
        let from = method_tag(text(&vector["from"]));
        let to = method_tag(text(&vector["to"]));
        let amount_in = vector["amountIn"].as_u64().unwrap();
        if let Some(threshold) = vector["threshold"].as_array() {
            let threshold = Ratio::new(
                threshold[0].as_u64().unwrap(),
                threshold[1].as_u64().unwrap(),
            )
            .unwrap();
            let swap = snapshot.swap_at_least(from, to, threshold).unwrap();
            assert_eq!(swap.amount_in, amount_in, "{vector}");
            assert_eq!(
                Some(swap.amount_out),
                vector["amountOut"].as_u64(),
                "{vector}"
            );
        } else if let Some(start) = vector["start"].as_u64() {
            let out = snapshot.packet_out(from, to, start, amount_in);
            assert_eq!(out, vector["amountOut"].as_u64(), "{vector}");
        } else {
            let out = snapshot.swap_out(from, to, amount_in);
            assert_eq!(out, vector["amountOut"].as_u64(), "{vector}");
        }
    }
}

#[test]
fn refused() {
    let vectors = vectors();
    let refused = vectors["refused"].as_array().unwrap();
    assert!(!refused.is_empty());
    for vector in refused {
        let name = text(&vector["name"]);
        let datagram = unhex(text(&vector["datagram"]));
        let checked_against = vector["key"].as_str().map_or_else(
            || key().public_key(),
            |weak| PublicKey::from_bytes(&unhex(weak).try_into().unwrap()).unwrap(),
        );
        let error = match decode_snapshot_datagram(&datagram) {
            Ok(SnapshotMessage::Snapshot(signed)) => match signed.verify(&checked_against) {
                Err(InvalidSnapshot::Signature) => "Signature",
                Err(InvalidSnapshot::PublicKey) => "PublicKey",
                other => panic!("{name}: {other:?}"),
            },
            Err(SnapshotWireError::Version(_)) => "Version",
            Err(SnapshotWireError::Decode(_)) => "Decode",
            Err(SnapshotWireError::Invalid(invalid)) => match invalid {
                InvalidSnapshot::ValidUntil => "ValidUntil",
                InvalidSnapshot::Grid => "Grid",
                InvalidSnapshot::TagOrder => "TagOrder",
                InvalidSnapshot::Quote => "Quote",
                InvalidSnapshot::Ladder => "Ladder",
                InvalidSnapshot::ClosedLadder => "ClosedLadder",
                other => panic!("{name}: {other:?}"),
            },
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(error, text(&vector["error"]), "{name}");
    }
}
