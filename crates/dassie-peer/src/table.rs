//! The node table (spec/areas/network.md#node-table): what a node knows
//! about the others.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};

use crate::config::Rate;
use crate::wire::{Endpoint, SchemeId, SignedNodeInfo, TerminationReason};

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
    /// Where the node takes QUIC, read once from its URL; `None` for a
    /// periphery node's, which has none.
    pub endpoint: Option<Endpoint>,
    /// The node's alias.
    pub alias: String,
    /// The neighbors it advertises, sorted.
    pub neighbors: Vec<NodeId>,
    /// Its settlement offers, in its order, each scheme once
    /// ([`Offered`]).
    pub offers: Vec<Offered>,
    /// Whether it is a periphery node's, without a URL: it never floods,
    /// and is reached only over the connection the node opened.
    pub periphery: bool,
    /// How many more core peers the node takes, if it says
    /// (spec/decisions/promotion-to-core.md).
    pub free_core: Option<u16>,
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
            endpoint: Endpoint::of_url(signed.info.url),
            alias: signed.info.alias.into(),
            neighbors,
            offers: first_of_each(&signed.info.settlement),
            periphery: signed.info.is_periphery(),
            free_core: signed.info.free_core(),
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

impl LinkState {
    /// The schemes it offers, in its order.
    pub fn schemes(&self) -> impl Iterator<Item = SchemeId> + '_ {
        self.offers.iter().map(|offer| offer.scheme)
    }
}

/// A settlement offer of a link state, as held: its scheme and the
/// scheme's details, unread. Only a node that knows the scheme reads them,
/// when it peers (spec/areas/peer-protocol.md#settlement-offers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offered {
    /// The scheme.
    pub scheme: SchemeId,
    /// Its details, as the link state carries them.
    pub details: Vec<u8>,
}

/// Each scheme of a link state's offers once, at its first place: the
/// sender's order is its preference, and an offer whose scheme an earlier
/// one names is taken as absent, whatever its details.
fn first_of_each(offers: &[crate::wire::SettlementOffer<'_>]) -> Vec<Offered> {
    let mut unique: Vec<Offered> = Vec::with_capacity(offers.len());
    for offer in offers {
        if !unique.iter().any(|held| held.scheme == offer.scheme) {
            unique.push(Offered {
                scheme: offer.scheme,
                details: offer.details.to_vec(),
            });
        }
    }
    unique
}

/// A settlement method of a peering: a scheme the pair settles on, with
/// what the scheme keeps for the peer on it and how long the room our
/// books give the peer on it has been short
/// (spec/decisions/settlement-methods.md). It is withdrawn while it has
/// been short past the grace ([`Method::withdrawn`]); the hold-down after
/// a withdrawal is the peering's ([`Advertise`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Method {
    /// The scheme it settles on.
    pub scheme: SchemeId,
    /// The scheme's state for this peer.
    pub state: Vec<u8>,
    /// The credit we extend to the peer on it. The credit it extends to us
    /// is not ours to know (spec/decisions/own-fulfills.md).
    pub credit_out: u64,
    /// How long the room our books give the peer on this method has been
    /// short of a packet, settling nothing
    /// (spec/decisions/peering-lifecycle.md): counted only between checks
    /// at which the node could ask it to settle, so neither the node's
    /// downtime nor an outage of its ledger counts. Kept across a restart,
    /// so a node that restarts often still ends a link that stays short. It
    /// withdraws the method.
    pub short_for_ms: u64,
    /// How much of [`short_for_ms`](Self::short_for_ms) passed while the
    /// peer could be reached: the last request to settle was not one that
    /// surely never arrived. Only this ends the peering for room, so a peer
    /// cut off from us is ended for its silence, if at all, never for room
    /// it had no chance to restore (spec/decisions/peering-lifecycle.md).
    /// Kept across a restart too.
    pub short_reached_ms: u64,
    /// The last check that found the method short while the node could
    /// ask: the next one adds the time since. Not kept across a restart.
    pub short_checked_ms: Option<u64>,
    /// Our decision to drop it, while we collect on it first
    /// (spec/decisions/settlement-methods.md): it takes no new packets from
    /// the peer, and counts as unavailable. Kept across a restart, which
    /// takes its steps again.
    pub winding: Option<Winding>,
    /// Since when its scheme has been unable to receive a peer's
    /// settlement, as the node last looked
    /// (spec/areas/settlement.md#when-peers-cannot-pay-us): once that has
    /// lasted as long as a silent peer is kept, the method is dropped at
    /// once. Not kept across a restart: counted from the node's start, as a
    /// peer's silence is.
    pub unpayable_since_ms: Option<u64>,
}

