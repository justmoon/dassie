//! The swap service (spec/decisions/peer-swaps.md,
//! spec/areas/connector.md#the-swap-service): with whom, how much and when
//! the node swaps by itself, and the swaps its owner asks for.
//!
//! - **When it swaps by itself.** A core node with `swapping` on swaps
//!   toward its curve's targets; a periphery node only toward targets its
//!   owner set, none by default. It swaps one asset `a` for another `b`
//!   with a peer while the peer's snapshot, valued at the node's own curve
//!   ([`PriceTable::value_rate`](dassie_connector::PriceTable::value_rate)),
//!   clears one plus its own costs ([`Core::swap_costs_ppm`]): each packet's
//!   least `b`, valued in `a` after the node's own move with every swap in
//!   flight counted against it, is at least what it pays times one plus the
//!   costs. The costs are the node's alone, from its hub's rules and its own
//!   settings; no message carries them.
//! - **With whom and how much.** For each pair of methods, the peer whose
//!   snapshot promises the most at the node's threshold
//!   (`Snapshot::swap_at_least`), ties to the lowest node ID. What a
//!   snapshot promises is for one swap from the state it was made in, so
//!   the node keeps, per peer and pair, what it swapped from that snapshot
//!   (its place on the walk), and starts again from nothing with the next.
//! - **Packets.** Each stays within one step of the ladders
//!   (`Snapshot::step_end`) and at most the largest packet, or the most an
//!   F08 named, and its minimum is what the snapshot promises it
//!   (`Snapshot::packet_out`), since a packet is priced at its last unit;
//!   an owner's, never less than its size at the owner's rate, rounded
//!   up.
//!   At most [`PACKETS_PER_PEER`] are in flight with a peer, within the cap
//!   on money in flight with it, and at most
//!   [`MAX_SWAPS_IN_FLIGHT`](super::MAX_SWAPS_IN_FLIGHT) in all.
//! - **Refusals.** T04, T05 and every other temporary or relative error
//!   back off from the peer, doubling, as STREAM does; F08 sizes the next
//!   packets by the amounts it names; F02 and every other final error stop
//!   the swaps with the peer until its snapshot changes, and so does a swap
//!   that came back below its minimum (F04 from the node's own swap
//!   endpoint).
//! - **What moved** is what the swap endpoint fulfilled on the way back and
//!   what the peer fulfilled on the way out, each on its own: a swap that
//!   reads as rejected may still have paid the node, if the peer took the
//!   return leg and let the outgoing one expire.
//!
//! No room is set aside for swaps: they back off as any sender does.
//! Everything is bounded: the peers read ([`MAX_SWAP_PEERS`]), the packets
//! in flight, and the owner's swaps at once ([`MAX_OWNER_SWAPS`]).

use std::collections::{BTreeMap, BTreeSet};

use dassie_connector::Targets;
use dassie_connector::pricing::RATE_ONE_E15;
use dassie_feed::{MethodTag, Ratio, Snapshot, method_tag};
use dassie_identity::NodeId;
use dassie_ilp::{ErrorClass, ErrorCode};
use dassie_ledger::{LedgerId, Name};

use super::packets::ledger_slug;
use super::{Core, EngineError, MAX_SWAPS_IN_FLIGHT, NewSwap, Output, SwapOutcome};

/// Most peers whose snapshots the service reads: the first by node ID of
/// those that pushed one.
pub const MAX_SWAP_PEERS: usize = 64;

/// Most of the node's swaps in flight with one peer: as a STREAM payment
/// keeps two of the largest packets in flight, within the cap on money in
/// flight with a peer (spec/areas/connector.md#money-in-flight-with-a-peer).
pub const PACKETS_PER_PEER: usize = 2;

/// How long a swap may take, both ways: as long as a STREAM packet.
pub const SWAP_TIMEOUT_MS: u64 = dassie_stream::Policy::DEFAULT.packet_timeout_ms;

/// Most swaps the owner asks for at once.
pub const MAX_OWNER_SWAPS: usize = 4;

/// How long a swap the owner asks for sends: past it, it waits only for
/// what is in flight.
pub const OWNER_SWAP_MS: u64 = 60_000;

/// The first wait after a temporary refusal, and the longest after many in
/// a row: STREAM's.
const FIRST_BACKOFF_MS: u64 = dassie_stream::Policy::DEFAULT.retry_delay_start_ms;
const MAX_BACKOFF_MS: u64 = dassie_stream::Policy::DEFAULT.retry_delay_max_ms;

/// The most the node's costs count as: all it pays.
const MAX_COSTS_PPM: u32 = 1_000_000;

const MILLION: u128 = 1_000_000;

// Every packet in flight with every peer the service reads fits the bound
// on swaps in flight, and the walk's threshold fits its ratio.
const _: () = assert!(PACKETS_PER_PEER <= MAX_SWAPS_IN_FLIGHT);
const _: () = assert!((MAX_COSTS_PPM as u128 + MILLION) * 1_000_000_000 <= u64::MAX as u128);

/// A swap the owner asks for (spec/areas/connector.md#the-swap-service).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OwnerSwap {
    /// The peer it swaps with.
    pub peer: NodeId,
    /// The ledger of the method it pays on.
    pub pay_on: LedgerId,
    /// The ledger of the method it is paid back on.
    pub back_on: LedgerId,
    /// The most it pays, on `pay_on`.
    pub amount: u64,
    /// The least it takes per unit it pays, in units of `back_on`:
    /// `numerator / denominator`.
    pub rate: (u64, u64),
}

/// Why a swap the owner asked for ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwapEnd {
    /// All of it was paid.
    #[default]
    Done,
    /// The peer's snapshot promises no more at the rate asked.
    Exhausted,
    /// The peer refused it, F02 or another final error: no more goes to
    /// it until its snapshot changes.
    Refused,
    /// Its time ran out, while it backed off.
    TimedOut,
}

