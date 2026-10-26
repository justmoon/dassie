//! The golden vectors of `testdata/feed` (spec/areas/rate-feed.md#test-vectors):
//! the code reads every datagram as the vector says, and makes exactly its
//! bytes again. Regenerate them only for a new version of the feed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

mod support;

use dassie_feed::{
    Cookie, Feed, InvalidFeed, Ledger, MAX_FEED_BYTES, Message, Rate, WireError, decode,
    encode_challenge, encode_subscribe, sign_feed,
};
use serde_json::Value;
use support::{hex, key, unhex};

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../../testdata/feed/vectors.json")).unwrap()
}

fn cookie(value: &Value) -> Cookie {
    Cookie::from_bytes(unhex(value.as_str().unwrap()).try_into().unwrap())
}

#[test]
fn the_key_is_the_one_the_vectors_name() {
    let vectors = vectors();
    let key = key();
    assert_eq!(vectors["seed"].as_str().unwrap(), hex(&support::SEED));
    assert_eq!(
        vectors["publicKey"].as_str().unwrap(),
        hex(key.public_key().as_bytes())
    );
    assert_eq!(vectors["nodeId"].as_str().unwrap(), key.node_id().as_str());
}

#[test]
fn subscribes_and_challenges() {
    let vectors = vectors();
    for vector in vectors["subscribe"].as_array().unwrap() {
        let datagram = unhex(vector["datagram"].as_str().unwrap());
        let expected = vector["cookie"].as_str().map(|_| cookie(&vector["cookie"]));
        assert_eq!(decode(&datagram).unwrap(), Message::Subscribe(expected));
        assert_eq!(encode_subscribe(expected.as_ref()).as_slice(), datagram);
    }
    for vector in vectors["challenge"].as_array().unwrap() {
        let datagram = unhex(vector["datagram"].as_str().unwrap());
        let expected = cookie(&vector["cookie"]);
        assert_eq!(decode(&datagram).unwrap(), Message::Challenge(expected));
        assert_eq!(encode_challenge(&expected).as_slice(), datagram);
    }
}

#[test]
fn feeds() {
    let vectors = vectors();
    let key = key();
    let feeds = vectors["feeds"].as_array().unwrap();
    assert!(!feeds.is_empty());
    for vector in feeds {
        let name = vector["name"].as_str().unwrap();
        let datagram = unhex(vector["datagram"].as_str().unwrap());
        let Message::Feed(signed) = decode(&datagram).unwrap() else {
            panic!("{name}: not a feed");
        };
        assert_eq!(signed.verify().unwrap(), key.public_key(), "{name}");
        let expected = Feed {
            public_key: *key.public_key().as_bytes(),
            sequence_ms: vector["sequenceMs"].as_u64().unwrap(),
            valid_until_ms: vector["validUntilMs"].as_u64().unwrap(),
            fee_ppm: vector["feePpm"].as_u64().unwrap() as u32,
            ledgers: vector["ledgers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|ledger| Ledger {
                    id: ledger[0].as_str().unwrap(),
                    max_packet_amount: ledger[1].as_u64().unwrap(),
                })
                .collect(),
            rates: vector["rates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|rate| Rate {
                    from: rate[0].as_u64().unwrap() as u8,
                    to: rate[1].as_u64().unwrap() as u8,
                    rate_e15: rate[2].as_u64().unwrap(),
                })
                .collect(),
        };
        assert_eq!(signed.feed, expected, "{name}");
        assert_eq!(sign_feed(&key, &expected).unwrap(), datagram, "{name}");
    }
    let largest = feeds
        .iter()
        .find(|vector| vector["name"] == "largest")
        .unwrap();
    let length = largest["datagram"].as_str().unwrap().len() / 2;
    assert_eq!(length, MAX_FEED_BYTES, "as large as a feed gets");
}

#[test]
fn refused() {
    let vectors = vectors();
    let refused = vectors["refused"].as_array().unwrap();
    assert!(!refused.is_empty());
    for vector in refused {
        let name = vector["name"].as_str().unwrap();
        let datagram = unhex(vector["datagram"].as_str().unwrap());
        let error = match decode(&datagram) {
            Ok(Message::Feed(signed)) => match signed.verify() {
                Err(InvalidFeed::Signature) => "Signature",
                Err(InvalidFeed::PublicKey) => "PublicKey",
                other => panic!("{name}: {other:?}"),
            },
            Err(WireError::Version(_)) => "Version",
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(error, vector["error"].as_str().unwrap(), "{name}");
    }
}
