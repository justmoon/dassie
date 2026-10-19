//! The peering lifecycle (D12, spec/areas/network.md#lifecycle): the
//! advertise flag and its damping, termination, bans, peers that went
//! silent, and peerings only one side still holds.

use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::{PeerNode, Problem, Purpose};
use crate::auth::Accepted;
use crate::hooks::PeeringHooks;
use crate::io::{Event, Reply};
use crate::table::{Advertise, PeerState, Sources, Termination, backoff};
use crate::wire::{Content, SignedNodeInfo, TerminationReason};

/// Why a node could not be banned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanError {
    /// It is us.
    Ourselves,
    /// The list is full.
    Full,
}

impl PeerNode {
    /// Ends the peering with `peer`: no new packets from now on, and once
    /// those in flight are done, the peering is over
    /// (spec/areas/network.md#termination).
    /// Returns whether a peering was there to end.
    pub fn terminate(&mut self, now_ms: u64, peer: &NodeId, reason: TerminationReason) -> bool {
        self.advance(now_ms);
        self.begin_termination(*peer, reason, true)
    }

    /// Moves an active peering to terminating. `by_us` says whether we
    /// began it; if not, the other side knows already.
    pub(crate) fn begin_termination(
        &mut self,
        peer: NodeId,
        reason: TerminationReason,
        by_us: bool,
    ) -> bool {
        let now = self.now_ms;
        let Some(entry) = self.table.get_mut(&peer) else {
            return false;
        };
        let PeerState::Peered(peering) = &entry.peer else {
            return false;
        };
        let termination = Termination {
            since_ms: now,
            reason,
            by_us,
            acknowledged: !by_us,
        };
        entry.peer = PeerState::Terminating(peering.clone(), termination);
        entry.advertise.on = false;
        self.dirty.insert(peer);
        self.gossip.own_changed = true;
        self.gossip.routes_dirty = true;
        self.event(Event::Terminating {
            peer,
            reason,
            by_us,
        });
        if by_us {
            self.telling.insert(peer, now);
            self.tell_termination(peer);
        }
        true
    }

    /// Tells `peer` we are ending the peering, if it does not know yet and
    /// we have not given up.
    fn tell_termination(&mut self, peer: NodeId) {
        let (now, timeout) = (self.now_ms, self.config.termination_timeout_ms);
        let Some(termination) = self.table.get(&peer).and_then(|e| e.termination().copied()) else {
            self.telling.remove(&peer);
            return;
        };
        if termination.told(now, timeout) {
            self.telling.remove(&peer);
            return;
        }
        if self.telling.get(&peer).is_none_or(|at| *at > now) {
            return;
        }
        // Not again until this one is answered.
        self.telling.insert(peer, u64::MAX);
        let content = Content::PeeringTermination(termination.reason);
        let wait = self.config.request_timeout_ms.min(timeout);
        if self
            .send(peer, &content, Purpose::Termination, wait)
            .is_none()
        {
            self.telling
                .insert(peer, now.saturating_add(self.config.min_backoff_ms));
        }
    }

    /// The answer to our termination message: a 200, or a 409 from a node
    /// that holds no peering, means it knows.
    pub(super) fn on_termination(&mut self, peer: NodeId, result: &Result<Vec<u8>, Problem>) {
        let known = matches!(result, Ok(_) | Err(Problem::Status(409)));
        if known {
            if let Some(entry) = self.table.get_mut(&peer)
                && let PeerState::Terminating(_, termination) = &mut entry.peer
            {
                termination.acknowledged = true;
                self.dirty.insert(peer);
            }
            self.telling.remove(&peer);
        } else if self.telling.contains_key(&peer) {
            let at = self.now_ms.saturating_add(self.config.min_backoff_ms);
            self.telling.insert(peer, at);
        }
    }