/// What a swap the owner asked for moved, and why it ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SwapReport {
    /// What the peer took of what the node paid, on the ledger it paid
    /// on: its packets fulfilled.
    pub paid: u64,
    /// What came back, on the ledger it named: what the node's swap
    /// endpoint fulfilled.
    pub received: u64,
    /// Its packets fulfilled.
    pub fulfilled: u64,
    /// Its packets rejected.
    pub rejected: u64,
    /// Why it ended.
    pub ended: SwapEnd,
}

/// What the swap service did since the node started, counted rather than
/// logged, for the owner (spec/areas/connector.md#the-swap-service).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SwapStats {
    /// Whether the node swaps by itself: a core node with `swapping` on,
    /// or a periphery node with targets of its owner's too.
    pub by_itself: bool,
    /// Peers whose snapshots it reads.
    pub peers: u64,
    /// Swaps in flight, its own and the owner's.
    pub in_flight: u64,
    /// Peers it backs off from now: it sends them nothing until the
    /// backoff ends, and then looks at them again.
    pub backing_off: u64,
    /// Peers it sends nothing until their snapshot changes, since they
    /// refused a swap or one came back below its minimum.
    pub waiting_for_snapshot: u64,
    /// Swaps sent.
    pub sent: u64,
    /// Swaps the peer fulfilled.
    pub fulfilled: u64,
    /// Swaps rejected.
    pub rejected: u64,
    /// Swaps rejected whose return leg the node had fulfilled: it was paid
    /// all the same.
    pub paid_though_rejected: u64,
    /// Times it backed off from a peer.
    pub backoffs: u64,
    /// Times a peer refused swaps (F02 or another final error).
    pub refused: u64,
    /// Swaps that came back below their minimum.
    pub below_minimum: u64,
    /// Swaps above a peer's largest packet (F08).
    pub too_large: u64,
    /// Outcomes dropped for want of room, unread.
    pub dropped: u64,
    /// Per ledger: what the node paid there and was fulfilled for, and
    /// what came back there.
    pub moved: BTreeMap<LedgerId, (u64, u64)>,
}

/// The service's state, in memory only.
#[derive(Default)]
pub(super) struct Swapping {
    /// The peers whose snapshots it reads, and those it has swaps in
    /// flight with.
    peers: BTreeMap<NodeId, PeerSwaps>,
    /// Its swaps in flight, by request ID.
    sent: BTreeMap<u32, Sent>,
    /// The swaps the owner asked for, by the engine's ID.
    owner: BTreeMap<u64, OwnerJob>,
    stats: SwapStats,
    /// Whether something it decides by changed since it last decided:
    /// only then does it look at its peers and pairs again.
    dirty: bool,
    /// What it last decided by, of the node's own.
    seen: Option<Seen>,
    /// When it next has something to do by the clock alone: a backoff
    /// that ends, or an owner's swap whose time runs out.
    wake_ms: Option<u64>,
    /// How many times it decided.
    rounds: u64,
}

/// What the service decides by of the node's own: its prices and what it
/// holds, which the books' counters follow, its settings, and whether it
/// is a periphery node.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Seen {
    table: dassie_connector::PriceTable,
    books: dassie_ledger::Stats,
    settings: crate::store::PeerSettings,
    periphery: bool,
}

/// What the service knows of one peer.
#[derive(Default)]
struct PeerSwaps {
    /// The snapshot's bytes, as the peer pushed them.
    bytes: Vec<u8>,
    /// The snapshot they are, as the node took it, verified under the
    /// peer's key and newer than the one before
    /// ([`Core::peer_snapshot`]), while it had not expired.
    snapshot: Option<Snapshot>,
    /// Whether the peer pushes one now.
    pushed: bool,
    /// What went in on each pair, by the ledgers it pays and gets, from
    /// this snapshot: fulfilled or in flight.
    used: BTreeMap<(LedgerId, LedgerId), u64>,
    /// Why no swaps go to it until its snapshot changes, if none do.
    stopped: Option<Stop>,
    /// The most the last F08 on each pair said passes, in what is paid:
    /// the peer's largest packet, which the pair's rate turns into more
    /// or less of what it pays. It outlives the snapshot.
    largest: BTreeMap<(LedgerId, LedgerId), u64>,
    /// The last backoff, and until when it holds.
    backoff_ms: u64,
    until_ms: u64,
    /// Swaps in flight with it.
    in_flight: usize,
}

impl PeerSwaps {
    /// The snapshot, while it counts at `now_ms`.
    fn usable(&self, now_ms: u64) -> Option<&Snapshot> {
        self.snapshot
            .as_ref()
            .filter(|snapshot| self.pushed && now_ms < snapshot.valid_until_ms)
    }

    /// Whether more can be sent to it at `now_ms`.
    fn open(&self, now_ms: u64) -> bool {
        self.stopped.is_none() && self.until_ms <= now_ms && self.in_flight < PACKETS_PER_PEER
    }

    fn used(&self, pair: (LedgerId, LedgerId)) -> u64 {
        self.used.get(&pair).copied().unwrap_or(0)
    }

