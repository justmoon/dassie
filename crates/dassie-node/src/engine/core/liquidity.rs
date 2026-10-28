//! Fewer peer slots when a hub's pool is too short
//! (spec/decisions/slots-by-liquidity.md,
//! spec/areas/network.md#slots-by-liquidity): what the node can have on
//! hold toward its peers over each hub it is in, its pool, against one
//! largest packet for each peering with a method over that hub.
//!
//! - **Funds first.** The node's funding keeps the pool at a packet for
//!   each peering and one more, with the margin
//!   (`Core::least_pool`, `Standing::least_pool`): below it, it deposits
//!   what the pool lacks and a step of the target more from the owner's own
//!   account, past the hub's ceiling if need be, and a withdrawal leaves
//!   the pool that step past its least, so its moves carry a step each. So a node whose account can pay
//!   neither refuses nor ends a peer for want of pool.
//! - **Newcomers next.** A newcomer over the hub is taken only while the
//!   pool covers it too: a downlink once it covers every peering and one
//!   more, a core peer once it covers the core peers and one more, since
//!   core peers come first; a node with no core peer over the hub takes one
//!   all the same. A newcomer refused is refused for capacity, as past the
//!   caps, and the node asks none ([`PeeringHooks::takes_newcomer`]). Each
//!   refusal lifts only once the pool covers its need with a margin, one
//!   request to settle's funding, so that one settlement either way does
//!   not flip it.
//! - **Ends last.** Only once the pool has stayed short of its peerings
//!   with no funds of the node's own coming, no deposit on its way or able
//!   to come (one backing off after failures still can),
//!   for a grace, does the node end as many as bring it back to a packet
//!   each: downlinks first, those that never paid us before those that
//!   did, the newest first; core peers only once no downlink is left over
//!   the hub, those that asked us before those we asked, the newest first,
//!   never the last, and only by a node that is core in its own view
//!   ([`PeerNode::is_core`]): one outside the core ends no uplink, as they
//!   are its only way into the network, and refuses more; it ends only the
//!   downlinks it still keeps while others route through it. A peering
//!   with a method over another hub loses only its method over this one,
//!   wound down; one without is ended. Either way the node collects first.
//!   The grace starts again once the pool covers every peering, or funds
//!   come.
//!
//! The state is in memory: a restart judges anew, and starts the grace
//! again.
//!
//! [`PeeringHooks::takes_newcomer`]: dassie_peer::PeeringHooks::takes_newcomer
//! [`PeerNode::is_core`]: dassie_peer::PeerNode::is_core

use std::cmp::Reverse;
use std::collections::BTreeMap;

use dassie_hub::member::UNITS_PER_DROP;
use dassie_identity::NodeId;
use dassie_ledger::Name;
use dassie_peer::MAX_OWN_HUBS;
use dassie_peer::wire::{SchemeId, TerminationReason};
use dassie_settlement::{LedgerStatus, policy};

use super::Core;
use super::hub_funding::{CREDIT_WAIT_MS, Direction};

/// How long a hub's pool may stay short of its peerings, once the node
/// can no longer fund it itself (its own account cannot pay the least
/// deposit, its moves are off, paused or given up, or the hub is
/// unavailable), before the node ends any of them: a funding check,
/// `check_ms` apart, and the time a deposit may take to be credited before
/// it is late ([`CREDIT_WAIT_MS`]), so that one the owner makes lands
/// first. While a deposit of its own is on its way and not late, or can
/// come, a failing one backing off included, no grace runs.
#[must_use]
pub const fn grace_ms(check_ms: u64) -> u64 {
    check_ms.saturating_add(CREDIT_WAIT_MS)
}

/// One peering with a method over the hub, neither being ended nor
/// winding that method down, as the judgement needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverHub {
    /// The peer.
    pub node: NodeId,
    /// Whether it is our downlink.
    pub downlink: bool,
    /// Whether it ever paid us a largest packet
    /// (spec/decisions/free-receivers-half.md).
    pub paid_us: bool,
    /// Whether it asked us.
    pub inbound: bool,
    /// When the peering began.
    pub since_ms: u64,
}

