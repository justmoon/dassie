//! The peering lifecycle (D12, spec/areas/network.md#lifecycle): the
//! advertise flag and its damping, termination, bans, peers that went
//! silent, and peerings only one side still holds. The original had none
//! of it: a peering lasted as long as the process (NET-13).

use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::{PeerNode, Problem, Purpose};
use crate::auth::Accepted;
use crate::hooks::PeeringHooks;
use crate::io::{Event, Reply};
use crate::table::{Advertise, Depeering, Entry, PeerState, Sources, Termination, backoff};
use crate::wire::{Content, SchemeId, TerminationReason};

/// Why a node could not be banned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanError {
    /// It is us.
    Ourselves,
    /// The list is full.
    Full,
}

impl PeerNode {
    /// Ends the peering with `peer` (spec/areas/network.md#termination):
    /// first privately, collecting what it owes, then by telling it; once
    /// the packets in flight are done, the peering is over (D28).
    /// Returns whether a peering was there to end.
    pub fn terminate(&mut self, now_ms: u64, peer: &NodeId, reason: TerminationReason) -> bool {
        self.advance(now_ms);
        self.begin_depeering(*peer, reason)
    }

    /// Decides to end an active peering, telling no one yet (D28): it
    /// takes nothing new from the peer, which still sees it as before.
    /// Returns whether a peering was there to end.
    pub(crate) fn begin_depeering(&mut self, peer: NodeId, reason: TerminationReason) -> bool {
        let now = self.now_ms;
        let Some(entry) = self.table.get_mut(&peer) else {
            return false;
        };
        let PeerState::Peered(peering) = &mut entry.peer else {
            return false;
        };
        match &mut peering.depeering {
            None => {
                peering.depeering = Some(Depeering::new(now, reason));
                self.dirty.insert(peer);
                // Remembered past this peering: a later one with it as our
                // senior raises no credit for it (spec/areas/settlement.md#credit).
                if reason == TerminationReason::ShortOfRoom
                    && let Some(channel) = self.channels.get_mut(&peer)
                    && !channel.ended_for_room
                {
                    channel.ended_for_room = true;
                    self.channels_dirty.insert(peer);
                }
                self.event(Event::Depeering { peer, reason });
            }
            // Its owner's decision outranks one the node took for silence
            // or room: it brings the owner's cooldown.
            Some(depeering) if reason == TerminationReason::Owner => {
                if depeering.reason != reason {
                    depeering.reason = reason;
                    self.dirty.insert(peer);
                }
            }
            Some(_) => {}
        }
        true
    }

    /// Takes the next step of each peering we are ending privately (D28):
    /// once our own settlements to the peer are done, or we waited long
    /// enough, ask it to settle once; once what it said is on its way is
    /// booked, or we waited long enough, tell it.
    fn advance_depeerings(&mut self, hooks: &dyn PeeringHooks) {
        let (now, wait) = (self.now_ms, self.config.depeer_wait_ms);
        let depeering: Vec<(NodeId, SchemeId, Depeering)> = self
            .table
            .iter()
            .filter_map(|(id, entry)| {
                let peering = entry.active_peering()?;
                Some((*id, peering.scheme, peering.depeering?))
            })
            .collect();
        for (peer, scheme, depeering) in depeering {
            match depeering.asked_ms {
                None => {
                    let ours = hooks.settling_to(peer) || self.settling(&peer);
                    if !ours || depeering.since_ms.saturating_add(wait) <= now {
                        self.ask_final(peer, scheme);
                    }
                }
                Some(asked) => {
                    if depeering.collected() || asked.saturating_add(wait) <= now {
                        self.begin_termination(peer, depeering.reason, true);
                    }
                }
            }
        }
    }

    /// Asks `peer` to settle a last time. A request that cannot be sent
    /// counts as asked, and is waited out.
    fn ask_final(&mut self, peer: NodeId, scheme: SchemeId) {
        let now = self.now_ms;
        if let Some(depeering) = self.depeering_mut(&peer) {
            depeering.asked_ms = Some(now);
        }
        let content = Content::SettlementRequest { scheme };
        let timeout = self.config.request_timeout_ms;
        if self
            .send(peer, &content, Purpose::FinalRequest, timeout)
            .is_some()
        {
            self.stats.settlements_requested += 1;
        }
    }

