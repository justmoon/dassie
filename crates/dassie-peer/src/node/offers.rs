//! Method offers (spec/decisions/method-offers.md,
//! spec/areas/network.md#settlement-methods): each node tells each peer the
//! methods it settles on with it, and a method is a scheme both offer that
//! the node adopted. Nobody asks for one.
//!
//! - **An offer is a commitment.** A scheme is offered only where the node
//!   could adopt it: a method that would open a core peer's accounts only
//!   within the core peers' share, each keeping its pair until it is
//!   adopted or leaves the offer, the fewest by scheme ID first, so two
//!   nodes short of room offer the same one.
//! - **Adopted before it is said.** The node adopts a scheme both offer
//!   before anything it sends carries it, so that a peer that has seen an
//!   offer of ours holds every method it carried.
//! - **The gate.** Our packets take a method only once the peer has seen an
//!   offer of ours as new as the one we held when we adopted it
//!   ([`Peering::confirms`](crate::Peering::confirms)).
//! - **Versions belong to the pair,** on its settlement channel, and never
//!   start again: an older offer, or a copy, is never taken.

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};

use super::inbound::{Accepted, ok};
use super::{MAX_METHODS, PeerNode, Problem, Purpose};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Reply};
use crate::table::{Entry, Method, PeerState, backoff, jittered};
use crate::wire::{Content, MethodOffer, OfferedMethod, SchemeId};

/// What the node keeps of its offers with one peer beside what is
/// persisted with the peering and its channel: lost with a restart, it
/// comes back with the next exchange, or only paces our own sends.
#[derive(Debug, Default)]
pub(crate) struct Talk {
    /// The peer's newest offer we took in, on schemes we run or hold a
    /// method on, each with its data: `None` until we took one in this run.
    theirs: Option<Vec<(SchemeId, Vec<u8>)>>,
    /// The version of our offer each scheme of it entered with.
    since: Vec<(SchemeId, u64)>,
    /// Our data on each scheme of our offer, as we last offered it.
    sent: Vec<(SchemeId, Vec<u8>)>,
    /// The schemes we leave out of our offer for a while, each with when
    /// it may come back and how often it was left out in a row.
    excluded: Vec<(SchemeId, u64, u32)>,
    /// Our offer's sends that failed in a row.
    failures: u32,
    /// When our offer may next be sent.
    at_ms: u64,
    /// Whether one is on its way.
    in_flight: bool,
    /// Whether it may go again as soon as the peer answers a request of
    /// ours, before its backoff ends.
    early: bool,
    /// Whether the peer takes offers: `Some(false)` once it answered one
    /// with 400 or asked us for a method, as a node of the release before
    /// does (spec/decisions/method-offers.md).
    speaks: Option<bool>,
}

impl Talk {
    fn excludes(&self, scheme: SchemeId, now_ms: u64) -> bool {
        self.excluded
            .iter()
            .any(|(excluded, until, _)| *excluded == scheme && *until > now_ms)
    }

    fn since(&self, scheme: SchemeId) -> Option<u64> {
        self.since
            .iter()
            .find(|(offered, _)| *offered == scheme)
            .map(|(_, version)| *version)
    }

    fn theirs_on(&self, scheme: SchemeId) -> Option<&[u8]> {
        self.theirs.as_ref().and_then(|theirs| {
            theirs
                .iter()
                .find(|(offered, _)| *offered == scheme)
                .map(|(_, data)| data.as_slice())
        })
    }
}

/// What a peering would take as newcomers, as worked out for the core
/// peers' share.
struct Wanted {
    peer: NodeId,
    /// Those that open no accounts.
    free: Vec<SchemeId>,
    /// Those that would, by scheme ID.
    needing: Vec<SchemeId>,
    /// How many of `needing` our offer has already.
    held: usize,
}

/// What the node offers one peer, as worked out for its next version.
struct Plan {
    peer: NodeId,
    /// The schemes it settles on with the peer, methods and newcomers,
    /// in the order of our own offers.
    schemes: Vec<SchemeId>,
}

impl PeerNode {
    /// Whether the peer takes offers, as far as we know: `None` until it
    /// answered one or sent one.
    #[must_use]
    pub fn speaks_offers(&self, peer: &NodeId) -> Option<bool> {
        self.talks.get(peer).and_then(|talk| talk.speaks)
    }

    fn talk_mut(&mut self, peer: NodeId) -> &mut Talk {
        let now = self.now_ms;
        self.talks.entry(peer).or_insert_with(|| Talk {
            at_ms: now,
            ..Talk::default()
        })
    }