    /// What the outcome of a swap `sent` to it says of the peer: the swap
    /// is no longer in flight, its place on the walk is free if nothing
    /// came in at the peer, and the peer is backed off from, sized down or
    /// stopped, as its code says. The end of an owner's swap it calls for.
    fn heard(
        &mut self,
        sent: &Sent,
        outcome: &SwapOutcome,
        stats: &mut SwapStats,
        now_ms: u64,
    ) -> Option<SwapEnd> {
        assert!(self.in_flight > 0, "a peer's swap in flight");
        self.in_flight -= 1;
        if outcome.result.is_err() && outcome.came_back.is_none() {
            let used = self.used.entry((sent.pay_on, sent.back_on)).or_default();
            *used = used.saturating_sub(sent.amount);
        }
        let Err(code) = outcome.result else {
            self.backoff_ms = 0;
            return None;
        };
        if code == ErrorCode::F08_AMOUNT_TOO_LARGE {
            stats.too_large += 1;
            // Scaled by what the reject names, as STREAM does, or halved
            // without them.
            let scaled = outcome.too_large.and_then(|named| {
                let most = u128::from(sent.amount) * u128::from(named.maximum_amount)
                    / u128::from(named.received_amount.max(1));
                u64::try_from(most).ok()
            });
            let largest = scaled.unwrap_or(sent.amount / 2).min(sent.amount - 1);
            self.largest.insert((sent.pay_on, sent.back_on), largest);
            None
        } else if code == ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT {
            stats.below_minimum += 1;
            self.stopped = Some(Stop::BelowMinimum);
            None
        } else if matches!(code.class(), ErrorClass::Temporary | ErrorClass::Relative) {
            stats.backoffs += 1;
            self.back_off(now_ms);
            None
        } else {
            stats.refused += 1;
            self.stopped = Some(Stop::Refused);
            Some(SwapEnd::Refused)
        }
    }

    /// Backs off, as STREAM does: the first wait, then twice the last.
    fn back_off(&mut self, now_ms: u64) {
        self.backoff_ms = if self.backoff_ms == 0 {
            FIRST_BACKOFF_MS
        } else {
            self.backoff_ms.saturating_mul(2).min(MAX_BACKOFF_MS)
        };
        self.until_ms = now_ms.saturating_add(self.backoff_ms);
    }
}

/// One of the service's swaps in flight.
#[derive(Clone, Copy, Debug)]
struct Sent {
    peer: NodeId,
    pay_on: LedgerId,
    back_on: LedgerId,
    amount: u64,
    /// The owner's swap it is part of, if any.
    owner: Option<u64>,
}

/// A swap the owner asked for, while it runs.
struct OwnerJob {
    swap: OwnerSwap,
    /// What it paid or has in flight.
    sent: u64,
    in_flight: usize,
    deadline_ms: u64,
    report: SwapReport,
    /// Why it ends, once it does: it reports once nothing is in flight.
    ended: Option<SwapEnd>,
}

/// What may be sent on a pair: the node's own threshold, or the owner's
/// rate and what is left of the owner's amount.
#[derive(Clone, Copy, Debug)]
enum Gate {
    Own {
        costs_ppm: u32,
        targets: Option<Targets>,
    },
    Owner {
        job: u64,
        rate: (u64, u64),
    },
}

/// Why no swaps go to a peer until its snapshot changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    /// It refused one: F02, or another final error.
    Refused,
    /// One came back below its minimum: its snapshot promises more than
    /// it gives now.
    BelowMinimum,
}

/// Why sending on a pair stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drive {
    /// Waiting: for the backoff, or for packets in flight.
    Waiting,
    /// The snapshot promises no more that the gate takes.
    Exhausted,
    /// The peer is stopped until its snapshot changes.
    Stopped(Stop),
}

/// `floor(amount × rate_e15 / 10^15)`, exactly: `amount` of one ledger
/// worth so much of another at `rate_e15`.
fn at_rate(amount: u64, rate_e15: u64) -> u128 {
    let one = u128::from(RATE_ONE_E15);
    let (whole, part) = (u128::from(amount) / one, u128::from(amount) % one);
    // Each product below 2^64 · 2^64 / 10^15 and 10^15 · 2^64.
    whole * u128::from(rate_e15) + part * u128::from(rate_e15) / one
}

/// The minimum of an owner's packet of `size` that the peer's snapshot
/// promises `promised`, at the owner's `rate` (`numerator / denominator`),
/// or `None` when the promise falls short of the rate.
///
/// The snapshot rounds its promise down, and the owner's least,
/// `ceil(size × rate)`, rounds up, so at exactly the rate promised the two
/// can differ by a unit: the promise clears when it is no less than
/// `floor(size × rate)`, the owner's least rounded as the promise is. The
/// minimum is the larger of the two, so the node never takes less than its
/// owner asked; the peer's own price, which the snapshot bounds from
/// below, makes up the unit.
fn owner_minimum(size: u64, promised: u64, (numerator, denominator): (u64, u64)) -> Option<u64> {
    assert!(denominator > 0, "an owner's rate is over something");
    // Below 2^128.
    let worth = u128::from(size) * u128::from(numerator);
    let denominator = u128::from(denominator);
    let (floor, least) = (worth / denominator, worth.div_ceil(denominator));
    if u128::from(promised) < floor {
        return None;
    }
    let minimum = u64::try_from(least.max(u128::from(promised))).ok()?;
    assert!(u128::from(minimum) >= least && minimum >= promised);
    Some(minimum)
}

impl Core {
    /// What the swap service did since the node started.
    pub fn swap_stats(&self) -> SwapStats {
        let mut stats = self.swapping.stats.clone();
        stats.by_itself = self.swaps_by_itself();
        stats.peers = self
            .swapping
            .peers
            .values()
            .filter(|peer| peer.pushed)
            .count() as u64;
        stats.in_flight = self.swapping.sent.len() as u64;
        let now = self.now_ms();
        let peers = self.swapping.peers.values();
        stats.backing_off = peers.clone().filter(|peer| peer.until_ms > now).count() as u64;
        stats.waiting_for_snapshot = peers.filter(|peer| peer.stopped.is_some()).count() as u64;
        stats.dropped = self.swaps.dropped();
        stats
    }

