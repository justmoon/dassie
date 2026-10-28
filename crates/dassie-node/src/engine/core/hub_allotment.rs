//! How a node shares what it can hold toward its peers over a settlement
//! hub among them (spec/decisions/hold-allotment.md,
//! spec/areas/settlement.md#credit). Over a hub, the credit a node extends
//! a peer is its hold toward the peer, funded when the peer asks. What it
//! can hold, the pool, is its balance in the hub and its holds there
//! ([`Held::pool`](super::hub_funding::Held::pool)). While the pool gives
//! every peer the default credit, nothing caps a hold. Otherwise each peer
//! has an allotment, its share of the pool:
//!
//! 1. Every peer is brought to the least hold, which carries a link filled
//!    to the cap on money in flight (`policy::least_hold`): core peers
//!    first, then downlinks, each tier in equal shares while the pool
//!    cannot cover it.
//! 2. What is left goes to the core peers up to the default credit, by
//!    what each earned us in fees on the hub's ledger, but for a share
//!    spread evenly, so that a new or idle peer still gets some and can
//!    earn.
//! 3. What is left then goes to the downlinks the same way.
//!
//! That is what each peer gets while every peer asks. What a peer can
//! always count on is its reserve: a largest packet, or while the pool
//! cannot give every peer one, its equal share of it, core peers first.
//! It pays its own hold's opening, the hub's fee and a new hold's reserve,
//! out of what it gets, and one opening stays spendable for the peers that
//! hold nothing yet. A request raises the hold toward a peer up to the
//! default credit, as far as what the hub account can spend leaves once
//! the other peers' reserves, what their holds still lack of them, are set
//! aside ([`Allotments::room`]). So an idle peer's share never strands what
//! a busy one could use, and an idle peer that turns busy starts at once,
//! with its packet, and grows from what is spare.
//!
//! Integer arithmetic only; what does not divide goes a unit at a time in
//! the order of priority, core peers by what they earned us and then
//! downlinks, ties by node ID. The same order has the requests the node
//! could not fund when they came answered late, so that while the spare is
//! short, the peers that earn the most take it first.

use core::cmp::Reverse;

use dassie_connector::EndpointId;
use dassie_identity::NodeId;
use dassie_ledger::Name;
use dassie_peer::wire::SchemeId;
use dassie_settlement::policy;
use dassie_xrpl::UNITS_PER_DROP;

use super::Core;

/// Parts per million: the whole of the remainder.
const PPM: u128 = 1_000_000;

/// A peer over a hub, as its allotment weighs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Peer {
    /// Its node.
    pub node: NodeId,
    /// Whether it is a downlink, served after the core peers.
    pub downlink: bool,
    /// What it earned us in fees on the hub's ledger, as the connector
    /// weighs next hops by it (spec/decisions/weighted-next-hops.md).
    pub earned: u64,
    /// What the node holds toward it: the credit it extends it, its hold
    /// and its settlements to it on their way.
    pub held: u64,
}

/// The peers' shares of what the node can hold toward them over a hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Allotments {
    /// Whether the pool gives every peer the default credit, or the node
    /// cannot tell what it holds: then nothing caps a hold, and requests
    /// are funded as the rule alone says.
    pub plentiful: bool,
    /// The default credit, the most any hold is raised to.
    pub full: u64,
    /// Each peer and its allotment, in the order of priority.
    pub peers: Vec<(NodeId, u64)>,
    /// What each peer's hold still lacks of its reserve, a largest packet
    /// or its equal share of the pool if that is less, in the same order: what the
    /// others' requests leave of what the account can spend.
    pub reserved: Vec<u64>,
    /// All of `reserved`.
    pub reserved_total: u64,
    /// Whether the node holds nothing toward each peer yet, in the same
    /// order, and how many such peers there are.
    pub unopened: Vec<bool>,
    pub unopened_count: usize,
    /// What opening a hold costs: the hub's fee and a new hold's reserve.
    /// One opening stays spendable for the peers that have no hold, which
    /// a peer that has one never takes.
    pub opening: u64,
}

