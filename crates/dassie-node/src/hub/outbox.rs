//! What a node's validators send other validators in batches
//! (spec/areas/hubs.md#the-validator-in-the-node,
//! spec/areas/peer-protocol.md#the-batch-stream): a queue per validator
//! node, which the validators of every hub the node validates fill, and at
//! most [`MAX_IN_FLIGHT`] batches on their way to it at once. A message
//! that finds fewer on their way goes at once, alone; those that come
//! while they are go together in the next, so a node that validates
//! several hubs, or a step that sends several messages, opens one stream
//! where it opened one per message.
//!
//! Each answer that comes back goes to its hub's validator as a message
//! from the node it was sent to, as soon as it comes. The wait before
//! sending again to a node is one per node, for every hub, so it grows only
//! with what says the node itself is out of reach ([`Backoffs`]): no
//! connection to it, or a reply that is no reply. A batch that brings any
//! answer but busy reached it; one that brings only busy answers, or none
//! because its stream broke off or ran out of time, as when the only hubs
//! in it are slow at the node or the node left them out, found the node up,
//! and goes again shortly, as one that found it busy.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use dassie_hub::HubId;
use dassie_identity::NodeId;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::backoffs::Backoffs;
use super::batch::{MAX_BATCH_BYTES, MAX_BATCH_FRAMES, encode_batch};
use super::driver::Peer;
use super::service::SEND_TIMEOUT;
use super::transport::{FRAME_LENGTH_BYTES, HUB_ID_BYTES, HubAnswer, HubExchangeError, Inbound};
use super::{HubClient, Sender};
use crate::log::target;

/// The most batches on their way to one node at once: with one, every
/// message that comes while a batch is on its way waits for its whole
/// exchange; with more, a node that sends to a dozen others takes more of
/// their places for validators' requests than it needs.
pub const MAX_IN_FLIGHT: usize = 2;

/// The most messages of one hub waiting for one node: past it, the newest
/// is dropped, as a network drops one, and consensus sends again what
/// matters.
pub const MAX_QUEUED_PER_HUB: usize = 64;

/// A message waiting for its batch: its hub, and where answers to it go.
#[derive(Debug)]
pub struct Queued {
    /// The hub it is of.
    pub hub: HubId,
    /// The message.
    pub message: Vec<u8>,
    /// Where what the node sends back goes: its hub's validator.
    pub loopback: mpsc::Sender<Inbound>,
}

/// What waits for one node, and how many batches are on their way to it,
/// without IO.
#[derive(Debug, Default)]
pub struct PeerQueue {
    in_flight: usize,
    /// What waits, by hub, each hub's in the order it came.
    waiting: BTreeMap<HubId, VecDeque<Queued>>,
    /// The hub the last batch began with: the next begins after it, so no
    /// hub's messages always go last.
    last_first: Option<HubId>,
}

impl PeerQueue {
    /// Queues `queued`, unless its hub has [`MAX_QUEUED_PER_HUB`] waiting
    /// already: whether it is queued.
    pub fn push(&mut self, queued: Queued) -> bool {
        let waiting = self.waiting.entry(queued.hub).or_default();
        if waiting.len() >= MAX_QUEUED_PER_HUB {
            return false;
        }
        waiting.push_back(queued);
        true
    }