impl Method {
    /// A method just agreed, on `scheme`.
    #[must_use]
    pub const fn new(scheme: SchemeId, state: Vec<u8>, credit_out: u64) -> Self {
        Self {
            scheme,
            state,
            credit_out,
            short_for_ms: 0,
            short_reached_ms: 0,
            short_checked_ms: None,
            winding: None,
            unpayable_since_ms: None,
        }
    }

    /// Whether we are winding it down: collecting on it before we drop it
    /// (spec/decisions/settlement-methods.md). It takes no new packets from
    /// the peer, and counts as unavailable.
    #[must_use]
    pub const fn winding_down(&self) -> bool {
        self.winding.is_some()
    }

    /// Whether it is withdrawn for room: short past `grace_ms`. Its packets
    /// take another method or another first hop, and the peer is listed as
    /// our neighbor only while one of its methods is not withdrawn
    /// (spec/decisions/settlement-methods.md).
    #[must_use]
    pub const fn withdrawn(&self, grace_ms: u64) -> bool {
        self.short_for_ms >= grace_ms
    }
}

/// Our decision to drop a method, kept to ourselves while we collect on it
/// (spec/decisions/settlement-methods.md), as a peering is ended
/// ([`Depeering`]): once our own settlements on it are done, or we waited
/// long enough, we ask the peer to settle on it once; once what it said is
/// on its way is booked, or we waited long enough, we drop it and tell the
/// peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Winding {
    /// When we decided.
    pub since_ms: u64,
    /// When we asked the peer to settle on it. Not kept across a restart,
    /// which takes the steps again.
    pub asked_ms: Option<u64>,
    /// What the peer said it has on its way to us on it, once it answered
    /// with an amount, in the method's units.
    pub promised: Option<u64>,
    /// What our books took from the peer on it since we asked.
    pub booked: u64,
}

