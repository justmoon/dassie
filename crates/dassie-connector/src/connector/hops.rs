//! A packet's next hop, and what each hop earned us
//! (spec/areas/connector.md#routing, spec/decisions/weighted-next-hops.md).
//!
//! Beyond our own address, each packet's next hop is drawn from a route's
//! usable options with a number the caller draws from its RNG, so a
//! simulation stays deterministic. Among the options on the ledger the
//! packet came in on, each is weighted by the fees we earned on packets
//! fulfilled through it that kept their ledger, halved every half-life; a
//! fixed share of the draws ignores the weights. Only when no option keeps
//! the ledger does a peer's packet go to any usable option, each as likely.
//! The owner's own packets never convert, so they go only to options on
//! their own ledger.

use dassie_ilp::Address;

use super::{Connector, MAX_ROUTE_OPTIONS};
use crate::config::PPM;
use crate::endpoint::EndpointId;

/// A usable option and its weight.
type Weighted = (EndpointId, u64);

impl Connector {
    /// The next hop for `destination`: below our own address only
    /// children (or F02); beyond it, the longest whole-segment
    /// prefix in the route table, and one of its options that is an
    /// endpoint, is not the packet's source (so packets do not bounce
    /// between peers) and is not draining, drawn with `draw`. Without a
    /// source, for [`first_hop`](Self::first_hop), the first such option.
    pub(crate) fn next_hop(
        &self,
        now_ms: u64,
        destination: Address<'_>,
        source: Option<EndpointId>,
        draw: u64,
    ) -> Option<EndpointId> {
        let own = Address::parse(&self.own_address).ok()?;
        if destination.has_prefix(own) {
            // A child may address itself; only routes between nodes could
            // loop.
            return self.endpoints.route_child(destination);
        }
        if let Some(below) = self.below_child_address(destination) {
            return self.endpoints.route_child_below(&self.own_address, below);
        }
        // A periphery node forwards nothing for a peer.
        if self.periphery.is_some() && source.is_some_and(EndpointId::is_peer) {
            return None;
        }
        let text = destination.as_str();
        let mut end = text.len();
        loop {
            let prefix = text.get(..end)?;
            if let Some(options) = self.routes.get(prefix)
                && let Some(hop) = self.draw_hop(now_ms, options, source, draw)
            {
                return Some(hop);
            }
            end = prefix.rfind('.')?;
        }
    }

    /// One of a route's usable `options` for a packet from `source`, drawn
    /// with `draw`: weighted among those on the packet's incoming ledger,
    /// and otherwise, for a peer's packet, each as likely. The owner's own
    /// packets with no option on their ledger take the first usable one,
    /// which refuses a packet that moves money (`amounts`).
    fn draw_hop(
        &self,
        now_ms: u64,
        options: &[Option<EndpointId>; MAX_ROUTE_OPTIONS],
        source: Option<EndpointId>,
        draw: u64,
    ) -> Option<EndpointId> {
        let usable = |hop: &&EndpointId| {
            Some(**hop) != source
                && self
                    .endpoints
                    .get(**hop)
                    .is_some_and(|endpoint| !endpoint.draining)
        };
        let first = options.iter().flatten().find(usable).copied();
        let Some(incoming) = source
            .and_then(|source| self.endpoints.get(source))
            .map(|endpoint| endpoint.ledger)
        else {
            return first;
        };
        let half_life_ms = self.config.earnings_half_life_ms;
        let mut keeping = [(EndpointId::Peer(0), 0); MAX_ROUTE_OPTIONS];
        let mut kept = 0;
        let mut any = [(EndpointId::Peer(0), 0); MAX_ROUTE_OPTIONS];
        let mut usable_count = 0;
        for hop in options.iter().flatten().filter(usable) {
            let Some(endpoint) = self.endpoints.get(*hop) else {
                continue;
            };
            if endpoint.ledger == incoming
                && let Some(slot) = keeping.get_mut(kept)
            {
                let (weight, _) = decayed(
                    endpoint.earned,
                    endpoint.earned_since_ms,
                    now_ms,
                    half_life_ms,
                );
                *slot = (*hop, weight);
                kept += 1;
            }
            if let Some(slot) = any.get_mut(usable_count) {
                // Converting packets are drawn evenly: no weight.
                *slot = (*hop, 0);
                usable_count += 1;
            }
        }
        if kept > 0 {
            return choose(keeping.get(..kept)?, draw, self.config.explore_ppm);
        }
        if source.is_some_and(EndpointId::is_peer) {
            return choose(any.get(..usable_count)?, draw, self.config.explore_ppm);
        }
        first
    }

