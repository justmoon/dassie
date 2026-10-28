//! Snapshots, requests and their readers (spec/areas/rate-feed.md#snapshots).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use dassie_feed::{
    Answer, AnswerLimits, Answerer, InvalidSnapshot, LEVELS, Ladder, MAX_DATAGRAM_BYTES,
    MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_METHODS, Message, Quote, REQUEST_BYTES, Ratio,
    SnapshotMessage, SnapshotReader, SnapshotRejected, SnapshotWireError, Unanswered,
    attach_signature, check_request, decode, decode_snapshot_datagram, encode_request,
    encode_subscribe, method_tag, sign_feed, sign_snapshot, snapshot_signing_message,
};
use dassie_identity::PublicKey;
use support::{
    closed_side_snapshot, empty_snapshot, key, largest_snapshot, open_method, other_key,
    snapshot_of, two_method_feed, two_method_snapshot,
};

const DSUSD: &str = "hub-xrpl-testnet-dsusd";
const XRP: &str = "hub-xrpl-testnet-xrp";

fn signed(snapshot: &dassie_feed::Snapshot) -> Vec<u8> {
    sign_snapshot(&key(), snapshot).unwrap()
}

fn peer() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)), 40_000)
}

#[test]
fn a_snapshot_round_trips_and_verifies() {
    for snapshot in [
        empty_snapshot(),
        two_method_snapshot(),
        closed_side_snapshot(),
        largest_snapshot(),
    ] {
        let bytes = signed(&snapshot);
        let Ok(SnapshotMessage::Snapshot(decoded)) = decode_snapshot_datagram(&bytes) else {
            panic!("not a snapshot");
        };
        assert_eq!(decoded.snapshot, snapshot);
        assert_eq!(decoded.bytes, bytes.as_slice());
        decoded.verify(&key().public_key()).unwrap();
        assert_eq!(
            decoded.verify(&other_key().public_key()),
            Err(InvalidSnapshot::Signature)
        );
    }
}

#[test]
fn the_largest_snapshot_fills_its_bound() {
    let bytes = signed(&largest_snapshot());
    assert_eq!(bytes.len(), MAX_SNAPSHOT_BYTES);
    assert_eq!(encode_request().len(), REQUEST_BYTES);
    // One method more is refused, before anything is written.
    let mut snapshot = largest_snapshot();
    let mut extra = snapshot.methods[0];
    extra.tag = [0xff; 4];
    snapshot.methods.push(extra);
    assert_eq!(snapshot.methods.len(), MAX_SNAPSHOT_METHODS + 1);
    assert_eq!(
        sign_snapshot(&key(), &snapshot),
        Err(InvalidSnapshot::TooManyMethods)
    );
}

#[test]
fn the_signature_covers_the_oid_and_every_byte() {
    let snapshot = two_method_snapshot();
    let unsigned = snapshot.unsigned_bytes().unwrap();
    let message = snapshot_signing_message(&unsigned);
    assert_eq!(
        &message[..dassie_feed::FEED_OID.len()],
        dassie_feed::FEED_OID
    );
    let signature = key().sign(&message);
    let bytes = attach_signature(unsigned.clone(), &signature);
    assert_eq!(bytes, signed(&snapshot));
    // Signed without the OID: refused.
    let bare = attach_signature(unsigned.clone(), &key().sign(&unsigned));
    let Ok(SnapshotMessage::Snapshot(decoded)) = decode_snapshot_datagram(&bare) else {
        panic!("not a snapshot");
    };
    assert_eq!(
        decoded.verify(&key().public_key()),
        Err(InvalidSnapshot::Signature)
    );
}

#[test]
fn a_weak_key_is_refused() {
    let snapshot = signed(&two_method_snapshot());
    let Ok(SnapshotMessage::Snapshot(decoded)) = decode_snapshot_datagram(&snapshot) else {
        panic!("not a snapshot");
    };
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let weak = PublicKey::from_bytes(&identity).unwrap();
    assert_eq!(decoded.verify(&weak), Err(InvalidSnapshot::PublicKey));
}

