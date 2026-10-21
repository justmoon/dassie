//! Message authentication, v1 (spec/decisions/peer-protocol-v1.md,
//! spec/areas/peer-protocol.md#keys-and-macs).
//!
//! A request names its sender's key, which must hash to the sender's node
//! ID; the MAC key comes from the two nodes' static keys; the MAC covers
//! the version, both node IDs, the session, counter and timestamp, and the
//! content. Responses are MACed under their own key and bound to the
//! request. Every table here is bounded, and a key is cached only once a
//! MAC made with it has verified (NET-9).
//!
//! Replay state is kept for good for the senders that matter, and in a
//! bounded pool, with a bounded share per network, for everyone else; a
//! request that only reads leaves none in the pool. The floors of the
//! senders kept for good are persisted, so that a restart reopens no
//! replay of theirs (spec/areas/peer-protocol.md#replay-protection).

mod keys;
mod replay;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::{NodeId, NodeKey, PublicKey};
use rand_core::CryptoRng;

pub use keys::{MacKey, PairKeys, macs_match, request_mac, response_mac};
pub use replay::{Guard, ReplayError, WINDOW};

use crate::table::Network;
use crate::wire::{Authentication, Envelope, HmacAuthentication, Kind, ResponseEnvelope};

/// The bounds and windows of authentication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthConfig {
    /// How far a timestamp may be from now, either way.
    pub skew_ms: u64,
    /// Most senders whose replay state is kept, and most destinations with
    /// a session of ours.
    pub max_senders: usize,
    /// Most of those senders whose state came from one network (a /24 or
    /// a /48), so that one network cannot hold every place.
    pub max_senders_per_network: usize,
    /// Most pairs of keys cached.
    pub max_keys: usize,
    /// How long one of our sessions lasts before a new one starts.
    pub session_lifetime_ms: u64,
}

impl AuthConfig {
    /// The production values.
    pub const DEFAULT: Self = Self {
        skew_ms: 60_000,
        max_senders: 1024,
        max_senders_per_network: 16,
        max_keys: 1024,
        session_lifetime_ms: 3_600_000,
    };
}

/// Why a request or response is not accepted as authentic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The message must be authenticated and is not.
    Missing,
    /// It uses a scheme this node does not speak.
    Unsupported,
    /// The sender's key is not a usable point.
    BadKey,
    /// The key is not the sender's, or the sender is this node.
    WrongSender,
    /// Its timestamp is outside the window.
    Stale,
    /// It was seen before.
    Replayed,
    /// The MAC does not verify.
    BadMac,
    /// Too many senders to keep track of one more.
    Busy,
}

impl From<ReplayError> for AuthError {
    fn from(error: ReplayError) -> Self {
        match error {
            ReplayError::Stale => Self::Stale,
            ReplayError::Replayed => Self::Replayed,
        }
    }
}

/// A request accepted: from whom, and how to answer it.
#[derive(Clone, Debug)]
pub struct Accepted {
    /// The sender, as claimed; proven if authenticated.
    pub sender: NodeId,
    /// The key it proved, if it authenticated.
    pub key: Option<PublicKey>,
    response: Option<(MacKey, [u8; 32])>,
}

impl Accepted {
    /// Whether the sender proved who it is.
    #[must_use]
    pub const fn is_authenticated(&self) -> bool {
        self.key.is_some()
    }

    /// The MAC for a response with `body`, if the request was
    /// authenticated.
    #[must_use]
    pub fn response_mac(&self, body: &[u8]) -> Option<[u8; 32]> {
        self.response
            .as_ref()
            .map(|(key, request)| response_mac(key, request, body))
    }
}

/// What checking the response to one of our requests needs.
#[derive(Clone, Debug)]
pub struct Pending {
    key: MacKey,
    request_mac: [u8; 32],
}

