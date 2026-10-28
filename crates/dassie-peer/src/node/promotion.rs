//! What an uplink checks of its downlink before promoting it, advertising it
//! as a neighbor (spec/decisions/promotion-to-core.md): computed from what
//! the node holds, and shown to the owner, but not acted on yet. Whether the
//! downlink answers at its URL is checked apart.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::{PeerNode, Problem, Purpose};
use crate::config::Config;
use crate::io::Event;
use crate::table::{Beats, Entry, Sources};
use crate::wire::{Content, Endpoint, SchemeId};

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
    /// Answered at its URL, with its own link state, the last time we
    /// checked.
    Url,
    /// Answered at its URL over HTTPS, with a certificate a public
    /// authority issued and its own node ID, the last time we checked.
    Https,
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
    /// Whether it answered at its URL the last time we checked, if we did.
    pub url_answered: Option<bool>,
    /// Whether it answered at its URL over HTTPS, with a publicly issued
    /// certificate, the last time we checked, if we did.
    pub https_answered: Option<bool>,
}

impl Promotion {
    /// The checks it does not pass yet, in their order.
    #[must_use]
    pub fn pending(&self, config: &Config) -> Vec<PromotionCheck> {
        let Beats { sent, answered } = self.heartbeats;
        // More than the share, as the decision says.
        let heartbeats = sent > 0
            && u64::from(answered) * 100
                > u64::from(sent) * u64::from(config.promotion_heartbeat_percent);
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
            (self.url_answered == Some(true), PromotionCheck::Url),
            (self.https_answered == Some(true), PromotionCheck::Https),
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
        let entry = self.table.get(peer).filter(|entry| is_candidate(entry))?;
        let peering = entry.active_peering()?;
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
            url_answered: entry.url.quic,
            https_answered: entry.url.https,
        })
    }

    /// Checks that each candidate answers at its URL, again once per
    /// bucket of its standing: a `linkStateRequest` for it at its URL, on a
    /// connection pinned to its key, which must return its own link state
    /// at the sequence we hold or later, as a probe for a registration does
    /// (spec/areas/network.md#registration), and over HTTPS by the node.
    /// Checked are a candidate that paid us, a settlement of at least one
    /// largest packet, which keys made for nothing never pay, so a node's
    /// free downlinks cost it no checks; and, on a bootstrap node, every
    /// node registered with it, whose registration a URL that fails for
    /// long enough ends ([`maintain`](Self::maintain)). The checks have a
    /// bucket of their own, sized for every downlink and registration the
    /// node may have, apart from the probes inbound requests wait on, so
    /// they never take those probes' room, and none goes twice at once to a
    /// node or host.
    pub(super) fn check_urls(&mut self) {
        let now = self.now_ms;
        let due: Vec<(NodeId, Endpoint, u64)> = self
            .table
            .iter()
            .filter(|(id, entry)| {
                let candidate = is_candidate(entry)
                    && self.channels.get(id).is_some_and(|channel| channel.paid_us);
                entry.url.check_at_ms <= now && (candidate || entry.registration.is_some())
            })
            .filter_map(|(id, entry)| {
                let link = entry.link.as_ref()?;
                Some((*id, link.endpoint.clone()?, link.sequence))
            })
            .collect();
        let (bucket, timeout) = (self.standing_bucket_ms(), self.config.poll_timeout_ms);
        let rate = url_check_rate(&self.config);
        for (node, endpoint, sequence) in due {
            if self
                .url_checking
                .iter()
                .any(|(id, checked)| *id == node || *checked == endpoint)
            {
                continue;
            }
            if !self.url_checks.take(rate, now) {
                return;
            }
            self.url_checking.push((node, endpoint.clone()));
            let content = Content::LinkStateRequest(alloc::vec![node]);
            let purpose = Purpose::UrlCheck(sequence);
            if self
                .send_via(node, Some(endpoint), &content, purpose, timeout)
                .is_none()
            {
                self.url_checking.retain(|(id, _)| *id != node);
                self.url_checks.give_back(rate);
                return;
            }
            if let Some(entry) = self.table.get_mut(&node) {
                entry.url.check_at_ms = now.saturating_add(bucket);
            }
            // And over HTTPS, by the node, under the same bucket.
            let url = self
                .table
                .get(&node)
                .and_then(|entry| entry.link.as_ref())
                .and_then(|link| link.endpoint.as_ref())
                .map(Endpoint::url);
            if let Some(url) = url {
                self.event(Event::CheckHttps { peer: node, url });
            }
        }
    }

    /// How a check of `peer`'s URL over HTTPS went, which
    /// [`Event::CheckHttps`] asked for: it answered at `url` with a
    /// certificate a public authority issued, and its own node ID, if
    /// `answered`. An answer for a URL it no longer has says nothing.
    pub fn https_checked(&mut self, now_ms: u64, peer: &NodeId, url: &str, answered: bool) {
        self.advance(now_ms);
        let Some(entry) = self.table.get_mut(peer) else {
            return;
        };
        let current = entry
            .link
            .as_ref()
            .and_then(|link| link.endpoint.as_ref())
            .is_some_and(|endpoint| endpoint.url() == url);
        if current {
            entry.url.https = Some(answered);
            entry.url.settle(self.now_ms);
        }
    }

    /// The checks of candidates' URLs in flight: each a request in flight,
    /// so bounded as those are.
    pub(super) fn check_url_checks(&self) {
        assert!(
            self.url_checking.len() <= self.config.max_requests,
            "URL checks within bounds"
        );
    }

    /// The answer to a check of `node`'s URL.
    pub(super) fn on_url_check(
        &mut self,
        node: NodeId,
        sequence: u64,
        result: Result<Vec<u8>, Problem>,
    ) {
        self.url_checking.retain(|(id, _)| *id != node);
        let answered = super::inbound::answers_as(node, sequence, result);
        let now = self.now_ms;
        if let Some(entry) = self.table.get_mut(&node) {
            entry.url.quic = Some(answered);
            entry.url.settle(now);
        }
    }
}

