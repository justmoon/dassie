//! The node table (spec 3 §9): what a node knows about the others.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};

use crate::config::Rate;
use crate::wire::{SchemeId, SignedNodeInfo};

/// Why a node is in the table (NET-29). A node with none left is removed
/// after a grace period (NET-18).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sources(u8);

impl Sources {
    /// Ourselves.
    pub const SELF: Self = Self(1);
    /// A configured bootstrap node.
    pub const BOOTSTRAP: Self = Self(2);
    /// On a majority of the bootstrap nodes' lists.
    pub const MAJORITY: Self = Self(4);
    /// Registered with us.
    pub const REGISTERED: Self = Self(8);
    /// Peered with us, or asking to be.
    pub const PEER: Self = Self(16);

    /// No reason at all.
    pub const NONE: Self = Self(0);

    /// The bits, as persisted.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// From persisted bits; unknown ones are dropped.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & 31)
    }

    /// Whether every source of `other` is here.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether there is any.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Adds `other`.
    pub const fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Removes `other`.
    pub const fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    /// The rank of the strongest reason, for choosing whom to keep when
    /// the table is full: higher stays.
    #[must_use]
    pub const fn rank(self) -> u8 {
        if self.contains(Self::SELF) {
            5
        } else if self.contains(Self::BOOTSTRAP) {
            4
        } else if self.contains(Self::PEER) {
            3
        } else if self.contains(Self::MAJORITY) {
            2
        } else if self.contains(Self::REGISTERED) {
            1
        } else {
            0
        }
    }
}

/// A token bucket, deterministic in `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bucket {
    tokens: u32,
    updated_ms: u64,
}

impl Bucket {
    /// A full bucket.
    #[must_use]
    pub const fn full(rate: Rate, now_ms: u64) -> Self {
        Self {
            tokens: rate.burst,
            updated_ms: now_ms,
        }
    }

    /// Whether a token is there, without taking it.
    pub fn has_token(&mut self, rate: Rate, now_ms: u64) -> bool {
        self.refill(rate, now_ms);
        self.tokens > 0
    }

    /// Takes a token, if one is there.
    pub fn take(&mut self, rate: Rate, now_ms: u64) -> bool {
        self.refill(rate, now_ms);
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }

    fn refill(&mut self, rate: Rate, now_ms: u64) {
        let refill = rate.refill_ms.max(1);
        let earned = now_ms.saturating_sub(self.updated_ms) / refill;
        if earned > 0 {
            let earned = u32::try_from(earned).unwrap_or(u32::MAX);
            self.tokens = self.tokens.saturating_add(earned).min(rate.burst);
            self.updated_ms = self
                .updated_ms
                .saturating_add(u64::from(earned).saturating_mul(refill));
        }
    }
}

/// A node's link state, as held: the signed bytes, and what they say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkState {
    /// The `SignedPeerNodeInfo` bytes, verified (NET-4).
    pub bytes: Vec<u8>,
    /// Its sequence, a timestamp in ms.
    pub sequence: u64,
    /// The node's URL.
    pub url: String,
    /// The node's alias.
    pub alias: String,
    /// The neighbors it advertises, sorted.
    pub neighbors: Vec<NodeId>,
    /// Its schemes, in its order.
    pub schemes: Vec<SchemeId>,
    /// Copies of this sequence received, for suppressing the flood.
    pub copies: u8,
    /// When to forward it, if it is to be.
    pub retransmit_at_ms: Option<u64>,
    /// The peer it came from, which it is not forwarded back to (NET-35).
    pub from: Option<NodeId>,
}

impl LinkState {
    /// Copies a verified link state.
    #[must_use]
    pub fn from_signed(signed: &SignedNodeInfo<'_>) -> Self {
        let mut neighbors: Vec<NodeId> = signed.info.neighbors().collect();
        neighbors.sort_unstable();
        neighbors.dedup();
        Self {
            bytes: signed.bytes.to_vec(),
            sequence: signed.info.sequence,
            url: signed.info.url.into(),
            alias: signed.info.alias.into(),
            neighbors,
            schemes: signed.info.schemes().collect(),
            copies: 1,
            retransmit_at_ms: None,
            from: None,
        }
    }

    /// Whether it is fresh enough to route over and not to refresh.
    #[must_use]
    pub const fn is_fresh(&self, now_ms: u64, stale_ms: u64, max_future_ms: u64) -> bool {
        self.sequence.saturating_add(stale_ms) >= now_ms
            && self.sequence <= now_ms.saturating_add(max_future_ms)
    }
}