impl Winding {
    /// A decision taken at `now_ms`.
    #[must_use]
    pub const fn new(now_ms: u64) -> Self {
        Self {
            since_ms: now_ms,
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

/// A peering that exists: one per pair of nodes, with its settlement
/// methods (spec/decisions/settlement-methods.md). It has one to
/// [`MAX_METHODS`](crate::MAX_METHODS) of them, each on a scheme of its
/// own, the first the one it was made on while that lasts; a peering left
/// with none ends. The rest is the pair's, whatever the methods, as is
/// its settlement channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peering {
    /// Its methods, in the order they were agreed: never empty.
    pub methods: Vec<Method>,
    /// When it began.
    pub since_ms: u64,
    /// Whether the peer asked.
    pub inbound: bool,
    /// The last message from it.
    pub last_seen_ms: u64,
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
    /// A peering made at `now_ms` on its first method.
    #[must_use]
    pub fn new(first: Method, now_ms: u64, inbound: bool) -> Self {
        Self {
            methods: alloc::vec![first],
            since_ms: now_ms,
            inbound,
            last_seen_ms: now_ms,
            depeering: None,
        }
    }

    /// Whether it was heard from within the last `window_ms`: online, as
    /// the advertise flag sees it (spec/decisions/peering-lifecycle.md).
    #[must_use]
    pub const fn heard_within(&self, now_ms: u64, window_ms: u64) -> bool {
        self.last_seen_ms.saturating_add(window_ms) >= now_ms
    }

    /// Its first method: the one it was made on, while that lasts, then
    /// the one [`remove`](Self::remove) makes first; the one a peering
    /// being ended collects on.
    ///
    /// # Panics
    ///
    /// If it has none, which no peering has.
    // Every peering is made with a method and ends with its last, which the
    // node's invariants check.
    #[must_use]
    #[allow(clippy::panic)]
    pub fn first(&self) -> &Method {
        let Some(first) = self.methods.first() else {
            panic!("a peering has a method");
        };
        first
    }

    /// Takes out its method on `scheme`. If that was the first, the one of
    /// the rest on the lowest scheme ID becomes the first: both sides of
    /// the peering pick it alike, whatever order each added the rest in,
    /// as when one side's acceptance of a method was lost and it was asked
    /// for again after another.
    pub fn remove(&mut self, scheme: SchemeId) {
        let first = self.methods.first().is_some_and(|m| m.scheme == scheme);
        self.methods.retain(|method| method.scheme != scheme);
        if first {
            self.methods.sort_unstable_by_key(|method| method.scheme);
        }
    }

    /// Its method on `scheme`, if it has one.
    #[must_use]
    pub fn method(&self, scheme: SchemeId) -> Option<&Method> {
        self.methods.iter().find(|method| method.scheme == scheme)
    }

    /// The same, to change it.
    pub fn method_mut(&mut self, scheme: SchemeId) -> Option<&mut Method> {
        self.methods
            .iter_mut()
            .find(|method| method.scheme == scheme)
    }

    /// Whether it has a method on `scheme`.
    #[must_use]
    pub fn has(&self, scheme: SchemeId) -> bool {
        self.method(scheme).is_some()
    }

    /// Whether it is being ended and collects on its method on `scheme`:
    /// the first, in whose units what the peer said is on its way is
    /// ([`Depeering`]).
    #[must_use]
    pub fn collecting_on(&self, scheme: SchemeId) -> bool {
        self.depeering.is_some()
            && self
                .methods
                .first()
                .is_some_and(|method| method.scheme == scheme)
    }

    /// The schemes of its methods, in order.
    pub fn schemes(&self) -> impl Iterator<Item = SchemeId> + '_ {
        self.methods.iter().map(|method| method.scheme)
    }

    /// Whether every method is withdrawn for room, so that the peer is no
    /// longer listed for room ([`Method::withdrawn`]).
    #[must_use]
    pub fn withdrawn(&self, grace_ms: u64) -> bool {
        self.methods.iter().all(|method| method.withdrawn(grace_ms))
    }

    /// Whether every method is withdrawn for room or being wound down, so
    /// that the peer is no longer listed as our neighbor
    /// (spec/decisions/settlement-methods.md).
    #[must_use]
    pub fn unavailable(&self, grace_ms: u64) -> bool {
        self.methods
            .iter()
            .all(|method| method.withdrawn(grace_ms) || method.winding_down())
    }

    /// Whether a method has been short, while the peer could be reached,
    /// for `limit_ms`: one such ends the peering
    /// (spec/decisions/settlement-methods.md).
    #[must_use]
    pub fn short_for_good(&self, limit_ms: u64) -> bool {
        self.methods
            .iter()
            .any(|method| method.short_reached_ms >= limit_ms)
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
    /// Whether the last request to settle that came back surely never
    /// reached the peer (spec/decisions/peering-lifecycle.md). Not kept
    /// across a restart, whose first check adds nothing anyway.
    pub settle_unreached: bool,
}

/// How many buckets a peer's heartbeats are counted in, over the standing a
/// promotion asks.
pub const STANDING_BUCKETS: u64 = 6;

/// Heartbeats to a peer within one bucket of time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beats {
    /// Sent to it.
    pub sent: u32,
    /// Answered by it.
    pub answered: u32,
}

/// What an uplink watches of a peer to promote it
/// (spec/decisions/promotion-to-core.md), in memory only: a restart starts
/// its good standing and its heartbeats' count again, which only delays a
/// promotion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    /// Since when it has been in good standing without a break: healthy as
    /// the advertise flag sees it, heard from, not banned, and with a
    /// method available.
    pub since_ms: Option<u64>,
    /// Our heartbeats to it in the last [`STANDING_BUCKETS`] buckets, the
    /// newest first.
    pub beats: [Beats; 6],
    /// When the newest bucket began.
    pub bucket_ms: u64,
    /// How far its clock was ahead of ours, negative behind, as the newest
    /// sequence of its link state that came straight from it showed, as an
    /// update of one we held: one we see first may be up to a reissue old.
    /// Kept across its peerings: it is the node's clock, not theirs.
    pub skew_ms: Option<i64>,
}