    /// The answer to our last request: what the peer has on its way. A
    /// 409, from a peer that holds no peering, means nothing is; an answer
    /// that does not say, or none, is waited out.
    pub(super) fn on_final_request(&mut self, peer: NodeId, result: &Result<Vec<u8>, Problem>) {
        let promised = match result {
            Ok(body) => crate::wire::decode_settling(body).ok().flatten(),
            Err(Problem::Status(409)) => Some(0),
            Err(_) => None,
        };
        if result.is_ok() {
            self.event(Event::SettlementRequestAnswered {
                peer,
                settling: promised,
            });
        }
        if let Some(depeering) = self.depeering_mut(&peer) {
            depeering.promised = promised;
        }
    }

    /// Our books took `amount` from `peer` in a settlement: what it said
    /// was on its way, if we are ending the peering, comes closer (D28).
    pub fn settlement_booked(&mut self, now_ms: u64, peer: &NodeId, amount: u64) {
        self.advance(now_ms);
        if let Some(depeering) = self.depeering_mut(peer)
            && depeering.asked_ms.is_some()
        {
            depeering.booked = depeering.booked.saturating_add(amount);
        }
    }

    /// A peering we are ending privately, to change its steps.
    fn depeering_mut(&mut self, peer: &NodeId) -> Option<&mut Depeering> {
        match &mut self.table.get_mut(peer)?.peer {
            PeerState::Peered(peering) => peering.depeering.as_mut(),
            _ => None,
        }
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
        // Told, or telling: whatever we were collecting is over.
        let mut peering = peering.clone();
        peering.depeering = None;
        entry.peer = PeerState::Terminating(peering, termination);
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
            Some(PeerState::Peered(peering)) => {
                // Told while we collect for our owner, or for room, the end
                // is still ours: its cooldown holds (D28, D33).
                let reason = match peering.depeering {
                    Some(depeering) if keeps_away(depeering.reason) => depeering.reason,
                    _ => reason,
                };
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
    /// ended at once. A ban is the owner's emphatic word, often about
    /// misbehaviour, so the node collects nothing first (D28): the peer is
    /// withdrawn from our link state and told now, and what it owes stays
    /// on our books.
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
        self.table.get(peer).is_some_and(|entry| {
            !entry.is_downlink() && entry.active_peering().is_some() && entry.advertise.on
        })
    }

    /// The peers listed as our neighbors, in order. Downlinks never are:
    /// their child addresses lie beneath ours, so the rest of the network
    /// needs no route to them (D26).
    pub(crate) fn advertised(&self) -> Vec<NodeId> {
        self.table
            .iter()
            .filter(|(_, entry)| {
                !entry.is_downlink() && entry.active_peering().is_some() && entry.advertise.on
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Looks after the peerings: ends silent ones, sets the advertise
    /// flags, tells terminations, and ends those that are done.
    pub(super) fn review_peers(&mut self, hooks: &dyn PeeringHooks) {
        self.end_downlinks();
        self.terminate_silent();
        self.terminate_short();
        self.advance_depeerings(hooks);
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
                    // Silent or short for good, it is ended: not again
                    // once we decided to (D28).
                    let ending = peering.depeering.is_some();
                    let silent = if ending {
                        u64::MAX
                    } else {
                        peering
                            .last_seen_ms
                            .max(start)
                            .saturating_add(config.peer_dead_ms)
                    };
                    let advertise = &entry.advertise;
                    // Short of room: withdrawn once the grace is over, and
                    // ended once it stays short, while it could be reached,
                    // as long as a silent peer. The time short grows only
                    // at checks, which come with an input: due at once when
                    // one passed a limit.
                    let stuck = peering.short_for_ms >= config.advertise_short_grace_ms;
                    let ended = if !ending && peering.short_reached_ms >= config.peer_dead_ms {
                        now
                    } else {
                        u64::MAX
                    };
                    let flag = if advertise.on {
                        // `heard_within` holds up to and including this.
                        let confirmed = peering
                            .last_seen_ms
                            .saturating_add(config.peer_confirm_ms)
                            .saturating_add(1);
                        if stuck { now } else { confirmed }
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
                    // A peering we are ending privately takes its next step
                    // when its wait ends, or when it was collected; whether
                    // our own settlements are done, only the hooks tell, so
                    // that is looked at with each peering check.
                    let depeer = peering.depeering.map_or(u64::MAX, |depeering| {
                        let wait = config.depeer_wait_ms;
                        match depeering.asked_ms {
                            None => depeering
                                .since_ms
                                .saturating_add(wait)
                                .min(self.peering.check_at_ms()),
                            Some(_) if depeering.collected() => now,
                            Some(asked) => asked.saturating_add(wait),
                        }
                    });
                    Some(silent.min(flag).min(ended).min(depeer))
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
    /// On a periphery node, ends the peerings with its downlinks, from
    /// when it was a core node: a periphery node takes none, and a node
    /// loses its URL other ways than its owner's choice
    /// (spec/areas/network.md#downlinks). Those not peered leave at once.
    pub(crate) fn end_downlinks(&mut self) {
        if !self.is_periphery() {
            return;
        }
        let downlinks: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| entry.is_downlink())
            .map(|(id, _)| *id)
            .collect();
        for node in downlinks {
            let peered = self.table.get(&node).and_then(Entry::peering).is_some();
            if peered {
                self.begin_depeering(node, TerminationReason::Unspecified);
            } else {
                self.drop_source(&node, Sources::DOWNLINK);
            }
        }
    }

    fn terminate_silent(&mut self) {
        let (now, dead, start) = (self.now_ms, self.config.peer_dead_ms, self.started_ms);
        let silent: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry.active_peering().is_some_and(|p| {
                    p.depeering.is_none() && p.last_seen_ms.max(start).saturating_add(dead) <= now
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for peer in silent {
            self.begin_depeering(peer, TerminationReason::Unresponsive);
        }
    }

    /// Terminates peers whose room in our books has been short of a packet,
    /// without a settlement, for as long as a silent peer is kept: a link
    /// that stuck, or drifted past what it survives, is not saved (D12).
    /// Only the time it could be reached counts: a peer our requests to
    /// settle surely never reached had no chance to settle, and is ended
    /// for its silence if it is also silent, never for room.
    fn terminate_short(&mut self) {
        let dead = self.config.peer_dead_ms;
        let short: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry.active_peering().is_some_and(|peering| {
                    peering.depeering.is_none() && peering.short_reached_ms >= dead
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for peer in short {
            self.begin_depeering(peer, TerminationReason::ShortOfRoom);
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
            let short = peering.short_for_ms >= config.advertise_short_grace_ms;
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
    /// the node again waits a while, a long while if an owner ended it or
    /// it was for room.
    fn end_peering(&mut self, peer: NodeId) {
        let now = self.now_ms;
        let (min, max) = (self.config.min_backoff_ms, self.config.max_backoff_ms);
        let cooldown = self.config.owner_termination_cooldown_ms;
        let Some(entry) = self.table.get_mut(&peer) else {
            return;
        };
        let PeerState::Terminating(_, termination) =
            core::mem::replace(&mut entry.peer, PeerState::None)
        else {
            return;
        };
        entry.advertise = Advertise::default();
        let wait = if keeps_away(termination.reason) {
            // Nor does it take the peer's requests: the owner meant it, and
            // a link that stayed short is not formed again at once (D33).
            entry.refuse_until_ms = now.saturating_add(cooldown);
            cooldown
        } else {
            backoff(min, max, entry.peering_failures.saturating_add(1))
        };
        entry.peering_at_ms = now.saturating_add(wait);
        self.dirty.insert(peer);
        self.telling.remove(&peer);
        self.drop_peer_source(&peer);
        self.gossip.routes_dirty = true;
        self.stats.terminations += 1;
        // Nothing is settled at the end: what the accounts show carries
        // into a later peering (D28).
        self.event(Event::PeeringEnded { peer });
    }

    /// An authenticated request from `sender` that presumes a peering,
    /// though we hold none with it and are not asking: its side is left
    /// over, as when our termination never reached it. It is told so, at
    /// most once per termination timeout, and its request still gets its
    /// refusal for not being peered (spec/areas/network.md#one-sided-peerings).
    /// A node that is asking holds back: the sender may have accepted our
    /// request, and only not told us yet.
    pub(crate) fn notice_one_sided(&mut self, sender: NodeId) {
        let now = self.now_ms;
        let period = self.config.termination_timeout_ms;
        let Some(entry) = self.table.get_mut(&sender) else {
            return;
        };
        if entry.peer != PeerState::None
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

/// Whether an end for `reason` keeps the two nodes apart for the owner's
/// cooldown, both ways: one the owner chose, and one for want of room,
/// which formed again at once would only stick again (D33).
const fn keeps_away(reason: TerminationReason) -> bool {
    matches!(
        reason,
        TerminationReason::Owner | TerminationReason::ShortOfRoom
    )
}