impl Pending {
    /// Checks that `response` comes from the node the request went to.
    ///
    /// # Errors
    ///
    /// The MAC is missing or does not verify.
    pub fn check(&self, response: &ResponseEnvelope<'_>) -> Result<(), AuthError> {
        let mac = response.mac.ok_or(AuthError::Missing)?;
        if macs_match(
            &mac,
            &response_mac(&self.key, &self.request_mac, response.body),
        ) {
            Ok(())
        } else {
            Err(AuthError::BadMac)
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Session {
    id: [u8; 16],
    next_counter: u64,
    started_ms: u64,
}

struct CachedKeys {
    keys: PairKeys,
    used_ms: u64,
}

/// A sender's replay state, and the network it was first kept for.
struct Kept {
    guard: Guard,
    network: Option<Network>,
}

/// What accepting a request records.
#[derive(Clone, Copy, Debug)]
enum Record {
    /// The request, in the sender's state: kept already, or new, taking
    /// the place of the idle stranger named, if any; and whether it only
    /// reads.
    Into(replay::Slot, Option<NodeId>, bool),
    /// Nothing: a request that only reads, from a sender whose state is
    /// not kept for good.
    Nothing,
}

/// The authentication state of a node: its key, the keys it shares with
/// others, their replay state, and its own sessions.
pub struct Authenticator {
    key: Arc<NodeKey>,
    own: NodeId,
    config: AuthConfig,
    keys: BTreeMap<NodeId, CachedKeys>,
    guards: BTreeMap<NodeId, Kept>,
    /// Senders whose replay state is kept for good: peers, bootstrap nodes,
    /// nodes we hold a settlement channel with, and nodes registered with
    /// us. Everyone else, a stranger, shares `max_senders` places, at most
    /// `max_senders_per_network` of them from one network, so a flood of
    /// new identities cannot crowd out the nodes that matter.
    pinned: BTreeSet<NodeId>,
    /// Pinned senders let go: their state is kept outside the pool until
    /// every message accepted from them is stale, and then forgotten. One
    /// that sends more meanwhile is recorded only by taking a stranger's
    /// place, so letting go never grows what is kept.
    releasing: BTreeSet<NodeId>,
    sessions: BTreeMap<NodeId, Session>,
    /// The floor of senders without one of their own: the start time.
    default_floor_ms: u64,
    /// Pinned senders whose newest timestamp moved for a request that
    /// does not only read, or that were pinned, since the last
    /// [`take_floors`](Self::take_floors): their floors are to be
    /// persisted. Each has replay state.
    floors_dirty: BTreeSet<NodeId>,
    /// Strangers that were let go with a floor persisted, and took a place
    /// in the pool before it was stale: their floors go with their places.
    /// At most `max_senders`, since each holds a place.
    placed_with_floor: BTreeSet<NodeId>,
    /// Senders forgotten since then, whose persisted floors protect
    /// nothing any more.
    forgotten: Vec<NodeId>,
}

impl Authenticator {
    /// Starts with no state: nothing at or before `started_ms` is accepted
    /// from anyone without a floor restored by [`restore_floor`](Self::restore_floor).
    #[must_use]
    pub fn new(key: Arc<NodeKey>, config: AuthConfig, started_ms: u64) -> Self {
        let own = key.node_id();
        Self {
            key,
            own,
            config,
            keys: BTreeMap::new(),
            guards: BTreeMap::new(),
            pinned: BTreeSet::new(),
            releasing: BTreeSet::new(),
            sessions: BTreeMap::new(),
            default_floor_ms: started_ms,
            floors_dirty: BTreeSet::new(),
            placed_with_floor: BTreeSet::new(),
            forgotten: Vec::new(),
        }
    }

    /// The node's own ID.
    #[must_use]
    pub const fn own(&self) -> &NodeId {
        &self.own
    }

    /// The node's key.
    #[must_use]
    pub fn node_key(&self) -> &NodeKey {
        &self.key
    }

    /// Restores a sender's floor, persisted from
    /// [`take_floors`](Self::take_floors) before a restart, and pins it. A
    /// sender that no longer matters is let go after it with
    /// [`unpin`](Self::unpin), and so kept until the floor is stale.
    pub fn restore_floor(&mut self, sender: NodeId, floor_ms: u64) {
        let floor = floor_ms.max(self.default_floor_ms);
        self.pin(sender);
        self.guards.insert(
            sender,
            Kept {
                guard: Guard::new(floor),
                network: None,
            },
        );
    }

    /// Keeps `sender`'s replay state for good: a peer, a bootstrap node, a
    /// node we hold a settlement channel with, or one registered with us.
    pub fn pin(&mut self, sender: NodeId) {
        if sender != self.own {
            self.pinned.insert(sender);
            self.releasing.remove(&sender);
            self.placed_with_floor.remove(&sender);
            // A stranger pinned for what its request did, such as a
            // registration, has its floor persisted with that effect.
            if self.guards.contains_key(&sender) {
                self.floors_dirty.insert(sender);
            }
        }
    }

    /// Lets `sender`'s replay state go, as it no longer matters: it is
    /// forgotten once every message accepted from it is stale, so that
    /// forgetting it opens nothing.
    pub fn unpin(&mut self, now_ms: u64, sender: &NodeId) {
        if self.pinned.contains(sender) {
            self.releasing.insert(*sender);
        }
        self.release(now_ms);
    }

    /// Whether `sender`'s replay state is kept for good.
    #[must_use]
    pub fn is_pinned(&self, sender: &NodeId) -> bool {
        self.pinned.contains(sender) && !self.releasing.contains(sender)
    }

    /// Forgets the senders let go whose messages are all stale.
    fn release(&mut self, now_ms: u64) {
        let skew = self.config.skew_ms;
        let (guards, pinned) = (&mut self.guards, &mut self.pinned);
        let (dirty, forgotten) = (&mut self.floors_dirty, &mut self.forgotten);
        self.releasing.retain(|id| {
            let idle = guards
                .get(id)
                .is_none_or(|kept| kept.guard.is_idle(now_ms, skew));
            if idle {
                guards.remove(id);
                pinned.remove(id);
                dirty.remove(id);
                forgotten.push(*id);
            }
            !idle
        });
    }

    /// Persists `sender`'s floor with the next batch, though its newest
    /// request only read: a peer's, since a replayed read would mark it as
    /// heard from.
    pub fn keep_floor(&mut self, sender: &NodeId) {
        if self.pinned.contains(sender) && self.guards.contains_key(sender) {
            self.floors_dirty.insert(*sender);
        }
    }

    /// The floors to persist, with the effects of the messages accepted
    /// since the last call, and the senders whose persisted floors can go.
    ///
    /// The floors are those of pinned senders: every request whose effects
    /// are committed comes from one, or makes its sender one (a
    /// registration, a peering), except link states, which anyone may
    /// relay. A stranger's floor is not persisted, so strangers cannot
    /// make the node write. Nor is a floor moved by a request that only
    /// reads, whose replay is harmless, unless [`keep_floor`](Self::keep_floor)
    /// asks for it.
    pub fn take_floors(&mut self, floors: &mut Vec<(NodeId, u64)>, forgotten: &mut Vec<NodeId>) {
        for id in core::mem::take(&mut self.floors_dirty) {
            let kept = self.guards.get(&id);
            assert!(kept.is_some(), "a dirty floor has replay state");
            if let Some(kept) = kept {
                floors.push((id, kept.guard.newest_ms()));
            }
        }
        forgotten.append(&mut self.forgotten);
    }

    /// The replay states of senders that are not pinned.
    fn strangers(&self) -> impl Iterator<Item = (&NodeId, &Kept)> {
        self.guards
            .iter()
            .filter(|(id, _)| !self.pinned.contains(id))
    }

    /// The newest timestamp accepted from `sender`, which is what to
    /// persist for it.
    #[must_use]
    pub fn newest(&self, sender: &NodeId) -> Option<u64> {
        self.guards.get(sender).map(|kept| kept.guard.newest_ms())
    }

    /// Checks the authentication of a request of `kind` from `network`,
    /// where a network is counted: whether its kind needs it, and what of
    /// it to record.
    ///
    /// # Errors
    ///
    /// Why it is not accepted.
    pub fn accept(
        &mut self,
        now_ms: u64,
        envelope: &Envelope<'_>,
        kind: Kind,
        network: Option<Network>,
    ) -> Result<Accepted, AuthError> {
        if envelope.sender == self.own {
            return Err(AuthError::WrongSender);
        }
        self.release(now_ms);
        let hmac = match envelope.authentication {
            Authentication::None if kind.requires_authentication() => {
                return Err(AuthError::Missing);
            }
            Authentication::None => {
                return Ok(Accepted {
                    sender: envelope.sender,
                    key: None,
                    response: None,
                });
            }
            Authentication::Reserved(_) => return Err(AuthError::Unsupported),
            Authentication::Hmac(hmac) => hmac,
        };
        let key = PublicKey::from_bytes(&hmac.sender_key).map_err(|_| AuthError::BadKey)?;
        if key.is_weak() {
            return Err(AuthError::BadKey);
        }
        if key.node_id() != envelope.sender {
            return Err(AuthError::WrongSender);
        }
        let record = self.check_replay(now_ms, &envelope.sender, &hmac, kind, network)?;
        let keys = self.keys_for(now_ms, &envelope.sender, &key)?;
        let expected = request_mac(
            &keys.incoming_request,
            &envelope.sender,
            &self.own,
            &hmac,
            envelope.content,
        );
        if !macs_match(&expected, &hmac.mac) {
            return Err(AuthError::BadMac);
        }
        let response = (keys.incoming_response.clone(), hmac.mac);
        // A request that leaves no replay state leaves no key either, so
        // that requests from new identities cannot evict the keys of peers.
        if let Record::Into(slot, evict, reads) = record {
            self.remember_keys(now_ms, envelope.sender, keys);
            self.commit_replay(envelope.sender, slot, evict, &hmac, network, reads);
        }
        Ok(Accepted {
            sender: envelope.sender,
            key: Some(key),
            response: Some(response),
        })
    }

    fn check_replay(
        &self,
        now_ms: u64,
        sender: &NodeId,
        hmac: &HmacAuthentication,
        kind: Kind,
        network: Option<Network>,
    ) -> Result<Record, AuthError> {
        let check = |guard: &Guard| {
            guard.check(
                now_ms,
                self.config.skew_ms,
                &hmac.session_id,
                hmac.counter,
                hmac.timestamp_ms,
            )
        };
        let pinned = self.is_pinned(sender);
        // A request that only reads changes nothing, and gets nothing a
        // replay of it could abuse. From a stranger it is checked and not
        // recorded: it neither takes a place nor keeps one fresh.
        let only_reads = kind.is_read_only() && !pinned;
        let slot = match self.guards.get(sender) {
            Some(kept) => check(&kept.guard)?,
            None => check(&Guard::new(self.default_floor_ms))?,
        };
        if only_reads {
            return Ok(Record::Nothing);
        }
        // A stranger with a place keeps it; a new one, or one let go, needs
        // one.
        let placed = self.guards.contains_key(sender) && !self.releasing.contains(sender);
        let evict = if pinned || placed {
            None
        } else {
            self.place_for(now_ms, network)?
        };
        Ok(Record::Into(slot, evict, kind.is_read_only()))
    }

    /// Room for one more stranger from `network`: free, or the place of an
    /// idle stranger, whose messages are all stale so that forgetting it
    /// opens nothing. Once the network has its share, only one of its own
    /// gives up a place.
    fn place_for(
        &self,
        now_ms: u64,
        network: Option<Network>,
    ) -> Result<Option<NodeId>, AuthError> {
        let (mut total, mut from_network) = (0, 0);
        for (_, kept) in self.strangers() {
            total += 1;
            if network.is_some() && kept.network == network {
                from_network += 1;
            }
        }
        let network_full = from_network >= self.config.max_senders_per_network;
        if !network_full && total < self.config.max_senders {
            return Ok(None);
        }
        self.strangers()
            .find(|(_, kept)| {
                (!network_full || kept.network == network)
                    && kept.guard.is_idle(now_ms, self.config.skew_ms)
            })
            .map(|(id, _)| Some(*id))
            .ok_or(AuthError::Busy)
    }

    fn commit_replay(
        &mut self,
        sender: NodeId,
        slot: replay::Slot,
        evict: Option<NodeId>,
        hmac: &HmacAuthentication,
        network: Option<Network>,
        reads: bool,
    ) {
        if let Some(idle) = evict {
            self.guards.remove(&idle);
            self.floors_dirty.remove(&idle);
            // Idle, it protects nothing: its persisted floor goes too.
            if self.placed_with_floor.remove(&idle) {
                self.forgotten.push(idle);
            }
        }
        let default = self.default_floor_ms;
        let kept = self.guards.entry(sender).or_insert_with(|| Kept {
            guard: Guard::new(default),
            network,
        });
        // A sender let go that was given a place joins the strangers.
        if self.releasing.remove(&sender) {
            self.pinned.remove(&sender);
            self.placed_with_floor.insert(sender);
            kept.network = network;
        }
        kept.guard
            .commit(slot, &hmac.session_id, hmac.counter, hmac.timestamp_ms);
        if self.pinned.contains(&sender) && !reads {
            self.floors_dirty.insert(sender);
        }
    }

    /// The keys shared with `peer`, from the cache or derived.
    fn keys_for(
        &mut self,
        now_ms: u64,
        peer: &NodeId,
        key: &PublicKey,
    ) -> Result<PairKeys, AuthError> {
        if let Some(cached) = self.keys.get_mut(peer)
            && cached.keys.peer == *key
        {
            cached.used_ms = now_ms;
            return Ok(cached.keys.clone());
        }
        PairKeys::derive(&self.key, key).map_err(|_| AuthError::BadKey)
    }

    fn remember_keys(&mut self, now_ms: u64, peer: NodeId, keys: PairKeys) {
        if !self.keys.contains_key(&peer) && self.keys.len() >= self.config.max_keys {
            let oldest = self
                .keys
                .iter()
                .min_by_key(|(_, cached)| cached.used_ms)
                .map(|(id, _)| *id);
            if let Some(oldest) = oldest {
                self.keys.remove(&oldest);
            }
        }
        self.keys.insert(
            peer,
            CachedKeys {
                keys,
                used_ms: now_ms,
            },
        );
    }

    /// Authenticates a request with `content` to `recipient`, whose key we
    /// know from its link state or the bootstrap list.
    ///
    /// # Errors
    ///
    /// The recipient's key is unusable.
    pub fn authenticate(
        &mut self,
        now_ms: u64,
        rng: &mut dyn CryptoRng,
        recipient: &NodeId,
        recipient_key: &PublicKey,
        content: &[u8],
    ) -> Result<(HmacAuthentication, Pending), AuthError> {
        let keys = self.keys_for(now_ms, recipient, recipient_key)?;
        let session = self.session_for(now_ms, rng, recipient);
        let mut hmac = HmacAuthentication {
            sender_key: *self.key.public_key().as_bytes(),
            session_id: session.id,
            counter: session.next_counter,
            timestamp_ms: now_ms,
            mac: [0; 32],
        };
        hmac.mac = request_mac(&keys.outgoing_request, &self.own, recipient, &hmac, content);
        let pending = Pending {
            key: keys.outgoing_response.clone(),
            request_mac: hmac.mac,
        };
        self.remember_keys(now_ms, *recipient, keys);
        Ok((hmac, pending))
    }

    /// Our session with `recipient`, counting the message about to be sent.
    fn session_for(&mut self, now_ms: u64, rng: &mut dyn CryptoRng, recipient: &NodeId) -> Session {
        let expired = self.sessions.get(recipient).is_none_or(|session| {
            session
                .started_ms
                .saturating_add(self.config.session_lifetime_ms)
                <= now_ms
        });
        if expired {
            if !self.sessions.contains_key(recipient)
                && self.sessions.len() >= self.config.max_senders
            {
                let oldest = self
                    .sessions
                    .iter()
                    .min_by_key(|(_, session)| session.started_ms)
                    .map(|(id, _)| *id);
                if let Some(oldest) = oldest {
                    self.sessions.remove(&oldest);
                }
            }
            self.sessions.remove(recipient);
        }
        let session = self.sessions.entry(*recipient).or_insert_with(|| {
            let mut id = [0u8; 16];
            rng.fill_bytes(&mut id);
            Session {
                id,
                next_counter: 0,
                started_ms: now_ms,
            }
        });
        let current = *session;
        session.next_counter = session.next_counter.saturating_add(1);
        current
    }

    /// Checks the bounds of every table (spec/decisions/tigerstyle.md).
    pub fn check_invariants(&self) {
        assert!(
            self.keys.len() <= self.config.max_keys,
            "key cache within bounds"
        );
        assert!(
            self.strangers().count() <= self.config.max_senders,
            "replay state within bounds"
        );
        let mut networks = BTreeMap::new();
        for network in self.strangers().filter_map(|(_, kept)| kept.network) {
            let count = networks.entry(network).or_insert(0_usize);
            *count += 1;
            assert!(
                *count <= self.config.max_senders_per_network,
                "replay state per network within bounds"
            );
        }
        assert!(
            self.releasing.is_subset(&self.pinned),
            "only pinned senders are let go"
        );
        assert!(!self.pinned.contains(&self.own), "we are not pinned");
        assert!(
            self.sessions.len() <= self.config.max_senders,
            "sessions within bounds"
        );
        assert!(
            !self.guards.contains_key(&self.own),
            "no replay state for ourselves"
        );
        assert!(
            self.floors_dirty
                .iter()
                .all(|id| self.guards.contains_key(id)),
            "a floor to persist has replay state"
        );
        assert!(
            self.placed_with_floor
                .iter()
                .all(|id| self.guards.contains_key(id) && !self.pinned.contains(id)),
            "a stranger with a floor holds a place"
        );
    }
}