#[test]
fn content_rules() {
    let mut late = two_method_snapshot();
    late.valid_until_ms = late.sequence_ms;
    assert_eq!(late.check(), Err(InvalidSnapshot::ValidUntil));

    let mut flat = two_method_snapshot();
    flat.grid_bp[3] = flat.grid_bp[2];
    assert_eq!(flat.check(), Err(InvalidSnapshot::Grid));
    let mut whole = two_method_snapshot();
    whole.grid_bp[LEVELS - 1] = 10_000;
    assert_eq!(whole.check(), Err(InvalidSnapshot::Grid));
    let mut zero = two_method_snapshot();
    zero.grid_bp[0] = 0;
    assert_eq!(zero.check(), Ok(()));

    let mut unordered = two_method_snapshot();
    unordered.methods.reverse();
    assert_eq!(unordered.check(), Err(InvalidSnapshot::TagOrder));
    let mut twice = two_method_snapshot();
    twice.methods[1].tag = twice.methods[0].tag;
    assert_eq!(twice.check(), Err(InvalidSnapshot::TagOrder));

    let mut closed = two_method_snapshot();
    closed.methods[0].bid = Quote::ZERO;
    assert_eq!(closed.check(), Err(InvalidSnapshot::ClosedLadder));
    closed.methods[0].bid_ladder = Ladder::ZERO;
    assert_eq!(closed.check(), Ok(()));
    // An open side may have an empty ladder.
    let mut shallow = two_method_snapshot();
    shallow.methods[0].ask_ladder = Ladder::ZERO;
    assert_eq!(shallow.check(), Ok(()));
}

/// A datagram with one byte at `offset` changed to `value`.
fn with_byte(bytes: &[u8], offset: usize, value: u8) -> Vec<u8> {
    let mut changed = bytes.to_vec();
    changed[offset] = value;
    changed
}

/// Where the first method's fields start in a snapshot.
const FIRST_METHOD: usize = 1 + 1 + 8 + 8 + 16 + 2;

#[test]
fn only_canonical_bytes_decode() {
    let bytes = signed(&two_method_snapshot());
    let refused = |bytes: &[u8]| decode_snapshot_datagram(bytes).err();
    // A quote that is not normalized.
    let quote = FIRST_METHOD + 4;
    assert_eq!(
        refused(&with_byte(&bytes, quote + 1, 0x00)),
        Some(SnapshotWireError::Invalid(InvalidSnapshot::Quote))
    );
    // A ladder's step that is not the smallest for its amount: the first
    // amount 1 never grows, so any step but 0 is refused.
    let mut small = two_method_snapshot();
    small.methods[0].bid_ladder = Ladder::floor([1; LEVELS]);
    let small = signed(&small);
    let step = FIRST_METHOD + 12 + 2;
    assert_eq!(small[step], 0);
    assert_eq!(
        refused(&with_byte(&small, step, 1)),
        Some(SnapshotWireError::Invalid(InvalidSnapshot::Ladder))
    );
    // Another version, a trailing byte, a cut-off datagram.
    assert_eq!(
        refused(&with_byte(&bytes, 0, 1)),
        Some(SnapshotWireError::Version(1))
    );
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(matches!(
        refused(&longer),
        Some(SnapshotWireError::Decode(_))
    ));
    assert!(matches!(
        refused(&bytes[..bytes.len() - 1]),
        Some(SnapshotWireError::Decode(_))
    ));
    // Too many methods, said before any is read.
    let count = FIRST_METHOD - 1;
    assert_eq!(bytes[count], 2);
    assert!(matches!(
        refused(&with_byte(&bytes, count, 37)),
        Some(SnapshotWireError::Decode(_))
    ));
    assert_eq!(
        refused(&vec![0; MAX_DATAGRAM_BYTES + 1]),
        Some(SnapshotWireError::TooLong)
    );
}