impl Allotments {
    /// The peer's allotment, its share of the pool; nothing for a peer it
    /// does not share the pool with.
    pub fn of(&self, node: &NodeId) -> Option<u64> {
        self.peers
            .iter()
            .find(|(peer, _)| peer == node)
            .map(|(_, allotment)| *allotment)
    }

    /// How far a request may raise the hold toward `node`, which holds
    /// `held` now, if anything caps it: up to the default credit, as far as
    /// `spendable`, what the account can spend toward it, leaves once the
    /// other peers' reserves are set aside, so that each of them can still
    /// be funded to a largest packet, and, for a peer that holds something
    /// already while another holds nothing yet, one hold's opening. Without
    /// `spendable`, up to the peer's allotment. Nothing for a peer it does
    /// not share the pool with; `None` while the pool is plentiful.
    pub fn room(&self, node: &NodeId, held: u64, spendable: Option<u64>) -> Option<u64> {
        if self.plentiful {
            return None;
        }
        let Some(at) = self.peers.iter().position(|(peer, _)| peer == node) else {
            return Some(0);
        };
        let allotment = self.peers.get(at).map_or(0, |(_, allotment)| *allotment);
        let own = self.reserved.get(at).copied().unwrap_or(0);
        assert!(own <= self.reserved_total, "a reserve within the reserves");
        let others = self.reserved_total - own;
        let unopened_others =
            self.unopened_count - usize::from(self.unopened.get(at).copied().unwrap_or(false));
        let others = if held > 0 && unopened_others > 0 {
            others.saturating_add(self.opening)
        } else {
            others
        };
        let spare = spendable.map_or(allotment.saturating_sub(held), |spendable| {
            spendable.saturating_sub(others)
        });
        let room = spare.min(self.full.saturating_sub(held));
        // What the others' reserves need stays spendable.
        assert!(spendable.is_none_or(|spendable| room == 0 || spendable - room >= others));
        Some(room)
    }

    /// Where `node` stands in the order of priority; past every peer for
    /// one not listed.
    pub fn rank(&self, node: &NodeId) -> usize {
        self.peers
            .iter()
            .position(|(peer, _)| peer == node)
            .unwrap_or(self.peers.len())
    }
}

/// `peers` in the order of priority: core peers before downlinks, each by
/// what they earned us, most first, and then by node ID.
pub(super) fn by_priority(peers: &[Peer]) -> Vec<Peer> {
    let mut ordered = peers.to_vec();
    ordered.sort_by_key(|peer| (peer.downlink, Reverse(peer.earned), peer.node));
    ordered
}