    /// A peer's termination message.
    pub(super) fn termination_request(
        &mut self,
        accepted: &Accepted,
        reason: TerminationReason,
    ) -> Reply {
        let sender = accepted.sender;
        let state = self.table.get(&sender).map(|entry| &entry.peer);
        match state {
            Some(PeerState::Peered(_)) => {
                self.begin_termination(sender, reason, false);
            }
            Some(PeerState::Terminating(..)) => {
                // Both ended it at once: each side's message tells the
                // other.
                if let Some(entry) = self.table.get_mut(&sender)
                    && let PeerState::Terminating(_, termination) = &mut entry.peer
                {
                    termination.acknowledged = true;
                    self.dirty.insert(sender);
                }
                self.telling.remove(&sender);
            }
            Some(PeerState::Requesting { .. }) => {
                self.abandon_attempt(sender);
                return Reply::refuse(409, "not peered");
            }
            Some(PeerState::None) | None => return Reply::refuse(409, "not peered"),
        }
        super::inbound::ok(accepted, &[])
    }

    /// Bans `node`: it is never asked, is refused, and a peering with it is
    /// ended.
    ///
    /// # Errors
    ///
    /// It is us, or the list is full.
    pub fn ban(&mut self, now_ms: u64, node: NodeId) -> Result<(), BanError> {
        self.advance(now_ms);
        if node == self.own {
            return Err(BanError::Ourselves);
        }
        if !self.bans.contains(&node) {
            if self.bans.len() >= self.config.max_bans {
                return Err(BanError::Full);
            }
            self.bans.insert(node);
            self.unbanned.retain(|id| *id != node);
            self.banned.push(node);
        }
        self.abandon_attempt(node);
        self.begin_termination(node, TerminationReason::Owner, true);
        Ok(())
    }

    /// Lifts a ban. Returns whether there was one.
    pub fn unban(&mut self, now_ms: u64, node: &NodeId) -> bool {
        self.advance(now_ms);
        if !self.bans.remove(node) {
            return false;
        }
        self.banned.retain(|id| id != node);
        self.unbanned.push(*node);
        true
    }

    /// Whether `node` is banned.
    #[must_use]
    pub fn is_banned(&self, node: &NodeId) -> bool {
        self.bans.contains(node)
    }

    /// The banned nodes.
    pub fn bans(&self) -> impl Iterator<Item = &NodeId> {
        self.bans.iter()
    }

    /// Whether `peer` is listed as our neighbor.
    #[must_use]
    pub fn is_advertised(&self, peer: &NodeId) -> bool {
        self.table
            .get(peer)
            .is_some_and(|entry| entry.active_peering().is_some() && entry.advertise.on)
    }

    /// The peers listed as our neighbors, in order.
    pub(crate) fn advertised(&self) -> Vec<NodeId> {
        self.table
            .iter()
            .filter(|(_, entry)| entry.active_peering().is_some() && entry.advertise.on)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Looks after the peerings: ends silent ones, sets the advertise
    /// flags, tells terminations, and ends those that are done.
    pub(super) fn review_peers(&mut self, hooks: &dyn PeeringHooks) {
        self.terminate_silent();
        self.terminate_short();
        self.update_advertised();
        let telling: Vec<NodeId> = self.telling.keys().copied().collect();
        for peer in telling {
            self.tell_termination(peer);
        }
        let (now, timeout) = (self.now_ms, self.config.termination_timeout_ms);
        let done: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(id, entry)| {
                entry
                    .termination()
                    .is_some_and(|t| t.told(now, timeout) && hooks.drained(**id))
                    && !self.answering(id)
            })
            .map(|(id, _)| *id)
            .collect();
        for peer in done {
            self.end_peering(peer);
        }
    }

