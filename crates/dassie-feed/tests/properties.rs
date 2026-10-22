//! Properties over arbitrary input (spec/areas/rate-feed.md): decoders never
//! panic and read only canonical bytes, feeds round trip, a spoofed address
//! never gets a feed, the table stays within its bounds, and the schedule
//! keeps its intervals. Fixed seed in CI (.cargo/config.toml).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::unreachable
)]

mod support;

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use dassie_feed::{
    COOKIE_BYTES, Cookie, CookieSecret, Decision, FEED_HEARTBEAT_MS, FEED_MIN_INTERVAL_MS, Feed,
    Limits, MAX_DATAGRAM_BYTES, MAX_LEDGERS, Message, Poll, Rate, Schedule, SubscriberTable,
    assemble_feed, decode, encode_challenge, encode_subscribe, sign_feed,
};
use dassie_identity::Signature;
use dassie_node_core::rate_limit::ClientKey;
use proptest::prelude::*;
use support::{feed_of, key, two_ledger_feed};

/// Re-encodes a decoded datagram.
fn encode(message: &Message<'_>) -> Vec<u8> {
    match message {
        Message::Subscribe(cookie) => encode_subscribe(cookie.as_ref()).to_vec(),
        Message::Challenge(cookie) => encode_challenge(cookie).to_vec(),
        Message::Feed(signed) => {
            assemble_feed(signed.signed, &Signature::from_bytes(signed.signature))
        }
    }
}

const IDS: [&str; 10] = [
    "a+b",
    "stub+usd",
    "xrpl+xrp",
    "xrpl-testnet+xrp",
    "xrpl-testnet-dsusd+usd",
    "xrpl-testnet-rlusd+usd",
    "z",
    "z0",
    "z1-+",
    "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
];

/// A valid feed: a sorted subset of [`IDS`], some of their pairs, and any
/// other fields within their rules.
fn feeds() -> impl Strategy<Value = Feed<'static>> {
    (
        proptest::sample::subsequence(IDS.to_vec(), 0..=MAX_LEDGERS),
        proptest::collection::vec(any::<(bool, u64)>(), MAX_LEDGERS * MAX_LEDGERS),
        any::<(u64, u64, u64)>(),
        0..=1_000_000u32,
    )
        .prop_map(
            |(ledgers, pairs, (sequence, validity, max_packet), fee_ppm)| {
                let count = ledgers.len() as u8;
                let mut rates = Vec::new();
                for from in 0..count {
                    for to in (0..count).filter(|to| *to != from) {
                        let (present, rate) =
                            pairs[usize::from(from) * MAX_LEDGERS + usize::from(to)];
                        if present {
                            rates.push(Rate {
                                from,
                                to,
                                rate_e15: rate.max(1),
                            });
                        }
                    }
                }
                let mut feed = feed_of(&key(), ledgers, rates);
                feed.sequence_ms = sequence.min(u64::MAX - 1);
                feed.valid_until_ms =
                    feed.sequence_ms + 1 + validity % (u64::MAX - feed.sequence_ms);
                feed.fee_ppm = fee_ppm;
                for (index, ledger) in feed.ledgers.iter_mut().enumerate() {
                    ledger.max_packet_amount = max_packet.wrapping_mul(index as u64 + 1);
                }
                feed
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn decoding_never_panics_and_reads_only_canonical_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..=MAX_DATAGRAM_BYTES + 8),
    ) {
        if let Ok(message) = decode(&bytes) {
            prop_assert_eq!(&encode(&message), &bytes);
            if let Message::Feed(signed) = message {
                let _ = signed.verify();
            }
        }
    }

    #[test]
    fn a_changed_feed_never_passes_for_the_original(
        edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0..3u8), 1..4),
    ) {
        let original = sign_feed(&key(), &two_ledger_feed()).unwrap();
        let Ok(Message::Feed(signed)) = decode(&original) else { unreachable!() };
        let original_signed = signed.signed.to_vec();
        let mut bytes = original.clone();
        for (index, byte, kind) in edits {
            let at = index.index(bytes.len().max(1));
            match kind {
                0 if at < bytes.len() => bytes[at] ^= byte | 1,
                1 => bytes.insert(at.min(bytes.len()), byte),
                _ if at < bytes.len() => { bytes.remove(at); }
                _ => {}
            }
        }
        let Ok(message) = decode(&bytes) else { return Ok(()) };
        prop_assert_eq!(&encode(&message), &bytes);
        if let Message::Feed(changed) = message
            && changed.verify().is_ok()
        {
            prop_assert_eq!(changed.signed, original_signed.as_slice());
        }
    }

    #[test]
    fn a_valid_feed_round_trips_within_a_datagram(feed in feeds()) {
        let datagram = sign_feed(&key(), &feed).unwrap();
        prop_assert!(datagram.len() <= MAX_DATAGRAM_BYTES);
        let Ok(Message::Feed(signed)) = decode(&datagram) else { panic!("a feed") };
        prop_assert_eq!(&signed.feed, &feed);
        prop_assert!(signed.verify().is_ok());
        for rate in &feed.rates {
            let (from, to) = (feed.ledgers[usize::from(rate.from)].id, feed.ledgers[usize::from(rate.to)].id);
            prop_assert_eq!(feed.rate(from, to), Some(rate.rate_e15));
        }
    }
}