    /// Starts a swap the owner asks for, under `id`: on `swap.peer`, from
    /// one method to another, up to the amount, at the rate, priced by the
    /// peer's snapshot and sent as the node's own swaps are.
    /// [`Output::SwapDone`] with `id` says what it moved.
    ///
    /// # Errors
    ///
    /// The swap is not one, the peering has no methods on its ledgers, no
    /// snapshot of the peer prices the pair, or too many of the owner's
    /// swaps run.
    pub fn start_owner_swap(&mut self, id: u64, swap: OwnerSwap) -> Result<(), EngineError> {
        // A rate of nothing would take whatever the peer's snapshot says,
        // which the peer alone makes: refused, as a missing one is.
        let (numerator, denominator) = swap.rate;
        if swap.amount == 0 || numerator == 0 || denominator == 0 || swap.pay_on == swap.back_on {
            return Err(EngineError::Invalid(
                "a swap pays something on one method for at least a rate on another".into(),
            ));
        }
        if self.swapping.owner.len() >= MAX_OWNER_SWAPS || self.swapping.owner.contains_key(&id) {
            return Err(EngineError::Exhausted("too many swaps at once".into()));
        }
        let ledgers = self.swap_ledgers(&swap.peer);
        if !(ledgers.contains(&swap.pay_on) && ledgers.contains(&swap.back_on)) {
            return Err(EngineError::NotFound(
                "the peering has no available method on that ledger".into(),
            ));
        }
        let priced = self.peer_snapshot(&swap.peer).and_then(|snapshot| {
            snapshot.swap_out(self.tag(swap.pay_on), self.tag(swap.back_on), 0)
        });
        if priced.is_none() {
            return Err(EngineError::Precondition(
                "no snapshot of the peer prices that pair".into(),
            ));
        }
        let job = OwnerJob {
            swap,
            sent: 0,
            in_flight: 0,
            deadline_ms: self.now_ms().saturating_add(OWNER_SWAP_MS),
            report: SwapReport::default(),
            ended: None,
        };
        self.swapping.owner.insert(id, job);
        self.swapping.dirty = true;
        Ok(())
    }

    /// The tag of the method on `ledger`, as a snapshot names it: of the
    /// ID of the scheme that runs it, which is the ledger's slug in every
    /// scheme the node runs but the tests' stand-ins for hubs.
    fn tag(&self, ledger: LedgerId) -> MethodTag {
        let scheme = self
            .schemes
            .iter()
            .find(|(_, active)| active.scheme.ledger() == ledger)
            .map_or(ledger_slug(ledger), |(name, _)| name.as_str());
        method_tag(scheme)
    }

    /// Stops the owner's swap `id` sending, as when its time runs out:
    /// what it has in flight still resolves, and then it ends. Its caller
    /// went away.
    pub fn stop_owner_swap(&mut self, id: u64) {
        let now = self.now_ms();
        if let Some(job) = self.swapping.owner.get_mut(&id) {
            job.deadline_ms = job.deadline_ms.min(now);
            self.swapping.dirty = true;
        }
    }

    /// Whether the node swaps by itself (spec/decisions/peer-swaps.md): a
    /// core node while its owner lets it swap, toward its curve's targets;
    /// a periphery node only toward targets its owner set.
    pub(super) fn swaps_by_itself(&self) -> bool {
        let settings = &self.peer_settings;
        settings.swapping && (!self.peer.is_periphery() || !settings.swap_targets.is_empty())
    }

    /// The service's step: what came of its swaps, the peers' snapshots,
    /// the owner's swaps, and its own.
    ///
    /// It looks at its peers and pairs again only when something it decides
    /// by changed: a snapshot, an outcome, a backoff that ended or an
    /// owner's swap whose time ran out, the node's prices, holdings or
    /// settings, or anything the step persists, such as a peering or a
    /// scheme's state, which say what methods are available.
    pub(super) fn run_swaps(&mut self) {
        self.orphaned_swaps();
        self.swap_outcomes();
        self.read_snapshots();
        let seen = Seen {
            table: self.prices.table,
            books: self.ledger.stats(),
            settings: self.peer_settings,
            periphery: self.peer.is_periphery(),
        };
        let now = self.now_ms();
        let swapping = &mut self.swapping;
        swapping.dirty |= swapping.seen != Some(seen)
            || swapping.wake_ms.is_some_and(|wake| wake <= now)
            || !self.batch.is_empty();
        swapping.seen = Some(seen);
        if swapping.dirty {
            swapping.dirty = false;
            swapping.rounds += 1;
            self.owner_swaps();
            if self.swaps_by_itself() {
                self.own_swaps();
            }
            // A swap the connector refused at once has its outcome already.
            self.swap_outcomes();
            self.swapping.dirty = false;
            self.swapping.wake_ms = self.swap_deadline();
        }
        self.end_owner_swaps();
    }

    /// How many times the service looked at its peers and pairs.
    #[cfg(test)]
    pub(super) const fn swap_rounds(&self) -> u64 {
        self.swapping.rounds
    }

    /// When the service next has something to do: a backoff that ends, or
    /// an owner's swap whose time runs out.
    pub(super) fn swap_deadline(&self) -> Option<u64> {
        let now = self.now_ms();
        let by_itself = self.swaps_by_itself();
        let owner_peers: BTreeSet<NodeId> = self
            .swapping
            .owner
            .values()
            .map(|job| job.swap.peer)
            .collect();
        let backoffs = self
            .swapping
            .peers
            .iter()
            .filter(|(node, peer)| peer.until_ms > now && (by_itself || owner_peers.contains(node)))
            .map(|(_, peer)| peer.until_ms);
        let owner = self
            .swapping
            .owner
            .values()
            .filter(|job| job.ended.is_none())
            .map(|job| job.deadline_ms.max(now));
        backoffs.chain(owner).min()
    }

