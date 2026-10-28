//! Snapshot subscriptions (spec/decisions/peer-swaps.md,
//! spec/areas/network.md#snapshot-subscriptions): a peer subscribes to a
//! node's snapshots over the peer protocol, and the node pushes each one it
//! publishes to its subscribers; the node subscribes to its peers' in turn,
//! and keeps the latest each sent it.
//!
//! A snapshot is opaque here: the node makes its own and judges a peer's
//! ([`PeeringHooks::takes_snapshot`]). Nothing here is persisted. A
//! subscription needs an active peering and ends with it, so both sides are
//! bounded by the peerings; a publisher that restarted has forgotten its
//! subscribers, and a subscriber that hears nothing for
//! [`SUBSCRIPTION_SILENCE_MS`] subscribes again, which the publisher takes
//! as often as it comes.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};

use super::inbound::{Accepted, ok};
use super::{PeerNode, Problem, Purpose};
use crate::hooks::PeeringHooks;
use crate::io::Reply;
use crate::table::Entry;
use crate::wire::{Content, MAX_SNAPSHOT_BYTES, RefusalReason, SubscriptionResponse};

/// How often the node looks at whom it subscribes to while it has an
/// active peering: a peering that comes to want snapshots, as one that
/// gains a method on a second ledger, is subscribed to within it.
pub const SUBSCRIPTION_CHECK_MS: u64 = 5_000;

/// How long a subscription may stay silent before the node subscribes
/// again: three of the publishers' heartbeats, so that one or two lost on
/// the way do not count, while a publisher that restarted, and forgot us,
/// is found out. The node checks it against the rate feed's heartbeat,
/// whose schedule the publishers keep.
pub const SUBSCRIPTION_SILENCE_MS: u64 = 90_000;

/// How long after a subscription that failed, or was refused for any
/// reason but [`RefusalReason::NoSwaps`], the node asks again.
pub const SUBSCRIPTION_RETRY_MS: u64 = 30_000;

/// How long after a peer said it offers no swaps the node asks again: its
/// owner may turn them on.
pub const SUBSCRIPTION_REFUSED_MS: u64 = 600_000;

/// Snapshots, both ways.
#[derive(Debug, Default)]
pub(crate) struct Snapshots {
    /// Our current snapshot, which a new subscriber is sent at once.
    own: Option<Arc<[u8]>>,
    /// The peers subscribed to our snapshots.
    subscribers: BTreeSet<NodeId>,
    /// The peers a push of ours to is in flight: at most one each. Kept
    /// apart from the subscribers, and cleared only by the push's answer
    /// or its expiry, so that a peer that unsubscribes and subscribes
    /// again, or a peering that ends and begins again, never has a second
    /// one sent while the first is out: each is a request in flight, so
    /// they are as many as those.
    pushing: BTreeSet<NodeId>,
    /// Our subscriptions to peers' snapshots: at most one per active
    /// peering with a core node.
    subscriptions: BTreeMap<NodeId, Subscription>,
    /// When the subscriptions are next looked at.
    check_at_ms: u64,
}

/// Our subscription to a peer's snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Subscription {
    /// Whether a `subscription` of ours to it is in flight.
    asking: bool,
    /// Whether it accepted the last we sent.
    accepted: bool,
    /// When to subscribe again: once a failure's or a refusal's wait is
    /// over, or once it has been silent for [`SUBSCRIPTION_SILENCE_MS`].
    due_ms: u64,
    /// The latest snapshot it sent that the node took.
    snapshot: Option<Vec<u8>>,
}

impl Snapshots {
    /// Forgets both sides of a peering that is no longer active.
    fn end(&mut self, peer: &NodeId) {
        self.subscribers.remove(peer);
        self.subscriptions.remove(peer);
    }
}