const _: () = assert!(STANDING_BUCKETS == 6, "the buckets' array");

impl Standing {
    /// Nothing watched yet.
    pub const NEW: Self = Self {
        since_ms: None,
        beats: [Beats {
            sent: 0,
            answered: 0,
        }; 6],
        bucket_ms: 0,
        skew_ms: None,
    };

    /// The newest bucket at `now_ms`, of buckets `bucket_ms` long: older
    /// ones move down, and those past the window are forgotten.
    fn bucket(&mut self, now_ms: u64, bucket_ms: u64) -> Option<&mut Beats> {
        let bucket_ms = bucket_ms.max(1);
        let passed = now_ms.saturating_sub(self.bucket_ms) / bucket_ms;
        if passed > 0 {
            let len = self.beats.len();
            let shift = usize::try_from(passed).map_or(len, |passed| passed.min(len));
            self.beats.rotate_right(shift);
            for beats in self.beats.iter_mut().take(shift) {
                *beats = Beats::default();
            }
            self.bucket_ms = now_ms - now_ms % bucket_ms;
        }
        self.beats.first_mut()
    }

    /// Counts a heartbeat sent at `now_ms`.
    pub fn sent(&mut self, now_ms: u64, bucket_ms: u64) {
        if let Some(newest) = self.bucket(now_ms, bucket_ms) {
            newest.sent = newest.sent.saturating_add(1);
        }
    }

    /// Counts a heartbeat answered at `now_ms`.
    pub fn answered(&mut self, now_ms: u64, bucket_ms: u64) {
        if let Some(newest) = self.bucket(now_ms, bucket_ms) {
            newest.answered = newest.answered.saturating_add(1);
        }
    }

    /// The heartbeats sent and answered over the window.
    #[must_use]
    pub fn counted(&self) -> Beats {
        self.beats
            .iter()
            .fold(Beats::default(), |all, beats| Beats {
                sent: all.sent.saturating_add(beats.sent),
                answered: all.answered.saturating_add(beats.answered),
            })
    }
}

/// What we found of a node's URL when we checked it
/// (spec/decisions/promotion-to-core.md): over QUIC, dialled anew, and over
/// HTTPS, with a certificate a public authority issued. In memory only: a
/// restart checks again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UrlCheck {
    /// When we next check it.
    pub check_at_ms: u64,
    /// Whether it answered over QUIC the last time we checked.
    pub quic: Option<bool>,
    /// Whether it answered over HTTPS the last time we checked.
    pub https: Option<bool>,
    /// Since when it has failed a check without passing both since.
    pub failing_since_ms: Option<u64>,
}

impl UrlCheck {
    /// Never checked.
    pub const NEW: Self = Self {
        check_at_ms: 0,
        quic: None,
        https: None,
        failing_since_ms: None,
    };

    /// Whether it answered both ways the last time we checked.
    #[must_use]
    pub const fn passes(&self) -> bool {
        matches!((self.quic, self.https), (Some(true), Some(true)))
    }

