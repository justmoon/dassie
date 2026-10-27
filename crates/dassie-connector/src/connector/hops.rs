//! A packet's next hop and the method it takes there, and what each hop
//! earned us (spec/areas/connector.md#routing,
//! spec/decisions/weighted-next-hops.md,
//! spec/decisions/settlement-methods.md).
//!
//! Each packet's next hop is drawn from a route's usable options with a
//! number the caller draws from its RNG, so a simulation stays
//! deterministic. A hop is usable while it is an endpoint, is not the
//! packet's source, is not draining, and has a method available. Among the
//! options with an available method on the ledger the packet came in on,
//! each is weighted by the fees we earned on packets fulfilled through it on
//! that ledger that kept it, halved every half-life, and the packet goes on
//! that method; a fixed share of the draws ignores the weights. Only when no
//! option keeps the ledger does a peer's packet go to any usable option,
//! each as likely, on the method the price table gives the best rate for
//! ([`route`](super::Connector)). The owner's own packets never convert, so
//! they go only to options with a method on their own ledger. Which method
//! a packet takes depends on the methods available, never on their room.

use dassie_ilp::Address;
use dassie_ledger::LedgerId;

use super::{Connector, MAX_ROUTE_OPTIONS};
use crate::config::PPM;
use crate::endpoint::EndpointId;

/// A usable option and its weight.
type Weighted = (EndpointId, u64);

/// Where a packet goes next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hop {
    /// To this endpoint, on its method on this ledger; with none, a peer's
    /// packet that converts, on the hop's method the price table gives the
    /// best rate for.
    To(EndpointId, Option<LedgerId>),
    /// A route has hops, but none with a method a packet may take now: a
    /// want of liquidity, which passes.
    NoMethod,
    /// No route.
    None,
}

impl Connector {
    /// The next hop for `destination`, for a packet from `source` on its
    /// slot on `incoming`: below our own address only children (or none);
    /// beyond it, the longest whole-segment prefix in the route table, and
    /// one of its usable options, drawn with `draw`, so packets do not
    /// bounce between peers. Without a draw, for
    /// [`first_hop`](Self::first_hop), the first usable option with a
    /// method available on `incoming`, or else the first usable option and
    /// its first available method.
    pub(crate) fn next_hop(
        &self,
        now_ms: u64,
        destination: Address<'_>,
        (source, incoming): (Option<EndpointId>, Option<LedgerId>),
        draw: Option<u64>,
    ) -> Hop {
        let Ok(own) = Address::parse(&self.own_address) else {
            return Hop::None;
        };
        // A child may address itself; only routes between nodes could
        // loop.
        if destination.has_prefix(own) {
            return self
                .endpoints
                .route_child(destination)
                .map_or(Hop::None, |child| {
                    self.draw_hop(now_ms, &[Some(child)], (source, incoming), false, draw)
                });
        }
        if let Some(below) = self.below_child_address(destination) {
            return self
                .endpoints
                .route_child_below(&self.own_address, below)
                .map_or(Hop::None, |child| {
                    self.draw_hop(now_ms, &[Some(child)], (source, incoming), false, draw)
                });
        }
        // A periphery node forwards nothing for a peer.
        if self.periphery.is_some() && source.is_some_and(EndpointId::is_peer) {
            return Hop::None;
        }
        let text = destination.as_str();
        let mut end = text.len();
        let mut no_method = false;
        while let Some(prefix) = text.get(..end) {
            if let Some(options) = self.routes.get(prefix) {
                match self.draw_hop(now_ms, options, (source, incoming), true, draw) {
                    hop @ Hop::To(..) => return hop,
                    Hop::NoMethod => no_method = true,
                    Hop::None => {}
                }
            }
            let Some(at) = prefix.rfind('.') else {
                break;
            };
            end = at;
        }
        if no_method { Hop::NoMethod } else { Hop::None }
    }