/// What a check reads of the hub, in internal units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// What the node can have on hold toward its peers there: its balance
    /// and its holds toward them.
    pub pool: u64,
    /// The largest packet, what each peering is to be covered by.
    pub packet: u64,
    /// How far past its need the pool must come before a refusal lifts.
    pub margin: u64,
    /// Whether the node is a core node in its own view, as the core it
    /// computes says: only one ends core peers, its uplinks being all a
    /// node outside the core has.
    pub core: bool,
    /// Whether the node can still fund the pool itself: a deposit of its
    /// own on its way to the hub and not late, or one due now. Meanwhile
    /// no grace runs, and nothing is ended.
    pub funds_coming: bool,
}

/// What the node keeps of one hub's pool against its peerings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shortfall {
    /// Since when the pool has fallen short of a packet for each peering
    /// with no funds of the node's own coming: the grace runs from here,
    /// and from each round of ends anew.
    pub since_ms: Option<u64>,
    /// Whether the node takes no new downlink over the hub.
    pub refusing_downlinks: bool,
    /// Whether it takes no new core peer over the hub, nor asks one.
    pub refusing_core: bool,
    /// The peerings over the hub at the last check, for the owner to see.
    pub peerings: usize,
    /// How many of them were our downlinks.
    pub downlinks: usize,
    /// How many of them the pool covered at a packet each.
    pub covered: u64,
    /// The pool at the last check, in internal units.
    pub pool: u64,
}

impl Shortfall {
    /// Judges the hub at `now_ms` from `reading`, with `peerings` over it:
    /// which newcomers it refuses, and whom to end now, in order, each
    /// once. The grace is `grace_ms`.
    pub fn judge(
        &mut self,
        now_ms: u64,
        grace_ms: u64,
        reading: &Reading,
        peerings: &[OverHub],
    ) -> Vec<NodeId> {
        assert!(reading.packet > 0, "a largest packet");
        let count = peerings.len();
        let core = peerings.iter().filter(|peering| !peering.downlink).count();
        let pool = reading.pool;
        let need = |count: usize| {
            reading
                .packet
                .saturating_mul(u64::try_from(count).unwrap_or(u64::MAX))
        };
        let short = pool < need(count);
        // A node with no core peer over the hub takes one all the same, as
        // it never ends its last: without one, a core node routes nothing
        // over the hub, and one outside the core reaches nothing.
        self.refusing_core =
            core > 0 && refused(self.refusing_core, pool, need(core + 1), reading.margin);
        self.refusing_downlinks = self.refusing_core
            || refused(
                self.refusing_downlinks,
                pool,
                need(count + 1),
                reading.margin,
            );
        // The grace runs from when the pool fell short with no funds of
        // the node's own coming, and starts again once it covers every
        // peering, or funds come.
        if short && !reading.funds_coming {
            self.since_ms.get_or_insert(now_ms);
        } else {
            self.since_ms = None;
        }
        self.peerings = count;
        self.downlinks = count.saturating_sub(core);
        self.covered = pool / reading.packet;
        self.pool = pool;
        let due = self
            .since_ms
            .is_some_and(|since| since.saturating_add(grace_ms) <= now_ms);
        if !due {
            return Vec::new();
        }
        let over = count.saturating_sub(usize::try_from(self.covered).unwrap_or(usize::MAX));
        let ending = chosen(peerings, over, reading.core);
        if !ending.is_empty() {
            // Each round of ends waits out a grace of its own.
            self.since_ms = Some(now_ms);
        }
        assert!(
            ending.len() <= over,
            "no more ended than the pool is short of"
        );
        ending
    }

    /// From when a check may end peerings, with a grace of `grace_ms`:
    /// the grace past when the shortfall began, or the last round of ends.
    #[must_use]
    pub fn ends_from_ms(&self, grace_ms: u64) -> Option<u64> {
        self.since_ms.map(|since| since.saturating_add(grace_ms))
    }