    /// The ledgers of the methods of the peering with `node` that are
    /// available and whose schemes run: what a swap with it pays on and is
    /// paid back on.
    fn swap_ledgers(&self, node: &NodeId) -> Vec<LedgerId> {
        let Some(peering) = self.peer.peering_of(node) else {
            return Vec::new();
        };
        if !self.peer.active_peers().any(|(peer, _)| peer == node) {
            return Vec::new();
        }
        peering
            .schemes()
            .filter(|&scheme| self.method_available(node, scheme))
            .filter_map(|scheme| {
                let active = self.schemes.get(&Name::new(scheme.as_str()).ok()?)?;
                active.failed.is_none().then(|| active.scheme.ledger())
            })
            .collect()
    }

    /// Takes the outcomes of the service's swaps.
    fn swap_outcomes(&mut self) {
        let ours: BTreeSet<u32> = self.swapping.sent.keys().copied().collect();
        for outcome in self.take_swap_outcomes(|id| ours.contains(&id)) {
            self.swapping.dirty = true;
            self.swap_ended(outcome);
        }
    }

    /// What one of the service's swaps did: what moved, and what it says
    /// of the peer.
    fn swap_ended(&mut self, outcome: SwapOutcome) {
        let Some(sent) = self.swapping.sent.remove(&outcome.request_id) else {
            return;
        };
        let now = self.now_ms();
        let stats = &mut self.swapping.stats;
        let paid = if outcome.result.is_ok() {
            sent.amount
        } else {
            0
        };
        let received = outcome.came_back.unwrap_or(0);
        let moved = stats.moved.entry(sent.pay_on).or_default();
        moved.0 = moved.0.saturating_add(paid);
        let moved = stats.moved.entry(sent.back_on).or_default();
        moved.1 = moved.1.saturating_add(received);
        match outcome.result {
            Ok(()) => stats.fulfilled += 1,
            Err(_) => {
                stats.rejected += 1;
                if outcome.came_back.is_some() {
                    stats.paid_though_rejected += 1;
                }
            }
        }
        let ended = self
            .swapping
            .peers
            .get_mut(&sent.peer)
            .and_then(|peer| peer.heard(&sent, &outcome, stats, now));
        if let Some(job) = sent.owner.and_then(|id| self.swapping.owner.get_mut(&id)) {
            assert!(job.in_flight > 0, "an owner's swap in flight");
            job.in_flight -= 1;
            job.report.paid = job.report.paid.saturating_add(paid);
            job.report.received = job.report.received.saturating_add(received);
            if outcome.result.is_ok() {
                job.report.fulfilled += 1;
            } else {
                job.report.rejected += 1;
                if outcome.came_back.is_none() {
                    job.sent = job.sent.saturating_sub(sent.amount);
                }
            }
            if job.ended.is_none() {
                job.ended = ended;
            }
        }
    }

    /// Follows the peers' snapshots: one that changed is read anew, and
    /// the swaps with its peer start again from nothing on its walk.
    fn read_snapshots(&mut self) {
        let pushed: BTreeMap<NodeId, &[u8]> = self
            .peer
            .snapshots()
            .take(MAX_SWAP_PEERS)
            .map(|(node, bytes)| (*node, bytes))
            .collect();
        let swapping = &mut self.swapping;
        let (peers, sent) = (&mut swapping.peers, &swapping.sent);
        let mut changed = Vec::new();
        for (node, bytes) in &pushed {
            let peer = peers.entry(*node).or_default();
            peer.pushed = true;
            if peer.bytes.as_slice() != *bytes {
                swapping.dirty = true;
                peer.bytes = bytes.to_vec();
                changed.push(*node);
                peer.stopped = None;
                // What is in flight with the peer may not be in the state
                // the snapshot was made in: it counts as gone in already.
                peer.used.clear();
                for swap in sent.values().filter(|swap| swap.peer == *node) {
                    let used = peer.used.entry((swap.pay_on, swap.back_on)).or_default();
                    *used = used.saturating_add(swap.amount);
                }
            }
        }
        // Read through the node, which took them verified.
        for node in changed {
            let snapshot = self.peer_snapshot(&node);
            if let Some(peer) = self.swapping.peers.get_mut(&node) {
                peer.snapshot = snapshot;
            }
        }
        let owner: BTreeSet<NodeId> = self
            .swapping
            .owner
            .values()
            .map(|job| job.swap.peer)
            .collect();
        let peers = &mut self.swapping.peers;
        let mut gone = false;
        for (node, peer) in peers.iter_mut() {
            if peer.pushed && !pushed.contains_key(node) {
                peer.pushed = false;
                gone = true;
            }
        }
        self.swapping.dirty |= gone;
        // A peer is kept while it pushes a snapshot, has swaps in flight,
        // or the owner swaps with it.
        peers.retain(|node, peer| peer.pushed || peer.in_flight > 0 || owner.contains(node));
        assert!(peers.len() <= MAX_SWAP_PEERS + MAX_SWAPS_IN_FLIGHT + MAX_OWNER_SWAPS);
    }

