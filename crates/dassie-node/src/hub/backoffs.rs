//! When a validator sends to another validator again after it could not be
//! reached (spec/areas/hubs.md#the-validator-in-the-node): one wait per
//! node, which every hub the node validates shares, since a node that is
//! down is down for each of its hubs, and one that answers in any hub is
//! up for all of them.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dassie_identity::NodeId;

use super::transport::{HubAnswer, HubExchangeError, Inbound};

/// The first wait before sending to a validator that could not be reached.
const BACKOFF_MIN: Duration = Duration::from_millis(250);

/// The longest wait before sending to a validator that could not be
/// reached: one that comes back is sent to again within it.
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// How long a validator that answered busy is left alone: it is up, and
/// its places for validators' requests were taken for a moment, as in a
/// burst of a node that validates several hubs. Consensus sends again what
/// matters.
const BUSY_RETRY: Duration = Duration::from_millis(50);

/// When each validator that could not be reached is sent to again, and the
/// wait its next failure doubles from.
#[derive(Clone, Debug, Default)]
pub struct Backoffs(Arc<Mutex<BTreeMap<NodeId, (tokio::time::Instant, Duration)>>>);

impl Backoffs {
    /// Whether `node` could not be reached lately, so nothing is sent to it
    /// until its wait is over.
    #[must_use]
    pub fn backing_off(&self, node: &NodeId) -> bool {
        self.held()
            .get(node)
            .is_some_and(|(until, _)| *until > tokio::time::Instant::now())
    }

    /// An exchange with `node` succeeded: it is sent to at once from now
    /// on, and its next failure waits the least.
    pub fn reached(&self, node: &NodeId) {
        self.held().remove(node);
    }

    /// Leaves `node` alone for a while after an exchange with it failed
    /// with `error`.
    pub fn failed(&self, node: NodeId, error: &HubExchangeError) {
        let mut held = self.held();
        let previous = held.get(&node).map(|(_, wait)| *wait);
        let (pause, wait) = after_failure(error, previous);
        held.insert(node, (tokio::time::Instant::now() + pause, wait));
    }

    /// Ends the wait before sending again to the node that sent `inbound`,
    /// if the transport proved its key: a validator that comes back is sent
    /// to as soon as anything of it arrives, in any hub
    /// (spec/areas/hubs.md#the-validator-in-the-node). Only the proved key
    /// counts, never the handle a message names, so nobody ends the wait
    /// for a node but that node itself, and all it can make us do is send
    /// to it, each exchange bounded by
    /// [`SEND_TIMEOUT`](super::service::SEND_TIMEOUT), which it can anyway by
    /// answering slowly. The wait the next failure doubles from stays: one
    /// that reaches us but that we cannot reach still waits longer after
    /// each failure, until an exchange with it succeeds. It adds no entry.
    pub fn heard_from(&self, inbound: &Inbound) {
        let Some(key) = inbound.node else {
            return;
        };
        let mut held = self.held();
        if held.is_empty() {
            return;
        }
        if let Some((until, _)) = held.get_mut(&key.node_id()) {
            *until = (*until).min(tokio::time::Instant::now());
        }
    }

    fn held(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<NodeId, (tokio::time::Instant, Duration)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How long to leave a validator alone after an exchange failed with
/// `error`, and the wait the next failure doubles from, after `previous`:
/// one that answered busy is tried again shortly, and its wait does not
/// grow; one that could not be reached waits twice as long as last time,
/// from [`BACKOFF_MIN`] up to [`BACKOFF_MAX`].
fn after_failure(error: &HubExchangeError, previous: Option<Duration>) -> (Duration, Duration) {
    if matches!(error, HubExchangeError::Refused(HubAnswer::Busy)) {
        return (BUSY_RETRY, previous.unwrap_or(Duration::ZERO));
    }
    let wait = previous
        .filter(|wait| !wait.is_zero())
        .map_or(BACKOFF_MIN, |wait| (wait * 2).min(BACKOFF_MAX));
    (wait, wait)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::Sender;
    use dassie_identity::NodeIdentity;

    // A validator that answered busy is up: it is tried again shortly, and
    // a burst of busy answers never backs off from it for longer.
    #[test]
    fn busy_is_no_failure_to_back_off_from() {
        let busy = HubExchangeError::Refused(HubAnswer::Busy);
        let unreachable = HubExchangeError::Unreachable;
        let mut wait = None;
        for _ in 0..10 {
            let (pause, next) = after_failure(&busy, wait);
            assert_eq!(pause, BUSY_RETRY);
            wait = Some(next);
        }
        assert_eq!(
            after_failure(&unreachable, wait),
            (BACKOFF_MIN, BACKOFF_MIN)
        );
        let mut wait = None;
        for _ in 0..10 {
            wait = Some(after_failure(&unreachable, wait).1);
        }
        assert_eq!(
            wait,
            Some(BACKOFF_MAX),
            "an unreachable one is left alone longer"
        );
        assert_eq!(after_failure(&busy, wait), (BUSY_RETRY, BACKOFF_MAX));
    }

    fn from(sender: NodeIdentity, proved: Option<NodeIdentity>) -> Inbound {
        Inbound {
            from: Sender(sender).peer(),
            node: proved,
            message: Vec::new(),
            reply: None,
        }
    }

    // A validator backed off from a peer it could not reach sends to it again
    // as soon as the peer is heard from, by the key the transport proved;
    // nothing else ends the wait, and the wait still grows with the next
    // failure.
    #[tokio::test(start_paused = true)]
    async fn hearing_from_a_validator_ends_the_wait_for_it_alone() {
        let peer = dassie_identity::TableKeygen::identity_key(&[1; 32]).identity();
        let other = dassie_identity::TableKeygen::identity_key(&[2; 32]).identity();
        let backoffs = Backoffs::default();
        for _ in 0..10 {
            backoffs.failed(peer.node_id(), &HubExchangeError::Unreachable);
        }
        backoffs.failed(other.node_id(), &HubExchangeError::Unreachable);
        assert!(backoffs.backing_off(&peer.node_id()));

        // Another node, one that names the peer's handle, and one whose key
        // nothing proved, end nothing.
        backoffs.heard_from(&from(other, Some(other)));
        backoffs.heard_from(&from(peer, Some(other)));
        backoffs.heard_from(&from(peer, None));
        assert!(
            backoffs.backing_off(&peer.node_id()),
            "only the peer itself ends its wait"
        );
        assert!(!backoffs.backing_off(&other.node_id()));

        // The peer itself does, and the other's wait is untouched by it.
        backoffs.failed(other.node_id(), &HubExchangeError::Unreachable);
        backoffs.heard_from(&from(peer, Some(peer)));
        assert!(!backoffs.backing_off(&peer.node_id()), "sent to at once");
        assert!(backoffs.backing_off(&other.node_id()));

        // Its wait still grows from where it was.
        backoffs.failed(peer.node_id(), &HubExchangeError::Unreachable);
        assert_eq!(
            backoffs.held().get(&peer.node_id()).map(|(_, wait)| *wait),
            Some(BACKOFF_MAX)
        );
        assert!(backoffs.backing_off(&peer.node_id()));

        // An exchange that succeeds ends it for good.
        backoffs.reached(&peer.node_id());
        assert!(!backoffs.backing_off(&peer.node_id()));
        assert_eq!(backoffs.held().get(&peer.node_id()), None);
    }
}
