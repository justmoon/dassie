//! Dassie's public rate feed, without IO (spec/areas/rate-feed.md).
//!
//! A core node tells anyone who asks what it converts at: for each ordered
//! pair of the settlement methods its peers pay it on, the rate a packet
//! gets, with its fee and each method's largest packet. It encodes and
//! signs one datagram, sends it over UDP to every subscriber, and forgets
//! it: no retries, no acknowledgements, and no state beyond the subscriber
//! table. A subscriber that misses one waits for the next.
//!
//! - [`wire`]: the datagrams, in canonical OER with a version byte, and the
//!   feed's Ed25519 signature;
//! - [`cookie`]: the stateless cookie that proves a subscriber receives at
//!   its address, so a spoofed source never gets a feed aimed at it;
//! - [`table`]: the bounded subscriber table, and what to answer a
//!   subscriber;
//! - [`schedule`]: when to publish;
//! - [`receiver`]: a subscriber's side, which verifies feeds and keeps only
//!   newer ones.
//!
//! The snapshot replaces the feed (spec/decisions/peer-swaps.md), and is
//! built beside it until the node serves it
//! (spec/roadmap.md#peer-swaps):
//!
//! - [`snapshot`]: a node's quotes per settlement method, with a ladder for
//!   each, signed; the request for one; and a reader's side;
//! - [`number`]: the quotes' and ladders' numbers, each with one encoding;
//! - [`swap`]: pricing a swap from a snapshot, as a lower bound;
//! - [`answer`]: whether to answer a request, within a budget per second.
//!
//! The crate is deterministic (spec/decisions/tigerstyle.md): time comes in
//! as `now_ms` (milliseconds since the Unix epoch), the cookie secret is
//! injected, and every decision is a value the caller carries out against
//! its socket. Decoders never panic, whatever the bytes.

#![no_std]

extern crate alloc;

pub mod answer;
pub mod cookie;
pub mod number;
pub mod receiver;
pub mod schedule;
pub mod snapshot;
pub mod swap;
pub mod table;
pub mod wire;

pub use answer::{
    ANSWER_BURST, ANSWERS_PER_SECOND, Answer, AnswerLimits, AnswerStats, Answerer, Unanswered,
    check_request,
};
pub use cookie::{COOKIE_EPOCH_MS, Cookie, CookieCheck, CookieSecret};
pub use number::{LADDER_BYTES, LEVELS, Ladder, Quote};
pub use receiver::{FeedReceiver, RENEW_INTERVAL_MS, Received, Rejected};
pub use schedule::{FEED_HEARTBEAT_MS, FEED_MIN_INTERVAL_MS, FEED_VALIDITY_MS, Poll, Schedule};
pub use snapshot::{
    DROP_BP_LIMIT, InvalidSnapshot, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_METHODS, MethodQuotes,
    MethodTag, REQUEST_BYTES, SNAPSHOT_VERSION, SignedSnapshot, Snapshot, SnapshotMessage,
    SnapshotReader, SnapshotRejected, SnapshotWireError, TAG_BYTES, attach_signature,
    decode_snapshot_datagram, encode_request, method_tag, sign_snapshot, snapshot_signing_message,
};
pub use swap::{Ratio, Swap};
pub use table::{
    Decision, Ignored, Limits, MAX_SUBSCRIBERS, MAX_SUBSCRIBERS_PER_CLIENT, Refused,
    SUBSCRIPTION_TTL_MS, SubscriberTable, TableStats,
};
pub use wire::{
    CHALLENGE_BYTES, COOKIE_BYTES, FEED_OID, Feed, InvalidFeed, MAX_DATAGRAM_BYTES, MAX_FEED_BYTES,
    MAX_METHOD_ID_BYTES, MAX_METHODS, MAX_PPM, MAX_RATES, Message, Method, RATE_PRECISION, Rate,
    SUBSCRIBE_BYTES, SignedFeed, VERSION, WireError, assemble_feed, decode, encode_challenge,
    encode_subscribe, sign_feed,
};

// How the timings fit together. A subscriber renews well within its
// entry's lifetime, so that one or two lost renewals do not drop it; a
// cookie it was given is still taken at its next renewal, since a cookie
// lives at least one epoch; and a feed outlives the heartbeats that could
// be lost before the next one arrives.
const _: () = assert!(RENEW_INTERVAL_MS * 3 <= SUBSCRIPTION_TTL_MS);
const _: () = assert!(RENEW_INTERVAL_MS < COOKIE_EPOCH_MS);
const _: () = assert!(FEED_MIN_INTERVAL_MS < FEED_HEARTBEAT_MS);
const _: () = assert!(FEED_HEARTBEAT_MS * 2 <= FEED_VALIDITY_MS);