/// Shares `pool` among `peers`: each gets at most `full`, the default
/// credit, and, while the pool covers it, at least `least`, the least
/// hold. Of what is left once each has the least, `explore_ppm` is spread
/// evenly and the rest by what each earned us. Each peer's reserve is
/// `packet`, a largest packet, or while the pool cannot give every peer
/// one, its equal share of it, core peers first.
///
/// # Panics
///
/// If `least` is above `full`, or the shares break their bounds.
pub(super) fn allot(
    pool: u64,
    (full, least, packet): (u64, u64, u64),
    explore_ppm: u32,
    peers: &[Peer],
) -> Allotments {
    assert!(least <= full, "the least hold within the default credit");
    assert!(packet <= least, "a packet within the least hold");
    assert!(u128::from(explore_ppm) <= PPM, "a share of the remainder");
    let ordered = by_priority(peers);
    let count = u64::try_from(ordered.len()).unwrap_or(u64::MAX);
    let plentiful = u128::from(pool) >= u128::from(full) * u128::from(count);
    let mut amounts = vec![0u64; ordered.len()];
    let mut reserves = vec![0u64; ordered.len()];
    let earned: Vec<u64> = ordered.iter().map(|peer| peer.earned).collect();
    if plentiful {
        amounts.fill(full);
    } else {
        let core = ordered.iter().take_while(|peer| !peer.downlink).count();
        // A packet each, core peers first, in equal shares while the pool
        // cannot cover a tier.
        let mut left = pool;
        for tier in [0..core, core..ordered.len()] {
            let slots = reserves.get_mut(tier).unwrap_or_default();
            left = spread(slots, None, left, packet);
        }
        let mut left = pool;
        // The least hold for each, core peers first, in equal shares.
        for tier in [0..core, core..ordered.len()] {
            let slots = amounts.get_mut(tier).unwrap_or_default();
            left = spread(slots, None, left, least);
        }
        // The rest up to the default credit, core peers first: a share
        // evenly, and the rest by what each earned us.
        for tier in [0..core, core..ordered.len()] {
            let explore =
                u64::try_from(u128::from(left) * u128::from(explore_ppm) / PPM).unwrap_or(left);
            let weights = earned.get(tier.clone()).unwrap_or_default();
            let slots = amounts.get_mut(tier).unwrap_or_default();
            let unspread = spread(slots, None, explore, full);
            let rest = left - explore + unspread;
            left = spread(slots, Some(weights), rest, full);
        }
    }
    let total: u128 = amounts.iter().map(|amount| u128::from(*amount)).sum();
    assert!(total <= u128::from(pool), "no more allotted than the pool");
    assert!(amounts.iter().all(|amount| *amount <= full));
    assert!(!plentiful || amounts.iter().all(|amount| *amount == full));
    let covered = u128::from(pool) >= u128::from(least) * u128::from(count);
    assert!(!covered || amounts.iter().all(|amount| *amount >= least));
    let reserved: Vec<u64> = ordered
        .iter()
        .zip(&reserves)
        .map(|(peer, reserve)| reserve.saturating_sub(peer.held))
        .collect();
    let unopened: Vec<bool> = ordered.iter().map(|peer| peer.held == 0).collect();
    let unopened_count = unopened.iter().filter(|unopened| **unopened).count();
    let reserved_total = reserved
        .iter()
        .fold(0u64, |sum, own| sum.saturating_add(*own));
    assert!(reserved_total <= pool, "no more reserved than the pool");
    Allotments {
        plentiful,
        full,
        peers: ordered
            .iter()
            .zip(amounts)
            .map(|(peer, amount)| (peer.node, amount))
            .collect(),
        reserved,
        reserved_total,
        unopened,
        unopened_count,
        opening: 0,
    }
}

/// Spreads `amount` over `slots` up to `cap` each, by `weights`, or evenly
/// without them or while every open slot weighs nothing; what does not
/// divide goes a unit at a time in their order. Returns what is left once
/// every slot is at `cap`.
///
/// Each round fills a slot to `cap`, or hands out all but less than a
/// unit per slot that weighs something; each round after that hands out a
/// unit at least, until less than a unit per open slot is left, which the
/// last round hands out: so at most twice as many rounds as slots, and two
/// more.
fn spread(slots: &mut [u64], weights: Option<&[u64]>, amount: u64, cap: u64) -> u64 {
    let rounds = slots.len().saturating_mul(2).saturating_add(2);
    let mut left = amount;
    for _ in 0..rounds {
        let open: Vec<usize> = (0..slots.len())
            .filter(|at| slots.get(*at).is_some_and(|slot| *slot < cap))
            .collect();
        if left == 0 || open.is_empty() {
            break;
        }
        let weight = |at: usize| weights.map_or(1, |weights| weights.get(at).copied().unwrap_or(0));
        let total: u128 = open.iter().map(|at| u128::from(weight(*at))).sum();
        let (even, total) = if total == 0 {
            (
                true,
                u128::from(u64::try_from(open.len()).unwrap_or(u64::MAX)),
            )
        } else {
            (false, total)
        };
        let mut given = 0u64;
        for at in &open {
            let weight = if even { 1 } else { u128::from(weight(*at)) };
            let share = u64::try_from(u128::from(left) * weight / total).unwrap_or(0);
            if let Some(slot) = slots.get_mut(*at) {
                let take = share.min(cap - *slot);
                *slot += take;
                given += take;
            }
        }
        if given == 0 {
            // Less than a unit each: a unit each, in their order.
            for at in &open {
                if given == left {
                    break;
                }
                if let Some(slot) = slots.get_mut(*at) {
                    *slot += 1;
                    given += 1;
                }
            }
        }
        left -= given;
    }
    assert!(
        left == 0 || slots.iter().all(|slot| *slot == cap),
        "spread in its rounds"
    );
    left
}

