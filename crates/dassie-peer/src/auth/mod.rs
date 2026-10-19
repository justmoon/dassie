//! Message authentication, v1 (D2, spec/areas/peer-protocol.md#keys-and-macs).
//!
//! A request names its sender's key, which must hash to the sender's node
//! ID; the MAC key comes from the two nodes' static keys; the MAC covers
//! the version, both node IDs, the session, counter and timestamp, and the
//! content. Responses are MACed under their own key and bound to the
//! request. Every table here is bounded, and a key is cached only once a
//! MAC made with it has verified (NET-9).

mod keys;
mod replay;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;

use dassie_identity::{NodeId, NodeKey, PublicKey};
use rand_core::CryptoRng;

pub use keys::{MacKey, PairKeys, macs_match, request_mac, response_mac};
pub use replay::{Guard, ReplayError, WINDOW};

use crate::wire::{Authentication, Envelope, HmacAuthentication, ResponseEnvelope};

/// The bounds and windows of authentication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthConfig {
    /// How far a timestamp may be from now, either way.
    pub skew_ms: u64,
    /// Most senders whose replay state is kept, and most destinations with
    /// a session of ours.
    pub max_senders: usize,
    /// Most pairs of keys cached.
    pub max_keys: usize,
    /// How long one of our sessions lasts before a new one starts.
    pub session_lifetime_ms: u64,
}

impl AuthConfig {
    /// The values of the spec (spec/areas/peer-protocol.md#sessions,
    /// spec/areas/peer-protocol.md#replay-protection).
    pub const DEFAULT: Self = Self {
        skew_ms: 60_000,
        max_senders: 1024,
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

/// The authentication state of a node: its key, the keys it shares with
/// others, their replay state, and its own sessions.
pub struct Authenticator {
    key: Arc<NodeKey>,
    own: NodeId,
    config: AuthConfig,
    keys: BTreeMap<NodeId, CachedKeys>,
    guards: BTreeMap<NodeId, Guard>,
    /// Senders whose replay state is never given up: peers and bootstrap
    /// nodes. Everyone else shares `max_senders` places, so a flood of new
    /// identities cannot crowd out the nodes that matter.
    pinned: BTreeSet<NodeId>,
    sessions: BTreeMap<NodeId, Session>,
    /// The floor of senders without one of their own: the start time.
    default_floor_ms: u64,
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
            sessions: BTreeMap::new(),
            default_floor_ms: started_ms,
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

    /// Restores a peer's floor, persisted from [`newest`](Self::newest)
    /// before a restart, and pins it.
    pub fn restore_floor(&mut self, sender: NodeId, floor_ms: u64) {
        let floor = floor_ms.max(self.default_floor_ms);
        self.pinned.insert(sender);
        self.guards.insert(sender, Guard::new(floor));
    }

    /// Keeps `sender`'s replay state for good: a peer or a bootstrap node.
    pub fn pin(&mut self, sender: NodeId) {
        if sender != self.own {
            self.pinned.insert(sender);
        }
    }

    /// Replay states of senders that are not pinned.
    fn strangers(&self) -> usize {
        self.guards
            .keys()
            .filter(|id| !self.pinned.contains(id))
            .count()
    }

    /// The newest timestamp accepted from `sender`, which is what to
    /// persist for it.
    #[must_use]
    pub fn newest(&self, sender: &NodeId) -> Option<u64> {
        self.guards.get(sender).map(Guard::newest_ms)
    }

    /// Checks a request's authentication. `required` says whether its kind
    /// needs it.
    ///
    /// # Errors
    ///
    /// Why it is not accepted.
    pub fn accept(
        &mut self,
        now_ms: u64,
        envelope: &Envelope<'_>,
        required: bool,
    ) -> Result<Accepted, AuthError> {
        if envelope.sender == self.own {
            return Err(AuthError::WrongSender);
        }
        let hmac = match envelope.authentication {
            Authentication::None if required => return Err(AuthError::Missing),
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
        let slot = self.check_replay(now_ms, &envelope.sender, &hmac)?;
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
        self.remember_keys(now_ms, envelope.sender, keys);
        self.commit_replay(now_ms, envelope.sender, slot, &hmac);
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
    ) -> Result<replay::Slot, AuthError> {
        let fresh;
        let guard = match self.guards.get(sender) {
            Some(guard) => guard,
            None => {
                if !self.pinned.contains(sender)
                    && self.strangers() >= self.config.max_senders
                    && self.idle_guard(now_ms).is_none()
                {
                    return Err(AuthError::Busy);
                }
                fresh = Guard::new(self.default_floor_ms);
                &fresh
            }
        };
        Ok(guard.check(
            now_ms,
            self.config.skew_ms,
            &hmac.session_id,
            hmac.counter,
            hmac.timestamp_ms,
        )?)
    }

    fn commit_replay(
        &mut self,
        now_ms: u64,
        sender: NodeId,
        slot: replay::Slot,
        hmac: &HmacAuthentication,
    ) {
        if !self.guards.contains_key(&sender)
            && !self.pinned.contains(&sender)
            && self.strangers() >= self.config.max_senders
        {
            // Its messages are all stale: forgetting it opens nothing.
            if let Some(idle) = self.idle_guard(now_ms) {
                self.guards.remove(&idle);
            }
        }
        let default = self.default_floor_ms;
        self.guards
            .entry(sender)
            .or_insert_with(|| Guard::new(default))
            .commit(slot, &hmac.session_id, hmac.counter, hmac.timestamp_ms);
    }

    fn idle_guard(&self, now_ms: u64) -> Option<NodeId> {
        self.guards
            .iter()
            .find(|(id, guard)| {
                !self.pinned.contains(id) && guard.is_idle(now_ms, self.config.skew_ms)
            })
            .map(|(id, _)| *id)
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

    /// Checks the bounds of every table (D16).
    pub fn check_invariants(&self) {
        assert!(
            self.keys.len() <= self.config.max_keys,
            "key cache within bounds"
        );
        assert!(
            self.strangers() <= self.config.max_senders,
            "replay state within bounds"
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
    }
}
