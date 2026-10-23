//! Settlement messages over channels
//! (spec/areas/settlement.md#settlement-channels): ours, sent until
//! acknowledged; theirs, applied once each. A channel left idle closes, and
//! what stays of it on disk opens it again
//! (spec/decisions/closing-channels.md).

use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::{PeerNode, Problem, Purpose, SendError};
use crate::auth::Accepted;
use crate::channel::{Channel, Closed, Outgoing, Retry};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Inbound, Reply};
use crate::table::backoff;
use crate::table::{Entry, PeerState};
use crate::wire::{Content, SchemeId};

impl PeerNode {
    /// The settlement channel with `node`, if any.
    #[must_use]
    pub fn channel(&self, node: &NodeId) -> Option<&Channel> {
        self.channels.get(node)
    }

    /// Every settlement channel.
    pub fn channels(&self) -> impl Iterator<Item = (&NodeId, &Channel)> {
        self.channels.iter()
    }

    /// Whether a channel with `node` exists or fits.
    pub(crate) fn channel_room(&self, node: &NodeId) -> bool {
        self.channel_room_for(node, self.is_downlink(node))
    }

    /// Whether a channel with `node` exists or fits, among the channels
    /// with downlinks if `downlink`, or among those with core nodes: each
    /// kind has its bound (spec/areas/network.md#downlinks).
    pub(crate) fn channel_room_for(&self, node: &NodeId, downlink: bool) -> bool {
        if self.channels.contains_key(node) {
            return true;
        }
        let (downlinks, core) = self.channel_counts();
        if downlink {
            downlinks < self.config.max_downlinks
        } else {
            core < self.config.max_channels
        }
    }

    /// The channels with downlinks, which have no URL, and those with core
    /// nodes.
    pub(crate) fn channel_counts(&self) -> (usize, usize) {
        let downlinks = self.downlink_channels;
        (downlinks, self.channels.len().saturating_sub(downlinks))
    }

    /// What stays of the closed channel with `node`, if it is closed: from
    /// memory while not written yet, otherwise from disk.
    pub(crate) fn closed_channel(&self, node: &NodeId, hooks: &dyn PeeringHooks) -> Option<Closed> {
        if self.channels.contains_key(node) {
            return None;
        }
        self.closed
            .get(node)
            .copied()
            .or_else(|| hooks.closed_channel(*node))
    }

    /// Whether we once ended a peering with `node` for want of room, as its
    /// channel says, open or closed (spec/decisions/ended-for-room.md).
    pub(crate) fn ended_for_room(&self, node: &NodeId, hooks: &dyn PeeringHooks) -> bool {
        self.channels.get(node).map_or_else(
            || {
                self.closed_channel(node, hooks)
                    .is_some_and(|closed| closed.ended_for_room)
            },
            |channel| channel.ended_for_room,
        )
    }

    /// Opens the channel with `node`, a new peer, or brings its contact up
    /// to date. A closed channel opens again where it was. Returns whether
    /// it exists.
    pub(crate) fn open_channel(&mut self, node: NodeId, hooks: &dyn PeeringHooks) -> bool {
        let closed = self.closed_channel(&node, hooks);
        self.open_channel_from(node, closed)
    }

    /// Opens the channel with `node` from `closed`, what stays of it if it
    /// was closed, or brings its contact up to date.
    pub(crate) fn open_channel_from(&mut self, node: NodeId, closed: Option<Closed>) -> bool {
        let Some((key, url)) = self.contact(&node) else {
            return self.channels.contains_key(&node);
        };
        if let Some(channel) = self.channels.get_mut(&node) {
            if channel.key != key || channel.url != url {
                // A node that went from core to periphery, or back, counts
                // with its new kind.
                match (channel.url.is_empty(), url.is_empty()) {
                    (false, true) => self.downlink_channels += 1,
                    (true, false) => self.downlink_channels -= 1,
                    _ => {}
                }
                channel.key = key;
                channel.url = url;
                self.channels_dirty.insert(node);
            }
            return true;
        }
        if !self.channel_room_for(&node, url.is_empty()) {
            return false;
        }
        if url.is_empty() {
            self.downlink_channels += 1;
        }
        // Written open, the channel replaces what stayed of it.
        self.closed.remove(&node);
        let channel = Channel::reopen(key, url, closed.unwrap_or(Closed::NEW));
        self.channels.insert(node, channel);
        self.channels_dirty.insert(node);
        self.channel_active.insert(node, self.now_ms);
        self.auth.pin(node);
        true
    }