    /// When the review of peerings next has something to do: an advertised
    /// peer goes unconfirmed, a peer goes silent for good, a hold-down ends,
    /// a withdrawn peer heard from again is listed, or a termination is to
    /// be told again, given up on, or ended once its packets drained.
    pub(super) fn next_review(&self) -> Option<u64> {
        let config = &self.config;
        let (now, start) = (self.now_ms, self.started_ms);
        let peerings = self
            .table
            .iter()
            .filter_map(|(id, entry)| match &entry.peer {
                PeerState::Peered(peering) => {
                    let silent = peering
                        .last_seen_ms
                        .max(start)
                        .saturating_add(config.peer_dead_ms);
                    let advertise = &entry.advertise;
                    // Short of room: withdrawn once the grace is over, and
                    // ended once it stays short as long as a silent peer.
                    let short = peering.short_since_ms.map(|since| {
                        (
                            since.saturating_add(config.advertise_short_grace_ms),
                            since.saturating_add(config.peer_dead_ms),
                        )
                    });
                    let stuck = short.is_some_and(|(withdrawn, _)| withdrawn <= now);
                    let ended = short.map_or(u64::MAX, |(_, ended)| ended);
                    let flag = if advertise.on {
                        // `heard_within` holds up to and including this.
                        let confirmed = peering
                            .last_seen_ms
                            .saturating_add(config.peer_confirm_ms)
                            .saturating_add(1);
                        short.map_or(confirmed, |(withdrawn, _)| confirmed.min(withdrawn))
                    } else if advertise.hold_until_ms > now {
                        advertise.hold_until_ms
                    } else if peering.heard_within(now, config.peer_confirm_ms)
                        && !self.bans.contains(id)
                        && !stuck
                    {
                        // Healthy again, as `update_advertised` judges it: it
                        // is listed at once, so this is never due twice.
                        now
                    } else {
                        u64::MAX
                    };
                    Some(silent.min(flag).min(ended))
                }
                PeerState::Terminating(_, termination) => {
                    Some(if termination.told(now, config.termination_timeout_ms) {
                        // Ends once its packets drained, which only the hooks
                        // tell: looked at with each peering check meanwhile, at
                        // an absolute time that inputs do not push back.
                        self.peering.check_at_ms()
                    } else {
                        termination
                            .since_ms
                            .saturating_add(config.termination_timeout_ms)
                    })
                }
                _ => None,
            });
        let telling = self.telling.values().copied().filter(|at| *at != u64::MAX);
        peerings.chain(telling).min()
    }