#[test]
fn requests_are_exactly_their_size_and_zero() {
    let request = encode_request();
    assert_eq!(
        decode_snapshot_datagram(&request),
        Ok(SnapshotMessage::Request)
    );
    assert!(decode_snapshot_datagram(&request[..REQUEST_BYTES - 1]).is_err());
    let mut longer = request.to_vec();
    longer.push(0);
    assert!(decode_snapshot_datagram(&longer).is_err());
    assert!(decode_snapshot_datagram(&with_byte(&request, REQUEST_BYTES - 1, 1)).is_err());
    assert!(decode_snapshot_datagram(&with_byte(&request, 1, 0x82)).is_err());
}

#[test]
fn the_feed_and_the_snapshot_refuse_each_other() {
    // Both are version 2 until the node serves snapshots: neither reads the
    // other's datagrams as its own.
    let feed = sign_feed(&key(), &two_method_feed()).unwrap();
    let subscribe = encode_subscribe(None);
    assert!(decode_snapshot_datagram(&feed).is_err());
    assert!(decode_snapshot_datagram(&subscribe).is_err());
    let snapshot = signed(&two_method_snapshot());
    let request = encode_request();
    assert!(decode(&snapshot).is_err());
    assert!(decode(&request).is_err());
    assert!(!matches!(decode(&request), Ok(Message::Subscribe(_))));
}

#[test]
fn a_reader_takes_only_newer_unexpired_snapshots_of_its_node() {
    let mut reader = SnapshotReader::new(key().public_key());
    let snapshot = two_method_snapshot();
    let now = snapshot.sequence_ms;
    let bytes = signed(&snapshot);
    assert_eq!(reader.receive(&bytes, now), Ok(snapshot.clone()));
    assert_eq!(reader.last_sequence_ms(), Some(snapshot.sequence_ms));
    assert_eq!(reader.receive(&bytes, now), Err(SnapshotRejected::Old));

    let mut newer = snapshot.clone();
    newer.sequence_ms += 1;
    assert_eq!(
        reader.receive(&signed(&newer), newer.valid_until_ms),
        Err(SnapshotRejected::Expired)
    );
    let foreign = sign_snapshot(&other_key(), &newer).unwrap();
    assert_eq!(
        reader.receive(&foreign, now),
        Err(SnapshotRejected::Invalid(InvalidSnapshot::Signature))
    );
    assert_eq!(
        reader.receive(&encode_request(), now),
        Err(SnapshotRejected::NotSnapshot)
    );
    assert!(matches!(
        reader.receive(&[2], now),
        Err(SnapshotRejected::Wire(_))
    ));
    // Nothing refused moved the reader on.
    assert_eq!(reader.last_sequence_ms(), Some(snapshot.sequence_ms));
    assert_eq!(reader.receive(&signed(&newer), now), Ok(newer));
}

#[test]
fn requests_are_answered_only_from_someone_and_within_budget() {
    let request = encode_request();
    assert_eq!(check_request(peer(), &request), Ok(()));
    let spoofed = [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 7)), 0),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), 40_000),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 40_000),
        SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1)),
            40_000,
        ),
    ];
    for from in spoofed {
        assert_eq!(
            check_request(from, &request),
            Err(Unanswered::NotUnicast),
            "{from}"
        );
    }
    let snapshot = signed(&two_method_snapshot());
    assert_eq!(
        check_request(peer(), &snapshot),
        Err(Unanswered::NotRequest)
    );
    assert!(matches!(
        check_request(peer(), &encode_subscribe(None)),
        Err(Unanswered::Malformed(_))
    ));
    // The answer is never larger than the request.
    assert!(snapshot.len() <= request.len());
}