    /// Sends what the owner's swaps may, and ends those whose time ran out.
    fn owner_swaps(&mut self) {
        let now = self.now_ms();
        let ids: Vec<u64> = self.swapping.owner.keys().copied().collect();
        for id in ids {
            let Some(job) = self.swapping.owner.get_mut(&id) else {
                continue;
            };
            if job.ended.is_some() {
                continue;
            }
            if now >= job.deadline_ms {
                job.ended = Some(SwapEnd::TimedOut);
                continue;
            }
            if job.sent >= job.swap.amount {
                if job.in_flight == 0 {
                    job.ended = Some(SwapEnd::Done);
                }
                continue;
            }
            let swap = job.swap;
            let (from, to) = (self.tag(swap.pay_on), self.tag(swap.back_on));
            let peer = self.swapping.peers.entry(swap.peer).or_default();
            let Some(snapshot) = peer.usable(now) else {
                continue;
            };
            let (numerator, denominator) = swap.rate;
            let Some(rate) = Ratio::new(numerator, denominator) else {
                continue;
            };
            let limit = snapshot
                .swap_at_least(from, to, rate)
                .map_or(0, |swap| swap.amount_in);
            let gate = Gate::Owner {
                job: id,
                rate: swap.rate,
            };
            let drive = self.drive(swap.peer, (swap.pay_on, swap.back_on), limit, gate);
            let ended = match drive {
                Drive::Stopped(Stop::Refused) => Some(SwapEnd::Refused),
                Drive::Exhausted => Some(SwapEnd::Exhausted),
                // Below its minimum, it waits for the next snapshot.
                Drive::Stopped(Stop::BelowMinimum) | Drive::Waiting => None,
            };
            // Ended for want of quotes only once nothing it sent is in
            // flight: what comes back may change them.
            let job = self.swapping.owner.get_mut(&id);
            if let (Some(job), Some(ended)) = (job, ended)
                && job.in_flight == 0
            {
                job.ended = Some(ended);
            }
        }
    }

    /// Reports the owner's swaps that ended and have nothing in flight.
    fn end_owner_swaps(&mut self) {
        let done: Vec<u64> = self
            .swapping
            .owner
            .iter()
            .filter(|(_, job)| job.ended.is_some() && job.in_flight == 0)
            .map(|(&id, _)| id)
            .collect();
        for id in done {
            if let Some(job) = self.swapping.owner.remove(&id) {
                let report = SwapReport {
                    ended: job.ended.unwrap_or_default(),
                    ..job.report
                };
                self.outputs_peer.push(Output::SwapDone { id, report });
            }
        }
    }

    /// The node's own swaps: for each pair of methods, the peer whose
    /// snapshot promises the most at the node's threshold, and as much of
    /// it as clears.
    fn own_swaps(&mut self) {
        let targets = self
            .peer
            .is_periphery()
            .then_some(self.peer_settings.swap_targets);
        let mut costs: BTreeMap<LedgerId, Option<u32>> = BTreeMap::new();
        // The best peer of each pair: how much it promises at the
        // threshold beyond where its walk stands, and where that ends.
        let mut best: BTreeMap<(LedgerId, LedgerId), (NodeId, u64, u64)> = BTreeMap::new();
        let owner: BTreeSet<NodeId> = self
            .swapping
            .owner
            .values()
            .map(|job| job.swap.peer)
            .collect();
        let nodes: Vec<NodeId> = self.swapping.peers.keys().copied().collect();
        for node in nodes.into_iter().filter(|node| !owner.contains(node)) {
            for (pair, left, limit) in self.promised(node, targets.as_ref(), &mut costs) {
                // Peers come in order of their IDs: a tie stays with the
                // first.
                if best.get(&pair).is_none_or(|&(_, most, _)| left > most) {
                    best.insert(pair, (node, left, limit));
                }
            }
        }
        for (pair, (node, _, limit)) in best {
            let Some(Some(costs_ppm)) = costs.get(&pair.0).copied() else {
                continue;
            };
            let gate = Gate::Own { costs_ppm, targets };
            self.drive(node, pair, limit, gate);
        }
    }

    /// What `node`'s snapshot promises the node at its threshold on each
    /// pair of their methods, while swaps may go to it: the pair, what is
    /// left beyond where its walk stands, and where that ends. `costs`
    /// keeps the node's costs per ledger it pays on.
    fn promised(
        &self,
        node: NodeId,
        targets: Option<&Targets>,
        costs: &mut BTreeMap<LedgerId, Option<u32>>,
    ) -> Vec<((LedgerId, LedgerId), u64, u64)> {
        let now = self.now_ms();
        let max_age = self.connector.config().max_rate_age_ms;
        let table = &self.prices.table;
        let fresh = |ledger: LedgerId| {
            table
                .ledger(ledger)
                .is_some_and(|entry| entry.as_of_ms <= now && now - entry.as_of_ms <= max_age)
        };
        let Some(peer) = self.swapping.peers.get(&node).filter(|peer| peer.open(now)) else {
            return Vec::new();
        };
        let Some(snapshot) = peer.usable(now) else {
            return Vec::new();
        };
        let holdings = table.live_holdings(&self.ledger);
        let ledgers = self.swap_ledgers(&node);
        let mut promised = Vec::new();
        for &pay_on in &ledgers {
            for &back_on in &ledgers {
                if pay_on == back_on || !(fresh(pay_on) && fresh(back_on)) {
                    continue;
                }
                let cost = *costs
                    .entry(pay_on)
                    .or_insert_with(|| self.swap_costs_ppm(pay_on));
                let worth = table.value_rate(back_on, pay_on, 0, 0, &holdings, targets);
                let (Some(cost), Ok(worth)) = (cost, worth) else {
                    continue;
                };
                // What it gets per unit it pays: one plus its costs, over
                // what its curve says a unit it gets is worth.
                let cost = u64::from(cost.min(MAX_COSTS_PPM));
                let threshold = Ratio::new((1_000_000 + cost) * 1_000_000_000, worth);
                let at = threshold.and_then(|threshold| {
                    snapshot.swap_at_least(self.tag(pay_on), self.tag(back_on), threshold)
                });
                let Some(at) = at else {
                    continue;
                };
                let pair = (pay_on, back_on);
                let left = at.amount_in.saturating_sub(peer.used(pair));
                if left > 0 {
                    promised.push((pair, left, at.amount_in));
                }
            }
        }
        promised
    }