    /// The next batch, if fewer than [`MAX_IN_FLIGHT`] are on their way and
    /// anything waits, counted on its way: one message of each hub in
    /// turn, each hub's in order, up to [`MAX_BATCH_FRAMES`] and
    /// [`MAX_BATCH_BYTES`]. The first message always fits.
    pub fn next_batch(&mut self) -> Option<Vec<Queued>> {
        if self.in_flight >= MAX_IN_FLIGHT || self.waiting.is_empty() {
            return None;
        }
        let mut hubs: Vec<HubId> = self.waiting.keys().copied().collect();
        if let Some(last) = self.last_first {
            let after = hubs.partition_point(|hub| *hub <= last);
            let count = hubs.len();
            hubs.rotate_left(after % count);
        }
        self.last_first = hubs.first().copied();
        let mut batch: Vec<Queued> = Vec::new();
        let mut bytes = 1_usize;
        'fill: loop {
            let mut took = false;
            for hub in &hubs {
                if batch.len() >= MAX_BATCH_FRAMES {
                    break 'fill;
                }
                let Some(waiting) = self.waiting.get_mut(hub) else {
                    continue;
                };
                let Some(front) = waiting.front() else {
                    continue;
                };
                let size = HUB_ID_BYTES + FRAME_LENGTH_BYTES + front.message.len();
                if !batch.is_empty() && bytes.saturating_add(size) > MAX_BATCH_BYTES {
                    break 'fill;
                }
                bytes = bytes.saturating_add(size);
                batch.extend(waiting.pop_front());
                took = true;
            }
            if !took {
                break;
            }
        }
        self.waiting.retain(|_, waiting| !waiting.is_empty());
        assert!(
            (1..=MAX_BATCH_FRAMES).contains(&batch.len()),
            "a batch has one to {MAX_BATCH_FRAMES} messages"
        );
        self.in_flight += 1;
        Some(batch)
    }

    /// A batch on its way is done.
    pub fn finished(&mut self) {
        assert!(self.in_flight > 0, "only a batch on its way finishes");
        self.in_flight -= 1;
    }

    /// Whether nothing waits and nothing is on its way.
    #[must_use]
    pub fn idle(&self) -> bool {
        self.in_flight == 0 && self.waiting.is_empty()
    }
}

/// How many batches the node's validators sent, and how many messages
/// they held, since the node started.
#[derive(Debug, Default)]
pub struct BatchCounters {
    batches: AtomicU64,
    messages: AtomicU64,
}

impl BatchCounters {
    /// The batches and the messages they held.
    #[must_use]
    pub fn counts(&self) -> (u64, u64) {
        (
            self.batches.load(Ordering::Relaxed),
            self.messages.load(Ordering::Relaxed),
        )
    }

    fn sent(&self, messages: usize) {
        self.batches.fetch_add(1, Ordering::Relaxed);
        let messages = u64::try_from(messages).unwrap_or(u64::MAX);
        self.messages.fetch_add(messages, Ordering::Relaxed);
    }
}

/// The node a queue is for: its ID and where it is reached.
type Destination = (NodeId, String);

struct Inner {
    client: HubClient,
    backoffs: Backoffs,
    counters: Arc<BatchCounters>,
    queues: Mutex<BTreeMap<Destination, PeerQueue>>,
    stop: CancellationToken,
}

/// Where the node's validators put what they send other validators in
/// batches. Batches stop going once it is dropped, or `stop` is.
#[derive(Clone)]
pub struct Outbox {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Outbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outbox").finish_non_exhaustive()
    }
}