#[test]
fn the_budget_is_a_token_bucket_on_the_callers_clock() {
    let limits = AnswerLimits {
        per_second: 10,
        burst: 3,
    };
    let mut answerer = Answerer::new(limits);
    let request = encode_request();
    let now = 1_000_000;
    for _ in 0..3 {
        assert_eq!(answerer.handle(peer(), &request, now), Answer::Snapshot);
    }
    assert_eq!(
        answerer.handle(peer(), &request, now),
        Answer::Ignored(Unanswered::OverBudget)
    );
    // Garbage costs no budget.
    assert!(matches!(
        answerer.handle(peer(), &[0], now + 100),
        Answer::Ignored(Unanswered::Malformed(_))
    ));
    // A tenth of a second brings one answer back, and no more.
    assert_eq!(
        answerer.handle(peer(), &request, now + 100),
        Answer::Snapshot
    );
    assert_eq!(
        answerer.handle(peer(), &request, now + 100),
        Answer::Ignored(Unanswered::OverBudget)
    );
    // A clock that went back fills nothing.
    assert_eq!(
        answerer.handle(peer(), &request, now),
        Answer::Ignored(Unanswered::OverBudget)
    );
    assert_eq!(
        answerer.handle(peer(), &request, now + 150),
        Answer::Ignored(Unanswered::OverBudget)
    );
    assert_eq!(
        answerer.handle(peer(), &request, now + 200),
        Answer::Snapshot
    );
    // A long quiet spell fills the burst, no more.
    let later = now + 3_600_000;
    for _ in 0..3 {
        assert_eq!(answerer.handle(peer(), &request, later), Answer::Snapshot);
    }
    assert_eq!(
        answerer.handle(peer(), &request, later),
        Answer::Ignored(Unanswered::OverBudget)
    );
    let stats = answerer.stats();
    assert_eq!(
        (stats.answered, stats.over_budget, stats.ignored),
        (8, 5, 1)
    );
}

#[test]
fn method_tags() {
    assert_ne!(method_tag(DSUSD), method_tag(XRP));
    assert_eq!(method_tag(XRP), method_tag(XRP));
    let snapshot = two_method_snapshot();
    assert!(snapshot.method(method_tag(XRP)).is_some());
    assert!(snapshot.method(method_tag("hub-xrpl-xrp")).is_none());
}

/// A snapshot of two methods: `in` bids `bid` and `out` asks `ask`, each
/// with ladders of `depth` doubling.
fn pair(bid: Quote, ask: Quote, depth: u64) -> dassie_feed::Snapshot {
    snapshot_of(vec![
        open_method(DSUSD, bid, bid, depth),
        open_method(XRP, ask, ask, depth),
    ])
}

#[test]
fn swaps_at_par_lose_only_the_drops() {
    let one = Quote::floor(1, 1);
    let snapshot = pair(one, one, 1_000);
    let (from, to) = (method_tag(DSUSD), method_tag(XRP));
    // Within the first level of both ladders: the first drop twice.
    let out = snapshot.swap_out(from, to, 1_000).unwrap();
    assert_eq!(out, 1_000 * 9_990 * 9_990 / 100_000_000);
    // Beyond the ladders: not priced.
    assert_eq!(snapshot.swap_out(from, to, 1_000 << LEVELS), None);
    assert!(
        snapshot
            .swap_out(from, to, (1_000 << (LEVELS - 1)) / 2)
            .is_some()
    );
    // Not a pair: one method, unknown methods.
    assert_eq!(snapshot.swap_out(from, from, 1), None);
    assert_eq!(snapshot.swap_out(from, method_tag("x"), 1), None);
    // Nothing in, nothing out.
    assert_eq!(snapshot.swap_out(from, to, 0), Some(0));
}