    /// Its invariants at `now_ms`: a node short of its peerings refuses
    /// downlinks, and one that refuses core peers refuses downlinks too.
    pub fn check_invariants(&self, now_ms: u64) {
        assert!(
            self.since_ms.is_none_or(|since| since <= now_ms),
            "a shortfall began in the past"
        );
        assert!(
            self.since_ms.is_none() || self.refusing_downlinks,
            "a node short of its peerings takes no new downlink"
        );
        assert!(
            !self.refusing_core || self.refusing_downlinks,
            "core peers come before downlinks"
        );
    }
}

/// Whether a newcomer that needs the pool to reach `need` is refused,
/// given whether it was refused at the last check: refused below the need,
/// taken again only once the pool covers it with `margin`.
const fn refused(was: bool, pool: u64, need: u64, margin: u64) -> bool {
    if was {
        pool < need.saturating_add(margin)
    } else {
        pool < need
    }
}

/// The `over` peerings to end, in order: downlinks while any is over the
/// hub, those that never paid us first, then the newest; once none is,
/// core peers, those that asked us first, then the newest, never the last,
/// and only if the node is `core` in its own view. Ties go by node ID, so
/// the choice is the same for the same peerings.
fn chosen(peerings: &[OverHub], over: usize, core: bool) -> Vec<NodeId> {
    let mut downlinks: Vec<&OverHub> = peerings.iter().filter(|p| p.downlink).collect();
    if !downlinks.is_empty() {
        downlinks.sort_by_key(|p| (p.paid_us, Reverse(p.since_ms), p.node));
        return downlinks.iter().take(over).map(|p| p.node).collect();
    }
    if !core {
        return Vec::new();
    }
    let mut core: Vec<&OverHub> = peerings.iter().collect();
    core.sort_by_key(|p| (!p.inbound, Reverse(p.since_ms), p.node));
    let most = over.min(core.len().saturating_sub(1));
    core.iter().take(most).map(|p| p.node).collect()
}

/// The node's shortfalls, one per hub it is in, and when it next checks.
#[derive(Debug, Default)]
pub(super) struct Liquidity {
    /// The next check.
    pub at_ms: u64,
    /// By the scheme over each hub.
    pub hubs: BTreeMap<Name, Shortfall>,
}

impl Liquidity {
    /// Whether the node takes no new downlink over the hub of `scheme`.
    pub fn refuses_downlinks(&self, scheme: Name) -> bool {
        self.hubs
            .get(&scheme)
            .is_some_and(|shortfall| shortfall.refusing_downlinks)
    }

    /// Whether it takes no new core peer over the hub of `scheme`, nor asks
    /// one.
    pub fn refuses_core(&self, scheme: Name) -> bool {
        self.hubs
            .get(&scheme)
            .is_some_and(|shortfall| shortfall.refusing_core)
    }
}

impl Core {
    /// Judges each hub's pool against its peerings, at the funding checks'
    /// pace: refuses newcomers over a hub whose pool would not cover them,
    /// and ends peerings over one that stayed short past the grace
    /// (spec/decisions/slots-by-liquidity.md).
    pub(super) fn review_liquidity(&mut self) {
        let now = self.now_ms;
        if now < self.liquidity.at_ms {
            return;
        }
        self.liquidity.at_ms = now.saturating_add(self.settle.interval_ms);
        let hubs = self.hub_schemes();
        self.liquidity
            .hubs
            .retain(|scheme, _| hubs.contains(scheme));
        let grace = grace_ms(self.settle.interval_ms);
        let mut ended = false;
        for scheme in hubs {
            let (Some(reading), Some(method)) = (
                self.liquidity_reading(scheme),
                SchemeId::parse(scheme.as_str()),
            ) else {
                continue;
            };
            let peerings = self.peerings_over(method);
            let shortfall = self.liquidity.hubs.entry(scheme).or_default();
            for node in shortfall.judge(now, grace, &reading, &peerings) {
                ended |= self.end_for_liquidity(node, method);
            }
        }
        assert!(
            self.liquidity.hubs.len() <= MAX_OWN_HUBS,
            "a shortfall per hub"
        );
        if ended {
            self.after_peer();
            self.sync_peer_slots();
        }
    }

