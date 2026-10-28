//! The datagrams' rules (spec/areas/rate-feed.md#wire-format): what is
//! refused, and why.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::type_complexity
)]

mod support;

use dassie_feed::{
    CHALLENGE_BYTES, Cookie, FEED_OID, Feed, InvalidFeed, MAX_DATAGRAM_BYTES, MAX_METHOD_ID_BYTES,
    MAX_METHODS, MAX_PPM, Message, Rate, SUBSCRIBE_BYTES, VERSION, WireError, assemble_feed,
    decode, encode_challenge, encode_subscribe, sign_feed,
};
use dassie_oer::DecodeErrorKind;
use support::{empty_feed, key, largest_feed, longest_ids, other_key, two_method_feed};

fn rate(from: u8, to: u8, rate_e15: u64) -> Rate {
    Rate { from, to, rate_e15 }
}

fn decode_kind(datagram: &[u8]) -> DecodeErrorKind {
    match decode(datagram) {
        Err(WireError::Decode(error)) => error.kind,
        other => panic!("{other:?}"),
    }
}

/// Encodes `feed` past the checks of [`Feed::check`], as a broken signer
/// would, by patching a field after encoding a valid feed.
fn signed_with(feed: &Feed<'_>, patch: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut signed = feed.signed_bytes().unwrap();
    patch(&mut signed);
    assemble_feed(&signed, &key().sign(&signed))
}

#[test]
fn a_subscribe_is_padded_to_its_size_with_zeros() {
    let cookie = Cookie::from_bytes([7; 16]);
    for datagram in [encode_subscribe(None), encode_subscribe(Some(&cookie))] {
        assert_eq!(datagram.len(), SUBSCRIBE_BYTES);
        assert!(datagram.len() >= CHALLENGE_BYTES);
        assert!(matches!(decode(&datagram), Ok(Message::Subscribe(_))));

        let mut short = datagram.to_vec();
        short.pop();
        assert_eq!(decode_kind(&short), DecodeErrorKind::SizeOutOfRange);
        let mut long = datagram.to_vec();
        long.push(0);
        assert_eq!(decode_kind(&long), DecodeErrorKind::SizeOutOfRange);
        let mut dirty = datagram;
        dirty[SUBSCRIBE_BYTES - 1] = 1;
        assert_eq!(decode_kind(&dirty), DecodeErrorKind::UnexpectedValue);
    }
    let mut unknown = encode_subscribe(None);
    unknown[2] = 0x82;
    assert_eq!(decode_kind(&unknown), DecodeErrorKind::UnexpectedTag);
}

#[test]
fn a_challenge_is_its_cookie() {
    let cookie = Cookie::from_bytes([9; 16]);
    let datagram = encode_challenge(&cookie);
    assert_eq!(decode(&datagram), Ok(Message::Challenge(cookie)));
    assert_eq!(
        decode_kind(&datagram[..CHALLENGE_BYTES - 1]),
        DecodeErrorKind::UnexpectedEnd
    );
    let mut long = datagram.to_vec();
    long.push(0);
    assert_eq!(decode_kind(&long), DecodeErrorKind::TrailingBytes);
}

// Version 2 names settlement methods, under an OID of its own; a datagram
// of version 1, which named ledgers, or of any other is refused
// (spec/areas/rate-feed.md#wire-format).
#[test]
fn another_version_or_message_is_refused() {
    assert_eq!(VERSION, 2);
    let arc = dassie_oer::oid::encode::<14>(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 3, 2]);
    assert_eq!(FEED_OID, arc);
    let mut datagram = encode_challenge(&Cookie::from_bytes([0; 16]));
    for version in [1, VERSION + 1] {
        datagram[0] = version;
        assert_eq!(decode(&datagram), Err(WireError::Version(version)));
    }
    datagram[0] = VERSION;
    datagram[1] = 0x83;
    assert_eq!(decode_kind(&datagram), DecodeErrorKind::UnexpectedTag);
    assert_eq!(decode_kind(&[]), DecodeErrorKind::UnexpectedEnd);
    assert_eq!(
        decode(&[0; MAX_DATAGRAM_BYTES + 1]),
        Err(WireError::TooLong)
    );
}

#[test]
fn a_feed_round_trips_and_verifies() {
    let key = key();
    let ids = longest_ids();
    for feed in [empty_feed(), two_method_feed(), largest_feed(&ids)] {
        let datagram = sign_feed(&key, &feed).unwrap();
        assert!(datagram.len() <= MAX_DATAGRAM_BYTES);
        let Ok(Message::Feed(signed)) = decode(&datagram) else {
            panic!("not a feed");
        };
        assert_eq!(signed.feed, feed);
        assert_eq!(signed.verify(), Ok(key.public_key()));
    }
}