    /// One of `options` for a packet from `source` on `incoming`, drawn
    /// with `draw`: weighted among those with an available method on
    /// `incoming`, which it then takes; otherwise, for a peer's packet,
    /// each as likely, its method left to the price table. The owner's own
    /// packets with no option on their ledger take the first usable one and
    /// its first available method, which refuses a packet that moves money
    /// (`amounts`). Without a draw, the first of those it would draw from,
    /// in the order of `options`. With `exclude_source`, the source is no
    /// option.
    fn draw_hop(
        &self,
        now_ms: u64,
        options: &[Option<EndpointId>],
        (source, incoming): (Option<EndpointId>, Option<LedgerId>),
        exclude_source: bool,
        draw: Option<u64>,
    ) -> Hop {
        let half_life_ms = self.config.earnings_half_life_ms;
        let mut reachable = false;
        let mut first = None;
        let mut keeping = [(EndpointId::Peer(0), 0); MAX_ROUTE_OPTIONS];
        let mut kept = 0;
        let mut any = [(EndpointId::Peer(0), 0); MAX_ROUTE_OPTIONS];
        let mut usable = 0;
        for hop in options.iter().flatten().copied() {
            if exclude_source && source == Some(hop) {
                continue;
            }
            let Some(endpoint) = self.endpoints.get(hop).filter(|e| !e.draining) else {
                continue;
            };
            reachable = true;
            let Some(slot) = endpoint.first_usable() else {
                continue;
            };
            first = first.or(Some((hop, slot.ledger)));
            if let Some(on) = incoming.and_then(|incoming| endpoint.usable_on(incoming))
                && let Some(entry) = keeping.get_mut(kept)
            {
                let (weight, _) = decayed(on.earned, on.earned_since_ms, now_ms, half_life_ms);
                *entry = (hop, weight);
                kept += 1;
            }
            if let Some(entry) = any.get_mut(usable) {
                // Converting packets are drawn evenly: no weight.
                *entry = (hop, 0);
                usable += 1;
            }
        }
        let explore_ppm = self.config.explore_ppm;
        let chosen = |options: &[Weighted], ledger| {
            let hop = match draw {
                Some(draw) => choose(options, draw, explore_ppm),
                None => options.first().map(|(hop, _)| *hop),
            };
            hop.map_or(Hop::None, |hop| Hop::To(hop, ledger))
        };
        match incoming {
            Some(incoming) if kept > 0 => {
                chosen(keeping.get(..kept).unwrap_or_default(), Some(incoming))
            }
            _ if source.is_some_and(EndpointId::is_peer) && usable > 0 => {
                chosen(any.get(..usable).unwrap_or_default(), None)
            }
            _ => match first {
                Some((hop, ledger)) => Hop::To(hop, Some(ledger)),
                None if reachable => Hop::NoMethod,
                None => Hop::None,
            },
        }
    }

    /// Counts `fee` toward `to`'s weight on its method on `ledger` in the
    /// draw: the fee we earned on a packet fulfilled through it that kept
    /// its ledger. Its earlier earnings there are halved first for every
    /// half-life since; with none left, a half-life starts now.
    pub(crate) fn earn(&mut self, now_ms: u64, to: EndpointId, ledger: LedgerId, fee: u64) {
        if fee == 0 || !to.is_peer() {
            return;
        }
        let half_life_ms = self.config.earnings_half_life_ms;
        if let Some(slot) = self.endpoints.slot_mut(to, ledger) {
            let (earned, since_ms) =
                decayed(slot.earned, slot.earned_since_ms, now_ms, half_life_ms);
            slot.earned = earned.saturating_add(fee);
            // Earnings from nothing start their half-life now.
            slot.earned_since_ms = if earned == 0 { now_ms } else { since_ms };
        }
    }

    /// What a peer earned us on its method on `ledger` as of `now_ms`: its
    /// weight in drawing next hops among those with a method on that
    /// ledger.
    #[must_use]
    pub fn earnings(&self, id: EndpointId, ledger: LedgerId, now_ms: u64) -> Option<u64> {
        let half_life_ms = self.config.earnings_half_life_ms;
        self.endpoints
            .slot(id, ledger)
            .map(|slot| decayed(slot.earned, slot.earned_since_ms, now_ms, half_life_ms).0)
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