    /// What a check reads of the hub of `scheme`: nothing until the hub
    /// agreed a state of the node's account, nor while its scheme is not in
    /// step with it or cannot be paid, as once the hub stalled. A node not
    /// in the hub yet, or in one that failed, is not judged.
    fn liquidity_reading(&self, scheme: Name) -> Option<Reading> {
        let account = self.hub_account(scheme)?;
        let active = self.schemes.get(&scheme)?;
        if active.scheme.status() != LedgerStatus::Synced || !active.scheme.can_receive() {
            return None;
        }
        let held = Self::held(account)?;
        let now = self.now_ms;
        // A deposit of the node's own on its way and not late, or one that
        // can come, now or once a backoff after failures ends: its funding
        // deposits what the pool lacks
        // ([`Standing::least_pool`](super::hub_funding::Standing::least_pool)),
        // as much as the owner's book can pay from the least deposit on,
        // unless its moves are off, paused or given up, or the hub is
        // unavailable. Ending peers would not mend a failing deposit.
        let funds_coming = self.funding.get(&scheme).is_some_and(|funding| {
            let on_its_way = funding
                .moving
                .is_some_and(|moving| moving.direction == Direction::Deposit)
                && !funding.late(now);
            on_its_way
                || self
                    .standing(scheme)
                    .is_some_and(|standing| funding.deposit_can_come(now, &standing))
        });
        Some(Reading {
            pool: held.pool().saturating_mul(UNITS_PER_DROP),
            packet: self.settle.max_packet,
            margin: self.pool_margin(scheme),
            core: self.peer.is_core(),
            funds_coming,
        })
    }

    /// How far past its need the pool of the hub of `scheme` comes before a
    /// refusal lifts, in internal units: one request to settle's funding,
    /// as the hub's target caps it (`policy::hub_spendable`).
    fn pool_margin(&self, scheme: Name) -> u64 {
        let packet = self.settle.max_packet;
        policy::hub_spendable(packet, self.peer_credit, 1, self.hub_target(scheme))
    }

    /// The least pool the hub of `scheme` needs, in internal units: a
    /// largest packet for each peering over it, as the judgement counts
    /// them, and one more, with the margin, which takes every refusal back
    /// (spec/decisions/slots-by-liquidity.md). The node's funding keeps
    /// the pool there while its own account can pay
    /// ([`Standing::least_pool`](super::hub_funding::Standing::least_pool)),
    /// so it refuses newcomers and ends peerings only for want of funds.
    pub(super) fn least_pool(&self, scheme: Name) -> u64 {
        let count =
            SchemeId::parse(scheme.as_str()).map_or(0, |method| self.peerings_over(method).len());
        let count = u64::try_from(count).unwrap_or(u64::MAX).saturating_add(1);
        self.settle
            .max_packet
            .saturating_mul(count)
            .saturating_add(self.pool_margin(scheme))
    }

    /// The peerings with a method on `method`, neither being ended nor
    /// winding that method down: those that count toward its hub.
    fn peerings_over(&self, method: SchemeId) -> Vec<OverHub> {
        self.peer
            .active_peers()
            .filter(|(_, peering)| peering.depeering.is_none())
            .filter(|(_, peering)| {
                peering
                    .method(method)
                    .is_some_and(|method| !method.winding_down())
            })
            .map(|(node, peering)| OverHub {
                node: *node,
                downlink: self.peer.is_downlink(node),
                paid_us: self
                    .peer
                    .channel(node)
                    .is_some_and(|channel| channel.paid_us),
                inbound: peering.inbound,
                since_ms: peering.since_ms,
            })
            .collect()
    }