    /// Leaves `scheme` out of our offer to `peer` for a backoff that
    /// doubles each time in a row, with jitter, so that two nodes that
    /// leave each other's out do not offer again in step.
    pub(super) fn exclude(&mut self, peer: NodeId, scheme: SchemeId) {
        if self.peering_of(&peer).is_none() {
            return;
        }
        let (now, min, max) = (
            self.now_ms,
            self.config.min_backoff_ms,
            self.config.max_backoff_ms,
        );
        let random = self.rng.next_u64();
        let talk = self.talk_mut(peer);
        let failures = talk
            .excluded
            .iter()
            .find(|(excluded, _, _)| *excluded == scheme)
            .map_or(0, |(_, _, failures)| *failures);
        let until = now.saturating_add(jittered(backoff(min, max, failures), random));
        talk.excluded.retain(|(excluded, _, _)| *excluded != scheme);
        talk.excluded
            .push((scheme, until, failures.saturating_add(1)));
        assert!(
            talk.excluded.len() <= super::MAX_SCHEMES + MAX_METHODS,
            "exclusions per peer stay bounded"
        );
    }

    /// The core peers' pairs of accounts our offers and our peering attempt
    /// keep: one for each scheme we offer a core peer and have not adopted,
    /// on whose ledger the peer has no accounts yet, and one for a peering
    /// we ask for that would open them. `except` is one not to count.
    pub(super) fn reserved_pairs(
        &self,
        hooks: &dyn PeeringHooks,
        except: Option<(NodeId, SchemeId)>,
    ) -> usize {
        let offered: usize = self
            .table
            .iter()
            .filter(|(_, entry)| !entry.is_downlink())
            .filter_map(|(id, entry)| entry.peering().map(|peering| (id, peering)))
            .map(|(id, peering)| {
                peering
                    .offered
                    .iter()
                    .filter(|scheme| {
                        !peering.has(**scheme)
                            && except != Some((*id, **scheme))
                            && !hooks.has_accounts(*id, **scheme)
                    })
                    .count()
            })
            .sum();
        offered + usize::from(self.attempt_reserves(hooks, except))
    }

    /// Whether our peering attempt keeps a pair: it asks a node on a scheme
    /// on whose ledger it has no accounts yet.
    fn attempt_reserves(
        &self,
        hooks: &dyn PeeringHooks,
        except: Option<(NodeId, SchemeId)>,
    ) -> bool {
        self.peering.attempt().is_some_and(|node| {
            matches!(
                self.table.get(&node).map(|entry| &entry.peer),
                Some(PeerState::Requesting { scheme, .. })
                    if except != Some((node, *scheme)) && !hooks.has_accounts(node, *scheme)
            )
        })
    }

    /// Whether one more core peer's pair fits, beside what offers and our
    /// attempt keep, unless `except` keeps it already.
    pub(super) fn core_pair_free(
        &self,
        hooks: &dyn PeeringHooks,
        peer: NodeId,
        scheme: SchemeId,
    ) -> bool {
        hooks.has_accounts(peer, scheme)
            || hooks.free_core_pairs() > self.reserved_pairs(hooks, Some((peer, scheme)))
    }

    /// Works out every active peering's offer, takes a new version where
    /// it changed, adopts the schemes both offer, and leaves out those a
    /// peer turned down, until nothing changes: three rounds at most, since
    /// a round after the first only adopts what the one before offered, or
    /// leaves out what it could not.
    pub(super) fn refresh_offers(&mut self, hooks: &mut dyn PeeringHooks) {
        for _ in 0..3 {
            let plans = self.plan_offers(hooks);
            for plan in plans {
                self.take_plan(hooks, plan);
            }
            let peers: Vec<NodeId> = self.talks.keys().copied().collect();
            let mut changed = false;
            for peer in peers {
                changed |= self.adopt(hooks, peer);
                changed |= self.judge_declines(hooks, peer);
            }
            if !changed {
                break;
            }
        }
    }

