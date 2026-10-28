//! Answering requests for a snapshot (spec/areas/rate-feed.md#requests).
//!
//! Anyone may ask a node for its snapshot over UDP, once. A request is
//! padded to exactly the largest snapshot's size, and its answer is the
//! node's cached snapshot, so a node never sends an address more than it got
//! from it, whatever that address is: a spoofed request reflects one to one,
//! and nothing amplifies it. A node keeps nothing per address: no cookie
//! and no table. A budget on answers per second bounds what all requests
//! together cost it.
//!
//! [`check_request`] is the pure decision for one datagram, and
//! [`Answerer`] adds the budget, a token bucket driven by the caller's
//! clock. The caller sends the snapshot it has cached, if any.

use core::net::{IpAddr, SocketAddr};

use crate::snapshot::{SnapshotMessage, SnapshotWireError, decode_snapshot_datagram};

/// Answers a node sends per second, at most, in the long run.
pub const ANSWERS_PER_SECOND: u32 = 64;

/// Answers a node sends at once, at most, after a quiet spell.
pub const ANSWER_BURST: u32 = 64;

/// A token, in thousandths: the bucket fills by `per_second` thousandths
/// a millisecond.
const MILLI: u64 = 1_000;

/// The budget's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnswerLimits {
    /// Answers per second, in the long run.
    pub per_second: u32,
    /// Answers at once, after a quiet spell.
    pub burst: u32,
}

impl AnswerLimits {
    /// The node's.
    pub const DEFAULT: Self = Self {
        per_second: ANSWERS_PER_SECOND,
        burst: ANSWER_BURST,
    };
}

/// What to do about a datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Send the cached snapshot to the address it came from.
    Snapshot,
    /// Send nothing.
    Ignored(Unanswered),
}

/// Why a datagram got no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unanswered {
    /// It does not decode: of another size, version or kind, or its
    /// padding is not zero.
    Malformed(SnapshotWireError),
    /// It is a snapshot, which only a node sends.
    NotRequest,
    /// It comes from an address an answer cannot go to: port 0, or an IP
    /// that is unspecified, multicast or broadcast.
    NotUnicast,
    /// The budget for answers is spent for now.
    OverBudget,
}

/// Whether a datagram from `from` is a request to answer, short of the
/// budget.
///
/// # Errors
///
/// Why it gets no answer.
pub fn check_request(from: SocketAddr, datagram: &[u8]) -> Result<(), Unanswered> {
    match decode_snapshot_datagram(datagram) {
        Ok(SnapshotMessage::Request) => {}
        Ok(SnapshotMessage::Snapshot(_)) => return Err(Unanswered::NotRequest),
        Err(error) => return Err(Unanswered::Malformed(error)),
    }
    if !is_unicast(from) {
        return Err(Unanswered::NotUnicast);
    }
    Ok(())
}

/// Whether an answer can go to `address`: a port, and an IP that is one
/// host's. A source that is not one is forged or broken, and an answer
/// sent there would go to no one, or to many. An IPv4 address mapped into
/// IPv6 is the IPv4 address.
fn is_unicast(address: SocketAddr) -> bool {
    let ip = address.ip().to_canonical();
    let one_host = match ip {
        IpAddr::V4(v4) => !v4.is_broadcast(),
        IpAddr::V6(_) => true,
    };
    address.port() != 0 && !ip.is_unspecified() && !ip.is_multicast() && one_host
}

/// What the answerer has done, for the node's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnswerStats {
    /// Requests answered.
    pub answered: u64,
    /// Requests not answered because the budget was spent.
    pub over_budget: u64,
    /// Datagrams that were not a request, or came from no one's address.
    pub ignored: u64,
}

/// Answers requests within a budget per second.
#[derive(Clone, Debug)]
pub struct Answerer {
    limits: AnswerLimits,
    /// Thousandths of an answer, at most `burst` answers' worth.
    tokens_milli: u64,
    /// The latest time the bucket was filled to.
    filled_at_ms: Option<u64>,
    stats: AnswerStats,
}

impl Answerer {
    /// An answerer with a full budget.
    ///
    /// # Panics
    ///
    /// If either bound is zero.
    #[must_use]
    pub fn new(limits: AnswerLimits) -> Self {
        assert!(limits.per_second > 0 && limits.burst > 0);
        Self {
            limits,
            tokens_milli: Self::capacity_milli(limits),
            filled_at_ms: None,
            stats: AnswerStats::default(),
        }
    }

    const fn capacity_milli(limits: AnswerLimits) -> u64 {
        limits.burst as u64 * MILLI
    }

    /// What to do about `datagram`, which came from `from` at `now_ms`. A
    /// datagram that is no request costs no budget.
    pub fn handle(&mut self, from: SocketAddr, datagram: &[u8], now_ms: u64) -> Answer {
        if let Err(reason) = check_request(from, datagram) {
            self.stats.ignored = self.stats.ignored.saturating_add(1);
            return Answer::Ignored(reason);
        }
        if !self.take(now_ms) {
            self.stats.over_budget = self.stats.over_budget.saturating_add(1);
            return Answer::Ignored(Unanswered::OverBudget);
        }
        self.stats.answered = self.stats.answered.saturating_add(1);
        Answer::Snapshot
    }

    /// Takes one answer from the budget at `now_ms`, if there is one. A
    /// clock that goes back fills nothing until it passes the latest time
    /// seen.
    pub fn take(&mut self, now_ms: u64) -> bool {
        let capacity = Self::capacity_milli(self.limits);
        let elapsed_ms = self
            .filled_at_ms
            .map_or(0, |filled| now_ms.saturating_sub(filled));
        let added = elapsed_ms.saturating_mul(u64::from(self.limits.per_second));
        self.tokens_milli = self.tokens_milli.saturating_add(added).min(capacity);
        self.filled_at_ms = Some(
            self.filled_at_ms
                .map_or(now_ms, |filled| filled.max(now_ms)),
        );
        let taken = self.tokens_milli >= MILLI;
        if taken {
            self.tokens_milli -= MILLI;
        }
        assert!(
            self.tokens_milli <= capacity,
            "the budget stays within its burst"
        );
        taken
    }

    /// What it has done since it was made.
    #[must_use]
    pub const fn stats(&self) -> AnswerStats {
        self.stats
    }
}