    /// Terminates peers not heard from for too long, counted from our
    /// start at the earliest, so a node that was down ends nothing for it.
    fn terminate_silent(&mut self) {
        let (now, dead, start) = (self.now_ms, self.config.peer_dead_ms, self.started_ms);
        let silent: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry
                    .active_peering()
                    .is_some_and(|p| p.last_seen_ms.max(start).saturating_add(dead) <= now)
            })
            .map(|(id, _)| *id)
            .collect();
        for peer in silent {
            self.begin_termination(peer, TerminationReason::Unresponsive, true);
        }
    }

    /// Terminates peers whose room in our books has been short of a packet,
    /// without a settlement, for as long as a silent peer is kept: a link
    /// that stuck, or drifted past what it survives, is not saved (D12).
    fn terminate_short(&mut self) {
        let (now, dead) = (self.now_ms, self.config.peer_dead_ms);
        let short: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry.active_peering().is_some_and(|peering| {
                    peering
                        .short_since_ms
                        .is_some_and(|since| since.saturating_add(dead) <= now)
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for peer in short {
            self.begin_termination(peer, TerminationReason::ShortOfRoom, true);
        }
    }

    /// A peer is advertised while it is confirmed, has had room for a
    /// packet within the grace, and is not held down; each withdrawal holds
    /// it down longer, until it has been stable a while (D12).
    fn update_advertised(&mut self) {
        let config = self.config;
        let now = self.now_ms;
        let mut changes = Vec::new();
        for (id, entry) in &mut self.table {
            let Some(peering) = entry.active_peering() else {
                continue;
            };
            let short = peering
                .short_since_ms
                .is_some_and(|since| since.saturating_add(config.advertise_short_grace_ms) <= now);
            let live = peering.heard_within(now, config.peer_confirm_ms) && !self.bans.contains(id);
            let healthy = live && !short;
            let advertise = &mut entry.advertise;
            if advertise.on
                && advertise
                    .on_since_ms
                    .saturating_add(config.advertise_stable_ms)
                    <= now
            {
                advertise.flaps = 0;
            }
            if advertise.on && !healthy {
                advertise.on = false;
                let hold = backoff(
                    config.advertise_hold_min_ms,
                    config.advertise_hold_max_ms,
                    advertise.flaps,
                );
                advertise.hold_until_ms = now.saturating_add(hold);
                advertise.flaps = advertise.flaps.saturating_add(1);
                // For room only if that is the one reason: a peer that
                // also fell silent is withdrawn for that.
                changes.push((*id, false, short && live));
            } else if !advertise.on && healthy && advertise.hold_until_ms <= now {
                advertise.on = true;
                advertise.on_since_ms = now;
                changes.push((*id, true, false));
            }
        }
        if !changes.is_empty() {
            self.gossip.own_changed = true;
        }
        for (peer, on, short) in changes {
            self.event(Event::Advertised { peer, on, short });
        }
    }

    /// Whether a response to `peer` is still being sent.
    fn answering(&self, peer: &NodeId) -> bool {
        self.answers.iter().any(|(_, answer)| answer.peer == *peer)
            || self.requests.values().any(|outstanding| {
                outstanding.to == *peer && matches!(outstanding.purpose, Purpose::Response(_))
            })
    }

    /// Ends a terminating peering. Its accounts and channel stay; asking
    /// the node again waits a while, a long while if an owner ended it.
    fn end_peering(&mut self, peer: NodeId) {
        let now = self.now_ms;
        let (min, max) = (self.config.min_backoff_ms, self.config.max_backoff_ms);
        let cooldown = self.config.owner_termination_cooldown_ms;
        let Some(entry) = self.table.get_mut(&peer) else {
            return;
        };
        let PeerState::Terminating(peering, termination) =
            core::mem::replace(&mut entry.peer, PeerState::None)
        else {
            return;
        };
        entry.advertise = Advertise::default();
        let wait = if termination.reason == TerminationReason::Owner {
            // Nor does it take the peer's requests: the owner meant it.
            entry.refuse_until_ms = now.saturating_add(cooldown);
            cooldown
        } else {
            backoff(min, max, entry.peering_failures.saturating_add(1))
        };
        entry.peering_at_ms = now.saturating_add(wait);
        self.dirty.insert(peer);
        self.telling.remove(&peer);
        self.drop_source(&peer, Sources::PEER);
        self.gossip.routes_dirty = true;
        self.stats.terminations += 1;
        if termination.acknowledged
            && let Some(channel) = self.channels.get_mut(&peer)
        {
            channel.final_due = Some(crate::channel::FinalDue {
                scheme: peering.scheme,
                state: peering.state,
            });
            self.channels_dirty.insert(peer);
        }
        self.event(Event::PeeringEnded {
            peer,
            settle: termination.acknowledged,
        });
    }

    /// An authenticated link state from its own subject that lists us as a
    /// neighbor, though we hold no peering with it and are not asking: its
    /// side is left over. It is told so, at most once per period.
    pub(crate) fn notice_one_sided(&mut self, sender: NodeId, info: &SignedNodeInfo<'_>) {
        let now = self.now_ms;
        let period = self.config.termination_timeout_ms;
        let Some(entry) = self.table.get_mut(&sender) else {
            return;
        };
        if entry.peer != PeerState::None
            || !info.info.neighbors().any(|neighbor| neighbor == self.own)
            || entry
                .one_sided_at_ms
                .is_some_and(|at| at.saturating_add(period) > now)
        {
            return;
        }
        entry.one_sided_at_ms = Some(now);
        let content = Content::PeeringTermination(TerminationReason::NotPeered);
        let timeout = self.config.request_timeout_ms;
        self.send(sender, &content, Purpose::Gossip, timeout);
    }
}