    /// Sends swaps with `node` from `pair.0` to `pair.1`, while `gate`
    /// takes them, up to `limit` on the walk of its snapshot.
    fn drive(&mut self, node: NodeId, pair: (LedgerId, LedgerId), limit: u64, gate: Gate) -> Drive {
        for _ in 0..PACKETS_PER_PEER {
            let swap = match self.next_swap(node, pair, limit, gate) {
                Ok(swap) => swap,
                Err(drive) => return drive,
            };
            let Ok(sent) = self.send_swap(swap) else {
                return Drive::Exhausted;
            };
            let owner = match gate {
                Gate::Owner { job, .. } => Some(job),
                Gate::Own { .. } => None,
            };
            let record = Sent {
                peer: node,
                pay_on: pair.0,
                back_on: pair.1,
                amount: swap.amount,
                owner,
            };
            assert!(self.swapping.sent.insert(sent.request_id, record).is_none());
            self.swapping.stats.sent += 1;
            if let Some(peer) = self.swapping.peers.get_mut(&node) {
                peer.in_flight += 1;
                let used = peer.used.entry(pair).or_default();
                *used = used.saturating_add(swap.amount);
            }
            if let Some(job) = owner.and_then(|id| self.swapping.owner.get_mut(&id)) {
                job.in_flight += 1;
                job.sent = job.sent.saturating_add(swap.amount);
            }
        }
        Drive::Waiting
    }

    /// The next swap with `node` on `pair` that `gate` takes, up to `limit`
    /// on the walk of its snapshot: within one step and the largest packet,
    /// with what the snapshot promises it as its minimum. Or why there is
    /// none now.
    fn next_swap(
        &self,
        node: NodeId,
        pair: (LedgerId, LedgerId),
        limit: u64,
        gate: Gate,
    ) -> Result<NewSwap, Drive> {
        let now = self.now_ms();
        let (pay_on, back_on) = pair;
        let peer = self.swapping.peers.get(&node).ok_or(Drive::Exhausted)?;
        let snapshot = peer.usable(now).ok_or(Drive::Exhausted)?;
        let (from, to) = (self.tag(pay_on), self.tag(back_on));
        let start = peer.used(pair);
        let end = snapshot
            .step_end(from, to, start)
            .filter(|_| start < limit)
            .ok_or(Drive::Exhausted)?;
        if let Some(stop) = peer.stopped {
            return Err(Drive::Stopped(stop));
        }
        if !peer.open(now) || self.swaps.in_flight() >= MAX_SWAPS_IN_FLIGHT {
            return Err(Drive::Waiting);
        }
        let left = match gate {
            Gate::Owner { job, .. } => self
                .swapping
                .owner
                .get(&job)
                .map_or(0, |job| job.swap.amount.saturating_sub(job.sent)),
            Gate::Own { .. } => u64::MAX,
        };
        let size = (end - start)
            .min(limit - start)
            .min(self.settle.max_packet)
            .min(peer.largest.get(&pair).copied().unwrap_or(u64::MAX))
            .min(left);
        let promised = snapshot
            .packet_out(from, to, start, size)
            .filter(|&promised| size > 0 && promised > 0)
            .ok_or(Drive::Exhausted)?;
        let minimum = match gate {
            Gate::Own { costs_ppm, targets } => self
                .clears(pair, size, promised, costs_ppm, targets.as_ref())
                .then_some(promised),
            Gate::Owner { rate, .. } => owner_minimum(size, promised, rate),
        };
        let minimum = minimum.ok_or(Drive::Exhausted)?;
        Ok(NewSwap {
            peer: node,
            pay_on,
            back_on,
            amount: size,
            minimum,
            timeout_ms: SWAP_TIMEOUT_MS,
        })
    }

    /// Whether a swap that pays `size` on `pair.0` for at least `minimum`
    /// on `pair.1` clears one plus the node's costs, valued at its own
    /// curve after its own move, with every swap in flight counted against
    /// it.
    fn clears(
        &self,
        pair: (LedgerId, LedgerId),
        size: u64,
        minimum: u64,
        costs_ppm: u32,
        targets: Option<&Targets>,
    ) -> bool {
        let (pay_on, back_on) = pair;
        let table = &self.prices.table;
        let holdings = table.live_holdings(&self.ledger);
        let Ok(worth) = table.value_rate(back_on, pay_on, minimum, minimum, &holdings, targets)
        else {
            return false;
        };
        let value = at_rate(minimum, worth);
        let costs = u128::from(costs_ppm.min(MAX_COSTS_PPM));
        // Below 2^80 · 2^20 and 2^64 · 2^21.
        value * MILLION >= u128::from(size) * (MILLION + costs)
    }

    /// The service's tables are within their bounds, and agree with the
    /// swaps in flight.
    pub(super) fn check_swapping(&self) {
        let swapping = &self.swapping;
        assert!(swapping.sent.len() <= MAX_SWAPS_IN_FLIGHT);
        assert!(swapping.owner.len() <= MAX_OWNER_SWAPS);
        assert!(swapping.peers.len() <= MAX_SWAP_PEERS + MAX_SWAPS_IN_FLIGHT + MAX_OWNER_SWAPS);
        let in_flight: usize = swapping.peers.values().map(|peer| peer.in_flight).sum();
        assert!(in_flight <= swapping.sent.len());
        for peer in swapping.peers.values() {
            assert!(peer.in_flight <= PACKETS_PER_PEER);
        }
    }
}

#[cfg(test)]
mod tests {
    use dassie_ilp::AmountTooLarge;

    use super::*;

    const NOW: u64 = 1_000_000;

    fn sent(amount: u64) -> Sent {
        Sent {
            peer: dassie_identity::TableKeygen::identity_key(&[1; 32]).node_id(),
            pay_on: LedgerId::StubUsd,
            back_on: LedgerId::HubXrplTestnetXrp,
            amount,
            owner: None,
        }
    }

    fn outcome(result: Result<(), ErrorCode>) -> SwapOutcome {
        SwapOutcome {
            request_id: 0,
            ledger: LedgerId::StubUsd,
            result,
            came_back: None,
            too_large: None,
        }
    }