/// A peering that exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peering {
    /// The scheme it settles with.
    pub scheme: SchemeId,
    /// The scheme's version both speak.
    pub version: u16,
    /// The scheme's state for this peer (ACC-15).
    pub state: Vec<u8>,
    /// The credit the peer extends to us (ACC-24).
    pub credit_in: u64,
    /// The credit we extend to the peer.
    pub credit_out: u64,
    /// When it began.
    pub since_ms: u64,
    /// Whether the peer asked.
    pub inbound: bool,
    /// The last authenticated message from it (NET-12).
    pub last_seen_ms: u64,
}

/// The peering lifecycle, as far as M4a has it (D12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PeerState {
    /// Not peered.
    None,
    /// We asked; the answer is pending.
    Requesting {
        /// The scheme asked for.
        scheme: SchemeId,
        /// The peering info the node gave, once it has.
        info: Option<Vec<u8>>,
    },
    /// Peered.
    Peered(Peering),
}

/// A registration with us, as a bootstrap node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Registration {
    /// When the node first registered.
    pub registered_ms: u64,
    /// When it last renewed.
    pub renewed_ms: u64,
    /// When it lapses without renewal.
    pub expires_ms: u64,
}

/// A node, as the table holds it.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Its key, once known from a link state or the bootstrap list.
    pub key: Option<PublicKey>,
    /// Its newest link state.
    pub link: Option<LinkState>,
    /// Why it is here.
    pub sources: Sources,
    /// When it came in, for choosing whom to keep.
    pub first_seen_ms: u64,
    /// Since when it has had no source.
    pub orphaned_ms: Option<u64>,
    /// Its registration with us.
    pub registration: Option<Registration>,
    /// Our peering with it.
    pub peer: PeerState,
    /// When to refresh it next, if stale.
    pub refresh_at_ms: u64,
    /// Failed refreshes in a row, for backoff.
    pub refresh_failures: u32,
    /// When to try peering with it again.
    pub peering_at_ms: u64,
    /// Failed peering attempts in a row, for backoff.
    pub peering_failures: u32,
    /// Since when routes have not reached it.
    pub unreachable_ms: Option<u64>,
    /// Its link states' rate limit.
    pub bucket: Bucket,
}

impl Entry {
    /// A node just learned of.
    #[must_use]
    pub const fn new(sources: Sources, now_ms: u64, rate: Rate) -> Self {
        Self {
            key: None,
            link: None,
            sources,
            first_seen_ms: now_ms,
            orphaned_ms: None,
            registration: None,
            peer: PeerState::None,
            refresh_at_ms: now_ms,
            refresh_failures: 0,
            peering_at_ms: now_ms,
            peering_failures: 0,
            unreachable_ms: None,
            bucket: Bucket::full(rate, now_ms),
        }
    }

    /// The peering, if peered.
    #[must_use]
    pub const fn peering(&self) -> Option<&Peering> {
        match &self.peer {
            PeerState::Peered(peering) => Some(peering),
            _ => None,
        }
    }

    /// Whether its link state advertises `node`.
    #[must_use]
    pub fn advertises(&self, node: &NodeId) -> bool {
        self.link
            .as_ref()
            .is_some_and(|link| link.neighbors.binary_search(node).is_ok())
    }
}

/// Exponential backoff: `min · 2^failures`, at most `max`.
#[must_use]
pub fn backoff(min_ms: u64, max_ms: u64, failures: u32) -> u64 {
    min_ms
        .saturating_mul(1u64.checked_shl(failures.min(32)).unwrap_or(u64::MAX))
        .min(max_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_refill_over_time() {
        let rate = Rate {
            burst: 2,
            refill_ms: 100,
        };
        let mut bucket = Bucket::full(rate, 0);
        assert!(bucket.take(rate, 0));
        assert!(bucket.take(rate, 0));
        assert!(!bucket.take(rate, 99));
        assert!(bucket.take(rate, 100));
        assert!(!bucket.take(rate, 150));
        assert!(bucket.take(rate, 10_000));
        assert!(bucket.take(rate, 10_000));
        assert!(!bucket.take(rate, 10_000));
    }

    #[test]
    fn backoff_doubles_up_to_its_bound() {
        assert_eq!(backoff(10, 1000, 0), 10);
        assert_eq!(backoff(10, 1000, 3), 80);
        assert_eq!(backoff(10, 1000, 7), 1000);
        assert_eq!(backoff(10, 1000, 100), 1000);
    }
}