    /// Counts `fee` toward `to`'s weight in the draw: the fee we earned on
    /// a packet fulfilled through it that kept its ledger. Its earlier
    /// earnings are halved first for every half-life since; with none
    /// left, a half-life starts now.
    pub(crate) fn earn(&mut self, now_ms: u64, to: EndpointId, fee: u64) {
        if fee == 0 || !to.is_peer() {
            return;
        }
        let half_life_ms = self.config.earnings_half_life_ms;
        if let Some(endpoint) = self.endpoints.get_mut(to) {
            let (earned, since_ms) = decayed(
                endpoint.earned,
                endpoint.earned_since_ms,
                now_ms,
                half_life_ms,
            );
            endpoint.earned = earned.saturating_add(fee);
            // Earnings from nothing start their half-life now.
            endpoint.earned_since_ms = if earned == 0 { now_ms } else { since_ms };
        }
    }

    /// What a peer earned us as of `now_ms`: its weight in drawing next
    /// hops among those on its ledger.
    #[must_use]
    pub fn earnings(&self, id: EndpointId, now_ms: u64) -> Option<u64> {
        let half_life_ms = self.config.earnings_half_life_ms;
        self.endpoints.get(id).map(|endpoint| {
            decayed(
                endpoint.earned,
                endpoint.earned_since_ms,
                now_ms,
                half_life_ms,
            )
            .0
        })
    }
}

/// `earned` as of `since_ms`, halved for every whole half-life until
/// `now_ms`, and when the half-life it is then in began. Integer and
/// exact: decaying in two steps gives what one gives.
fn decayed(earned: u64, since_ms: u64, now_ms: u64, half_life_ms: u64) -> (u64, u64) {
    assert!(half_life_ms > 0, "earnings decay");
    let halvings = now_ms.saturating_sub(since_ms) / half_life_ms;
    let shift = u32::try_from(halvings).unwrap_or(u32::MAX);
    let earned = earned.checked_shr(shift).unwrap_or(0);
    // At most the time elapsed, so it stays at or before `now_ms`.
    (earned, since_ms + halvings * half_life_ms)
}

/// One of `options`, drawn with `draw`: its high half decides whether the
/// draw explores, which `explore_ppm` of draws do, and its low half picks.
/// Exploring, or while none has a weight, each option is as likely;
/// otherwise each in proportion to its weight.
fn choose(options: &[Weighted], draw: u64, explore_ppm: u32) -> Option<EndpointId> {
    let total: u128 = options.iter().map(|(_, weight)| u128::from(*weight)).sum();
    let explores = ((draw >> 32) * u64::from(PPM)) >> 32 < u64::from(explore_ppm);
    if explores || total == 0 {
        let count = u128::try_from(options.len()).ok()?;
        let at = usize::try_from(pick(draw, count)).ok()?;
        return options.get(at).map(|(hop, _)| *hop);
    }
    let mut target = pick(draw, total);
    for (hop, weight) in options {
        let weight = u128::from(*weight);
        if target < weight {
            return Some(*hop);
        }
        target -= weight;
    }
    // `pick` stays below the total, so some option holds it.
    None
}

/// A number below `bound` from the low half of `draw`, each as likely as
/// 32 bits allow.
const fn pick(draw: u64, bound: u128) -> u128 {
    ((draw & 0xFFFF_FFFF) as u128 * bound) >> 32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earnings_halve_once_per_half_life() {
        assert_eq!(decayed(1_000, 0, 999, 1_000), (1_000, 0));
        assert_eq!(decayed(1_000, 0, 1_000, 1_000), (500, 1_000));
        assert_eq!(decayed(1_000, 500, 2_600, 1_000), (250, 2_500));
        assert_eq!(decayed(u64::MAX, 0, u64::MAX, 1), (0, u64::MAX));
        // In two steps as in one.
        let (half, since) = decayed(1_000, 0, 1_700, 1_000);
        assert_eq!(
            decayed(half, since, 3_100, 1_000),
            decayed(1_000, 0, 3_100, 1_000)
        );
    }

    #[test]
    fn picks_stay_below_their_bound() {
        for draw in [0, 1, u64::MAX, 0xFFFF_FFFF, 1 << 32] {
            for bound in [1, 2, 3, 4, u128::from(u64::MAX) * 4] {
                assert!(pick(draw, bound) < bound);
            }
        }
    }

    #[test]
    fn the_exploring_share_is_of_the_high_half() {
        let options = [(EndpointId::Peer(1), 0), (EndpointId::Peer(2), 1)];
        // Never exploring, the weightless option is never drawn.
        for low in [0, 1 << 31, 0xFFFF_FFFF] {
            assert_eq!(choose(&options, low, 0), Some(EndpointId::Peer(2)));
        }
        // Always exploring, the low half picks evenly.
        assert_eq!(choose(&options, 0, PPM), Some(EndpointId::Peer(1)));
        assert_eq!(choose(&options, 1 << 31, PPM), Some(EndpointId::Peer(2)));
        // A tenth: the lowest tenth of the high half explores.
        let tenth = u64::from(u32::MAX / 10) << 32;
        assert_eq!(choose(&options, tenth, 100_000), Some(EndpointId::Peer(1)));
        let past = u64::from(u32::MAX / 10 + 2) << 32;
        assert_eq!(choose(&options, past, 100_000), Some(EndpointId::Peer(2)));
    }
}