#[test]
fn rates_are_looked_up_by_method() {
    let feed = two_method_feed();
    let (dsusd, xrp) = ("hub-xrpl-testnet-dsusd", "hub-xrpl-testnet-xrp");
    assert_eq!(feed.rate(dsusd, xrp), Some(2_345_000_000_000_000));
    assert_eq!(feed.rate(xrp, dsusd), Some(421_000_000_000_000));
    assert_eq!(feed.rate(xrp, xrp), None);
    assert_eq!(feed.rate("stub", xrp), None);
    // A ledger's ID names no method.
    assert_eq!(feed.rate("hub-xrpl-testnet-xrp+xrp", dsusd), None);
}

#[test]
fn signing_refuses_a_feed_that_breaks_a_rule() {
    let key = key();
    let cases: [(fn(&mut Feed<'static>), InvalidFeed); 12] = [
        (|feed| feed.fee_ppm = MAX_PPM + 1, InvalidFeed::FeePpm),
        (
            |feed| feed.valid_until_ms = feed.sequence_ms,
            InvalidFeed::ValidUntil,
        ),
        (
            |feed| feed.methods = vec![feed.methods[0]; MAX_METHODS + 1],
            InvalidFeed::TooManyMethods,
        ),
        (|feed| feed.methods[0].id = "XRP", InvalidFeed::MethodId),
        (|feed| feed.methods[0].id = "", InvalidFeed::MethodId),
        (|feed| feed.methods[0].id = "a", InvalidFeed::MethodId),
        // A ledger's ID is not a method's.
        (|feed| feed.methods[0].id = "hub+usd", InvalidFeed::MethodId),
        (|feed| feed.methods.swap(0, 1), InvalidFeed::MethodOrder),
        (|feed| feed.rates[0].to = 2, InvalidFeed::RatePair),
        (|feed| feed.rates[0].to = 0, InvalidFeed::RatePair),
        (|feed| feed.rates.swap(0, 1), InvalidFeed::RateOrder),
        (|feed| feed.rates[1].rate_e15 = 0, InvalidFeed::ZeroRate),
    ];
    for (break_it, expected) in cases {
        let mut feed = two_method_feed();
        break_it(&mut feed);
        assert_eq!(sign_feed(&key, &feed), Err(expected));
    }
    let mut feed = two_method_feed();
    feed.rates.push(rate(0, 1, 5));
    assert_eq!(sign_feed(&key, &feed), Err(InvalidFeed::RateOrder));
    let long = "a".repeat(MAX_METHOD_ID_BYTES + 1);
    let mut feed = two_method_feed();
    feed.methods[0].id = &long;
    assert_eq!(sign_feed(&key, &feed), Err(InvalidFeed::MethodId));
}

#[test]
fn signing_refuses_another_nodes_feed() {
    let feed = two_method_feed();
    assert_eq!(sign_feed(&other_key(), &feed), Err(InvalidFeed::PublicKey));
}

#[test]
fn decoding_refuses_a_feed_that_breaks_a_rule() {
    // The fee sits right after the OID (14), the key (32) and two u64s.
    let feed = empty_feed();
    let fee_at = 14 + 32 + 16;
    let datagram = signed_with(&feed, |signed| {
        signed[fee_at..fee_at + 4].copy_from_slice(&(MAX_PPM + 1).to_be_bytes());
    });
    assert_eq!(
        decode(&datagram),
        Err(WireError::Invalid(InvalidFeed::FeePpm))
    );

    let mut feed = two_method_feed();
    feed.rates[1].rate_e15 = 1;
    let datagram = signed_with(&feed, |signed| {
        let last = signed.len() - 1;
        signed[last] = 0;
    });
    assert_eq!(
        decode(&datagram),
        Err(WireError::Invalid(InvalidFeed::ZeroRate))
    );

    let datagram = signed_with(&two_method_feed(), |signed| {
        let from = signed.len() - 10;
        signed[from] = 7;
    });
    assert_eq!(
        decode(&datagram),
        Err(WireError::Invalid(InvalidFeed::RatePair))
    );
}

#[test]
fn decoding_refuses_lists_past_their_bounds() {
    // A count of methods past the bound is refused before any is read.
    let feed = empty_feed();
    let count_at = 14 + 32 + 8 + 8 + 4;
    let datagram = signed_with(&feed, |signed| {
        signed[count_at + 1] = (MAX_METHODS + 1) as u8;
    });
    assert_eq!(decode_kind(&datagram), DecodeErrorKind::LimitExceeded);
}

#[test]
fn verifying_checks_exactly_the_bytes_received() {
    let key = key();
    let datagram = sign_feed(&key, &two_method_feed()).unwrap();
    // Any change to the signed part that still decodes fails the signature:
    // here the first method's largest packet, which no rule constrains.
    let first_id = "hub-xrpl-testnet-dsusd".len();
    let at = 2 + 2 + 14 + 32 + 8 + 8 + 4 + 2 + 1 + first_id + 7;
    let mut changed = datagram.clone();
    changed[at] ^= 1;
    let Ok(Message::Feed(signed)) = decode(&changed) else {
        panic!("still a feed");
    };
    assert_eq!(signed.verify(), Err(InvalidFeed::Signature));
}