/// The rate of our checks of nodes' URLs: as many as the node may have
/// downlinks and registrations, once per bucket of a downlink's standing,
/// in bursts as large as the probes'.
pub(super) fn url_check_rate(config: &Config) -> crate::config::Rate {
    let bucket_ms = config.promotion_standing_ms / crate::table::STANDING_BUCKETS;
    let checked = config
        .max_downlinks
        .saturating_add(config.max_registrations)
        .max(1);
    crate::config::Rate {
        burst: config.probe_rate.burst,
        refill_ms: (bucket_ms / u64::try_from(checked).unwrap_or(u64::MAX)).max(1),
    }
}

/// Whether `entry` is a candidate for promotion: our downlink, its peering
/// active, whose link state has a URL.
fn is_candidate(entry: &Entry) -> bool {
    entry.is_downlink()
        && entry.active_peering().is_some()
        && entry
            .link
            .as_ref()
            .is_some_and(|link| !link.periphery && link.endpoint.is_some())
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
                answered: 91,
            },
            room_packets: PROMOTION_ROOM_PACKETS,
            paid_us: true,
            skew_ms: Some(-i64::try_from(Config::DEFAULT.max_future_ms).unwrap()),
            banned: false,
            url_answered: Some(true),
            https_answered: Some(true),
        }
    }

    /// [`passing`], changed by `change`.
    fn with(change: impl FnOnce(&mut Promotion)) -> Promotion {
        let mut promotion = passing();
        change(&mut promotion);
        promotion
    }

    // spec/decisions/promotion-to-core.md: each check, at its bound.
    #[test]
    fn each_check_at_its_bound() {
        let config = Config::DEFAULT;
        assert!(passing().pending(&config).is_empty());
        let over = i64::try_from(config.max_future_ms).unwrap() + 1;
        let failing = [
            (with(|p| p.registered = false), PromotionCheck::Registered),
            (
                with(|p| p.standing_ms = config.promotion_standing_ms - 1),
                PromotionCheck::Standing,
            ),
            // More than the share, not as much.
            (
                with(|p| p.heartbeats.answered = 90),
                PromotionCheck::Heartbeats,
            ),
            (
                with(|p| p.heartbeats = Beats::default()),
                PromotionCheck::Heartbeats,
            ),
            (
                with(|p| p.room_packets = PROMOTION_ROOM_PACKETS - 1),
                PromotionCheck::Room,
            ),
            (with(|p| p.paid_us = false), PromotionCheck::Settled),
            (with(|p| p.skew_ms = Some(over)), PromotionCheck::Clock),
            (with(|p| p.skew_ms = Some(-over)), PromotionCheck::Clock),
            (with(|p| p.skew_ms = None), PromotionCheck::Clock),
            (with(|p| p.url_answered = Some(false)), PromotionCheck::Url),
            (with(|p| p.url_answered = None), PromotionCheck::Url),
            (
                with(|p| p.https_answered = Some(false)),
                PromotionCheck::Https,
            ),
            (with(|p| p.https_answered = None), PromotionCheck::Https),
            (with(|p| p.banned = true), PromotionCheck::NotBanned),
        ];
        for (promotion, check) in failing {
            assert_eq!(
                promotion.pending(&config),
                alloc::vec![check],
                "{promotion:?}"
            );
        }
    }
}