    /// What we offer each active peering: its methods; the schemes a
    /// peering with it would settle on that open no core peer's accounts;
    /// and of those that would, as many as the core peers' share keeps
    /// room for, the lowest scheme IDs first, those offered already
    /// keeping theirs.
    fn plan_offers(&self, hooks: &dyn PeeringHooks) -> Vec<Plan> {
        let wanted: Vec<Wanted> = self
            .table
            .iter()
            .filter_map(|(id, entry)| Some((*id, self.newcomers(hooks, id, entry)?)))
            .map(|(peer, (free, needing, held))| Wanted {
                peer,
                free,
                needing,
                held,
            })
            .collect();
        // The core peers' share, less what our attempt keeps: first to
        // those offered already, then to more, peer by peer in ID order.
        let mut room = hooks
            .free_core_pairs()
            .saturating_sub(usize::from(self.attempt_reserves(hooks, None)));
        let mut granted: Vec<usize> = Vec::with_capacity(wanted.len());
        for want in &wanted {
            let kept = want.held.min(room);
            room -= kept;
            granted.push(kept);
        }
        for (want, kept) in wanted.iter().zip(granted.iter_mut()) {
            let more = want.needing.len().saturating_sub(*kept).min(room);
            room -= more;
            *kept += more;
        }
        wanted
            .into_iter()
            .zip(granted)
            .map(|(want, kept)| self.plan_for(want, kept))
            .collect()
    }

    /// The newcomers we would offer the peer of `entry`, if it is an
    /// active peering: those that open no accounts, and those that would,
    /// by scheme ID, with how many of the latter our offer has already. A
    /// peering being ended, or with a peer of the release before, which
    /// takes no offers, has none.
    fn newcomers(
        &self,
        hooks: &dyn PeeringHooks,
        id: &NodeId,
        entry: &Entry,
    ) -> Option<(Vec<SchemeId>, Vec<SchemeId>, usize)> {
        let peering = entry.active_peering()?;
        let talk = self.talks.get(id);
        let legacy = talk.is_some_and(|talk| talk.speaks == Some(false));
        let mut free = Vec::new();
        let mut needing = Vec::new();
        if peering.depeering.is_none()
            && !legacy
            && !self.bans.contains(id)
            && let Some(link) = entry.link.as_ref()
        {
            let downlink = entry.is_downlink();
            for scheme in self.methods_with(hooks, link) {
                let committed = peering.offered.contains(&scheme);
                if peering.has(scheme)
                    || !hooks.speaks(scheme)
                    || talk.is_some_and(|talk| talk.excludes(scheme, self.now_ms))
                    || !(committed
                        || hooks.takes_newcomer(scheme, downlink)
                            && hooks.wants_method(*id, downlink, scheme))
                {
                    continue;
                }
                if downlink || hooks.has_accounts(*id, scheme) {
                    free.push(scheme);
                } else {
                    needing.push(scheme);
                }
            }
        }
        needing.sort_unstable();
        let held = needing
            .iter()
            .filter(|scheme| peering.offered.contains(scheme))
            .count();
        Some((free, needing, held))
    }

    /// The offer to `want`'s peer, with the first `kept` of the schemes
    /// that would open accounts: its methods first among our own offers'
    /// order, and a method on a scheme we no longer offer in our link
    /// state, as one that stopped, after them while it lasts.
    fn plan_for(&self, want: Wanted, kept: usize) -> Plan {
        let Wanted {
            peer,
            free,
            needing,
            ..
        } = want;
        let methods: Vec<SchemeId> = self
            .peering_of(&peer)
            .map(|peering| peering.schemes().collect())
            .unwrap_or_default();
        let chosen = needing.get(..kept).unwrap_or(&needing);
        let mut schemes: Vec<SchemeId> = self
            .offers
            .iter()
            .map(|offer| offer.scheme)
            .filter(|scheme| {
                methods.contains(scheme) || free.contains(scheme) || chosen.contains(scheme)
            })
            .collect();
        for scheme in methods {
            if !schemes.contains(&scheme) {
                schemes.push(scheme);
            }
        }
        Plan { peer, schemes }
    }

    /// Takes `plan` as our offer to its peer: a new version if its schemes,
    /// or our data on one of them, changed.
    fn take_plan(&mut self, hooks: &mut dyn PeeringHooks, plan: Plan) {
        let Plan { peer, schemes } = plan;
        let mut data: Vec<(SchemeId, Vec<u8>)> = Vec::with_capacity(schemes.len());
        for scheme in &schemes {
            let mut out = Vec::new();
            if hooks.peering_info(*scheme, &mut out).is_err() {
                // A method on a scheme that does not run says what it said.
                out = self
                    .talks
                    .get(&peer)
                    .and_then(|talk| talk.sent.iter().find(|(sent, _)| sent == scheme))
                    .map(|(_, data)| data.clone())
                    .unwrap_or_default();
            }
            data.push((*scheme, out));
        }
        let same = self.peering_of(&peer).is_some_and(|peering| {
            peering.offered == schemes
                && self.talks.get(&peer).is_some_and(|talk| talk.sent == data)
        });
        if same {
            return;
        }
        let Some(channel) = self.channels.get_mut(&peer) else {
            return;
        };
        channel.offer_out = channel.offer_out.saturating_add(1);
        let version = channel.offer_out;
        self.channels_dirty.insert(peer);
        if let Some(peering) = self.table.get_mut(&peer).and_then(Entry::peering_mut) {
            peering.offered.clone_from(&schemes);
        }
        self.dirty.insert(peer);
        let now = self.now_ms;
        let talk = self.talk_mut(peer);
        talk.since.retain(|(scheme, _)| schemes.contains(scheme));
        for scheme in &schemes {
            if !talk.since.iter().any(|(since, _)| since == scheme) {
                talk.since.push((*scheme, version));
            }
        }
        talk.sent = data;
        if !talk.in_flight {
            talk.at_ms = now;
        }
    }