#[test]
fn swaps_follow_bid_over_ask() {
    // A unit in is worth 2.49, a unit out costs 2.51 / 1000: about 992
    // units out per unit in, less the drops.
    let snapshot = pair(Quote::floor(249, 100), Quote::ceil(251, 100_000), 1 << 30);
    let (from, to) = (method_tag(DSUSD), method_tag(XRP));
    let out = snapshot.swap_out(from, to, 1_000).unwrap();
    // At most the exact ratio less the first drop twice, and within a
    // ten-millionth of it: the quotes round by less than that.
    let exact = 1_000u128 * 249 * 100_000 * 9_990 * 9_990 / (251 * 100 * 100_000_000);
    let out = u128::from(out);
    assert!(out <= exact, "{out} {exact}");
    assert!(out * 10_000_000 >= exact * 9_999_999, "{out} {exact}");
}

#[test]
fn closed_sides_price_nothing() {
    let snapshot = closed_side_snapshot();
    let (dsusd, xrp) = (method_tag(DSUSD), method_tag(XRP));
    assert_eq!(snapshot.swap_out(xrp, dsusd, 1), None);
    assert!(snapshot.swap_out(dsusd, xrp, 1).is_some());
    let any = Ratio::new(0, 1).unwrap();
    assert_eq!(snapshot.swap_at_least(xrp, dsusd, any), None);
}

#[test]
fn the_ask_ladder_can_end_first() {
    let one = Quote::floor(1, 1);
    let mut snapshot = pair(one, one, 1_000);
    let to = method_tag(XRP);
    for method in &mut snapshot.methods {
        if method.tag == to {
            method.ask_ladder = Ladder::floor([10; LEVELS]);
        }
    }
    let from = method_tag(DSUSD);
    // Ten out at most, whatever comes in.
    assert_eq!(snapshot.swap_out(from, to, 100), None);
    let all = snapshot
        .swap_at_least(from, to, Ratio::new(0, 1).unwrap())
        .unwrap();
    assert_eq!(all.amount_out, 10);
    // Ten out at 0.998 each takes eleven in: rounded against the reader.
    assert_eq!(all.amount_in, 11);
}

#[test]
fn swaps_stop_where_the_rate_falls_below_the_threshold() {
    let one = Quote::floor(1, 1);
    let snapshot = pair(one, one, 1_000);
    let (from, to) = (method_tag(DSUSD), method_tag(XRP));
    // At a rate of 0.99 or better: the first drops of both ladders, 0.1%,
    // 0.25% and 0.5% with each other, and 1% with none.
    let at_least = |numerator, denominator| {
        snapshot
            .swap_at_least(from, to, Ratio::new(numerator, denominator).unwrap())
            .unwrap()
    };
    let strict = at_least(9_990 * 9_990, 100_000_000);
    assert_eq!(strict.amount_in, 1_000);
    let none = at_least(1, 1);
    assert_eq!(none, dassie_feed::Swap::default());
    let all = at_least(0, 1);
    assert_eq!(all.amount_in, 1_000 << (LEVELS - 1));
    assert!(strict.amount_out < all.amount_out);
    // A looser threshold never takes less.
    let mut last = 0;
    for percent in (50..=100).rev() {
        let swap = at_least(percent, 100);
        assert!(swap.amount_in >= last);
        last = swap.amount_in;
    }
    assert_eq!(Ratio::new(1, 0), None);
}

#[test]
fn extreme_quotes_saturate_rather_than_overflow() {
    let largest = Quote::from_bits(u32::MAX).unwrap();
    let smallest = Quote::from_bits(1 << 23).unwrap();
    let snapshot = pair(largest, smallest, u64::MAX >> LEVELS);
    let (from, to) = (method_tag(DSUSD), method_tag(XRP));
    // Out is bounded by the ask ladder, in comes back at the least.
    let swap = snapshot
        .swap_at_least(from, to, Ratio::new(u64::MAX, 1).unwrap())
        .unwrap();
    assert!(swap.amount_out > 0);
    // Each level of the ask ladder costs at least a unit in, rounded up.
    assert!(swap.amount_in <= LEVELS as u64);
    let snapshot = pair(smallest, largest, u64::MAX >> LEVELS);
    assert_eq!(snapshot.swap_out(from, to, u64::MAX >> LEVELS), Some(0));
}