    /// Closes the channels idle for as long as a silent peer is kept, with
    /// nodes we no longer peer with or ask to, and nothing on its way
    /// either way (spec/decisions/closing-channels.md). Their accounts stay
    /// with what they show, which a later peering settles.
    pub(super) fn close_idle_channels(&mut self, hooks: &dyn PeeringHooks) {
        let now = self.now_ms;
        if now < self.channel_check_at_ms {
            return;
        }
        self.channel_check_at_ms = now.saturating_add(self.config.channel_check_ms);
        let idle: Vec<NodeId> = self
            .channels
            .keys()
            .filter(|node| self.channel_closes_at(node).is_some_and(|at| at <= now))
            .filter(|node| !hooks.settling_to(**node) && hooks.closable(**node))
            .copied()
            .collect();
        for node in idle {
            self.close_channel(node);
        }
    }

    /// When a channel next closes, if one may. One idle long enough closes
    /// once the node's books let it, which only the hooks tell: it is
    /// looked at with each channel check, at an absolute time that inputs
    /// do not push back.
    pub(super) fn next_channel_close(&self) -> Option<u64> {
        self.channels
            .keys()
            .filter_map(|node| self.channel_closes_at(node))
            .min()
            .map(|at| at.max(self.channel_check_at_ms))
    }

    /// When the channel with `node` closes if it stays idle, if it may
    /// close at all: not while the node is peered or being asked, nor with
    /// a message on its way.
    fn channel_closes_at(&self, node: &NodeId) -> Option<u64> {
        let channel = self.channels.get(node)?;
        let idle = channel.outbox.is_none()
            && !self.retries.contains_key(node)
            && !self.peering.is_asking(node)
            && self
                .table
                .get(node)
                .is_none_or(|entry| entry.peer == PeerState::None);
        if !idle {
            return None;
        }
        let since = self
            .channel_active
            .get(node)
            .copied()
            .unwrap_or(self.started_ms)
            .max(self.started_ms);
        Some(since.saturating_add(self.config.peer_dead_ms))
    }

    /// Closes the channel with `node`: it leaves memory, and what stays of
    /// it is written with the next batch.
    fn close_channel(&mut self, node: NodeId) {
        let Some(channel) = self.channels.remove(&node) else {
            return;
        };
        if channel.url.is_empty() {
            self.downlink_channels -= 1;
        }
        self.closed.insert(node, channel.closed());
        self.channels_dirty.remove(&node);
        self.channel_active.remove(&node);
        // Its replay state goes once nothing else keeps it: a late
        // settlement is told from a copy by its sequence, which stays.
        if !self.keeps_replay_state(&node) {
            self.auth.unpin(self.now_ms, &node);
        }
        self.stats.channels_closed += 1;
        self.event(Event::ChannelClosed { peer: node });
    }

    /// Sends a settlement to `node`, which we hold a channel with. It goes
    /// again until acknowledged, across restarts; one at a time per
    /// channel (ACC-17). Returns its sequence.
    ///
    /// # Errors
    ///
    /// There is no channel, or a settlement is already on its way.
    pub fn send_settlement(
        &mut self,
        now_ms: u64,
        node: &NodeId,
        scheme: SchemeId,
        amount: u64,
        data: Vec<u8>,
    ) -> Result<u64, SendError> {
        self.advance(now_ms);
        let channel = self.channels.get_mut(node).ok_or(SendError::NotPeered)?;
        if channel.outbox.is_some() {
            return Err(SendError::Busy);
        }
        channel.settled_out = channel.settled_out.saturating_add(1);
        let sequence = channel.settled_out;
        self.channel_active.insert(*node, self.now_ms);
        channel.outbox = Some(Outgoing {
            sequence,
            scheme,
            amount,
            data,
            attempts: 0,
        });
        self.channels_dirty.insert(*node);
        self.retries.insert(
            *node,
            Retry {
                at_ms: self.now_ms,
                in_flight: false,
                early: true,
                went_early: false,
            },
        );
        self.deliver_settlement(*node);
        Ok(sequence)
    }

    /// Whether a settlement to `node` is on its way.
    #[must_use]
    pub fn settling(&self, node: &NodeId) -> bool {
        self.channels
            .get(node)
            .is_some_and(|channel| channel.outbox.is_some())
    }