    /// `peer` hears `result` of a swap of `amount` it had in flight.
    fn hear(peer: &mut PeerSwaps, outcome: &SwapOutcome, amount: u64) -> Option<SwapEnd> {
        peer.in_flight += 1;
        let mut stats = SwapStats::default();
        peer.heard(&sent(amount), outcome, &mut stats, NOW)
    }

    // T04 and T05 back off as STREAM does: the first wait, then twice the
    // last, up to the longest; a swap fulfilled starts it again.
    #[test]
    fn temporary_refusals_back_off_doubling() {
        let mut peer = PeerSwaps::default();
        let codes = [
            ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
            ErrorCode::T05_RATE_LIMITED,
            ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
        ];
        let mut wait = FIRST_BACKOFF_MS;
        for code in codes {
            assert_eq!(hear(&mut peer, &outcome(Err(code)), 10), None);
            assert_eq!(peer.until_ms, NOW + wait);
            assert!(!peer.open(NOW + wait - 1) && peer.open(NOW + wait));
            wait *= 2;
        }
        for _ in 0..32 {
            hear(&mut peer, &outcome(Err(ErrorCode::T05_RATE_LIMITED)), 10);
        }
        assert_eq!(peer.until_ms, NOW + MAX_BACKOFF_MS);
        hear(&mut peer, &outcome(Ok(())), 10);
        hear(&mut peer, &outcome(Err(ErrorCode::T05_RATE_LIMITED)), 10);
        assert_eq!(peer.until_ms, NOW + FIRST_BACKOFF_MS);
        assert_eq!(peer.in_flight, 0);
    }

    // F08 sizes the pair's next packets by what it names; F02 stops the
    // swaps with the peer and ends an owner's; one below its minimum stops
    // them too, until the next snapshot, but ends nothing.
    #[test]
    fn final_refusals_size_down_or_stop() {
        let mut peer = PeerSwaps::default();
        let mut too_large = outcome(Err(ErrorCode::F08_AMOUNT_TOO_LARGE));
        too_large.too_large = Some(AmountTooLarge {
            received_amount: 300,
            maximum_amount: 100,
        });
        assert_eq!(hear(&mut peer, &too_large, 900), None);
        let pair = (LedgerId::StubUsd, LedgerId::HubXrplTestnetXrp);
        assert_eq!(peer.largest.get(&pair), Some(&300));
        assert!(peer.open(NOW));
        let below = outcome(Err(ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT));
        assert_eq!(hear(&mut peer, &below, 10), None);
        assert_eq!(peer.stopped, Some(Stop::BelowMinimum));
        assert!(!peer.open(NOW));
        let unreachable = outcome(Err(ErrorCode::F02_UNREACHABLE));
        assert_eq!(hear(&mut peer, &unreachable, 10), Some(SwapEnd::Refused));
        assert_eq!(peer.stopped, Some(Stop::Refused));
    }

    // What came in at the peer keeps its place on the walk; what did not
    // frees it.
    #[test]
    fn a_refused_swap_frees_its_place_unless_it_was_paid() {
        let mut peer = PeerSwaps::default();
        let pair = (LedgerId::StubUsd, LedgerId::HubXrplTestnetXrp);
        peer.used.insert(pair, 30);
        hear(
            &mut peer,
            &outcome(Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)),
            10,
        );
        assert_eq!(peer.used(pair), 20);
        let mut paid = outcome(Err(ErrorCode::R00_TRANSFER_TIMED_OUT));
        paid.came_back = Some(19);
        hear(&mut peer, &paid, 10);
        assert_eq!(peer.used(pair), 20);
    }

    // An owner's packet clears while its promise, rounded down, is no less
    // than its size at the owner's rate rounded down too, and asks at
    // least that size at the rate rounded up: at exactly the rate promised
    // it goes, a unit above the promise.
    #[test]
    fn an_owner_packet_asks_its_size_at_the_rate_rounded_up() {
        // Three at 1.9 is 5.7.
        assert_eq!(owner_minimum(3, 5, (19, 10)), Some(6));
        assert_eq!(owner_minimum(3, 6, (19, 10)), Some(6));
        assert_eq!(owner_minimum(3, 7, (19, 10)), Some(7));
        assert_eq!(owner_minimum(3, 4, (19, 10)), None);
        // Ten at 1.9 is 19, exactly.
        assert_eq!(owner_minimum(10, 19, (19, 10)), Some(19));
        assert_eq!(owner_minimum(10, 18, (19, 10)), None);
        // At the bounds.
        assert_eq!(owner_minimum(u64::MAX, u64::MAX, (1, 1)), Some(u64::MAX));
        assert_eq!(owner_minimum(u64::MAX, u64::MAX, (2, 1)), None);
        assert_eq!(
            owner_minimum(u64::MAX, u64::MAX - 1, (u64::MAX, u64::MAX)),
            None
        );
        let rate = (u64::MAX, u64::MAX - 1);
        assert_eq!(owner_minimum(u64::MAX - 1, u64::MAX, rate), Some(u64::MAX));
        // One whose least, rounded up, is a unit above the most a minimum
        // holds is none: at 6/5, the largest minimum and three fifths.
        let size = u64::try_from((5 * u128::from(u64::MAX) + 3) / 6).unwrap();
        assert_eq!(owner_minimum(size, u64::MAX, (6, 5)), None);
    }

    #[test]
    fn a_value_at_a_rate_is_exact() {
        let one = RATE_ONE_E15;
        assert_eq!(at_rate(10, one), 10);
        assert_eq!(at_rate(10, one / 2), 5);
        assert_eq!(
            at_rate(u64::MAX, u64::MAX),
            u128::from(u64::MAX) * u128::from(u64::MAX) / u128::from(one)
        );
        assert_eq!(at_rate(3, one / 2), 1);
    }
}
