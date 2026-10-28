//! Dassie's snapshots, without IO (spec/areas/rate-feed.md,
//! spec/decisions/peer-swaps.md).
//!
//! A core node that offers swaps states what it converts at: per
//! settlement method its peers pay it on, a bid and an ask in its virtual
//! accounting unit, each with a ladder of how far a swap can go before its
//! price falls by more. It signs that as one datagram, a snapshot, and
//! caches it: its peers get it over the peer protocol, and anyone else by
//! asking once over UDP with a padded request.
//!
//! - [`snapshot`]: the snapshot and the request, in canonical OER with a
//!   version byte, the snapshot's Ed25519 signature, and a reader's side;
//! - [`number`]: the quotes' and ladders' numbers, each with one encoding;
//! - [`swap`]: pricing a swap from a snapshot, as a lower bound;
//! - [`answer`]: whether to answer a request, within a budget per second;
//! - [`schedule`]: when to make a new snapshot.
//!
//! The crate is deterministic (spec/decisions/tigerstyle.md): time comes in
//! as `now_ms` (milliseconds since the Unix epoch), and every decision is a
//! value the caller carries out against its socket. Decoders never panic,
//! whatever the bytes.

#![no_std]

extern crate alloc;

pub mod answer;
pub mod number;
pub mod schedule;
pub mod snapshot;
pub mod swap;

pub use answer::{
    ANSWER_BURST, ANSWERS_PER_SECOND, Answer, AnswerLimits, AnswerStats, Answerer, Unanswered,
    check_request,
};
pub use number::{LADDER_BYTES, LEVELS, Ladder, Quote};
pub use schedule::{FEED_HEARTBEAT_MS, FEED_MIN_INTERVAL_MS, FEED_VALIDITY_MS, Poll, Schedule};
pub use snapshot::{
    DROP_BP_LIMIT, InvalidSnapshot, MAX_DATAGRAM_BYTES, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_METHODS,
    MethodQuotes, MethodTag, REQUEST_BYTES, SNAPSHOT_OID, SNAPSHOT_VERSION, SignedSnapshot,
    Snapshot, SnapshotMessage, SnapshotReader, SnapshotRejected, SnapshotWireError, TAG_BYTES,
    attach_signature, decode_snapshot_datagram, encode_request, method_tag, sign_snapshot,
    snapshot_signing_message, take_snapshot,
};
pub use swap::{Ratio, Swap};

// How the timings fit together: a snapshot outlives the heartbeats that
// could be lost before the next one arrives.
const _: () = assert!(FEED_MIN_INTERVAL_MS < FEED_HEARTBEAT_MS);
const _: () = assert!(FEED_HEARTBEAT_MS * 2 <= FEED_VALIDITY_MS);