    /// Ends `node`'s peering for the node's liquidity over the hub of
    /// `method`, collecting first: only its method there, wound down, if it
    /// has another it keeps; the whole peering otherwise. Returns whether
    /// anything was ended.
    fn end_for_liquidity(&mut self, node: NodeId, method: SchemeId) -> bool {
        let now = self.now_ms;
        let others = self.peer.peering_of(&node).is_some_and(|peering| {
            peering
                .methods
                .iter()
                .any(|other| other.scheme != method && !other.winding_down())
        });
        if others {
            self.peer.wind_down(now, &node, method)
        } else {
            self.peer
                .terminate(now, &node, TerminationReason::ShortOfLiquidity)
        }
    }

    /// The shortfalls' invariants.
    pub(super) fn check_liquidity(&self) {
        assert!(
            self.liquidity.hubs.len() <= MAX_OWN_HUBS,
            "a shortfall per hub"
        );
        for shortfall in self.liquidity.hubs.values() {
            shortfall.check_invariants(self.now_ms);
        }
    }

    /// When the next check of the hubs' pools is due, while the node is in
    /// a hub.
    pub(super) fn liquidity_deadline(&self) -> Option<u64> {
        let judged = self
            .funding
            .keys()
            .any(|scheme| self.hub_account(*scheme).is_some());
        judged.then_some(self.liquidity.at_ms)
    }