/// What a party does to the table.
#[derive(Clone, Debug)]
enum Op {
    /// A client at this address joins, as a real one does.
    Join(usize),
    /// The client at this address renews with the cookie it holds.
    Renew(usize),
    /// Someone sends from the victim's address (spoofed) with a guessed
    /// cookie, or one handed to this other address.
    Spoof(Option<[u8; COOKIE_BYTES]>, usize),
    /// Time passes.
    Wait(u64),
    /// The node drops what expired.
    Expire,
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let op = prop_oneof![
        4 => (0..ADDRESSES).prop_map(Op::Join),
        2 => (0..ADDRESSES).prop_map(Op::Renew),
        2 => (proptest::option::of(any::<[u8; COOKIE_BYTES]>()), 0..ADDRESSES)
            .prop_map(|(cookie, other)| Op::Spoof(cookie, other)),
        2 => (0..200_000u64).prop_map(Op::Wait),
        1 => Just(Op::Expire),
    ];
    proptest::collection::vec(op, 1..200)
}

/// How many addresses the clients use: few clients, several ports and
/// hosts each, so that clients reach their share.
const ADDRESSES: usize = 24;

fn address(index: usize) -> SocketAddr {
    let port = 4000 + (index % 3) as u16;
    match index % 4 {
        0 => SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, (index / 4) as u8)),
            port,
        ),
        1 => SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), port + index as u16),
        2 => SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(
                0x2001,
                0xdb8,
                0,
                0x100,
                index as u16,
                0,
                0,
                1,
            )),
            port,
        ),
        _ => SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, index as u16, 0, 0, 0, 0, 1)),
            port,
        ),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn a_spoofed_address_never_gets_a_feed_and_the_bounds_hold(ops in ops()) {
        let limits = Limits { max_subscribers: 6, max_per_client: 2, ttl_ms: 300_000 };
        let mut table = SubscriberTable::new(limits, CookieSecret::from_bytes([5; 32]));
        let victim = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9)), 443);
        let mut cookies: Vec<Option<Cookie>> = vec![None; ADDRESSES];
        let mut now = 1_790_000_000_000u64;
        for op in ops {
            match op {
                Op::Join(index) => {
                    let from = address(index);
                    if let Decision::Challenge(cookie) = table.handle(from, &encode_subscribe(None), now) {
                        cookies[index] = Some(cookie);
                        table.handle(from, &encode_subscribe(Some(&cookie)), now);
                    }
                }
                Op::Renew(index) => {
                    let datagram = encode_subscribe(cookies[index].as_ref());
                    match table.handle(address(index), &datagram, now) {
                        Decision::Subscribed { refresh: Some(cookie), .. } | Decision::Challenge(cookie) => {
                            cookies[index] = Some(cookie);
                        }
                        _ => {}
                    }
                }
                Op::Spoof(guess, other) => {
                    let cookie = guess.map_or(cookies[other], |bytes| Some(Cookie::from_bytes(bytes)));
                    let decision = table.handle(victim, &encode_subscribe(cookie.as_ref()), now);
                    prop_assert!(matches!(decision, Decision::Challenge(_)), "{:?}", decision);
                }
                Op::Wait(ms) => now += ms,
                Op::Expire => { table.expire(now); }
            }
            table.check_invariants();
            prop_assert!(!table.contains(victim, 0));
            prop_assert!(table.recipients(now).all(|to| to != victim));
            prop_assert!(table.len() <= limits.max_subscribers);
            // Each client's count is what its addresses in the table add
            // up to, and within its share.
            for index in 0..ADDRESSES {
                let client = ClientKey::from_ip(address(index).ip());
                let held = (0..ADDRESSES)
                    .filter(|other| ClientKey::from_ip(address(*other).ip()) == client)
                    .filter(|other| table.contains(address(*other), 0))
                    .count();
                prop_assert_eq!(table.client_count(client) as usize, held);
                prop_assert!(held <= limits.max_per_client as usize);
            }
        }
    }

    #[test]
    fn the_schedule_keeps_its_intervals(
        polls in proptest::collection::vec((0..40_000u64, any::<bool>()), 1..300),
    ) {
        let mut schedule = Schedule::new();
        let mut now = 1_790_000_000_000u64;
        let mut last: Option<(u64, u64)> = None;
        let mut changed_since: Option<u64> = None;
        for (step, changed) in polls {
            // Poll no later than the deadline it gave, as the node does.
            let deadline = match last { Some((at, _)) => at + FEED_HEARTBEAT_MS, None => now };
            now = (now + step).min(deadline.max(now));
            if changed && changed_since.is_none() {
                changed_since = Some(now);
            }
            match schedule.poll(now, changed) {
                Poll::Publish { sequence_ms, valid_until_ms, next_ms } => {
                    if let Some((at, sequence)) = last {
                        prop_assert!(sequence_ms > sequence);
                        prop_assert!(now >= at + FEED_MIN_INTERVAL_MS);
                    }
                    prop_assert!(valid_until_ms > sequence_ms);
                    prop_assert!(next_ms <= now + FEED_HEARTBEAT_MS);
                    last = Some((now, sequence_ms));
                    changed_since = None;
                }
                Poll::Wait { next_ms } => {
                    let (at, _) = last.unwrap();
                    prop_assert!(next_ms > now);
                    prop_assert!(now < at + FEED_HEARTBEAT_MS);
                    // A change waits only for the interval since the last feed.
                    if changed_since.is_some() {
                        prop_assert_eq!(next_ms, at + FEED_MIN_INTERVAL_MS);
                    }
                }
            }
        }
    }
}
