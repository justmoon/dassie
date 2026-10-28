//! The peering lifecycle (spec/decisions/peering-lifecycle.md,
//! spec/areas/network.md#lifecycle): the advertise flag and its damping,
//! termination, bans, peers that went silent, and peerings only one side
//! still holds. The original had none of it: a peering lasted as long as the
//! process.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::inbound::Accepted;
use super::{PeerNode, Problem, Purpose};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Reply};
use crate::table::{
    Advertise, Depeering, Entry, PeerState, Peering, Sources, Termination, backoff,
};
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
    /// the packets in flight are done, the peering is over
    /// (spec/decisions/ending-a-peering.md). Returns whether a peering was
    /// there to end.
    pub fn terminate(&mut self, now_ms: u64, peer: &NodeId, reason: TerminationReason) -> bool {
        self.advance(now_ms);
        self.begin_depeering(*peer, reason)
    }

    /// Decides to end an active peering, telling no one yet
    /// (spec/decisions/ending-a-peering.md): it takes nothing new from the
    /// peer, which still sees it as before. Returns whether a peering was
    /// there to end.
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

    /// Takes the next step of each peering we are ending privately
    /// (spec/decisions/ending-a-peering.md): once our own settlements to the
    /// peer are done, or we waited long enough, ask it to settle once; once
    /// what it said is on its way is booked, or we waited long enough, tell
    /// it. The request names the peering's first method, whose units the
    /// answer is in: the node collects on that method alone, and what a
    /// further method's accounts show stays owed
    /// (spec/decisions/settlement-methods.md).
    fn advance_depeerings(&mut self, hooks: &dyn PeeringHooks) {
        let (now, wait) = (self.now_ms, self.config.depeer_wait_ms);
        let depeering: Vec<(NodeId, SchemeId, Depeering)> = self
            .table
            .iter()
            .filter_map(|(id, entry)| {
                let peering = entry.active_peering()?;
                Some((*id, peering.first().scheme, peering.depeering?))
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

    /// When the next step of ending `peering` privately is due: when its
    /// wait ends, or when it was collected; whether our own settlements are
    /// done, only the hooks tell, so that is looked at with each peering
    /// check.
    fn next_depeer(&self, peering: &Peering) -> u64 {
        let (now, wait) = (self.now_ms, self.config.depeer_wait_ms);
        peering
            .depeering
            .map_or(u64::MAX, |depeering| match depeering.asked_ms {
                None => depeering
                    .since_ms
                    .saturating_add(wait)
                    .min(self.peering.check_at_ms()),
                Some(_) if depeering.collected() => now,
                Some(asked) => asked.saturating_add(wait),
            })
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
    /// it cannot read, or none, is waited out.
    pub(super) fn on_final_request(&mut self, peer: NodeId, result: &Result<Vec<u8>, Problem>) {
        let answered = match result {
            Ok(body) => crate::wire::decode_settling(body).ok(),
            Err(_) => None,
        };
        if let Some(settling) = answered {
            self.event(Event::SettlementRequestAnswered { peer, settling });
        }
        let promised = match result {
            Err(Problem::Status(409)) => Some(0),
            _ => answered,
        };
        if let Some(depeering) = self.depeering_mut(&peer) {
            depeering.promised = promised;
        }
    }

    /// Our books took `amount` from `peer` in a settlement on its method on
    /// `scheme`: what it said was on its way, if we are ending the peering
    /// and collect on that method, or winding the method down, comes closer
    /// (spec/decisions/ending-a-peering.md,
    /// spec/decisions/settlement-methods.md). Each answer is in its own
    /// method's units, so a settlement on another method counts toward
    /// neither.
    pub fn settlement_booked(&mut self, now_ms: u64, peer: &NodeId, scheme: SchemeId, amount: u64) {
        self.advance(now_ms);
        let collecting = self
            .peering_of(peer)
            .is_some_and(|peering| peering.collecting_on(scheme));
        if collecting
            && let Some(depeering) = self.depeering_mut(peer)
            && depeering.asked_ms.is_some()
        {
            depeering.booked = depeering.booked.saturating_add(amount);
        }
        if let Some(winding) = self.winding_mut(peer, scheme)
            && winding.asked_ms.is_some()
        {
            winding.booked = winding.booked.saturating_add(amount);
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
        // A subscription needs an active peering
        // (spec/areas/network.md#snapshot-subscriptions).
        self.end_snapshots(&peer);
        self.advertised_changed();
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
                // is still ours: its cooldown holds
                // (spec/decisions/ending-a-peering.md,
                // spec/decisions/ended-for-room.md).
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
        super::inbound::ok(&[])
    }

    /// Bans `node`: it is never asked, is refused, and a peering with it is
    /// ended at once. A ban is the owner's emphatic word, often about
    /// misbehaviour, so the node collects nothing first
    /// (spec/decisions/ending-a-peering.md): the peer is withdrawn from our
    /// link state and told now, and what it owes stays on our books.
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

    /// The peers listed as our neighbors, in order. A downlink is only
    /// once we promoted it: its child addresses lie beneath ours, so the
    /// rest of the network needs no route to it
    /// (spec/decisions/core-and-periphery.md), until it is to be core
    /// (spec/decisions/promotion-to-core.md).
    pub(crate) fn advertised(&self) -> Vec<NodeId> {
        self.table
            .iter()
            .filter(|(_, entry)| entry.active_peering().is_some() && entry.advertise.on)
            .map(|(id, _)| *id)
            .collect()
    }

    /// The downlinks we promoted: advertised, so on their way into the core.
    pub(crate) fn promoted(&self) -> impl Iterator<Item = (&NodeId, &Entry)> {
        self.table.iter().filter(|(_, entry)| {
            entry.is_downlink() && entry.active_peering().is_some() && entry.advertise.on
        })
    }

    /// How many more core relationships we take: core peers, and downlinks
    /// promoted, which turn into core peers, both within `max_peers`, and
    /// inbound ones, as a promoted downlink is, within the inbound bound
    /// (spec/decisions/promotion-to-core.md).
    pub(crate) fn free_core(&self) -> usize {
        let promoted = self.promoted().count();
        let core = self.core_peers().count() + promoted;
        let inbound = self.core_peers().filter(|(_, p)| p.inbound).count() + promoted;
        self.config
            .max_peers
            .saturating_sub(core)
            .min(self.config.max_inbound().saturating_sub(inbound))
    }

    /// The advertised peers changed. A node with a URL lists them in its
    /// link state, so it issues a new one, and its routes follow from that.
    /// One without lists none, and its link state stays as it was; its
    /// routes go through them, and are recomputed
    /// (spec/areas/network.md#periphery-nodes).
    pub(crate) fn advertised_changed(&mut self) {
        if self.has_url() {
            self.gossip.own_changed = true;
        }
        self.gossip.routes_dirty = true;
    }

    /// Looks after the peerings: ends silent ones and those on a scheme
    /// the node does not run or that would settle directly, sets the
    /// advertise flags, tells terminations, and ends those that are done.
    /// A method dropped for its scheme goes before room is judged, so its
    /// shortness never ends the peering.
    pub(super) fn review_peers(&mut self, hooks: &dyn PeeringHooks) {
        self.end_downlinks();
        self.terminate_silent();
        self.end_unrun(hooks);
        self.terminate_short();
        self.drop_unpayable(hooks);
        self.advance_depeerings(hooks);
        self.advance_windings(hooks);
        self.update_advertised(hooks);
        let telling: Vec<NodeId> = self.telling.keys().copied().collect();
        for peer in telling {
            self.tell_termination(peer);
        }
        self.tell_drops();
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
        self.close_idle_channels(hooks);
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
                    // once we decided to
                    // (spec/decisions/ending-a-peering.md).
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
                    // Short of room: withdrawn once every method is short
                    // past the grace, and ended once one stays short, while
                    // it could be reached, as long as a silent peer. The
                    // time short grows only at checks, which come with an
                    // input: due at once when one passed a limit.
                    let stuck = peering.unavailable(config.advertise_short_grace_ms);
                    let ended = if !ending && peering.short_for_good(config.peer_dead_ms) {
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
                    } else if checked_for_promotion(entry) {
                        // Listed once promoted: its standing is long enough
                        // at a time we know; the other checks change with an
                        // input, or are looked at with each heartbeat's
                        // review.
                        entry
                            .standing
                            .since_ms
                            .map(|since| since.saturating_add(config.promotion_standing_ms))
                            .filter(|at| *at > now)
                            .unwrap_or(u64::MAX)
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
                    // Ending it privately, winding a method down, or dropping
                    // one that cannot receive.
                    let collect = self
                        .next_depeer(peering)
                        .min(self.next_wind(peering))
                        .min(self.next_unpayable(peering));
                    Some(silent.min(flag).min(ended).min(collect))
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
        peerings
            .chain(telling)
            .chain(self.next_drop())
            .chain(self.next_channel_close())
            .min()
    }

    /// Changes the kind of each active peering whose peer's status no
    /// longer matches it, in place (spec/decisions/promotion-to-core.md): a
    /// peer that joins the core turns from our downlink into a core peer,
    /// one that leaves it the other way. Only a core node has kinds to
    /// follow: a periphery node's peers are all its uplinks, and its
    /// downlinks from when it was core end
    /// ([`end_downlinks`](Self::end_downlinks)). A peering being ended
    /// keeps its kind until it is over.
    pub(super) fn follow_kinds(&mut self, hooks: &mut dyn PeeringHooks) {
        if !self.is_core() {
            return;
        }
        let changing: Vec<(NodeId, bool)> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry
                    .active_peering()
                    .is_some_and(|peering| peering.depeering.is_none())
            })
            .filter_map(|(id, entry)| {
                let link = entry.link.as_ref()?;
                let core = self.is_core_node(id, !link.periphery);
                (entry.is_downlink() == core).then_some((*id, !core))
            })
            .collect();
        for (id, downlink) in changing {
            self.change_kind(hooks, id, downlink);
        }
    }

    /// Changes peer `id`'s kind in place, to our downlink if `downlink`,
    /// else to a core peer: the peering, its methods, its channel and
    /// whether we list it stay; its place in the table, its bounds and how
    /// it is reached follow the new kind. It must fit as a new peering of
    /// that kind would, but for a probe of its URL, which the bootstrap
    /// nodes made as it registered: a core peer within the inbound bound if
    /// it asked us, with its accounts open; a downlink within the
    /// downlinks' bounds and among the free receivers'. Otherwise the
    /// peering ends instead. A downlink we listed is one we promoted, and
    /// a core peer we listed one whose edges keep it core: listing it on
    /// keeps its edge, where a withdrawal would push it out of the core
    /// again. A downlink is still listed only while it passes the checks of
    /// a promotion ([`update_advertised`](Self::update_advertised)).
    fn change_kind(&mut self, hooks: &mut dyn PeeringHooks, id: NodeId, downlink: bool) {
        let now = self.now_ms;
        let fits = if downlink {
            self.fits_as_downlink(hooks, &id)
        } else {
            self.fits_as_core_peer(&id) && {
                let schemes: Vec<SchemeId> = self
                    .peering_of(&id)
                    .map(|peering| peering.schemes().collect())
                    .unwrap_or_default();
                hooks.open_core_accounts(now, id, &schemes)
            }
        };
        // A core peer's place in the table, made only once it is sure.
        let placed = fits
            && (downlink
                || self.table.len() - self.downlinks() < self.config.max_nodes
                || self.evict_for(Sources::PEER));
        if !placed {
            self.begin_depeering(id, TerminationReason::Unspecified);
            return;
        }
        let Some(entry) = self.table.get_mut(&id) else {
            return;
        };
        let (from, to) = if downlink {
            (Sources::PEER, Sources::DOWNLINK)
        } else {
            (Sources::DOWNLINK, Sources::PEER)
        };
        entry.sources.remove(from);
        entry.sources.insert(to);
        entry.was_core |= !downlink;
        self.dirty.insert(id);
        // Only core peers are subscribed to (spec/areas/network.md#snapshot-subscriptions).
        if downlink {
            self.unsubscribe(id);
        }
        // Its channel counts with its kind, and is reached as it is now.
        self.open_channel_from(id, None);
        // How many core relationships are free changed.
        self.gossip.own_changed |= self.is_core();
        self.gossip.routes_dirty = true;
        self.event(Event::KindChanged { peer: id, downlink });
    }

    /// Whether peer `id`, our downlink, fits as a core peer, as a request
    /// of a core node's would ([`check_request`](Self::check_request)).
    /// One we promoted has its place kept for it already
    /// ([`free_core`](Self::free_core)).
    fn fits_as_core_peer(&self, id: &NodeId) -> bool {
        let Some(entry) = self.table.get(id) else {
            return false;
        };
        let inbound = entry.peering().is_some_and(|peering| peering.inbound);
        let inbound_peers = self.core_peers().filter(|(_, p)| p.inbound).count();
        let (_, core_channels) = self.channel_counts();
        self.is_core()
            && (!inbound || inbound_peers < self.config.max_inbound())
            && self.core_peers().count() < self.config.max_peers
            && core_channels < self.config.max_channels
            && (self.table.len() - self.downlinks() < self.config.max_nodes
                || self.eviction_for(Sources::PEER).is_some())
    }

    /// Whether core peer `id` fits as our downlink, as a periphery node's
    /// request would ([`downlink_room`](Self::downlink_room)).
    fn fits_as_downlink(&self, hooks: &dyn PeeringHooks, id: &NodeId) -> bool {
        let downlinks = self.peers().count() - self.core_peers().count();
        let (downlink_channels, _) = self.channel_counts();
        self.takes_downlinks()
            && self.downlinks() < self.config.max_downlinks
            && downlinks < self.config.max_downlinks
            && downlink_channels < self.config.max_downlinks
            && (self.free_downlinks() < self.config.max_free_downlinks() || self.paid_us(id, hooks))
    }

    /// On a node that keeps no downlinks ([`Self::keeps_downlinks`]), ends
    /// the peerings with its downlinks, from when it did: nothing routes to
    /// them (spec/areas/network.md#downlinks). It does so once it has kept
    /// none for as long as its status takes to settle, so that a dip while
    /// a peer restarts ends nothing. Those not peered leave at once.
    pub(crate) fn end_downlinks(&mut self) {
        let settled = self
            .status
            .stranded_since_ms
            .is_some_and(|since| since.saturating_add(self.config.core_settle_ms) <= self.now_ms);
        if !settled {
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
                self.drop_downlink(&node);
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

    /// Terminates peers whose room in our books on a method has been short
    /// of a packet, without a settlement, for as long as a silent peer is
    /// kept: a link that stuck, or drifted past what it survives, is not
    /// saved, and a method short for so long ends the peering
    /// (spec/decisions/peering-lifecycle.md,
    /// spec/decisions/settlement-methods.md). Only the time it could be
    /// reached counts: a peer our requests to settle surely never reached had
    /// no chance to settle, and is ended for its silence if it is also
    /// silent, never for room.
    fn terminate_short(&mut self) {
        let dead = self.config.peer_dead_ms;
        let short: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry.active_peering().is_some_and(|peering| {
                    peering.depeering.is_none() && peering.short_for_good(dead)
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for peer in short {
            self.begin_depeering(peer, TerminationReason::ShortOfRoom);
        }
    }

    /// Drops the methods on a scheme the node does not run, as one disabled
    /// or of the other realm, which the node starts without
    /// (spec/areas/settlement.md#schemes-and-ledgers), and those on a scheme
    /// that would settle directly, as one a peering stored from when the
    /// XRP Ledger's schemes peered still has (spec/decisions/hubs-only.md),
    /// and ends a peering that has no other: no packet crosses such a
    /// method, no settlement on it can be booked, and while one lasts the
    /// pair cannot settle on a scheme it shares. So nothing is collected
    /// first, as for a ban: the
    /// peer is told at once (spec/decisions/settlement-methods.md). A
    /// decision already taken to end the peering keeps its reason, so an
    /// end for the owner or for room keeps its cooldown; otherwise the
    /// reason is unspecified, and the pair may peer again after a backoff
    /// (spec/decisions/ending-a-peering.md). Looked at with each review,
    /// which runs at least once a heartbeat while the node has a peer, so
    /// at start too.
    fn end_unrun(&mut self, hooks: &dyn PeeringHooks) {
        let mut ended: Vec<(NodeId, TerminationReason)> = Vec::new();
        let mut dropped: Vec<(NodeId, SchemeId)> = Vec::new();
        for (id, entry) in &self.table {
            let Some(peering) = entry.active_peering() else {
                continue;
            };
            let gone = |scheme: SchemeId| !hooks.speaks(scheme) || hooks.settles_directly(scheme);
            let unrun = peering.schemes().filter(|scheme| gone(*scheme));
            if peering.schemes().all(gone) {
                let reason = peering
                    .depeering
                    .map_or(TerminationReason::Unspecified, |depeering| depeering.reason);
                ended.push((*id, reason));
            } else {
                dropped.extend(unrun.map(|scheme| (*id, scheme)));
            }
        }
        for (peer, reason) in ended {
            self.begin_termination(peer, reason, true);
        }
        for (peer, scheme) in dropped {
            self.drop_method_now(peer, scheme);
        }
    }

    /// A peer is advertised while it is confirmed, has had room for a
    /// packet within the grace on one of its methods at least that is not
    /// being wound down, and is not held down; each withdrawal holds it down longer, until it has been
    /// stable a while (spec/decisions/peering-lifecycle.md,
    /// spec/decisions/settlement-methods.md). A downlink is advertised only
    /// once promoted, by a core node: while it passes every check of a
    /// promotion, and, to start, while a core relationship is free
    /// (spec/decisions/promotion-to-core.md).
    fn update_advertised(&mut self, hooks: &dyn PeeringHooks) {
        let config = self.config;
        let now = self.now_ms;
        let core = self.is_core();
        let promotable: BTreeSet<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| entry.is_downlink() && core)
            .filter(|(id, _)| {
                self.promotion(id, |scheme| hooks.room_packets(**id, scheme))
                    .is_some_and(|promotion| promotion.pending(&config).is_empty())
            })
            .map(|(id, _)| *id)
            .collect();
        let mut free = self.free_core();
        let mut changes = Vec::new();
        for (id, entry) in &mut self.table {
            let Some(peering) = entry.active_peering() else {
                continue;
            };
            // A method being wound down is no more available than one
            // withdrawn for room, but only room is reported as the reason.
            let short = peering.withdrawn(config.advertise_short_grace_ms);
            let unavailable = peering.unavailable(config.advertise_short_grace_ms);
            let live = peering.heard_within(now, config.peer_confirm_ms) && !self.bans.contains(id);
            let healthy = live && !unavailable;
            // Good standing, which a promotion counts the time of, is what
            // listing a peer asks (spec/decisions/promotion-to-core.md).
            if healthy {
                entry.standing.since_ms.get_or_insert(now);
            } else {
                entry.standing.since_ms = None;
            }
            entry.was_core |= !entry.is_downlink();
            let by_health = !checked_for_promotion(entry);
            let promoted = by_health || promotable.contains(id);
            let advertise = &mut entry.advertise;
            if advertise.on
                && advertise
                    .on_since_ms
                    .saturating_add(config.advertise_stable_ms)
                    <= now
            {
                advertise.flaps = 0;
            }
            if advertise.on && !(healthy && promoted) {
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
            } else if !advertise.on
                && healthy
                && promoted
                && (by_health || free > 0)
                && advertise.hold_until_ms <= now
            {
                free = free.saturating_sub(usize::from(!by_health));
                advertise.on = true;
                advertise.on_since_ms = now;
                changes.push((*id, true, false));
            }
        }
        if !changes.is_empty() {
            self.advertised_changed();
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

    /// Ends a terminating peering. Its accounts and channel stay, the
    /// channel until it has been idle for as long as a silent peer is kept
    /// (spec/decisions/closing-channels.md); asking the node again waits a
    /// while, a long while if an owner ended it or it was for room.
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
        entry.was_core = false;
        let wait = if keeps_away(termination.reason) {
            // Nor does it take the peer's requests: the owner meant it, and
            // a link that stayed short is not formed again at once
            // (spec/decisions/ended-for-room.md).
            entry.refuse_until_ms = now.saturating_add(cooldown);
            cooldown
        } else {
            backoff(min, max, entry.peering_failures.saturating_add(1))
        };
        entry.peering_at_ms = now.saturating_add(wait);
        self.dirty.insert(peer);
        if self.channels.contains_key(&peer) {
            self.channel_active.insert(peer, now);
        }
        self.telling.remove(&peer);
        self.dropping.retain(|(node, _), _| *node != peer);
        self.unreached.remove(&peer);
        self.drop_peer_source(&peer);
        self.gossip.routes_dirty = true;
        // A core relationship may be free again.
        self.gossip.own_changed |= self.is_core();
        self.stats.terminations += 1;
        // Nothing is settled at the end: what the accounts show carries
        // into a later peering (spec/decisions/ending-a-peering.md).
        self.event(Event::PeeringEnded { peer });
    }

    /// A request from `sender` that presumes a peering,
    /// though we hold none with it and are not asking: its side is left
    /// over, as when our termination never reached it. It is told so, at
    /// most once per termination timeout, and its request still gets its
    /// refusal for not being peered (spec/areas/network.md#one-sided-peerings).
    /// A node that is asking holds back: the sender may have accepted our
    /// request, and only not told us yet. A downlink leaves the table when
    /// its peering ends, and is reached by the channel it keeps.
    pub(crate) fn notice_one_sided(&mut self, sender: NodeId) {
        let now = self.now_ms;
        let period = self.config.termination_timeout_ms;
        if self
            .table
            .get(&sender)
            .is_some_and(|entry| entry.peer != PeerState::None)
        {
            return;
        }
        let contact = self
            .contact(&sender)
            .map(|(_, endpoint)| endpoint)
            .or_else(|| {
                self.channels
                    .get(&sender)
                    .map(|channel| channel.endpoint.clone())
            });
        let Some(endpoint) = contact else {
            return;
        };
        self.told_not_peered
            .retain(|_, at| at.saturating_add(period) > now);
        if self.told_not_peered.contains_key(&sender)
            || self.told_not_peered.len() >= MAX_TOLD_NOT_PEERED
        {
            return;
        }
        self.told_not_peered.insert(sender, now);
        let content = Content::PeeringTermination(TerminationReason::NotPeered);
        let timeout = self.config.request_timeout_ms;
        self.send_via(sender, endpoint, &content, Purpose::Gossip, timeout);
    }
}

/// Most nodes told within a termination timeout that they hold a peering
/// we do not: past it, a node is told at its next request after the
/// oldest is forgotten.
pub(super) const MAX_TOLD_NOT_PEERED: usize = 1024;

/// Whether an end for `reason` keeps the two nodes apart for the owner's
/// cooldown, both ways: one the owner chose, and one for want of room,
/// which formed again at once would only stick again
/// (spec/decisions/ended-for-room.md).
const fn keeps_away(reason: TerminationReason) -> bool {
    matches!(
        reason,
        TerminationReason::Owner | TerminationReason::ShortOfRoom
    )
}

/// Whether `entry`'s peer is listed only while it passes the checks of a
/// promotion: our downlink, but for one whose peering was core while it is
/// registered, which is listed by its health, as a core peer's. A node that
/// left the core for a moment, silent past the confirmation or while a peer
/// restarted, comes back after its hold-down, not after a new promotion;
/// one whose registration ended is out (spec/decisions/promotion-to-core.md).
fn checked_for_promotion(entry: &Entry) -> bool {
    entry.is_downlink() && !(entry.was_core && entry.sources.contains(Sources::MAJORITY))
}