    /// The shortfall of the hub of `scheme` as of the last check, and when
    /// its ends would begin, if any: never on a node outside the core
    /// without downlinks over the hub.
    pub(super) fn shortfall(&self, scheme: Name) -> Option<(Shortfall, Option<u64>)> {
        let shortfall = *self.liquidity.hubs.get(&scheme)?;
        let ends = if self.peer.is_core() || shortfall.downlinks > 0 {
            shortfall.ends_from_ms(grace_ms(self.settle.interval_ms))
        } else {
            None
        };
        Some((shortfall, ends))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest packet.
    const P: u64 = 100;
    /// The margin: a request's funding.
    const MARGIN: u64 = 475;
    const GRACE: u64 = 604_000;

    fn node(n: u8) -> NodeId {
        NodeId::from_bytes(&[n; 24])
    }

    fn downlink(n: u8, since_ms: u64, paid_us: bool) -> OverHub {
        OverHub {
            node: node(n),
            downlink: true,
            paid_us,
            inbound: true,
            since_ms,
        }
    }

    fn core(n: u8, since_ms: u64, inbound: bool) -> OverHub {
        OverHub {
            node: node(n),
            downlink: false,
            paid_us: false,
            inbound,
            since_ms,
        }
    }

    fn reading(pool: u64) -> Reading {
        Reading {
            pool,
            packet: P,
            margin: MARGIN,
            core: true,
            funds_coming: false,
        }
    }

    /// Judges `peerings` at `pool` every second from `from` to `to`, and
    /// returns every peering ended, in order, with when.
    fn run(
        shortfall: &mut Shortfall,
        pool: u64,
        peerings: &[OverHub],
        from: u64,
        to: u64,
    ) -> Vec<(u64, NodeId)> {
        let mut ended = Vec::new();
        let mut now = from;
        while now <= to {
            for node in shortfall.judge(now, GRACE, &reading(pool), peerings) {
                ended.push((now, node));
            }
            shortfall.check_invariants(now);
            now += 1_000;
        }
        ended
    }

    // A pool that covers every peering and one more takes newcomers of
    // both kinds; one that covers them all but no more refuses a new
    // downlink and ends nothing.
    #[test]
    fn newcomers_need_a_packet_each() {
        let peerings = [core(1, 0, false), downlink(2, 10, false)];
        let mut shortfall = Shortfall::default();
        assert!(run(&mut shortfall, 3 * P, &peerings, 0, 0).is_empty());
        assert!(!shortfall.refusing_downlinks && !shortfall.refusing_core);
        let mut shortfall = Shortfall::default();
        assert!(run(&mut shortfall, 2 * P, &peerings, 0, 2 * GRACE).is_empty());
        assert!(shortfall.refusing_downlinks, "no room for one more");
        assert!(!shortfall.refusing_core, "the core peers come first");
        assert_eq!(shortfall.since_ms, None, "not short: nothing to end");
    }

    // Short, the node refuses newcomers at once, and ends nothing before
    // the grace; then as many downlinks as bring the pool back to a packet
    // each, those that never paid us first, then the newest; and only once
    // a grace passed again, more.
    #[test]
    fn downlinks_go_after_the_grace_free_and_newest_first() {
        let peerings = [
            core(1, 0, false),
            downlink(2, 10, false),
            downlink(3, 20, true),
            downlink(4, 30, false),
            downlink(5, 40, true),
        ];
        let mut shortfall = Shortfall::default();
        let ended = run(&mut shortfall, 3 * P, &peerings, 1_000, GRACE);
        assert!(ended.is_empty(), "not within the grace");
        assert!(shortfall.refusing_downlinks && shortfall.since_ms == Some(1_000));
        let ended = run(
            &mut shortfall,
            3 * P,
            &peerings,
            GRACE + 1_000,
            GRACE + 1_000,
        );
        assert_eq!(
            ended,
            [(GRACE + 1_000, node(4)), (GRACE + 1_000, node(2))],
            "the free ones, newest first, and no more than the pool lacks"
        );
        assert_eq!(shortfall.since_ms, Some(GRACE + 1_000), "the grace again");
        // The pool falls further while those two are still being ended: the
        // three left are judged again only after a grace.
        let left = [peerings[0], peerings[2], peerings[4]];
        let ended = run(&mut shortfall, P, &left, GRACE + 2_000, 2 * GRACE);
        assert!(ended.is_empty());
        let ended = run(
            &mut shortfall,
            P,
            &left,
            2 * GRACE + 1_000,
            2 * GRACE + 1_000,
        );
        assert_eq!(
            ended,
            [(2 * GRACE + 1_000, node(5)), (2 * GRACE + 1_000, node(3))],
            "then the paying ones, newest first"
        );
    }

    // Core peers go only once no downlink is left over the hub, those that
    // asked us first, then the newest, and never the last.
    #[test]
    fn core_peers_go_last_and_one_stays() {
        let peerings = [
            core(1, 0, false),
            core(2, 10, false),
            core(3, 5, true),
            downlink(4, 30, false),
        ];
        let mut shortfall = Shortfall::default();
        let ended = run(&mut shortfall, 0, &peerings, 0, GRACE);
        assert_eq!(ended, [(GRACE, node(4))], "only the downlink, at first");
        assert!(shortfall.refusing_core, "no core peer either");
        let core_only = [peerings[0], peerings[1], peerings[2]];
        let ended = run(&mut shortfall, 0, &core_only, GRACE + 1_000, 2 * GRACE);
        assert_eq!(
            ended,
            [(2 * GRACE, node(3)), (2 * GRACE, node(2))],
            "the one that asked us, then the newest we asked"
        );
        let last = [peerings[0]];
        assert!(run(&mut shortfall, 0, &last, 2 * GRACE + 1_000, 4 * GRACE).is_empty());
    }

    // A node with no core peer over the hub takes one, however short: it
    // never ends its last either.
    #[test]
    fn a_node_without_a_core_peer_takes_one() {
        let mut shortfall = Shortfall::default();
        run(&mut shortfall, 0, &[downlink(4, 30, false)], 0, 0);
        assert!(!shortfall.refusing_core && shortfall.refusing_downlinks);
    }

    // Refusals lift only once the pool covers the need with the margin, so
    // that one settlement either way does not flip them; the grace starts
    // again as soon as the pool covers every peering, so a dip later gets
    // a fresh one.
    #[test]
    fn refusals_lift_past_a_margin() {
        let peerings = [core(1, 0, false), downlink(2, 10, false)];
        let mut shortfall = Shortfall::default();
        run(&mut shortfall, P, &peerings, 0, 0);
        assert!(shortfall.refusing_downlinks && shortfall.since_ms == Some(0));
        // Covering every peering, but not one more with the margin: still
        // refusing, and the grace starts again.
        run(&mut shortfall, 2 * P, &peerings, 1_000, 1_000);
        assert!(shortfall.refusing_downlinks && shortfall.since_ms.is_none());
        run(&mut shortfall, P, &peerings, 2 * GRACE, 2 * GRACE);
        assert_eq!(shortfall.since_ms, Some(2 * GRACE), "a fresh grace");
        assert!(
            run(
                &mut shortfall,
                P,
                &peerings,
                2 * GRACE + 1_000,
                3 * GRACE - 1_000
            )
            .is_empty()
        );
        run(&mut shortfall, 3 * P, &peerings, 3 * GRACE, 3 * GRACE);
        assert!(shortfall.refusing_downlinks && shortfall.since_ms.is_none());
        run(
            &mut shortfall,
            3 * P + MARGIN - 1,
            &peerings,
            3 * GRACE + 1_000,
            3 * GRACE + 1_000,
        );
        assert!(shortfall.refusing_downlinks);
        run(
            &mut shortfall,
            3 * P + MARGIN,
            &peerings,
            3 * GRACE + 2_000,
            3 * GRACE + 2_000,
        );
        assert!(!shortfall.refusing_downlinks && shortfall.since_ms.is_none());
        // Dipping back to exactly one more does not refuse again.
        run(
            &mut shortfall,
            3 * P,
            &peerings,
            3 * GRACE + 3_000,
            3 * GRACE + 3_000,
        );
        assert!(!shortfall.refusing_downlinks);
    }

    // A node outside the core in its own view refuses more uplinks, and
    // ends none; the downlinks it still keeps, while others route through
    // it, it ends as a core node does.
    #[test]
    fn a_node_outside_the_core_ends_no_uplink() {
        let uplinks = [core(1, 0, false), core(2, 10, false)];
        let mut shortfall = Shortfall::default();
        let outside = Reading {
            core: false,
            ..reading(0)
        };
        let mut now = 0;
        while now <= 3 * GRACE {
            assert!(shortfall.judge(now, GRACE, &outside, &uplinks).is_empty());
            now += 1_000;
        }
        assert!(shortfall.refusing_core);
        let with_downlink = [uplinks[0], uplinks[1], downlink(3, 20, false)];
        let mut shortfall = Shortfall::default();
        assert!(
            shortfall
                .judge(0, GRACE, &outside, &with_downlink)
                .is_empty()
        );
        assert_eq!(
            shortfall.judge(GRACE, GRACE, &outside, &with_downlink),
            [node(3)]
        );
        assert!(
            shortfall
                .judge(3 * GRACE, GRACE, &outside, &uplinks)
                .is_empty()
        );
    }

    // While the node can fund the pool itself, from funds on its own
    // account or a deposit on its way, no grace runs and nothing is ended,
    // however long; once it cannot, as with an empty book or its moves
    // off, the grace starts then, and ends follow it.
    #[test]
    fn funds_of_its_own_hold_the_grace_back() {
        let peerings = [core(1, 0, false), downlink(2, 10, false)];
        let mut shortfall = Shortfall::default();
        let coming = Reading {
            funds_coming: true,
            ..reading(P)
        };
        let mut now = 0;
        while now <= 3 * GRACE {
            assert!(shortfall.judge(now, GRACE, &coming, &peerings).is_empty());
            assert_eq!(shortfall.since_ms, None, "no grace runs");
            now += 1_000;
        }
        assert!(shortfall.refusing_downlinks, "newcomers wait all the same");
        let empty = reading(P);
        assert!(shortfall.judge(now, GRACE, &empty, &peerings).is_empty());
        assert_eq!(shortfall.since_ms, Some(now), "from when no funds come");
        assert!(
            shortfall
                .judge(now + GRACE - 1, GRACE, &empty, &peerings)
                .is_empty()
        );
        assert_eq!(
            shortfall.judge(now + GRACE, GRACE, &empty, &peerings),
            [node(2)]
        );
    }

    // The grace follows the funding check and a deposit's credit time.
    #[test]
    fn the_grace_lets_a_deposit_land() {
        assert_eq!(grace_ms(4_000), 4_000 + CREDIT_WAIT_MS);
    }
}