impl Core {
    /// The shares of what the node can hold toward its peers over the hub
    /// of `scheme` (spec/decisions/hold-allotment.md), with the peers in the
    /// order of priority: every active peering with a method on it that the
    /// node still funds, not one being ended or a method being wound down,
    /// which no request funds. `None` on a scheme over no hub. Plentiful,
    /// capping nothing, while
    /// the scheme has no hub account whose agreed state tells the pool, as
    /// the stub with commitments, which stands in for a hub in tests and
    /// simulations, or one not in step with its hub.
    pub(super) fn hub_allotments(&self, scheme: SchemeId) -> Option<Allotments> {
        let name = Name::new(scheme.as_str()).ok()?;
        let active = self.schemes.get(&name).filter(|active| active.over_hub)?;
        let ledger = active.scheme.ledger();
        let peers: Vec<Peer> = self
            .peer
            .active_peers()
            .filter(|(_, peering)| {
                peering.depeering.is_none()
                    && peering
                        .method(scheme)
                        .is_some_and(|method| !method.winding_down())
            })
            .map(|(node, _)| Peer {
                node: *node,
                downlink: self.peer.is_downlink(node),
                earned: self
                    .peer_endpoints
                    .get(node)
                    .and_then(|id| {
                        self.connector
                            .earnings(EndpointId::Peer(*id), ledger, self.now_ms)
                    })
                    .unwrap_or(0),
                held: self
                    .hub_books(name, *node)
                    .map_or(0, |(credit, _, _)| credit),
            })
            .collect();
        let account = self.hub_account(name);
        let pool = account
            .and_then(Self::held)
            .map(|held| held.pool().saturating_mul(UNITS_PER_DROP));
        let Some(pool) = pool else {
            let peers: Vec<(NodeId, u64)> = by_priority(&peers)
                .iter()
                .map(|peer| (peer.node, self.peer_credit))
                .collect();
            return Some(Allotments {
                plentiful: true,
                full: self.peer_credit,
                reserved: vec![0; peers.len()],
                unopened: vec![false; peers.len()],
                peers,
                reserved_total: 0,
                unopened_count: 0,
                opening: 0,
            });
        };
        let max_packet = self.settle.max_packet;
        let least = policy::least_hold(max_packet, self.connector.config().max_in_flight());
        let explore_ppm = self.connector.config().explore_ppm;
        let full = self.peer_credit;
        let packet = policy::minimum(max_packet);
        let mut allotments = allot(pool, (full, least, packet), explore_ppm, &peers);
        allotments.opening = account.map_or(0, |account| {
            let fees = account.terms().fees;
            fees.operation
                .saturating_add(fees.hold_reserve)
                .saturating_mul(UNITS_PER_DROP)
        });
        Some(allotments)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest packet, and the default credit and least hold in it.
    const P: u64 = 100_000_000;
    const FULL: u64 = 11 * P;
    const LEAST: u64 = 45 * P / 10;
    /// The connector's share of draws that explore.
    const EXPLORE: u32 = 100_000;

    fn node(n: u8) -> NodeId {
        NodeId::from_bytes(&[n; 24])
    }

    fn core(n: u8, earned: u64) -> Peer {
        Peer {
            node: node(n),
            downlink: false,
            earned,
            held: 0,
        }
    }

    fn downlink(n: u8, earned: u64) -> Peer {
        Peer {
            node: node(n),
            downlink: true,
            earned,
            held: 0,
        }
    }

    /// The allotments of `peers` from `pool`, by node.
    fn of(pool: u64, peers: &[Peer]) -> Vec<u64> {
        let allotments = allot(pool, (FULL, LEAST, P), EXPLORE, peers);
        peers
            .iter()
            .map(|peer| {
                allotments
                    .peers
                    .iter()
                    .find(|(node, _)| *node == peer.node)
                    .map(|(_, amount)| *amount)
                    .unwrap()
            })
            .collect()
    }

    // A pool that gives every peer the default credit caps nothing: from
    // exactly that much up, each is allotted all of it, and the allotments
    // say so.
    #[test]
    fn a_plentiful_pool_caps_nothing() {
        let peers = [core(1, 5), downlink(2, 0), core(3, 0)];
        for pool in [3 * FULL, 3 * FULL + 1, u64::MAX] {
            let allotments = allot(pool, (FULL, LEAST, P), EXPLORE, &peers);
            assert!(allotments.plentiful);
            assert!(allotments.peers.iter().all(|(_, amount)| *amount == FULL));
            assert_eq!(allotments.of(&node(2)), Some(FULL));
            assert_eq!(allotments.room(&node(2), 0, Some(0)), None);
        }
        let short = allot(3 * FULL - 1, (FULL, LEAST, P), EXPLORE, &peers);
        assert!(!short.plentiful);
        assert_eq!(short.of(&node(9)), None);
        assert_eq!(
            short.room(&node(9), 0, Some(u64::MAX)),
            Some(0),
            "a peer not sharing the pool"
        );
        // No peers: nothing to share, whatever the pool.
        assert!(allot(0, (FULL, LEAST, P), EXPLORE, &[]).plentiful);
    }

    // Core peers come first, by what they earned us, then downlinks, ties
    // by node ID.
    #[test]
    fn priority_is_core_by_earnings_then_downlinks() {
        let peers = [
            downlink(1, 900),
            core(4, 0),
            core(3, 7),
            core(2, 7),
            downlink(0, 0),
        ];
        let order: Vec<NodeId> = by_priority(&peers).iter().map(|peer| peer.node).collect();
        assert_eq!(order, [node(2), node(3), node(4), node(1), node(0)]);
        let allotments = allot(0, (FULL, LEAST, P), EXPLORE, &peers);
        assert_eq!(allotments.rank(&node(2)), 0);
        assert_eq!(allotments.rank(&node(0)), 4);
        assert_eq!(allotments.rank(&node(9)), 5);
    }

    // Short of the least hold for everyone, core peers get theirs first,
    // in equal shares, and downlinks share what is left; units that do not
    // divide go in the order of priority.
    #[test]
    fn the_least_hold_goes_to_core_peers_first() {
        let peers = [core(1, 0), core(2, 9), downlink(3, 0), downlink(4, 0)];
        // Not even the core peers' least: equal shares, the odd unit to the
        // one that earned more.
        assert_eq!(of(LEAST + 1, &peers), [LEAST / 2, LEAST / 2 + 1, 0, 0]);
        // The core peers' least, and what is left to the downlinks.
        assert_eq!(of(2 * LEAST + 7, &peers), [LEAST, LEAST, 4, 3]);
        // Exactly everyone's least.
        assert_eq!(of(4 * LEAST, &peers), [LEAST; 4]);
        // Nothing: nothing for anyone.
        assert_eq!(of(0, &peers), [0; 4]);
    }

    // With one peer, it gets the whole pool up to the default credit.
    #[test]
    fn one_peer_gets_the_pool() {
        for peer in [core(1, 0), downlink(1, 5)] {
            assert_eq!(of(3 * P, &[peer]), [3 * P]);
            assert_eq!(of(LEAST, &[peer]), [LEAST]);
            assert_eq!(of(FULL - 1, &[peer]), [FULL - 1]);
            assert_eq!(of(FULL, &[peer]), [FULL]);
        }
    }

    // Past everyone's least, the rest goes to core peers by what they
    // earned us, but for a tenth spread evenly: one that earned all of it
    // gets the most, one that earned nothing still a share.
    #[test]
    fn the_rest_follows_the_earnings_with_a_share_for_all() {
        let peers = [core(1, 1_000), core(2, 0), downlink(3, 50)];
        let rest = 2 * P;
        let allotments = of(3 * LEAST + rest, &peers);
        let explore = rest / 10;
        assert_eq!(
            allotments,
            [
                LEAST + explore / 2 + (rest - explore),
                LEAST + explore / 2,
                LEAST
            ]
        );
        // No core peer earned anything: the rest evenly.
        let idle = [core(1, 0), core(2, 0), downlink(3, 50)];
        assert_eq!(of(3 * LEAST + rest, &idle), [LEAST + P, LEAST + P, LEAST]);
    }

    // A core peer full, what it would have had of the rest goes on to the
    // others, and then to the downlinks, the same way.
    #[test]
    fn what_full_peers_cannot_take_goes_on() {
        let peers = [core(1, 1_000), core(2, 1), downlink(3, 0), downlink(4, 7)];
        // The core peers full, the downlinks share the rest.
        let rest = 2 * (FULL - LEAST) + 3 * P;
        let allotments = of(4 * LEAST + rest, &peers);
        assert_eq!(&allotments[..2], [FULL, FULL]);
        let explore = 3 * P / 10;
        assert_eq!(
            &allotments[2..],
            [LEAST + explore / 2, LEAST + explore / 2 + (3 * P - explore)]
        );
        // The one that earned nearly all of it fills, and the other takes
        // the rest.
        let rest = (FULL - LEAST) + P;
        let allotments = of(4 * LEAST + rest, &peers);
        assert_eq!(allotments, [FULL, LEAST + P, LEAST, LEAST]);
    }

    // Whatever the pool and the peers, no more is allotted than the pool,
    // no peer more than the default credit, and while the pool covers it,
    // each at least the least hold.
    #[test]
    fn the_allotments_keep_their_bounds() {
        let peers = [
            core(1, 3),
            core(2, u64::MAX),
            core(3, 0),
            downlink(4, 1),
            downlink(5, u64::MAX),
            downlink(6, 0),
        ];
        let pools = [
            0,
            1,
            LEAST - 1,
            3 * LEAST,
            6 * LEAST - 1,
            6 * LEAST,
            6 * LEAST + 5,
            6 * FULL - 1,
            6 * FULL,
            u64::MAX,
        ];
        for pool in pools {
            for count in 0..=peers.len() {
                let some = &peers[..count];
                let allotted = of(pool, some);
                let total: u128 = allotted.iter().map(|amount| u128::from(*amount)).sum();
                assert!(total <= u128::from(pool), "{pool} {count}");
                assert!(allotted.iter().all(|amount| *amount <= FULL));
                // Nothing kept back while anyone could take more.
                let full = allotted.iter().all(|amount| *amount == FULL);
                assert!(full || total == u128::from(pool), "{pool} {count}");
            }
        }
    }

    // spec/decisions/hold-allotment.md: two hundred downlinks share a
    // thousand packets, five each, and each has a largest packet reserved.
    // A busy one takes more than its allotment, up to the default credit,
    // from what the others leave unused, but never what their reserves
    // need: as busy ones fill, one after another, eighty reach the default
    // credit, and the hundred and twenty after them their packet, all the
    // account has left for them.
    #[test]
    fn a_busy_peer_takes_what_idle_shares_leave() {
        let mut peers: Vec<Peer> = (0..200u8).map(|n| downlink(n, 0)).collect();
        let pool = 1_000 * P;
        let allotments = allot(pool, (FULL, LEAST, P), EXPLORE, &peers);
        assert!(allotments.peers.iter().all(|(_, amount)| *amount == 5 * P));
        assert_eq!(allotments.reserved_total, 200 * P);
        assert_eq!(allotments.room(&node(0), 0, Some(pool)), Some(FULL));

        let mut spendable = pool;
        let mut rooms = Vec::new();
        for n in 0..200u8 {
            let shares = allot(pool, (FULL, LEAST, P), EXPLORE, &peers);
            let room = shares.room(&node(n), 0, Some(spendable)).unwrap();
            let others = shares.reserved_total - P;
            assert!(spendable - room >= others, "the others' reserves stay");
            spendable -= room;
            peers[usize::from(n)].held = room;
            rooms.push(room);
        }
        assert!(rooms[..80].iter().all(|room| *room == FULL));
        assert!(rooms[80..].iter().all(|room| *room == P));
        assert_eq!(spendable, 0);
    }

    // The same two hundred, each hold's opening at the hub's reserve of two
    // packets, paid from what is spare: sixty-six reach the default credit,
    // one more takes the seven packets left, and the others' packets stay
    // set aside, though none can open its hold until an opening is spare.
    #[test]
    fn openings_come_from_what_is_spare() {
        let opening = 2 * P;
        let mut peers: Vec<Peer> = (0..200u8).map(|n| downlink(n, 0)).collect();
        let mut balance = 1_000 * P;
        let mut rooms = Vec::new();
        for n in 0..200u8 {
            let mut shares = allot(1_000 * P, (FULL, LEAST, P), EXPLORE, &peers);
            shares.opening = opening;
            // What the scheme could fund toward a peer with no hold.
            let spendable = balance.saturating_sub(opening);
            let room = shares.room(&node(n), 0, Some(spendable)).unwrap();
            let room = if room < P { 0 } else { room };
            let others = shares.reserved_total - P;
            assert!(room == 0 || spendable - room >= others);
            if room > 0 {
                balance -= room + opening;
                peers[usize::from(n)].held = room;
            }
            rooms.push(room);
        }
        assert!(rooms[..66].iter().all(|room| *room == FULL));
        assert_eq!(rooms[66], 7 * P);
        assert!(rooms[67..].iter().all(|room| *room == 0));
        assert_eq!(balance, 133 * P, "a packet for each of the others");
    }

    // Short of a packet for every peer, the reserve is an equal share of
    // the pool, core peers first.
    #[test]
    fn short_of_a_packet_each_the_reserve_is_the_share() {
        let peers = [core(1, 0), downlink(2, 0), downlink(3, 0)];
        let allotments = allot(P + P / 2, (FULL, LEAST, P), EXPLORE, &peers);
        assert_eq!(allotments.reserved, [P, P / 4, P / 4]);
        // The least shares go to the core peer first all the same.
        assert_eq!(allotments.of(&node(1)), Some(P + P / 2));
        assert_eq!(allotments.reserved_total, P + P / 2);
    }

    // One hold's opening stays spendable while a peer has none: a peer that
    // holds something already leaves it, and one that holds nothing yet may
    // take it.
    #[test]
    fn one_opening_stays_for_the_peers_without_a_hold() {
        let mut peers = [downlink(1, 0), downlink(2, 0), downlink(3, 0)];
        peers[0].held = P;
        let mut allotments = allot(20 * P, (FULL, LEAST, P), EXPLORE, &peers);
        allotments.opening = 2 * P;
        let spendable = 8 * P;
        // The others' packets, and an opening for them.
        assert_eq!(allotments.room(&node(1), P, Some(spendable)), Some(4 * P));
        // A peer without a hold takes the opening itself.
        assert_eq!(allotments.room(&node(2), 0, Some(spendable)), Some(7 * P));
        // Once every peer holds something, no opening stays.
        for peer in &mut peers {
            peer.held = P;
        }
        let mut opened = allot(20 * P, (FULL, LEAST, P), EXPLORE, &peers);
        opened.opening = 2 * P;
        assert_eq!(opened.room(&node(1), P, Some(spendable)), Some(8 * P));
    }

    // Without a reading of what the account can spend, a request goes up
    // to the peer's allotment, and never past the default credit.
    #[test]
    fn without_spendable_the_allotment_caps() {
        let mut peers = [core(1, 0), downlink(2, 0)];
        peers[0].held = P;
        let allotments = allot(8 * P, (FULL, LEAST, P), EXPLORE, &peers);
        assert_eq!(allotments.of(&node(1)), Some(LEAST));
        assert_eq!(allotments.room(&node(1), P, None), Some(LEAST - P));
        assert_eq!(allotments.room(&node(1), FULL, Some(u64::MAX)), Some(0));
        // Holding its packet already, the core peer reserves nothing more.
        assert_eq!(allotments.reserved, [0, P]);
    }
}