    /// Adopts each scheme both we and `peer` offer that is no method yet,
    /// the lowest scheme ID first: its accounts open, and it is ours to
    /// take the peer's packets on at once and to send on once confirmed.
    /// One the node cannot adopt leaves our offer for a while. Returns
    /// whether anything changed.
    fn adopt(&mut self, hooks: &mut dyn PeeringHooks, peer: NodeId) -> bool {
        let Some(peering) = self.table.get(&peer).and_then(Entry::active_peering) else {
            return false;
        };
        let Some(talk) = self.talks.get(&peer) else {
            return false;
        };
        let mut both: Vec<(SchemeId, Vec<u8>)> = peering
            .offered
            .iter()
            .filter(|scheme| !peering.has(**scheme))
            .filter_map(|scheme| talk.theirs_on(*scheme).map(|data| (*scheme, data.to_vec())))
            .collect();
        both.sort_unstable_by_key(|(scheme, _)| *scheme);
        let mut changed = false;
        for (scheme, data) in both {
            changed = true;
            if !self.adopt_one(hooks, peer, scheme, &data) {
                self.exclude(peer, scheme);
            }
        }
        changed
    }

    /// Adopts `scheme` with `peer`, with the peer's `data`: the scheme
    /// opens its accounts, and the method is recorded with the version of
    /// our offer that carries it. Returns whether it is a method now.
    pub(super) fn adopt_one(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        peer: NodeId,
        scheme: SchemeId,
        data: &[u8],
    ) -> bool {
        let room = self
            .peering_of(&peer)
            .is_some_and(|peering| peering.methods.len() < MAX_METHODS && !peering.has(scheme));
        if !room || !hooks.speaks(scheme) {
            self.stats.adoptions_failed += 1;
            return false;
        }
        let downlink = self.is_downlink(&peer);
        let agreed = match hooks.accept(self.now_ms, peer, downlink, scheme, data) {
            Ok(agreed) => agreed,
            Err(_) => {
                self.stats.adoptions_failed += 1;
                return false;
            }
        };
        let version = self
            .channels
            .get(&peer)
            .map_or(0, |channel| channel.offer_out);
        let mut method = Method::new(scheme, agreed.state, agreed.credit_out);
        method.confirm_at = version;
        self.add_method(peer, method);
        if let Some(talk) = self.talks.get_mut(&peer) {
            talk.excluded.retain(|(excluded, _, _)| *excluded != scheme);
        }
        true
    }

    /// Leaves out of our offer to `peer` each scheme that would open its
    /// accounts and that the peer has turned down: it has seen an offer of
    /// ours with it, and its own, as new as that, does not carry it. The
    /// pair it kept goes to others meanwhile. Returns whether any was.
    fn judge_declines(&mut self, hooks: &dyn PeeringHooks, peer: NodeId) -> bool {
        let Some(entry) = self.table.get(&peer) else {
            return false;
        };
        let (Some(peering), Some(talk)) = (entry.active_peering(), self.talks.get(&peer)) else {
            return false;
        };
        if entry.is_downlink() || talk.theirs.is_none() {
            return false;
        }
        let out = self
            .channels
            .get(&peer)
            .map_or(0, |channel| channel.offer_out);
        let declined: Vec<SchemeId> = peering
            .offered
            .iter()
            .copied()
            .filter(|scheme| {
                !peering.has(*scheme)
                    && !hooks.has_accounts(peer, *scheme)
                    && peering.offer_acked >= talk.since(*scheme).unwrap_or(out)
                    && talk.theirs_on(*scheme).is_none()
            })
            .collect();
        let any = !declined.is_empty();
        for scheme in declined {
            self.exclude(peer, scheme);
        }
        any
    }

