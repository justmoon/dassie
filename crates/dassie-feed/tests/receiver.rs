//! A subscriber's side (spec/areas/rate-feed.md#subscribing): cookies,
//! and which feeds it takes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use core::net::{IpAddr, Ipv4Addr, SocketAddr};

use dassie_feed::{
    CookieSecret, Decision, FeedReceiver, InvalidFeed, Limits, Received, Rejected, SubscriberTable,
    WireError, encode_challenge, encode_subscribe, sign_feed,
};
use support::{key, other_key, two_ledger_feed};

#[test]
fn a_subscriber_and_a_table_agree() {
    let secret = CookieSecret::from_bytes([1; 32]);
    let mut table = SubscriberTable::new(Limits::DEFAULT, secret);
    let mut receiver = FeedReceiver::new(key().node_id());
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)), 5000);
    let now = 1_790_000_000_000;

    let Decision::Challenge(cookie) = table.handle(address, &receiver.subscribe(), now) else {
        panic!("challenged");
    };
    assert_eq!(
        receiver.receive(&encode_challenge(&cookie), now),
        Ok(Received::Challenge)
    );
    assert_eq!(receiver.subscribe(), encode_subscribe(Some(&cookie)));
    assert!(matches!(
        table.handle(address, &receiver.subscribe(), now),
        Decision::Subscribed { new: true, .. }
    ));
}

#[test]
fn only_newer_verified_unexpired_feeds_of_its_node_are_taken() {
    let key = key();
    let mut receiver = FeedReceiver::new(key.node_id());
    let feed = two_ledger_feed();
    let now = feed.sequence_ms;
    let datagram = sign_feed(&key, &feed).unwrap();

    let Ok(Received::Feed(taken)) = receiver.receive(&datagram, now) else {
        panic!("taken");
    };
    assert_eq!(taken, feed);
    assert_eq!(receiver.last_sequence_ms(), Some(feed.sequence_ms));
    // The same feed again, replayed, is old.
    assert_eq!(receiver.receive(&datagram, now), Err(Rejected::Old));

    let mut newer = two_ledger_feed();
    newer.sequence_ms += 1;
    let newer = sign_feed(&key, &newer).unwrap();
    assert_eq!(
        receiver.receive(&newer, feed.valid_until_ms),
        Err(Rejected::Expired)
    );
    assert!(matches!(
        receiver.receive(&newer, now),
        Ok(Received::Feed(_))
    ));

    // Another node's feed, though it verifies, is not this subscription's.
    let other = other_key();
    let mut theirs = two_ledger_feed();
    theirs.public_key = *other.public_key().as_bytes();
    theirs.node_id = other.node_id();
    theirs.sequence_ms += 10;
    let theirs = sign_feed(&other, &theirs).unwrap();
    assert_eq!(receiver.receive(&theirs, now), Err(Rejected::OtherNode));

    let mut forged = sign_feed(&key, &{
        let mut feed = two_ledger_feed();
        feed.sequence_ms += 20;
        feed
    })
    .unwrap();
    let last = forged.len() - 1;
    forged[last] ^= 1;
    assert_eq!(
        receiver.receive(&forged, now),
        Err(Rejected::Invalid(InvalidFeed::Signature))
    );
    assert_eq!(receiver.last_sequence_ms(), Some(feed.sequence_ms + 1));

    assert_eq!(
        receiver.receive(&encode_subscribe(None), now),
        Err(Rejected::NotForUs)
    );
    assert!(matches!(
        receiver.receive(&[], now),
        Err(Rejected::Wire(WireError::Decode(_)))
    ));
}
