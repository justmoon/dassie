//! Settlement messages over channels
//! (spec/areas/settlement.md#settlement-channels): ours, sent until
//! acknowledged; theirs, applied once each.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::{PeerNode, Problem, Purpose, SendError};
use crate::auth::Accepted;
use crate::channel::{Channel, Outgoing, Retry};
use crate::io::{Event, Inbound, Reply};
use crate::table::Entry;
use crate::table::backoff;
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
        self.channels.contains_key(node) || self.channels.len() < self.config.max_channels
    }

    /// Opens the channel with `node`, a new peer, or brings its contact up
    /// to date. A peering again settles normally: a final settlement still
    /// due is part of what it settles. Returns whether it exists.
    pub(crate) fn open_channel(&mut self, node: NodeId) -> bool {
        if let Some(channel) = self.channels.get_mut(&node)
            && channel.final_due.take().is_some()
        {
            self.channels_dirty.insert(node);
        }
        let Some((key, url)) = self.contact(&node) else {
            return self.channels.contains_key(&node);
        };
        if let Some(channel) = self.channels.get_mut(&node) {
            if channel.key != key || channel.url != url {
                channel.key = key;
                channel.url = url;
                self.channels_dirty.insert(node);
            }
            return true;
        }
        if self.channels.len() >= self.config.max_channels {
            return false;
        }
        self.channels.insert(node, Channel::new(key, url));
        self.channels_dirty.insert(node);
        self.auth.pin(node);
        true
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
            self.settlement_failed(node, false);
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
                // Refused for what it says (400) or for want of a channel
                // (409), it waits out its backoff: that does not change
                // because the node answers again. Refused before it was
                // read (401, 503), as by a node that just started, it was
                // not delivered.
                let answered = matches!(problem, Problem::Status(400 | 409));
                self.settlement_failed(node, answered);
            }
        }
    }

    /// `node` answered a request of ours, so what we send reaches it again:
    /// a settlement to it that could not be delivered goes now, rather than
    /// when its backoff ends, which may be up to an hour after a long
    /// outage. Only a request's answer counts, not a message from the node,
    /// which may have been sent before the outage. A settlement the node
    /// refused waits out its backoff, and a copy that arrives twice is
    /// acknowledged and not applied again, so going early costs at most one
    /// more request.
    pub(crate) fn settlement_reachable(&mut self, node: &NodeId) {
        let now = self.now_ms;
        if let Some(retry) = self.retries.get_mut(node)
            && retry.early
            && !retry.in_flight
            && retry.at_ms > now
        {
            retry.early = false;
            retry.at_ms = now;
        }
    }

    /// Waits before sending the channel's outbox again, longer each time.
    /// It may go early once its node answers a request of ours (see
    /// [`settlement_reachable`](Self::settlement_reachable)) only if this
    /// failure was one to deliver it, not an `answered` refusal.
    fn settlement_failed(&mut self, node: NodeId, answered: bool) {
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
        let early = !answered;
        self.retries.insert(
            node,
            Retry {
                at_ms: now.saturating_add(wait),
                in_flight: false,
                early,
            },
        );
    }

    /// A settlement message: from a node we hold a channel with, peered or
    /// not, since a final settlement may arrive after the peering ended.
    /// A copy already applied is acknowledged again, and nothing more.
    pub(super) fn settlement_request<'a>(
        &mut self,
        accepted: &Accepted,
        scheme: SchemeId,
        amount: u64,
        data: &'a [u8],
        sequence: u64,
    ) -> Inbound<'a> {
        let from = accepted.sender;
        let Some(channel) = self.channels.get(&from) else {
            return Inbound::Reply(Reply::refuse(409, "no accounts with the sender"));
        };
        if sequence <= channel.settled_in {
            self.stats.settlements_duplicate += 1;
            return Inbound::Reply(super::inbound::ok(accepted, &[]));
        }
        // The sender sends one at a time, in order: anything but the next
        // is a bug, or an attempt to push the sequence out of range.
        if Some(sequence) != channel.settled_in.checked_add(1) {
            return Inbound::Reply(Reply::refuse(400, "settlement out of order"));
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

    /// The final settlement with `node` is made, or nothing was owed.
    pub fn final_settled(&mut self, node: &NodeId) {
        if let Some(channel) = self.channels.get_mut(node)
            && channel.final_due.take().is_some()
        {
            self.channels_dirty.insert(*node);
        }
    }

    /// Records that the settlement `sequence` from `node` was booked: a
    /// copy of it is never applied again.
    pub fn settlement_applied(&mut self, now_ms: u64, node: &NodeId, sequence: u64) {
        self.advance(now_ms);
        if let Some(channel) = self.channels.get_mut(node)
            && sequence > channel.settled_in
        {
            channel.settled_in = sequence;
            self.channels_dirty.insert(*node);
            self.stats.settlements_applied += 1;
        }
    }

    /// Asks `peer` to settle on `scheme`, once: the node asks again later
    /// if it still has to (D23). The peer decides how much.
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
        if self
            .table
            .get(peer)
            .and_then(Entry::active_peering)
            .is_none()
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
