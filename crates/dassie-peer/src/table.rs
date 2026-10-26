//! The node table (spec/areas/network.md#node-table): what a node knows
//! about the others.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};

use crate::config::Rate;
use crate::wire::{HubMembership, SchemeId, SignedNodeInfo, TerminationReason};

/// Why a node is in the table. A node with none left is removed
/// after a grace period.
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
    /// A periphery node peered with us as our downlink, or asking to be
    /// (spec/areas/network.md#downlinks): in the table in place of
    /// [`PEER`](Self::PEER), under a bound of its own, and never in the
    /// way of the nodes the others bring.
    pub const DOWNLINK: Self = Self(32);

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
        Self(bits & 63)
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
        } else if self.contains(Self::PEER) || self.contains(Self::DOWNLINK) {
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

    /// Puts back a token taken for something that turned out not to count.
    pub fn give_back(&mut self, rate: Rate) {
        self.tokens = self.tokens.saturating_add(1).min(rate.burst);
    }

    /// The tokens there now.
    pub fn tokens(&mut self, rate: Rate, now_ms: u64) -> u32 {
        self.refill(rate, now_ms);
        self.tokens
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
    /// The `SignedPeerNodeInfo` bytes, verified.
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
    /// Whether it takes the peer protocol over QUIC.
    pub quic: bool,
    /// Whether it is a periphery node's, without a URL: it never floods,
    /// and is reached only over the connection the node opened.
    pub periphery: bool,
    /// The settlement hub the node is a member of, if it names one.
    pub hub: Option<HubMembership>,
    /// Copies of this sequence received, for suppressing the flood.
    pub copies: u8,
    /// When to forward it, if it is to be.
    pub retransmit_at_ms: Option<u64>,
    /// The peer it came from, which it is not forwarded back to.
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
            quic: signed.info.speaks_quic(),
            periphery: signed.info.is_periphery(),
            hub: signed.info.hub(),
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
    /// The scheme's state for this peer.
    pub state: Vec<u8>,
    /// The credit we extend to the peer. The credit it extends to us is
    /// not ours to know (spec/decisions/own-fulfills.md).
    pub credit_out: u64,
    /// When it began.
    pub since_ms: u64,
    /// Whether the peer asked.
    pub inbound: bool,
    /// The last authenticated message from it.
    pub last_seen_ms: u64,
    /// How long the room our books give the peer has been short of a
    /// packet, settling nothing (spec/decisions/peering-lifecycle.md):
    /// counted only between checks at which the node could ask it to settle,
    /// so neither the node's downtime nor an outage of its ledger counts.
    /// Kept across a restart, so a node that restarts often still ends a link
    /// that stays short. It withdraws the peer.
    pub short_for_ms: u64,
    /// How much of [`short_for_ms`](Self::short_for_ms) passed while the
    /// peer could be reached: the last request to settle was not one that
    /// surely never arrived. Only this ends the peering for room, so a peer
    /// cut off from us is ended for its silence, if at all, never for room
    /// it had no chance to restore (spec/decisions/peering-lifecycle.md).
    /// Kept across a restart too.
    pub short_reached_ms: u64,
    /// Our decision to end it, while we collect first
    /// (spec/decisions/ending-a-peering.md).
    pub depeering: Option<Depeering>,
}

/// Our decision to end a peering, kept to ourselves while we collect what
/// the peer owes (spec/decisions/ending-a-peering.md,
/// spec/areas/network.md#termination): for the peer, the peering goes on as
/// before, but we take nothing new from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depeering {
    /// When we decided.
    pub since_ms: u64,
    /// Why, which `peeringTermination` says in the end.
    pub reason: TerminationReason,
    /// When we asked the peer to settle, once our own settlements to it
    /// were done or we stopped waiting for them. Not kept across a
    /// restart, which takes the steps again.
    pub asked_ms: Option<u64>,
    /// What the peer said it has on its way to us, once it answered with
    /// an amount.
    pub promised: Option<u64>,
    /// What our books took from the peer since we asked.
    pub booked: u64,
}

impl Depeering {
    /// A decision taken at `now_ms`, for `reason`.
    #[must_use]
    pub const fn new(now_ms: u64, reason: TerminationReason) -> Self {
        Self {
            since_ms: now_ms,
            reason,
            asked_ms: None,
            promised: None,
            booked: 0,
        }
    }

    /// Whether what the peer said it has on its way is booked.
    #[must_use]
    pub const fn collected(&self) -> bool {
        match self.promised {
            Some(promised) => self.booked >= promised,
            None => false,
        }
    }
}

impl Peering {
    /// Whether it was heard from within the last `window_ms`: online, as
    /// the advertise flag sees it (spec/decisions/peering-lifecycle.md).
    #[must_use]
    pub const fn heard_within(&self, now_ms: u64, window_ms: u64) -> bool {
        self.last_seen_ms.saturating_add(window_ms) >= now_ms
    }
}