    /// Our offer to `peer`, as a message would carry it.
    fn our_offer(&self, peer: &NodeId) -> MethodOffer<'_> {
        let channel = self.channels.get(peer);
        let methods = self
            .talks
            .get(peer)
            .map(|talk| {
                talk.sent
                    .iter()
                    .map(|(scheme, data)| OfferedMethod {
                        scheme: *scheme,
                        data,
                    })
                    .collect()
            })
            .unwrap_or_default();
        MethodOffer {
            version: channel.map_or(0, |channel| channel.offer_out),
            seen: channel.map_or(0, |channel| channel.offer_in),
            methods,
        }
    }

    /// Takes in an offer of `peer`'s, from its request or its answer to
    /// ours: what it has seen of ours, and, if newer than the last we took,
    /// what it offers. Then our offers are worked out again, the schemes
    /// both offer adopted, the methods it no longer offers dropped, and
    /// each method the peer has now seen confirmed.
    ///
    /// # Errors
    ///
    /// The offer says it has seen a version of ours we never took.
    fn take_offer(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        peer: NodeId,
        offer: &MethodOffer<'_>,
    ) -> Result<(), ()> {
        let Some(channel) = self.channels.get(&peer) else {
            return Err(());
        };
        if offer.seen > channel.offer_out {
            return Err(());
        }
        let offer_in = channel.offer_in;
        let Some(peering) = self.table.get(&peer).and_then(Entry::active_peering) else {
            return Ok(());
        };
        // One of an earlier peering of the pair, delayed: nothing of it is
        // this peering's.
        if offer.version <= peering.offer_floor {
            self.stats.offers_stale += 1;
            return Ok(());
        }
        let acked_before = peering.offer_acked;
        let unconfirmed: Vec<SchemeId> = peering
            .methods
            .iter()
            .filter(|method| !peering.confirms(method))
            .map(|method| method.scheme)
            .collect();
        let held: Vec<SchemeId> = peering.schemes().collect();
        let known = self
            .talks
            .get(&peer)
            .is_some_and(|talk| talk.theirs.is_some());
        let newer = offer.version > offer_in || (offer.version == offer_in && !known);
        if let Some(peering) = self.table.get_mut(&peer).and_then(Entry::peering_mut) {
            peering.offer_acked = peering.offer_acked.max(offer.seen);
        }
        if acked_before < offer.seen {
            self.dirty.insert(peer);
        }
        let changed = self.take_content(hooks, peer, offer, newer, &held);
        self.follow_their_offer(hooks, peer, &held, changed);
        self.refresh_offers(hooks);
        if let Some(peering) = self.peering_of(&peer) {
            let confirmed: Vec<SchemeId> = unconfirmed
                .into_iter()
                .filter(|scheme| peering.confirmed(*scheme))
                .collect();
            for scheme in confirmed {
                self.event(Event::MethodConfirmed { peer, scheme });
            }
        }
        Ok(())
    }

    /// Takes in what `peer`'s `offer` carries if it is `newer` than what we
    /// took last, on schemes we run or `held` as methods. Returns the
    /// methods whose data it changed, with the new data.
    fn take_content(
        &mut self,
        hooks: &dyn PeeringHooks,
        peer: NodeId,
        offer: &MethodOffer<'_>,
        newer: bool,
        held: &[SchemeId],
    ) -> Vec<(SchemeId, Vec<u8>)> {
        let talk = self.talk_mut(peer);
        talk.speaks = Some(true);
        if !newer {
            self.stats.offers_stale += 1;
            return Vec::new();
        }
        let theirs: Vec<(SchemeId, Vec<u8>)> = offer
            .methods
            .iter()
            .filter(|method| hooks.speaks(method.scheme) || held.contains(&method.scheme))
            .map(|method| (method.scheme, method.data.to_vec()))
            .collect();
        let mut changed = Vec::new();
        for (scheme, data) in &theirs {
            let before = talk.theirs_on(*scheme);
            if held.contains(scheme) && before.is_some() && before != Some(data.as_slice()) {
                changed.push((*scheme, data.clone()));
            }
        }
        talk.theirs = Some(theirs);
        if let Some(channel) = self.channels.get_mut(&peer) {
            channel.offer_in = channel.offer_in.max(offer.version);
        }
        self.channels_dirty.insert(peer);
        self.stats.offers_taken += 1;
        changed
    }

    /// Follows `peer`'s offer as we hold it: each of the methods we `held`
    /// that it no longer carries is dropped, as it dropped it; one whose
    /// data `changed` takes the new data, or goes, and its scheme stays out
    /// of our offer a while.
    fn follow_their_offer(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        peer: NodeId,
        held: &[SchemeId],
        changed: Vec<(SchemeId, Vec<u8>)>,
    ) {
        let gone: Vec<SchemeId> = held
            .iter()
            .copied()
            .filter(|scheme| {
                self.talks
                    .get(&peer)
                    .is_some_and(|talk| talk.theirs.is_some() && talk.theirs_on(*scheme).is_none())
            })
            .collect();
        for scheme in gone {
            self.peer_dropped(peer, scheme);
        }
        for (scheme, data) in changed {
            if !self
                .peering_of(&peer)
                .is_some_and(|peering| peering.has(scheme))
            {
                continue;
            }
            let downlink = self.is_downlink(&peer);
            if hooks
                .accept(self.now_ms, peer, downlink, scheme, &data)
                .is_err()
            {
                self.exclude(peer, scheme);
                self.drop_method_now(peer, scheme);
            }
        }
    }

    /// The peer no longer holds its method on `scheme`: it collected on it
    /// first, so we drop it at once and settle nothing more on it, as when
    /// it told us with a drop. A peering it leaves with none ends, told; one
    /// we are ending, whose first method it was, ends too.
    pub(super) fn peer_dropped(&mut self, peer: NodeId, scheme: SchemeId) {
        let Some(peering) = self.peering_of(&peer) else {
            return;
        };
        if !peering.has(scheme) {
            return;
        }
        if peering.methods.len() > 1 && !peering.collecting_on(scheme) {
            self.remove_method(peer, scheme, false);
        } else if self
            .table
            .get(&peer)
            .and_then(Entry::active_peering)
            .is_some()
        {
            self.begin_termination(peer, crate::wire::TerminationReason::Unspecified, true);
        }
    }

    /// A peer's `methodOffer`: taken in, and answered with our own offer,
    /// as it stands once taken in. One from a node we hold no peering with
    /// gets 409, and the node is told, as for any request that presumes
    /// one; one to a peering being ended changes nothing.
    pub(super) fn method_offer_request(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        accepted: &Accepted,
        offer: &MethodOffer<'_>,
    ) -> Reply {
        let sender = accepted.sender;
        let state = self.table.get(&sender).map(|entry| &entry.peer);
        match state {
            Some(PeerState::Peered(_)) => {
                if self.take_offer(hooks, sender, offer).is_err() {
                    return Reply::refuse(400, "an offer of ours it cannot have seen");
                }
            }
            Some(PeerState::Terminating(..)) => {}
            _ => {
                self.notice_one_sided(sender);
                return Reply::refuse(409, "not peered");
            }
        }
        ok(&self.our_offer(&sender).to_vec())
    }

    /// Sends our offer to each peer that has not seen its newest version,
    /// one at a time: not to a peering being ended, nor to a peer of the
    /// release before.
    pub(super) fn send_offers(&mut self) {
        let now = self.now_ms;
        let due: Vec<NodeId> = self
            .talks
            .iter()
            .filter(|(_, talk)| !talk.in_flight && talk.at_ms <= now && talk.speaks != Some(false))
            .map(|(peer, _)| *peer)
            .filter(|peer| self.wants_offer_sent(peer))
            .collect();
        for peer in due {
            let sent = self
                .talks
                .get(&peer)
                .map(|talk| talk.sent.clone())
                .unwrap_or_default();
            let (version, seen) = self
                .channels
                .get(&peer)
                .map_or((0, 0), |channel| (channel.offer_out, channel.offer_in));
            let content = Content::MethodOffer(MethodOffer {
                version,
                seen,
                methods: sent
                    .iter()
                    .map(|(scheme, data)| OfferedMethod {
                        scheme: *scheme,
                        data,
                    })
                    .collect(),
            });
            let timeout = self.config.request_timeout_ms;
            if self.send(peer, &content, Purpose::Offer, timeout).is_some() {
                self.stats.offers_sent += 1;
                if let Some(talk) = self.talks.get_mut(&peer) {
                    talk.in_flight = true;
                }
            } else {
                self.offer_failed(peer);
            }
        }
    }

    /// Whether our offer to `peer` is one it has not seen, on an active
    /// peering.
    fn wants_offer_sent(&self, peer: &NodeId) -> bool {
        let out = self
            .channels
            .get(peer)
            .map_or(0, |channel| channel.offer_out);
        self.table
            .get(peer)
            .and_then(Entry::active_peering)
            .is_some_and(|peering| peering.offer_acked < out)
    }

    /// When an offer of ours is next due, or a scheme left out may come
    /// back.
    pub(super) fn next_offer(&self) -> Option<u64> {
        let now = self.now_ms;
        self.talks
            .iter()
            .flat_map(|(peer, talk)| {
                let send =
                    (!talk.in_flight && talk.speaks != Some(false) && self.wants_offer_sent(peer))
                        .then_some(talk.at_ms);
                let back = talk
                    .excluded
                    .iter()
                    .map(|(_, until, _)| *until)
                    .filter(|until| *until > now)
                    .min();
                send.into_iter().chain(back)
            })
            .min()
    }

    /// The answer to our offer: the peer's own, taken in. A 400 is a node
    /// of the release before, which takes no offers: every method we hold
    /// with it counts as confirmed, as its methods did then, and it is
    /// offered nothing more. Anything else is tried again after a backoff.
    pub(super) fn on_offer(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        peer: NodeId,
        result: Result<alloc::vec::Vec<u8>, Problem>,
    ) {
        if let Some(talk) = self.talks.get_mut(&peer) {
            talk.in_flight = false;
        }
        // Its peering ended meanwhile: nothing is left to agree.
        if self
            .table
            .get(&peer)
            .and_then(Entry::active_peering)
            .is_none()
        {
            return;
        }
        match result {
            Ok(body) => match MethodOffer::decode_exact(&body) {
                Ok(offer) => {
                    if let Some(talk) = self.talks.get_mut(&peer) {
                        talk.failures = 0;
                        talk.early = false;
                    }
                    if self.take_offer(hooks, peer, &offer).is_err() {
                        self.offer_failed(peer);
                    }
                }
                Err(_) => self.offer_failed(peer),
            },
            Err(Problem::Status(400)) => self.older_release(peer),
            Err(_) => self.offer_failed(peer),
        }
    }

    /// `peer` runs the release before, which takes no offers and asks for
    /// methods itself (spec/decisions/method-offers.md): every method we
    /// hold with it counts as confirmed, as it did, and we offer it no
    /// newcomer; it asks, and is told of what we drop, as before.
    pub(super) fn older_release(&mut self, peer: NodeId) {
        if self.peering_of(&peer).is_none() {
            return;
        }
        let out = self
            .channels
            .get(&peer)
            .map_or(0, |channel| channel.offer_out);
        let talk = self.talk_mut(peer);
        talk.speaks = Some(false);
        talk.failures = 0;
        let Some(peering) = self.table.get_mut(&peer).and_then(Entry::peering_mut) else {
            return;
        };
        let unconfirmed: Vec<SchemeId> = peering
            .methods
            .iter()
            .filter(|method| !peering.confirms(method))
            .map(|method| method.scheme)
            .collect();
        if peering.offer_acked < out {
            peering.offer_acked = out;
            self.dirty.insert(peer);
        }
        for scheme in unconfirmed {
            self.event(Event::MethodConfirmed { peer, scheme });
        }
    }

    /// Backs off from sending our offer to `peer` again, with jitter.
    fn offer_failed(&mut self, peer: NodeId) {
        let (now, min, max) = (
            self.now_ms,
            self.config.min_backoff_ms,
            self.config.max_backoff_ms,
        );
        let random = self.rng.next_u64();
        if let Some(talk) = self.talks.get_mut(&peer) {
            talk.in_flight = false;
            talk.failures = talk.failures.saturating_add(1);
            talk.at_ms = now.saturating_add(jittered(backoff(min, max, talk.failures), random));
            talk.early = true;
        }
    }

    /// `peer` answered a request of ours: an offer of ours that got no
    /// answer goes again now, rather than after its backoff, once per
    /// silence.
    pub(super) fn offer_reachable(&mut self, peer: &NodeId) {
        let now = self.now_ms;
        if let Some(talk) = self.talks.get_mut(peer)
            && talk.early
            && !talk.in_flight
        {
            talk.early = false;
            talk.at_ms = talk.at_ms.min(now);
        }
    }

    /// Takes `scheme` out of our offer to `peer` at once, as a new version:
    /// we dropped its method, so the offer commits to it no more.
    pub(super) fn withdraw_from_offer(&mut self, peer: NodeId, scheme: SchemeId) {
        let Some(peering) = self.table.get_mut(&peer).and_then(Entry::peering_mut) else {
            return;
        };
        if !peering.offered.contains(&scheme) {
            return;
        }
        peering.offered.retain(|offered| *offered != scheme);
        self.dirty.insert(peer);
        if let Some(channel) = self.channels.get_mut(&peer) {
            channel.offer_out = channel.offer_out.saturating_add(1);
            self.channels_dirty.insert(peer);
        }
        let now = self.now_ms;
        let talk = self.talk_mut(peer);
        talk.sent.retain(|(sent, _)| *sent != scheme);
        talk.since.retain(|(since, _)| *since != scheme);
        if !talk.in_flight {
            talk.at_ms = now;
        }
    }

    /// Forgets what the node kept of its offers with `peer`, whose peering
    /// is over: its channel keeps the versions.
    pub(super) fn forget_talk(&mut self, peer: &NodeId) {
        self.talks.remove(peer);
    }

    /// A peering was made: the first version of our offer, carrying its
    /// first method, is taken now, and sent with the next round. A method
    /// we accepted waits for the peer to see it; one the peer accepted is
    /// confirmed by its acceptance.
    pub(super) fn offers_begin(&mut self, peer: NodeId, scheme: SchemeId, accepted_theirs: bool) {
        let Some(channel) = self.channels.get_mut(&peer) else {
            return;
        };
        channel.offer_out = channel.offer_out.saturating_add(1);
        let version = channel.offer_out;
        self.channels_dirty.insert(peer);
        let floor = channel.offer_in;
        if let Some(peering) = self.table.get_mut(&peer).and_then(Entry::peering_mut) {
            peering.offered = alloc::vec![scheme];
            peering.offer_acked = 0;
            peering.offer_floor = floor;
            if let Some(method) = peering.method_mut(scheme) {
                method.confirm_at = if accepted_theirs { version } else { 0 };
            }
        }
        self.talks.remove(&peer);
        let talk = self.talk_mut(peer);
        talk.since.push((scheme, version));
    }

    /// A request for a peering we hold already, on `scheme`, with the
    /// requester's `data`: a copy, a crossing's second half, or a request
    /// whose acceptance got lost. It offers the scheme: adopted if we offer
    /// it too, so its acceptance tells the requester the truth.
    pub(super) fn requested_on_peering(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        sender: NodeId,
        scheme: SchemeId,
        data: &[u8],
    ) -> bool {
        if self
            .peering_of(&sender)
            .is_some_and(|peering| peering.has(scheme))
        {
            return true;
        }
        if self
            .table
            .get(&sender)
            .and_then(Entry::active_peering)
            .is_none()
        {
            return false;
        }
        self.refresh_offers(hooks);
        let offered = self
            .peering_of(&sender)
            .is_some_and(|peering| peering.offered.contains(&scheme));
        if !offered || !self.adopt_one(hooks, sender, scheme, data) {
            return false;
        }
        if let Some(talk) = self.talks.get_mut(&sender)
            && let Some(theirs) = talk.theirs.as_mut()
            && !theirs.iter().any(|(offered, _)| *offered == scheme)
        {
            theirs.push((scheme, data.to_vec()));
        }
        true
    }

    /// The release before's request for a further method (`methodAdd`),
    /// which such a node makes as the one that asked for the peering: its
    /// sender takes no offers. Adopted if we would offer it the scheme as
    /// a newcomer, and answered as a peering request is; every method with
    /// it counts as confirmed, since it sends on one once we accept it.
    pub(super) fn legacy_method_add(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        sender: NodeId,
        scheme: SchemeId,
        data: &[u8],
    ) -> bool {
        if self.peering_of(&sender).is_none() {
            return false;
        }
        self.talk_mut(sender).speaks = Some(false);
        let Some(entry) = self.table.get(&sender) else {
            return false;
        };
        let downlink = entry.is_downlink();
        let eligible = entry
            .active_peering()
            .is_some_and(|peering| peering.depeering.is_none() && !peering.has(scheme))
            && entry
                .link
                .as_ref()
                .is_some_and(|link| self.methods_with(hooks, link).contains(&scheme))
            && hooks.speaks(scheme)
            && hooks.takes_newcomer(scheme, downlink)
            && hooks.wants_method(sender, downlink, scheme)
            && (downlink || self.core_pair_free(hooks, sender, scheme));
        if !eligible {
            return false;
        }
        // Offered now, as a commitment is: the method is adopted with it.
        if let Some(channel) = self.channels.get_mut(&sender) {
            channel.offer_out = channel.offer_out.saturating_add(1);
            self.channels_dirty.insert(sender);
        }
        if let Some(peering) = self.table.get_mut(&sender).and_then(Entry::peering_mut)
            && !peering.offered.contains(&scheme)
        {
            peering.offered.push(scheme);
        }
        self.dirty.insert(sender);
        let adopted = self.adopt_one(hooks, sender, scheme, data);
        self.older_release(sender);
        adopted
    }
}