    /// Takes in what a check found at `now_ms`: failing since the first
    /// failure, until it answers over QUIC with nothing failing. An HTTPS
    /// check we could not make, its client busy or missing, says nothing
    /// either way.
    pub fn settle(&mut self, now_ms: u64) {
        if self.quic == Some(false) || self.https == Some(false) {
            self.failing_since_ms.get_or_insert(now_ms);
        } else if self.quic == Some(true) {
            self.failing_since_ms = None;
        }
    }
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
    /// as once its hub stalled: the time counts
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
    /// The endpoint that was probed: a renewal from another is probed
    /// again.
    pub endpoint: Endpoint,
    /// The network the registration came from, for the cap per network
    /// (spec/decisions/membership-economics.md): a /24 or /48.
    pub network: Option<Network>,
}

/// A /24 of IPv4 or a /48 of IPv6, from which registrations, probes and
/// commitments to names are capped.
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
    /// When to try adding a method to our peering with it again. Not kept
    /// across a restart.
    pub method_at_ms: u64,
    /// Failed attempts to add a method in a row, for backoff.
    pub method_failures: u32,
    /// The scheme of our last request for a method, so the next asks for
    /// the one after it: a scheme the peer refuses holds up no other.
    pub method_tried: Option<SchemeId>,
    /// Since when routes have not reached it.
    pub unreachable_ms: Option<u64>,
    /// Its link states' rate limit.
    pub bucket: Bucket,
    /// Whether, as a peer, it is our advertised neighbor.
    pub advertise: Advertise,
    /// What an uplink watches of a peer to promote it
    /// (spec/decisions/promotion-to-core.md): reset with each peering.
    pub standing: Standing,
    /// What we found of its URL when we checked it, as an uplink of a
    /// downlink asking to be promoted, or as the bootstrap node it is
    /// registered with: anew with each URL.
    pub url: UrlCheck,
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
            method_at_ms: now_ms,
            method_failures: 0,
            method_tried: None,
            unreachable_ms: None,
            bucket: Bucket::full(rate, now_ms),
            advertise: Advertise {
                on: false,
                on_since_ms: 0,
                hold_until_ms: 0,
                flaps: 0,
                settle_unreached: false,
            },
            standing: Standing::NEW,
            url: UrlCheck::NEW,
            refuse_until_ms: 0,
        }
    }

    /// What a new link state, `new`, in place of the one it holds, tells of
    /// its node for its promotion (spec/decisions/promotion-to-core.md).
    /// Straight from its subject (`direct`), a new sequence of a link state
    /// we held shows its clock, which issued it a moment ago and sent it on
    /// at once; one we see first may be up to a reissue old. A new URL is
    /// checked anew: the old one's answer says nothing of it.
    pub fn watch_link(&mut self, new: &LinkState, direct: bool, now_ms: u64) {
        let Some(old) = &self.link else {
            return;
        };
        if direct {
            let sequence = i64::try_from(new.sequence).unwrap_or(i64::MAX);
            let now = i64::try_from(now_ms).unwrap_or(i64::MAX);
            self.standing.skew_ms = Some(sequence.saturating_sub(now));
        }
        if old.endpoint != new.endpoint {
            self.url = UrlCheck::NEW;
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

    // spec/areas/network.md#settlement-methods: a peering that loses its
    // first method takes the rest's lowest scheme ID as its new first, so
    // both sides carry packets on the same one, whatever order each added
    // the rest in.
    #[test]
    fn a_peering_s_new_first_method_is_the_same_on_both_sides() {
        let method = |id: &str| Method::new(SchemeId::parse(id).unwrap(), Vec::new(), 0);
        let peered = |order: [&str; 3]| {
            let mut peering = Peering::new(method(order[0]), 0, false);
            peering
                .methods
                .extend(order[1..].iter().map(|id| method(id)));
            peering
        };
        let id = |id| SchemeId::parse(id).unwrap();
        let mut other = peered(["xx", "zz", "yy"]);
        other.remove(id("yy"));
        assert_eq!(other.first().scheme, id("xx"), "the first stays");
        let mut a = peered(["xx", "zz", "yy"]);
        let mut b = peered(["xx", "yy", "zz"]);
        for peering in [&mut a, &mut b] {
            peering.remove(id("xx"));
            assert_eq!(peering.first().scheme, id("yy"));
        }
        assert_eq!(a, b);
    }

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