    /// Sends the channel's outbox, if one is due and none is in flight.
    fn deliver_settlement(&mut self, node: NodeId) {
        let now = self.now_ms;
        let due = self
            .retries
            .get(&node)
            .is_some_and(|retry| !retry.in_flight && retry.at_ms <= now);
        let Some(channel) = self.channels.get(&node).filter(|_| due) else {
            return;
        };
        let Some(outbox) = channel.outbox.clone() else {
            self.retries.remove(&node);
            return;
        };
        let (key, url) = self
            .contact(&node)
            .unwrap_or_else(|| (channel.key, channel.url.clone()));
        let content = Content::Settlement {
            scheme: outbox.scheme,
            amount: outbox.amount,
            data: &outbox.data,
            sequence: outbox.sequence,
        };
        let purpose = Purpose::Settlement(outbox.sequence);
        let timeout = self.config.request_timeout_ms;
        if self
            .send_via(node, key, url, &content, purpose, timeout)
            .is_some()
        {
            if let Some(retry) = self.retries.get_mut(&node) {
                retry.in_flight = true;
            }
        } else {
            self.settlement_failed(node, None);
        }
    }

    /// Sends the settlements whose time has come again.
    pub(super) fn retry_settlements(&mut self) {
        let now = self.now_ms;
        let due: Vec<NodeId> = self
            .retries
            .iter()
            .filter(|(_, retry)| !retry.in_flight && retry.at_ms <= now)
            .map(|(node, _)| *node)
            .collect();
        for node in due {
            self.deliver_settlement(node);
        }
    }

    /// When a settlement is next due again.
    pub(super) fn next_settlement_retry(&self) -> Option<u64> {
        self.retries
            .values()
            .filter(|retry| !retry.in_flight)
            .map(|retry| retry.at_ms)
            .min()
    }

    /// The answer to a settlement message of ours.
    pub(super) fn on_settlement(
        &mut self,
        node: NodeId,
        sequence: u64,
        result: &Result<Vec<u8>, Problem>,
    ) {
        if let Some(retry) = self.retries.get_mut(&node) {
            retry.in_flight = false;
        }
        let current = self
            .channels
            .get(&node)
            .and_then(|channel| channel.outbox.as_ref())
            .is_some_and(|outbox| outbox.sequence == sequence);
        if !current {
            return;
        }
        match result {
            Ok(_) => {
                if let Some(channel) = self.channels.get_mut(&node) {
                    channel.outbox = None;
                }
                self.channels_dirty.insert(node);
                self.channel_active.insert(node, self.now_ms);
                self.retries.remove(&node);
                self.stats.settlements_delivered += 1;
                self.event(Event::SettlementDelivered {
                    peer: node,
                    sequence,
                });
            }
            Err(problem) => {
                if let Problem::Status(status @ 400) = problem {
                    self.event(Event::SettlementRefused {
                        peer: node,
                        sequence,
                        status: *status,
                    });
                }
                self.settlement_failed(node, Some(*problem));
            }
        }
    }

    /// `node` answered a request of ours, so what we send reaches it again:
    /// a settlement to it that could not be delivered goes now, rather than
    /// when its backoff ends, which may be up to an hour after a long
    /// outage. Only a request's answer counts, not a message from the node,
    /// which may have been sent before the outage. A settlement the node
    /// answered waits out its backoff (see
    /// [`settlement_failed`](Self::settlement_failed)), and a copy that
    /// arrives twice is acknowledged and not applied again, so going early
    /// costs at most one more request per try in its backoff that got no
    /// answer.
    pub(crate) fn settlement_reachable(&mut self, node: &NodeId) {
        let now = self.now_ms;
        if let Some(retry) = self.retries.get_mut(node)
            && retry.early
            && !retry.in_flight
            && retry.at_ms > now
        {
            retry.early = false;
            retry.went_early = true;
            retry.at_ms = now;
        }
    }

    /// Waits before sending the channel's outbox again, longer each time,
    /// after a try that ended in `problem`, or was never sent (`None`). It
    /// may go early once its node answers a request of ours (see
    /// [`settlement_reachable`](Self::settlement_reachable)) only after a
    /// try in its backoff that got no answer, and not after one that went
    /// early itself, so that a node that never answers it gets it at most
    /// twice as often as the backoff alone would send it. A 401 leaves that
    /// as it was: a node that just started refuses messages dated before
    /// its start, and the copy that goes early is dated anew. Any other
    /// answer refuses it for good reason (it is malformed, has no channel,
    /// or the node is busy), and the node answering something else changes
    /// none of that.
    fn settlement_failed(&mut self, node: NodeId, problem: Option<Problem>) {
        let (min, max) = (
            self.config.settlement_retry_min_ms,
            self.config.settlement_retry_max_ms,
        );
        let now = self.now_ms;
        let Some(outbox) = self
            .channels
            .get_mut(&node)
            .and_then(|channel| channel.outbox.as_mut())
        else {
            return;
        };
        let wait = backoff(min, max, outbox.attempts);
        outbox.attempts = outbox.attempts.saturating_add(1);
        self.channels_dirty.insert(node);
        let last = self.retries.get(&node).copied().unwrap_or_default();
        let early = match problem {
            None | Some(Problem::Failed(_)) => !last.went_early,
            Some(Problem::Status(401)) => last.early,
            Some(Problem::Status(_) | Problem::Invalid) => false,
        };
        self.retries.insert(
            node,
            Retry {
                at_ms: now.saturating_add(wait),
                in_flight: false,
                early,
                went_early: false,
            },
        );
    }

