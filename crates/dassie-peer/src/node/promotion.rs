//! What an uplink checks of its downlink before promoting it, advertising it
//! as a neighbor (spec/decisions/promotion-to-core.md): computed from what
//! the node holds, and shown to the owner, but not acted on yet. Whether the
//! downlink answers at its URL is checked apart.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::PeerNode;
use crate::config::Config;
use crate::table::{Beats, Sources};
use crate::wire::SchemeId;

/// The room in our books, in the largest packets, a downlink must have on a
/// method of its peering to be promoted.
pub const PROMOTION_ROOM_PACKETS: u64 = 20;

/// A check a downlink must pass to be promoted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromotionCheck {
    /// On the majority of the bootstrap nodes' lists, which probed its URL.
    Registered,
    /// In good standing for [`Config::promotion_standing_ms`] without a
    /// break.
    Standing,
    /// Answered at least [`Config::promotion_heartbeat_percent`] of our
    /// heartbeats over that time.
    Heartbeats,
    /// At least [`PROMOTION_ROOM_PACKETS`] of room in our books.
    Room,
    /// Paid us a settlement of at least one largest packet.
    Settled,
    /// Its clock within bounds of ours, as its link states show.
    Clock,
    /// Not banned.
    NotBanned,
}

/// Where a downlink stands toward its promotion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Promotion {
    /// Whether it is registered.
    pub registered: bool,
    /// How long it has been in good standing without a break.
    pub standing_ms: u64,
    /// Our heartbeats to it over [`Config::promotion_standing_ms`], and
    /// those it answered.
    pub heartbeats: Beats,
    /// Its most room in our books on a method of its peering, in the
    /// largest packets.
    pub room_packets: u64,
    /// Whether it ever paid us a settlement of at least one largest packet.
    pub paid_us: bool,
    /// How far its clock is ahead of ours, if a link state of its came
    /// straight from it.
    pub skew_ms: Option<i64>,
    /// Whether it is banned.
    pub banned: bool,
}

impl Promotion {
    /// The checks it does not pass yet, in their order.
    #[must_use]
    pub fn pending(&self, config: &Config) -> Vec<PromotionCheck> {
        let Beats { sent, answered } = self.heartbeats;
        let heartbeats = sent > 0
            && u64::from(answered) * 100
                >= u64::from(sent) * u64::from(config.promotion_heartbeat_percent);
        let clock = self
            .skew_ms
            .is_some_and(|skew| skew.unsigned_abs() <= config.max_future_ms);
        [
            (self.registered, PromotionCheck::Registered),
            (
                self.standing_ms >= config.promotion_standing_ms,
                PromotionCheck::Standing,
            ),
            (heartbeats, PromotionCheck::Heartbeats),
            (
                self.room_packets >= PROMOTION_ROOM_PACKETS,
                PromotionCheck::Room,
            ),
            (self.paid_us, PromotionCheck::Settled),
            (clock, PromotionCheck::Clock),
            (!self.banned, PromotionCheck::NotBanned),
        ]
        .into_iter()
        .filter(|(passes, _)| !passes)
        .map(|(_, check)| check)
        .collect()
    }
}

impl PeerNode {
    /// Where `peer` stands toward its promotion, if it is a candidate: our
    /// downlink, its peering active, whose link state has a URL. Asking is
    /// having one: nothing else in its link state says it wants to be core
    /// (spec/decisions/promotion-to-core.md). `room_packets` is its room in
    /// our books on a scheme, in the largest packets: what it may still owe
    /// us there, our own position for it, the only side a node knows
    /// (spec/decisions/own-fulfills.md).
    #[must_use]
    pub fn promotion(
        &self,
        peer: &NodeId,
        room_packets: impl Fn(SchemeId) -> u64,
    ) -> Option<Promotion> {
        let entry = self.table.get(peer)?;
        let peering = entry.active_peering()?;
        let link = entry.link.as_ref()?;
        if !entry.is_downlink() || link.periphery || link.endpoint.is_none() {
            return None;
        }
        let room_packets = peering.schemes().map(room_packets).max().unwrap_or(0);
        Some(Promotion {
            registered: entry.sources.contains(Sources::MAJORITY),
            standing_ms: entry
                .standing
                .since_ms
                .map_or(0, |since| self.now_ms.saturating_sub(since)),
            heartbeats: entry.standing.counted(),
            room_packets,
            // An active peering's channel is open.
            paid_us: self
                .channels
                .get(peer)
                .is_some_and(|channel| channel.paid_us),
            skew_ms: entry.standing.skew_ms,
            banned: self.bans.contains(peer),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing() -> Promotion {
        Promotion {
            registered: true,
            standing_ms: Config::DEFAULT.promotion_standing_ms,
            heartbeats: Beats {
                sent: 100,
                answered: 90,
            },
            room_packets: PROMOTION_ROOM_PACKETS,
            paid_us: true,
            skew_ms: Some(-i64::try_from(Config::DEFAULT.max_future_ms).unwrap()),
            banned: false,
        }
    }

    // spec/decisions/promotion-to-core.md: each check, at its bound.
    #[test]
    fn each_check_at_its_bound() {
        let config = Config::DEFAULT;
        assert!(passing().pending(&config).is_empty());
        let fails = |promotion: Promotion, check| {
            assert_eq!(promotion.pending(&config), alloc::vec![check]);
        };
        fails(
            Promotion {
                registered: false,
                ..passing()
            },
            PromotionCheck::Registered,
        );
        fails(
            Promotion {
                standing_ms: config.promotion_standing_ms - 1,
                ..passing()
            },
            PromotionCheck::Standing,
        );
        fails(
            Promotion {
                heartbeats: Beats {
                    sent: 100,
                    answered: 89,
                },
                ..passing()
            },
            PromotionCheck::Heartbeats,
        );
        fails(
            Promotion {
                heartbeats: Beats::default(),
                ..passing()
            },
            PromotionCheck::Heartbeats,
        );
        fails(
            Promotion {
                room_packets: PROMOTION_ROOM_PACKETS - 1,
                ..passing()
            },
            PromotionCheck::Room,
        );
        fails(
            Promotion {
                paid_us: false,
                ..passing()
            },
            PromotionCheck::Settled,
        );
        let over = i64::try_from(config.max_future_ms).unwrap() + 1;
        for skew in [Some(over), Some(-over), None] {
            fails(
                Promotion {
                    skew_ms: skew,
                    ..passing()
                },
                PromotionCheck::Clock,
            );
        }
        fails(
            Promotion {
                banned: true,
                ..passing()
            },
            PromotionCheck::NotBanned,
        );
    }
}