impl Outbox {
    /// An outbox that sends with `client`, leaves nodes alone by
    /// `backoffs`, and counts into `counters`, until `stop`.
    #[must_use]
    pub fn new(
        client: HubClient,
        backoffs: Backoffs,
        counters: Arc<BatchCounters>,
        stop: CancellationToken,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                client,
                backoffs,
                counters,
                queues: Mutex::default(),
                stop,
            }),
        }
    }

    /// Sends `message` of `hub` to `peer` in its next batch, and what
    /// `peer` sends back to `loopback`; nothing while `peer` is left alone.
    pub fn send(&self, peer: &Peer, hub: HubId, message: Vec<u8>, loopback: mpsc::Sender<Inbound>) {
        if self.inner.backoffs.backing_off(&peer.node) || self.inner.stop.is_cancelled() {
            return;
        }
        let destination = (peer.node, peer.endpoint.clone());
        let batch = {
            let mut queues = self.queues();
            let queue = queues.entry(destination).or_default();
            if !queue.push(Queued {
                hub,
                message,
                loopback,
            }) {
                tracing::debug!(target: target::HUB, node = %peer.node, "too many messages wait for a validator: one was dropped");
            }
            queue.next_batch()
        };
        if let Some(batch) = batch {
            let outbox = self.clone();
            let peer = peer.clone();
            tokio::spawn(async move { outbox.carry(peer, batch).await });
        }
    }

    /// Sends `batch` to `peer`, then each that waits for it while fewer are
    /// on their way, until none waits.
    async fn carry(self, peer: Peer, mut batch: Vec<Queued>) {
        loop {
            tokio::select! {
                () = self.inner.stop.cancelled() => {}
                () = self.exchange(&peer, &batch) => {}
            }
            let mut queues = self.queues();
            let destination = (peer.node, peer.endpoint.clone());
            let Some(queue) = queues.get_mut(&destination) else {
                return;
            };
            queue.finished();
            let next = if self.inner.stop.is_cancelled() {
                None
            } else {
                queue.next_batch()
            };
            match next {
                Some(next) => batch = next,
                None => {
                    if queue.idle() {
                        queues.remove(&destination);
                    }
                    return;
                }
            }
        }
    }

    /// One batch's exchange with `peer`: each answer goes to its hub's
    /// validator as it comes, and how it went to the node's backoff.
    async fn exchange(&self, peer: &Peer, batch: &[Queued]) {
        let frames: Vec<(HubId, &[u8])> = batch
            .iter()
            .map(|queued| (queued.hub, queued.message.as_slice()))
            .collect();
        let request = encode_batch(&frames);
        self.inner.counters.sent(batch.len());
        let (records, mut answers) = mpsc::channel(MAX_BATCH_FRAMES);
        let client = &self.inner.client;
        let sent = async move {
            let sent = client
                .exchange_batch(
                    peer.node,
                    &peer.endpoint,
                    &request,
                    frames.len(),
                    &records,
                    SEND_TIMEOUT,
                )
                .await;
            drop(records);
            sent
        };
        let from = Sender(peer.key).peer();
        let delivered = async {
            let (mut reached, mut busy) = (false, false);
            while let Some((index, answer)) = answers.recv().await {
                let Some(queued) = batch.get(usize::from(index)) else {
                    continue;
                };
                match answer {
                    HubAnswer::Taken(messages) => {
                        reached = true;
                        for message in messages {
                            let inbound = Inbound {
                                from,
                                node: Some(peer.key),
                                message,
                                reply: None,
                            };
                            let _ = queued.loopback.try_send(inbound);
                        }
                    }
                    HubAnswer::Busy => busy = true,
                    HubAnswer::UnknownHub | HubAnswer::Malformed => reached = true,
                }
            }
            (reached, busy)
        };
        let (sent, (reached, busy)) = tokio::join!(sent, delivered);
        match waited(&sent, reached, busy) {
            None => self.inner.backoffs.reached(&peer.node),
            Some(wait) => {
                self.inner.backoffs.failed(peer.node, &wait);
                if let Err(error) = sent {
                    tracing::debug!(target: target::HUB, node = %peer.node, %error, "a batch to a validator went unanswered");
                }
            }
        }
    }

    /// Ends the wait before sending a batch again to the node that sent
    /// `inbound`, in any hub, as [`Backoffs::heard_from`] says.
    pub fn heard_from(&self, inbound: &Inbound) {
        self.inner.backoffs.heard_from(inbound);
    }

    fn queues(&self) -> std::sync::MutexGuard<'_, BTreeMap<Destination, PeerQueue>> {
        self.inner
            .queues
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// What a batch's outcome says of its node, for the wait before the next
/// ([`Backoffs::failed`]): `None` if it reached the node, otherwise the
/// failure to wait after. Only what says the node itself is out of reach
/// grows the wait, no connection or a reply that is none; a batch that
/// brought only busy answers, or none because its stream broke off or ran
/// out of time, found the node up, and goes again shortly, as busy: what
/// failed may be one hub's at the node, and the wait is every hub's.
fn waited(
    sent: &Result<(), HubExchangeError>,
    reached: bool,
    busy: bool,
) -> Option<HubExchangeError> {
    let busy_now = HubExchangeError::Refused(HubAnswer::Busy);
    match sent {
        _ if reached => None,
        Ok(()) if busy => Some(busy_now),
        Ok(()) => None,
        Err(HubExchangeError::Unreachable) => Some(HubExchangeError::Unreachable),
        Err(HubExchangeError::BadReply) => Some(HubExchangeError::BadReply),
        Err(_) => Some(busy_now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dassie_hub::digest::Digest;

    fn hub(n: u8) -> HubId {
        HubId(Digest([n; 32]))
    }

    fn queued(hub_: u8, n: u8) -> Queued {
        let (loopback, _) = mpsc::channel(1);
        Queued {
            hub: hub(hub_),
            message: vec![n],
            loopback,
        }
    }

    fn messages(batch: &[Queued]) -> Vec<(u8, u8)> {
        batch
            .iter()
            .map(|q| (q.hub.0.as_bytes()[0], q.message[0]))
            .collect()
    }

    // A message that finds fewer than the most batches on their way goes at
    // once, alone; those that come while they are go together in the next.
    #[test]
    fn what_comes_while_batches_are_on_their_way_goes_together() {
        let mut queue = PeerQueue::default();
        assert!(queue.push(queued(1, 1)));
        assert_eq!(queue.next_batch().map(|b| messages(&b)), Some(vec![(1, 1)]));
        assert!(queue.push(queued(1, 2)));
        assert_eq!(queue.next_batch().map(|b| messages(&b)), Some(vec![(1, 2)]));
        for n in 3..6 {
            assert!(queue.push(queued(1, n)));
            assert!(queue.next_batch().is_none(), "two are on their way");
        }
        queue.finished();
        assert_eq!(
            queue.next_batch().map(|b| messages(&b)),
            Some(vec![(1, 3), (1, 4), (1, 5)])
        );
        queue.finished();
        queue.finished();
        assert!(queue.idle());
    }

    // Each hub's messages keep their order; hubs take turns, and the next
    // batch begins after the hub the last one began with.
    #[test]
    fn hubs_take_turns_and_keep_their_order() {
        let mut queue = PeerQueue {
            in_flight: MAX_IN_FLIGHT,
            ..PeerQueue::default()
        };
        for n in 1..=3 {
            assert!(queue.push(queued(1, n)));
        }
        assert!(queue.push(queued(2, 9)));
        queue.finished();
        assert_eq!(
            queue.next_batch().map(|b| messages(&b)),
            Some(vec![(1, 1), (2, 9), (1, 2), (1, 3)])
        );
        assert!(queue.push(queued(1, 4)));
        assert!(queue.push(queued(2, 8)));
        queue.finished();
        assert_eq!(
            queue.next_batch().map(|b| messages(&b)),
            Some(vec![(2, 8), (1, 4)]),
            "the hub after the last first goes first"
        );
    }

    // A batch holds at most its frames, and past its bytes the rest wait;
    // a hub has at most its share waiting.
    #[test]
    fn a_batch_and_a_queue_keep_their_bounds() {
        let mut queue = PeerQueue {
            in_flight: MAX_IN_FLIGHT,
            ..PeerQueue::default()
        };
        for n in 0..u8::try_from(MAX_QUEUED_PER_HUB).unwrap() {
            assert!(queue.push(queued(1, n)));
        }
        assert!(
            !queue.push(queued(1, 200)),
            "the newest past the share is dropped"
        );
        assert!(queue.push(queued(2, 0)), "another hub has its own");
        queue.finished();
        let batch = queue.next_batch().unwrap();
        assert_eq!(batch.len(), MAX_BATCH_FRAMES);

        let mut queue = PeerQueue::default();
        let (loopback, _) = mpsc::channel(1);
        let large = |hub_| Queued {
            hub: hub(hub_),
            message: vec![0; crate::hub::transport::MAX_MESSAGE_BYTES],
            loopback: loopback.clone(),
        };
        queue.in_flight = MAX_IN_FLIGHT;
        assert!(queue.push(large(1)));
        assert!(queue.push(large(2)));
        queue.finished();
        assert_eq!(queue.next_batch().map(|b| b.len()), Some(1), "one fits");
        queue.finished();
        assert_eq!(queue.next_batch().map(|b| b.len()), Some(1));
    }

    // Only the node's own failures grow the wait every hub shares: a batch
    // that went unanswered, as when its only hub is slow at the node or
    // was left out there, goes again shortly; any answer but busy ends it.
    #[test]
    fn only_a_node_out_of_reach_grows_the_wait() {
        let busy = Some(HubExchangeError::Refused(HubAnswer::Busy));
        let timed_out = Err(HubExchangeError::Failed("timed out".into()));
        assert_eq!(waited(&timed_out, false, false), busy);
        let reset = Err(HubExchangeError::Failed("the reply broke off".into()));
        assert_eq!(waited(&reset, false, false), busy);
        let full = Err(HubExchangeError::Refused(HubAnswer::Busy));
        assert_eq!(waited(&full, false, false), busy);
        assert_eq!(waited(&Ok(()), false, true), busy);
        assert_eq!(
            waited(&Err(HubExchangeError::Unreachable), false, false),
            Some(HubExchangeError::Unreachable)
        );
        assert_eq!(
            waited(&Err(HubExchangeError::BadReply), false, false),
            Some(HubExchangeError::BadReply)
        );
        assert_eq!(waited(&timed_out, true, true), None, "an answer came");
        assert_eq!(waited(&Ok(()), false, false), None);
    }
}