    /// A settlement message: from a node we hold a channel with, peered or
    /// not, since a settlement may arrive after the peering ended, and open
    /// or closed, since one may arrive after the channel closed. A copy
    /// already applied is acknowledged again, and nothing more.
    pub(super) fn settlement_request<'a>(
        &mut self,
        hooks: &dyn PeeringHooks,
        accepted: &Accepted,
        scheme: SchemeId,
        amount: u64,
        data: &'a [u8],
        sequence: u64,
    ) -> Inbound<'a> {
        let from = accepted.sender;
        let (settled_in, closed) = if let Some(channel) = self.channels.get(&from) {
            (channel.settled_in, None)
        } else if let Some(closed) = self.closed_channel(&from, hooks) {
            (closed.settled_in, Some(closed))
        } else {
            return Inbound::Reply(Reply::refuse(409, "no accounts with the sender"));
        };
        if sequence <= settled_in {
            self.stats.settlements_duplicate += 1;
            return Inbound::Reply(super::inbound::ok(accepted, &[]));
        }
        // The sender sends one at a time, in order: anything but the next
        // is a bug, or an attempt to push the sequence out of range.
        if Some(sequence) != settled_in.checked_add(1) {
            return Inbound::Reply(Reply::refuse(400, "settlement out of order"));
        }
        if let Some(closed) = closed {
            // Kept in memory until the batch, so that applying it moves
            // the sequence written for the closed channel.
            self.closed.insert(from, closed);
        }
        Inbound::Settlement {
            from,
            scheme,
            amount,
            data,
            sequence,
            reply: super::inbound::ok(accepted, &[]),
        }
    }

    /// Records that the settlement `sequence` from `node` was booked: a
    /// copy of it is never applied again, over its channel open or closed.
    pub fn settlement_applied(&mut self, now_ms: u64, node: &NodeId, sequence: u64) {
        self.advance(now_ms);
        if let Some(channel) = self.channels.get_mut(node)
            && sequence > channel.settled_in
        {
            channel.settled_in = sequence;
            self.channels_dirty.insert(*node);
            self.channel_active.insert(*node, self.now_ms);
            self.stats.settlements_applied += 1;
        } else if let Some(closed) = self.closed.get_mut(node)
            && sequence > closed.settled_in
        {
            closed.settled_in = sequence;
            self.stats.settlements_applied += 1;
        }
    }

    /// Asks `peer` to settle on `scheme`, once: the node asks again later
    /// if it still has to (spec/decisions/own-fulfills.md). The peer decides
    /// how much.
    ///
    /// # Errors
    ///
    /// The node is not a peer, or too much is in flight.
    pub fn request_settlement(
        &mut self,
        now_ms: u64,
        peer: &NodeId,
        scheme: SchemeId,
    ) -> Result<(), SendError> {
        self.advance(now_ms);
        // A peering we are ending asks once more, by itself
        // (spec/decisions/ending-a-peering.md).
        if self
            .table
            .get(peer)
            .and_then(Entry::active_peering)
            .is_none_or(|peering| peering.depeering.is_some())
        {
            return Err(SendError::NotPeered);
        }
        let content = Content::SettlementRequest { scheme };
        let timeout = self.config.request_timeout_ms;
        self.send(*peer, &content, Purpose::SettlementRequest, timeout)
            .map(|_| ())
            .ok_or(SendError::Busy)?;
        self.stats.settlements_requested += 1;
        Ok(())
    }

    /// Sends a scheme's message to a peer, once: schemes do not depend on
    /// them arriving.
    ///
    /// # Errors
    ///
    /// The node is not a peer, or too much is in flight.
    pub fn send_scheme_message(
        &mut self,
        now_ms: u64,
        peer: &NodeId,
        scheme: SchemeId,
        message: &[u8],
    ) -> Result<(), SendError> {
        self.advance(now_ms);
        if self.peering_of(peer).is_none() {
            return Err(SendError::NotPeered);
        }
        let content = Content::SettlementMessage { scheme, message };
        let timeout = self.config.request_timeout_ms;
        self.send(*peer, &content, Purpose::Gossip, timeout)
            .map(|_| ())
            .ok_or(SendError::Busy)
    }
}