impl PeerNode {
    /// Sets the node's current snapshot, signed, which it pushes to its
    /// subscribers ([`push_snapshot`](Self::push_snapshot)) and sends a new
    /// subscriber at once; `None` while it has none to give. Returns
    /// whether it differs from the one before.
    ///
    /// # Panics
    ///
    /// If it is empty or longer than [`MAX_SNAPSHOT_BYTES`]: it is the
    /// node's own.
    pub fn set_snapshot(&mut self, snapshot: Option<Arc<[u8]>>) -> bool {
        if let Some(bytes) = &snapshot {
            assert!(
                (1..=MAX_SNAPSHOT_BYTES).contains(&bytes.len()),
                "a snapshot of a size the wire takes"
            );
        }
        let changed = match (&self.snapshots.own, &snapshot) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new) && old[..] != new[..],
            (None, None) => false,
            _ => true,
        };
        self.snapshots.own = snapshot;
        changed
    }

    /// The node's current snapshot, as last set.
    #[must_use]
    pub fn own_snapshot(&self) -> Option<&[u8]> {
        self.snapshots.own.as_deref()
    }

    /// Pushes the node's current snapshot to every subscriber, but one a
    /// push to which is still in flight: it gets the next. Nothing is sent
    /// again: the next snapshot supersedes a lost one.
    pub fn push_snapshot(&mut self, now_ms: u64) {
        self.advance(now_ms);
        let subscribers: Vec<NodeId> = self.snapshots.subscribers.iter().copied().collect();
        for subscriber in subscribers {
            self.push_to(subscriber);
        }
    }

    /// Whether any peer is subscribed to our snapshots.
    #[must_use]
    pub fn has_subscribers(&self) -> bool {
        !self.snapshots.subscribers.is_empty()
    }

    /// The peers subscribed to our snapshots.
    pub fn subscribers(&self) -> impl Iterator<Item = &NodeId> {
        self.snapshots.subscribers.iter()
    }

    /// Whether we hold a subscription to `peer`'s snapshots that it
    /// accepted.
    #[must_use]
    pub fn subscribed_to(&self, peer: &NodeId) -> bool {
        self.snapshots
            .subscriptions
            .get(peer)
            .is_some_and(|subscription| subscription.accepted)
    }

    /// The latest snapshot `peer` sent us that the node took, while we
    /// subscribe to it: one per peering, at most [`MAX_SNAPSHOT_BYTES`].
    #[must_use]
    pub fn snapshot_of(&self, peer: &NodeId) -> Option<&[u8]> {
        self.snapshots
            .subscriptions
            .get(peer)
            .and_then(|subscription| subscription.snapshot.as_deref())
    }

    /// Every peer's latest snapshot, as [`snapshot_of`](Self::snapshot_of)
    /// gives each.
    pub fn snapshots(&self) -> impl Iterator<Item = (&NodeId, &[u8])> {
        self.snapshots
            .subscriptions
            .iter()
            .filter_map(|(peer, subscription)| Some((peer, subscription.snapshot.as_deref()?)))
    }

    /// Sends our snapshot to `subscriber`, unless a push to it is in
    /// flight or we have none.
    fn push_to(&mut self, subscriber: NodeId) {
        let Some(own) = self.snapshots.own.clone() else {
            return;
        };
        if !self.snapshots.subscribers.contains(&subscriber)
            || self.snapshots.pushing.contains(&subscriber)
        {
            return;
        }
        let timeout = self.config.poll_timeout_ms;
        let content = Content::Snapshot(&own);
        if self
            .send(subscriber, &content, Purpose::Snapshot, timeout)
            .is_some()
        {
            self.snapshots.pushing.insert(subscriber);
        }
    }

    /// A peer's `subscription`: it needs a peering, and to subscribe an
    /// active one. A node that offers no swaps, a periphery node among
    /// them, refuses with [`RefusalReason::NoSwaps`]; otherwise the peer is
    /// subscribed, as often as it asks, and sent our snapshot at once.
    pub(super) fn subscription_request(
        &mut self,
        hooks: &dyn PeeringHooks,
        accepted: &Accepted,
        subscribe: bool,
    ) -> Reply {
        let sender = accepted.sender;
        let Some(entry) = self
            .table
            .get(&sender)
            .filter(|entry| entry.peering().is_some())
        else {
            self.notice_one_sided(sender);
            return Reply::refuse(409, "not peered");
        };
        if !subscribe {
            self.snapshots.subscribers.remove(&sender);
            return ok(&SubscriptionResponse::Accepted.to_vec());
        }
        if entry.active_peering().is_none() {
            return Reply::refuse(409, "the peering is ending");
        }
        if self.is_periphery() || !hooks.offers_swaps() {
            let refused = SubscriptionResponse::Refused(RefusalReason::NoSwaps);
            return ok(&refused.to_vec());
        }
        // A push still in flight to it, from before it last unsubscribed,
        // stands for the one it is sent at once.
        self.snapshots.subscribers.insert(sender);
        self.push_to(sender);
        ok(&SubscriptionResponse::Accepted.to_vec())
    }

    /// A peer's `snapshot`: taken only from an active peer we subscribe
    /// to, or are subscribing to, since its first push may overtake its
    /// acceptance. Any other is refused with 409, which tells its sender
    /// we are not subscribed. Whether the node keeps it is the node's to
    /// judge ([`PeeringHooks::takes_snapshot`]): the answer is the same.
    pub(super) fn snapshot_request(
        &mut self,
        hooks: &dyn PeeringHooks,
        accepted: &Accepted,
        snapshot: &[u8],
    ) -> Reply {
        let sender = accepted.sender;
        if self
            .table
            .get(&sender)
            .and_then(Entry::active_peering)
            .is_none()
        {
            self.notice_one_sided(sender);
            return Reply::refuse(409, "not peered");
        }
        let now = self.now_ms;
        let Some(subscription) = self.snapshots.subscriptions.get_mut(&sender) else {
            return Reply::refuse(409, "not subscribed");
        };
        if hooks.takes_snapshot(
            sender,
            &accepted.key,
            snapshot,
            subscription.snapshot.as_deref(),
        ) {
            subscription.snapshot = Some(snapshot.to_vec());
            if !subscription.asking {
                subscription.due_ms = now.saturating_add(SUBSCRIPTION_SILENCE_MS);
            }
        }
        ok(&[])
    }

    /// The answer to a `subscription` of ours.
    pub(super) fn on_subscription(&mut self, peer: NodeId, result: &Result<Vec<u8>, Problem>) {
        let now = self.now_ms;
        // One no longer wanted was unsubscribed from already, and its
        // pushes are refused.
        let Some(subscription) = self.snapshots.subscriptions.get_mut(&peer) else {
            return;
        };
        subscription.asking = false;
        let answer = result
            .as_ref()
            .ok()
            .and_then(|body| SubscriptionResponse::decode_exact(body).ok());
        let wait = match answer {
            Some(SubscriptionResponse::Accepted) => {
                subscription.accepted = true;
                SUBSCRIPTION_SILENCE_MS
            }
            // What it said before holds no longer.
            Some(SubscriptionResponse::Refused(RefusalReason::NoSwaps)) => {
                subscription.accepted = false;
                subscription.snapshot = None;
                SUBSCRIPTION_REFUSED_MS
            }
            Some(SubscriptionResponse::Refused(_)) | None => SUBSCRIPTION_RETRY_MS,
        };
        subscription.due_ms = now.saturating_add(wait);
    }

    /// The answer to a push of ours: a 409 says the peer does not take our
    /// snapshots, not peered or not subscribed, and it is a subscriber no
    /// more. Nothing else changes anything.
    pub(super) fn on_snapshot_pushed(&mut self, peer: NodeId, result: &Result<Vec<u8>, Problem>) {
        self.snapshots.pushing.remove(&peer);
        if matches!(result, Err(Problem::Status(409))) {
            self.snapshots.subscribers.remove(&peer);
        }
    }

    /// Forgets both sides of the subscriptions with `peer`, whose peering
    /// is active no more.
    pub(super) fn end_snapshots(&mut self, peer: &NodeId) {
        self.snapshots.end(peer);
    }

    /// Looks at whom we subscribe to, every [`SUBSCRIPTION_CHECK_MS`]:
    /// every active peer that is not our downlink, a periphery node that
    /// offers no swaps, and whose snapshots the node wants
    /// ([`PeeringHooks::wants_snapshots`]). One no longer wanted is told
    /// we unsubscribe; one wanted is asked once its wait is over. And a
    /// node that offers swaps no longer forgets its subscribers, which
    /// subscribe again in vain once they hear nothing.
    pub(super) fn tend_snapshots(&mut self, hooks: &dyn PeeringHooks) {
        let now = self.now_ms;
        if now < self.snapshots.check_at_ms {
            return;
        }
        self.snapshots.check_at_ms = now.saturating_add(SUBSCRIPTION_CHECK_MS);
        if self.is_periphery() || !hooks.offers_swaps() {
            self.snapshots.subscribers.clear();
        }
        let wanted: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| !entry.is_downlink())
            .filter_map(|(id, entry)| {
                let peering = entry.active_peering()?;
                hooks.wants_snapshots(*id, peering).then_some(*id)
            })
            .collect();
        let unwanted: Vec<NodeId> = self
            .snapshots
            .subscriptions
            .keys()
            .filter(|peer| wanted.binary_search(peer).is_err())
            .copied()
            .collect();
        let timeout = self.config.poll_timeout_ms;
        for peer in unwanted {
            self.snapshots.subscriptions.remove(&peer);
            let content = Content::Subscription { subscribe: false };
            // Lost, it is made good by our refusal of the peer's next push.
            let _ = self.send(peer, &content, Purpose::Gossip, timeout);
        }
        for peer in wanted {
            let subscription = self
                .snapshots
                .subscriptions
                .entry(peer)
                .or_insert(Subscription {
                    asking: false,
                    accepted: false,
                    due_ms: now,
                    snapshot: None,
                });
            if subscription.asking || subscription.due_ms > now {
                continue;
            }
            let content = Content::Subscription { subscribe: true };
            let sent = self
                .send(peer, &content, Purpose::Subscription, timeout)
                .is_some();
            if let Some(subscription) = self.snapshots.subscriptions.get_mut(&peer) {
                if sent {
                    subscription.asking = true;
                } else {
                    subscription.due_ms = now.saturating_add(SUBSCRIPTION_RETRY_MS);
                }
            }
        }
    }

    /// When the subscriptions are next looked at: only while the node has
    /// an active peering, since only one can subscribe or be subscribed
    /// to.
    pub(super) fn next_snapshot_check(&self) -> Option<u64> {
        self.active_peers()
            .next()
            .map(|_| self.snapshots.check_at_ms)
    }

    /// The invariants of the subscriptions (spec/decisions/tigerstyle.md):
    /// each side is bounded by the active peerings, ours only with core
    /// nodes, and every snapshot within the wire's bound.
    pub(super) fn check_snapshots(&self) {
        let snapshots = &self.snapshots;
        assert!(
            snapshots.subscribers.iter().all(|peer| {
                self.table
                    .get(peer)
                    .and_then(Entry::active_peering)
                    .is_some()
            }),
            "only active peers subscribe"
        );
        assert!(
            snapshots.subscriptions.keys().all(|peer| {
                self.table
                    .get(peer)
                    .is_some_and(|entry| entry.active_peering().is_some() && !entry.is_downlink())
            }),
            "only active core peers are subscribed to"
        );
        let peerings = self.config.max_core_peers() + self.config.max_downlinks;
        assert!(
            snapshots.subscribers.len() <= peerings && snapshots.subscriptions.len() <= peerings,
            "subscriptions within the peerings"
        );
        // Each push in flight is a request in flight, which the bound on
        // requests holds.
        assert!(
            snapshots.pushing.len() <= self.requests.len(),
            "a request per push in flight"
        );
        assert!(
            snapshots
                .own
                .iter()
                .map(|own| own.len())
                .chain(
                    snapshots
                        .subscriptions
                        .values()
                        .filter_map(|subscription| subscription.snapshot.as_ref().map(Vec::len))
                )
                .all(|length| (1..=MAX_SNAPSHOT_BYTES).contains(&length)),
            "snapshots within the wire's bound"
        );
    }
}