/// A peering being ended (spec/decisions/peering-lifecycle.md,
/// spec/areas/network.md#termination).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Termination {
    /// When it began.
    pub since_ms: u64,
    /// Why.
    pub reason: TerminationReason,
    /// Whether we began it.
    pub by_us: bool,
    /// Whether the other side knows: it told us, or answered our message.
    /// Only then is it settled with when the peering ends.
    pub acknowledged: bool,
}

impl Termination {
    /// Whether telling the other side is over: it knows, or we gave up.
    #[must_use]
    pub const fn told(&self, now_ms: u64, timeout_ms: u64) -> bool {
        self.acknowledged || !self.by_us || self.since_ms.saturating_add(timeout_ms) <= now_ms
    }
}

/// The peering lifecycle (spec/decisions/peering-lifecycle.md).
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
    /// Being ended: no new packets, waiting for those in flight.
    Terminating(Peering, Termination),
}

/// Whether a peer is listed as our neighbor, with its damping
/// (spec/decisions/peering-lifecycle.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Advertise {
    /// Whether it is listed now.
    pub on: bool,
    /// Since when it has been listed without a break.
    pub on_since_ms: u64,
    /// Not listed again before this.
    pub hold_until_ms: u64,
    /// Withdrawals not yet forgotten.
    pub flaps: u32,
    /// The last check that found the peer short while the node could ask:
    /// the next one adds the time since. Not kept across a restart.
    pub short_checked_ms: Option<u64>,
    /// Whether the last request to settle that came back surely never
    /// reached the peer (spec/decisions/peering-lifecycle.md). Not kept
    /// across a restart, whose first check adds nothing anyway.
    pub settle_unreached: bool,
}

/// What a settlement check found of the room our books give a peer
/// (spec/decisions/peering-lifecycle.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Room {
    /// Short of a packet: the time since the last check counts, up to
    /// `most_ms`. A few check intervals: a node that stalled (its host
    /// suspended, or its clock jumped) did not ask, so its gap is not the
    /// peer's. It counts toward ending the peering only while the peer
    /// could be reached: the last request to settle was not one that surely
    /// never arrived.
    Short {
        /// The most this check adds.
        most_ms: u64,
    },
    /// Short of a packet while no settlement from the peer can reach us,
    /// as while our line on the ledger cannot receive: the time counts
    /// toward withdrawing the peer, up to `most_ms`, as for
    /// [`Short`](Self::Short), but never toward ending the peering for room,
    /// which the peer had no way to restore.
    ShortUnpayable {
        /// The most this check adds.
        most_ms: u64,
    },
    /// Room for a packet, or a settlement from the peer just booked: the
    /// time short starts again from nothing.
    Enough,
    /// The node cannot ask the peer to settle (its scheme does not see its
    /// ledger): nothing is judged, and the time short neither grows nor
    /// starts again.
    Unjudged,
}

/// A registration with us, as a bootstrap node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    /// When the node first registered.
    pub registered_ms: u64,
    /// When it last renewed.
    pub renewed_ms: u64,
    /// When it lapses without renewal.
    pub expires_ms: u64,
    /// The URL that was probed: a renewal from another URL is probed
    /// again.
    pub url: String,
    /// The network the registration came from, for the cap per network
    /// (spec/decisions/membership-economics.md): a /24 or /48.
    pub network: Option<Network>,
}

/// A /24 of IPv4 or a /48 of IPv6, from which registrations and the replay
/// state of strangers are capped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Network(pub [u8; 7]);

impl Network {
    /// The network of `address`. An IPv4 address mapped into IPv6, as a
    /// dual-stack socket reports IPv4 clients, is the IPv4 address.
    #[must_use]
    pub const fn of(address: core::net::IpAddr) -> Self {
        match address.to_canonical() {
            core::net::IpAddr::V4(v4) => {
                let [a, b, c, _] = v4.octets();
                Self([4, a, b, c, 0, 0, 0])
            }
            core::net::IpAddr::V6(v6) => {
                let o = v6.octets();
                Self([6, o[0], o[1], o[2], o[3], o[4], o[5]])
            }
        }
    }
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
    /// Whether, as a peer, it is our advertised neighbor.
    pub advertise: Advertise,
    /// Until when its peering requests are refused: an owner ended our
    /// peering with it.
    pub refuse_until_ms: u64,
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
            advertise: Advertise {
                on: false,
                on_since_ms: 0,
                hold_until_ms: 0,
                flaps: 0,
                short_checked_ms: None,
                settle_unreached: false,
            },
            refuse_until_ms: 0,
        }
    }

    /// The peering, if peered or terminating.
    #[must_use]
    pub const fn peering(&self) -> Option<&Peering> {
        match &self.peer {
            PeerState::Peered(peering) | PeerState::Terminating(peering, _) => Some(peering),
            _ => None,
        }
    }

    /// The peering, if peered and not terminating.
    #[must_use]
    pub const fn active_peering(&self) -> Option<&Peering> {
        match &self.peer {
            PeerState::Peered(peering) => Some(peering),
            _ => None,
        }
    }

    /// Whether it is our downlink, or asking to be: a periphery node,
    /// kept under a bound of its own (spec/areas/network.md#downlinks).
    #[must_use]
    pub const fn is_downlink(&self) -> bool {
        self.sources.contains(Sources::DOWNLINK)
    }

    /// The termination under way, if any.
    #[must_use]
    pub const fn termination(&self) -> Option<&Termination> {
        match &self.peer {
            PeerState::Terminating(_, termination) => Some(termination),
            _ => None,
        }
    }
}

/// A token bucket for each network, of at most `capacity` networks. A full
/// bucket is the same as none, so only networks that have spent something
/// are kept; when a new one finds no room, the network that has spent
/// least loses its bucket, which hands it at worst a fresh one.
#[derive(Clone, Debug)]
pub struct NetworkBuckets {
    buckets: BTreeMap<Network, Bucket>,
    capacity: usize,
}

impl NetworkBuckets {
    /// No network has spent anything.
    #[must_use]
    pub const fn new(capacity: usize) -> Self {
        Self {
            buckets: BTreeMap::new(),
            capacity,
        }
    }

    /// Whether `network` has a token, without taking it.
    pub fn has_token(&mut self, network: Network, rate: Rate, now_ms: u64) -> bool {
        self.buckets
            .get_mut(&network)
            .is_none_or(|bucket| bucket.has_token(rate, now_ms))
    }

    /// Takes one of `network`'s tokens, if it has one.
    pub fn take(&mut self, network: Network, rate: Rate, now_ms: u64) -> bool {
        if !self.buckets.contains_key(&network) && self.buckets.len() >= self.capacity {
            self.evict(rate, now_ms);
        }
        let bucket = self
            .buckets
            .entry(network)
            .or_insert_with(|| Bucket::full(rate, now_ms));
        bucket.take(rate, now_ms)
    }

    /// Puts back a token `network` took.
    pub fn give_back(&mut self, network: Network, rate: Rate, now_ms: u64) {
        let Some(bucket) = self.buckets.get_mut(&network) else {
            return;
        };
        bucket.give_back(rate);
        if bucket.tokens(rate, now_ms) >= rate.burst {
            self.buckets.remove(&network);
        }
    }

    /// Networks with a bucket.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.buckets.len()
    }

    /// Frees the bucket with the most tokens, the first such by network.
    fn evict(&mut self, rate: Rate, now_ms: u64) {
        let mut fullest: Option<(Network, u32)> = None;
        for (network, bucket) in &mut self.buckets {
            let tokens = bucket.tokens(rate, now_ms);
            if fullest.is_none_or(|(_, most)| tokens > most) {
                fullest = Some((*network, tokens));
            }
        }
        if let Some((network, _)) = fullest {
            self.buckets.remove(&network);
        }
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
    fn a_mapped_ipv4_address_is_in_its_ipv4_network() {
        let ip = |text: &str| text.parse::<core::net::IpAddr>().unwrap();
        assert_eq!(
            Network::of(ip("::ffff:198.51.1.1")),
            Network::of(ip("198.51.1.1"))
        );
        assert_ne!(
            Network::of(ip("::ffff:198.51.1.1")),
            Network::of(ip("::ffff:198.51.2.1"))
        );
        assert_eq!(
            Network::of(ip("2001:db8:1::1")),
            Network::of(ip("2001:db8:1:ffff::2"))
        );
        assert_ne!(
            Network::of(ip("2001:db8:1::1")),
            Network::of(ip("2001:db8:2::1"))
        );
    }

    #[test]
    fn each_network_spends_only_its_own_bucket() {
        let rate = Rate {
            burst: 2,
            refill_ms: 100,
        };
        let net = |last: u8| Network::of(core::net::IpAddr::from([198, 51, last, 1]));
        let mut buckets = NetworkBuckets::new(2);
        assert!(buckets.take(net(1), rate, 0));
        assert!(buckets.take(net(1), rate, 0));
        assert!(!buckets.has_token(net(1), rate, 0));
        assert!(!buckets.take(net(1), rate, 0));
        assert!(buckets.has_token(net(2), rate, 0));
        // A token put back is there again; a bucket full again is dropped.
        buckets.give_back(net(1), rate, 0);
        assert!(buckets.take(net(1), rate, 0));
        assert!(buckets.take(net(2), rate, 0));
        buckets.give_back(net(2), rate, 0);
        assert_eq!(buckets.len(), 1);
        // With no room, the network that has spent least makes way, never
        // one that has spent everything.
        assert!(buckets.take(net(2), rate, 0));
        assert!(buckets.take(net(3), rate, 0));
        assert_eq!(buckets.len(), 2);
        assert!(
            !buckets.take(net(1), rate, 0),
            "net 1 kept its empty bucket"
        );
        assert!(buckets.take(net(1), rate, 100), "and it refills");
    }

    #[test]
    fn backoff_doubles_up_to_its_bound() {
        assert_eq!(backoff(10, 1000, 0), 10);
        assert_eq!(backoff(10, 1000, 3), 80);
        assert_eq!(backoff(10, 1000, 7), 1000);
        assert_eq!(backoff(10, 1000, 100), 1000);
    }
}
