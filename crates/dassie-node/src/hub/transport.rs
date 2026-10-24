//! The hub's stream (spec/areas/peer-protocol.md#the-hubs-stream): how
//! validators and members exchange the hub's messages, over the node's
//! QUIC endpoint, or `POST /hub` for members whose QUIC cannot reach a
//! validator.
//!
//! Every exchange is a request and its reply. The request names the hub and
//! carries one message (`dassie_hub::service::wire::Message`); the reply
//! carries what the validator sent back while it handled it, in order, and
//! nothing else: the statements answering a member's request, a consensus
//! answer to a validator's fetch, or nothing at all. A validator's messages
//! to the others are exchanges whose reply is usually empty. Nothing is
//! pushed: a validator reaches a member only in the reply to its request,
//! so a member behind a network that takes no connections is served alike.
//!
//! ```text
//! request = hub ID (32 bytes) || message
//! reply   = status (1 byte) || frames          (over QUIC)
//! frames  = *(length (4 bytes, big-endian) || message)
//! ```
//!
//! Over HTTPS the request is the body of `POST /hub`, and the reply is the
//! frames with a status of 200, or 404, 503, 400 or 429 for the statuses
//! below.
//!
//! A validator far behind fetches the state after a final block from
//! another on a stream of its own, over QUIC only, which serves the hub's
//! validators alone, with a bound of its own: a state can be far larger
//! than a message.
//!
//! ```text
//! state request = hub ID (32 bytes) || height (8 bytes, big-endian)
//! state reply   = status (1 byte) || state     (the state only with 0)
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dassie_hub::HubId;
use dassie_hub::client::MAX_HOLDS_ANSWER_BYTES;
use dassie_hub::consensus::PeerId;
use dassie_hub::digest::Digest;
use dassie_hub::ledger::Phase;
use dassie_hub::service::MAX_SINCE_ANSWER_BYTES;
use dassie_hub::service::wire::{ENVELOPE, MAX_MEMBER_MESSAGE, MAX_MESSAGE, Message};
use dassie_identity::{NodeId, PublicKey};
use dassie_node_core::rate_limit::{ClientKey, RateLimitPolicy, RateLimiter};
use sha2::{Digest as _, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::node::Node;
use crate::peer_client::{PeerClient, WebError, WebRequest};
use crate::quic::Quic;
use crate::store::Store;

/// The bytes of a hub's ID, ahead of each request.
pub const HUB_ID_BYTES: usize = 32;

/// The largest message on the hub's stream: its envelope and the most
/// content a message carries.
pub const MAX_MESSAGE_BYTES: usize = MAX_MESSAGE;

/// The largest request: the hub's ID and one message.
pub const MAX_REQUEST_BYTES: usize = HUB_ID_BYTES + MAX_MESSAGE_BYTES;

/// The largest message read from anyone but a validator of the hub: a
/// member's request, the largest of which submits a claim with the most
/// commitments any rules allow.
pub const MAX_MEMBER_MESSAGE_BYTES: usize = MAX_MEMBER_MESSAGE;

/// The largest request read from anyone but a validator of the hub, over
/// QUIC or as the body of `POST /hub`.
pub const MAX_MEMBER_REQUEST_BYTES: usize = HUB_ID_BYTES + MAX_MEMBER_MESSAGE_BYTES;

/// The most requests on the hub's stream and `POST /hub` from anyone but a
/// hub's validators read and waiting at once, node-wide: each holds at most
/// [`MAX_MEMBER_REQUEST_BYTES`], so together a few tens of MiB, about what
/// the peer protocol's requests may hold.
pub const MAX_MEMBER_REQUESTS: usize = 128;

/// The members one address may stand for at a validator, for the rate of
/// `POST /hub`: a few nodes behind one NAT.
const MEMBERS_PER_ADDRESS: u64 = 4;

/// The requests a member sends one validator a second at most, in
/// thousandths: a poll every `POLL_MS` while it has holds or operations
/// (one every `IDLE_POLL_MS` otherwise), its own account every
/// `ACCOUNT_EVERY_MS`, and an operation again every `RESUBMIT_MS`. Lookups
/// of peers' keys come in short bursts, which the burst covers.
const MEMBER_MILLI_REQUESTS_PER_S: u64 = 1_000_000 / dassie_hub::member::account::POLL_MS
    + 1_000_000 / dassie_hub::member::account::ACCOUNT_EVERY_MS
    + 1_000_000 / dassie_hub::member::account::RESUBMIT_MS;

/// Requests to `POST /hub` per address: [`MEMBERS_PER_ADDRESS`] members
/// polling at their full rate, with two seconds of them at once. It is a
/// limiter of its own: a member on the HTTPS fallback never takes the
/// rate, places or charges of the peer protocol from its address.
pub const HUB_RATE: RateLimitPolicy = RateLimitPolicy {
    client_burst: HUB_BURST,
    client_refill_ms: 1_000_000 / (MEMBERS_PER_ADDRESS * MEMBER_MILLI_REQUESTS_PER_S),
    client_slots: 1024,
};

/// Two seconds of [`HUB_RATE`].
const HUB_BURST: u32 = 92;

const _: () =
    assert!(HUB_BURST as u64 == MEMBERS_PER_ADDRESS * MEMBER_MILLI_REQUESTS_PER_S * 2 / 1_000);
const _: () = assert!(HUB_RATE.client_refill_ms > 0);

/// The most requests from hubs' validators read and waiting at once,
/// node-wide: each holds at most [`MAX_REQUEST_BYTES`], a consensus
/// message of up to 4 MiB, so together at most 64 MiB. A hub has at most
/// 16 validators, which send one message per exchange.
pub const MAX_VALIDATOR_REQUESTS: usize = 16;

/// The most messages one reply carries: an answer to `Since` takes a few,
/// one to `Holds` a page each, and a validator's answer to another's fetch
/// one.
pub const MAX_REPLY_MESSAGES: usize = 256;

/// The largest reply read: the largest message, or the longest answer a
/// validator gives a member, whichever is larger, with every frame's
/// length and envelope.
pub const MAX_REPLY_BYTES: usize = 2 * MAX_MESSAGE_BYTES;

/// The bytes of a request for a state: the hub's ID and a height.
pub const STATE_REQUEST_BYTES: usize = HUB_ID_BYTES + 8;

/// The largest reply to a request for a state: its status and the largest
/// state the node stores.
pub const MAX_STATE_REPLY_BYTES: usize = 1 + crate::store::MAX_HUB_LEDGER_BYTES;

/// The media type of `POST /hub`'s body and of its reply.
pub const MEDIA_TYPE: &str = "application/dassie-hub";

/// The longest a request waits for the validator's reply, in its queue and
/// in its step.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// The most messages waiting for one hub's validator: past it, a request
/// is answered busy, and a validator's message dropped, as a network drops
/// one (consensus sends again).
pub const INBOX: usize = 256;

const FRAME_LENGTH_BYTES: usize = 4;

// One message of the largest size fits in a reply, with its status, and
// so does a validator's longest answer to a member, framed.
const FRAMING: usize = MAX_REPLY_MESSAGES * (FRAME_LENGTH_BYTES + ENVELOPE);
const _: () = assert!(1 + FRAME_LENGTH_BYTES + MAX_MESSAGE_BYTES <= MAX_REPLY_BYTES);
const _: () = assert!(1 + FRAMING + MAX_SINCE_ANSWER_BYTES <= MAX_REPLY_BYTES);
const _: () = assert!(1 + FRAMING + MAX_HOLDS_ANSWER_BYTES <= MAX_REPLY_BYTES);

/// How a validator answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubAnswer {
    /// It took the message; what it sent back, each a message.
    Taken(Vec<Vec<u8>>),
    /// The node validates no hub of that ID.
    UnknownHub,
    /// Its validator's queue is full: nothing was taken.
    Busy,
    /// The request is no hub request, or not one this transport takes.
    Malformed,
}

impl HubAnswer {
    const fn status(&self) -> u8 {
        match self {
            Self::Taken(_) => 0,
            Self::UnknownHub => 1,
            Self::Busy => 2,
            Self::Malformed => 3,
        }
    }

    /// The reply as the QUIC stream carries it: the status, then the frames.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![self.status()];
        if let Self::Taken(messages) = self {
            bytes.extend_from_slice(&encode_frames(messages));
        }
        bytes
    }

    /// Reads a reply from the QUIC stream.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (status, rest) = bytes.split_first()?;
        match status {
            0 => Some(Self::Taken(decode_frames(rest)?)),
            1..=3 if rest.is_empty() => Some(match status {
                1 => Self::UnknownHub,
                2 => Self::Busy,
                _ => Self::Malformed,
            }),
            _ => None,
        }
    }
}

/// A request: the hub's ID, then the message.
#[must_use]
pub fn encode_request(hub: &HubId, message: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HUB_ID_BYTES + message.len());
    bytes.extend_from_slice(hub.0.as_bytes());
    bytes.extend_from_slice(message);
    bytes
}

/// The hub and the message of a request, if it is one.
#[must_use]
pub fn decode_request(bytes: &[u8]) -> Option<(HubId, &[u8])> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return None;
    }
    let (hub, message) = bytes.split_first_chunk::<HUB_ID_BYTES>()?;
    (!message.is_empty()).then_some((HubId(Digest(*hub)), message))
}

/// Messages, each with its length ahead of it.
#[must_use]
pub fn encode_frames(messages: &[Vec<u8>]) -> Vec<u8> {
    let total = messages
        .iter()
        .map(|message| FRAME_LENGTH_BYTES.saturating_add(message.len()))
        .sum();
    let mut bytes = Vec::with_capacity(total);
    for message in messages {
        let length = u32::try_from(message.len()).unwrap_or(u32::MAX);
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(message);
    }
    bytes
}

/// The messages of `bytes`, if they are frames that fill it, at most
/// [`MAX_REPLY_MESSAGES`], none empty or over [`MAX_MESSAGE_BYTES`].
#[must_use]
pub fn decode_frames(mut bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut messages = Vec::new();
    while !bytes.is_empty() {
        if messages.len() >= MAX_REPLY_MESSAGES {
            return None;
        }
        let (length, rest) = bytes.split_first_chunk::<FRAME_LENGTH_BYTES>()?;
        let length = usize::try_from(u32::from_be_bytes(*length)).ok()?;
        if length == 0 || length > MAX_MESSAGE_BYTES {
            return None;
        }
        let (message, rest) = rest.split_at_checked(length)?;
        messages.push(message.to_vec());
        bytes = rest;
    }
    Some(messages)
}

/// Keeps the messages of a reply within its bounds: those past
/// [`MAX_REPLY_MESSAGES`] or [`MAX_REPLY_BYTES`] are left out, and the
/// asker asks again.
#[must_use]
pub fn bounded(messages: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut total = 1_usize;
    messages
        .into_iter()
        .filter(|message| !message.is_empty() && message.len() <= MAX_MESSAGE_BYTES)
        .take(MAX_REPLY_MESSAGES)
        .take_while(|message| {
            total = total.saturating_add(FRAME_LENGTH_BYTES.saturating_add(message.len()));
            total <= MAX_REPLY_BYTES
        })
        .collect()
}

/// Who a sender is to a hub, by the key its connection proved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubAccess {
    /// The node validates no hub of that ID.
    Unknown,
    /// The hub is the node's, and the sender none of its validators: a
    /// member, or anyone.
    Member,
    /// A validator of a set of the hub the node knows.
    Validator,
}

/// Who sent a message, as the transport proved it. Taken apart at once:
/// boxing would only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sender {
    /// The node whose key the QUIC connection proved.
    Node(PublicKey),
    /// A client of `POST /hub`, which proves no key: its address.
    Address(IpAddr),
}

impl Sender {
    /// Its handle for the validator: the same for every message from the
    /// same node, or address, so its requests find their place again. A
    /// handle names nobody: a statement is signed whoever it goes to.
    #[must_use]
    pub fn peer(&self) -> PeerId {
        let mut hash = Sha256::new();
        match self {
            Self::Node(node) => {
                hash.update(b"dassie hub node\0");
                hash.update(node.node_id().as_str().as_bytes());
            }
            Self::Address(address) => {
                hash.update(b"dassie hub address\0");
                hash.update(address.to_canonical().to_string().as_bytes());
            }
        }
        let digest: [u8; 32] = hash.finalize().into();
        let mut first = [0_u8; 8];
        first.copy_from_slice(digest.get(..8).unwrap_or(&[0; 8]));
        PeerId(u64::from_be_bytes(first))
    }
}

/// A message for a hub's validator, and where its reply goes, if anyone
/// waits for one.
#[derive(Debug)]
pub struct Inbound {
    /// Who sent it.
    pub from: PeerId,
    /// The node key the transport proved it by, if any: only a validator
    /// is served catch-up.
    pub node: Option<PublicKey>,
    /// The message.
    pub message: Vec<u8>,
    /// Where the reply goes.
    pub reply: Option<oneshot::Sender<Vec<Vec<u8>>>>,
}

/// What a validator's owner sees of it (spec/areas/hubs.md#the-validator-in-the-node).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubStatus {
    /// The hub.
    pub hub: HubId,
    /// Our index in the set in force, if we are in it.
    pub index: Option<usize>,
    /// How many validators the set in force has.
    pub validators: usize,
    /// The height of the last final block.
    pub height: u64,
    /// Its timestamp.
    pub timestamp_ms: u64,
    /// The view consensus is in.
    pub view: u64,
    /// Where the hub is in its life.
    pub phase: Phase,
    /// Whether we declared the hub stalled: we never vote in it again.
    pub stalled: bool,
    /// Whether a quorum checked the escrow, so deposits are credited.
    pub escrow_checked: bool,
    /// Whether the node follows the escrow on its XRPL server now.
    pub xrpl_connected: bool,
    /// How often it took a peer's state since it started, being further
    /// behind than peers keep blocks.
    pub caught_up: u64,
}

/// Where what arrives for one hub goes: what its validators send to a
/// queue of its own, which members cannot fill, and everything else to
/// another.
#[derive(Clone, Debug)]
pub struct Route {
    /// The hub's other validators' messages.
    pub validators: mpsc::Sender<Inbound>,
    /// Everyone else's.
    pub members: mpsc::Sender<Inbound>,
    /// The nodes of the hub's validators, which the connection proves.
    pub nodes: BTreeSet<NodeId>,
}

/// The most exchanges one of the node's hub accounts has in flight with
/// one validator: what one step of the account asks a validator at once
/// (its account, what was executed since its cursor, an operation, a
/// lookup); while a validator is slow, a poll beyond it is skipped, and
/// the account asks again. So the exchanges in flight are at most this
/// many times the accounts times their validators, and a busy account
/// never takes the room of another.
pub const MAX_EXCHANGES_PER_VALIDATOR: u8 = 4;

/// The exchanges in flight, by account and validator.
type InFlight = Arc<Mutex<BTreeMap<(String, usize), u8>>>;

/// A place for one exchange of an account's with a validator, given back
/// when it drops.
#[derive(Debug)]
pub struct ExchangeSlot {
    in_flight: InFlight,
    key: (String, usize),
}

impl Drop for ExchangeSlot {
    fn drop(&mut self) {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = in_flight.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_flight.remove(&self.key);
            }
        }
    }
}

/// The hubs the node validates: where what arrives for each goes, and how
/// each stands; and the places for its own exchanges as a member.
#[derive(Debug)]
pub struct Hubs {
    routes: Mutex<BTreeMap<HubId, Route>>,
    status: Mutex<BTreeMap<HubId, HubStatus>>,
    /// The node's own exchanges as a member, by account and validator.
    exchanges: InFlight,
    /// Places for requests from members, and from validators.
    incoming_members: Arc<tokio::sync::Semaphore>,
    incoming_validators: Arc<tokio::sync::Semaphore>,
    /// The rate of `POST /hub` per address ([`HUB_RATE`]).
    limiter: Mutex<RateLimiter>,
    /// Places for states being served ([`MAX_STATE_SERVES`]).
    state_serves: Arc<tokio::sync::Semaphore>,
}

impl Default for Hubs {
    fn default() -> Self {
        Self {
            routes: Mutex::default(),
            status: Mutex::default(),
            exchanges: Arc::default(),
            incoming_members: Arc::new(tokio::sync::Semaphore::new(MAX_MEMBER_REQUESTS)),
            incoming_validators: Arc::new(tokio::sync::Semaphore::new(MAX_VALIDATOR_REQUESTS)),
            limiter: Mutex::new(RateLimiter::new(HUB_RATE)),
            state_serves: Arc::new(tokio::sync::Semaphore::new(MAX_STATE_SERVES)),
        }
    }
}

impl Hubs {
    /// A place for one exchange of the account `asset` with its validator
    /// at `endpoint`, if it has fewer than [`MAX_EXCHANGES_PER_VALIDATOR`]
    /// in flight there.
    #[must_use]
    pub fn exchange_slot(&self, asset: &str, endpoint: usize) -> Option<ExchangeSlot> {
        let key = (asset.to_string(), endpoint);
        let mut in_flight = self
            .exchanges
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let count = in_flight.entry(key.clone()).or_insert(0);
        if *count >= MAX_EXCHANGES_PER_VALIDATOR {
            return None;
        }
        *count = count.saturating_add(1);
        Some(ExchangeSlot {
            in_flight: Arc::clone(&self.exchanges),
            key,
        })
    }

    /// Charges `client` for a request to `POST /hub` at `now`: whether it
    /// may make one ([`HUB_RATE`]).
    pub fn charge(&self, client: IpAddr, now_ms: u64) -> bool {
        self.limiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .check(ClientKey::from_ip(client), now_ms)
            .is_ok()
    }

    /// Who `from` is to the hub named `hub`: whether the node validates it,
    /// and whether `from` is one of its validators.
    #[must_use]
    pub fn access(&self, from: Option<&PublicKey>, hub: &[u8; 32]) -> HubAccess {
        let routes = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(route) = routes.get(&HubId(Digest(*hub))) else {
            return HubAccess::Unknown;
        };
        match from {
            Some(key) if route.nodes.contains(&key.node_id()) => HubAccess::Validator,
            _ => HubAccess::Member,
        }
    }

    /// A place for one incoming request from a sender of `access`, if one
    /// is free: [`MAX_MEMBER_REQUESTS`] or [`MAX_VALIDATOR_REQUESTS`].
    #[must_use]
    pub fn request_permit(&self, access: HubAccess) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let places = match access {
            HubAccess::Unknown => return None,
            HubAccess::Member => &self.incoming_members,
            HubAccess::Validator => &self.incoming_validators,
        };
        Arc::clone(places).try_acquire_owned().ok()
    }

    /// Messages for `hub` go along `route` from now on.
    pub fn register(&self, hub: HubId, route: Route) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(hub, route);
    }

    /// The nodes of `hub`'s validators are `nodes` now.
    pub fn set_validators(&self, hub: &HubId, nodes: BTreeSet<NodeId>) {
        if let Some(route) = self
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(hub)
        {
            route.nodes = nodes;
        }
    }

    /// The node no longer takes messages for `hub`, nor reports it.
    pub fn unregister(&self, hub: &HubId) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(hub);
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(hub);
    }

    /// How a hub's validator stands now.
    pub fn set_status(&self, status: HubStatus) {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(status.hub, status);
    }

    /// How each hub's validator stands, by hub.
    #[must_use]
    pub fn statuses(&self) -> Vec<HubStatus> {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Hands `request` from `from` to its hub's validator and waits for the
    /// reply. With `requests_only`, as over HTTPS, which validators never
    /// use, only a member's request is taken.
    pub async fn answer(&self, from: Sender, request: &[u8], requests_only: bool) -> HubAnswer {
        let Some((hub, message)) = decode_request(request) else {
            return HubAnswer::Malformed;
        };
        // Which hub, and who asks, before anything of the message is read:
        // only a validator of the hub sends more than a member's request.
        let route = self
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&hub)
            .map(|route| match from {
                Sender::Node(node) if route.nodes.contains(&node.node_id()) => {
                    (HubAccess::Validator, route.validators.clone())
                }
                Sender::Node(_) | Sender::Address(_) => (HubAccess::Member, route.members.clone()),
            });
        let Some((access, inbox)) = route else {
            return HubAnswer::UnknownHub;
        };
        if access == HubAccess::Member && message.len() > MAX_MEMBER_MESSAGE_BYTES {
            return HubAnswer::Malformed;
        }
        match Message::decode_exact(message) {
            Ok(Message::Request(_)) => {}
            Ok(_) if !requests_only => {}
            _ => return HubAnswer::Malformed,
        }
        let (reply, replied) = oneshot::channel();
        let inbound = Inbound {
            from: from.peer(),
            node: match from {
                Sender::Node(node) => Some(node),
                Sender::Address(_) => None,
            },
            message: message.to_vec(),
            reply: Some(reply),
        };
        if inbox.try_send(inbound).is_err() {
            return HubAnswer::Busy;
        }
        match tokio::time::timeout(REPLY_TIMEOUT, replied).await {
            Ok(Ok(messages)) => HubAnswer::Taken(messages),
            // The validator stopped, or took too long: nothing it did is
            // known to have reached the asker.
            Ok(Err(_)) | Err(_) => HubAnswer::Busy,
        }
    }

    /// Answers a request for the state after one of a hub's final blocks
    /// from `from`, if it is a validator of the hub, with the state if
    /// `store` keeps it.
    #[must_use]
    pub async fn answer_state(
        &self,
        store: Arc<Store>,
        from: &PublicKey,
        request: &[u8],
    ) -> StateReply {
        let status = |status: StateStatus| StateReply {
            bytes: vec![status as u8],
            _place: None,
        };
        let Some((hub, height)) = decode_state_request(request) else {
            return status(StateStatus::Refused);
        };
        let validator = self
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&hub)
            .map(|route| route.nodes.contains(&from.node_id()));
        match validator {
            None => return status(StateStatus::UnknownHub),
            Some(false) => return status(StateStatus::Refused),
            Some(true) => {}
        }
        // A state may be hundreds of MiB: few are read and held at once, off
        // the runtime, and the place goes only once the reply is written.
        let Ok(place) = Arc::clone(&self.state_serves).try_acquire_owned() else {
            return status(StateStatus::Busy);
        };
        let read = tokio::task::spawn_blocking(move || store.hub_state(hub.0.as_bytes(), height));
        match read.await {
            Ok(Ok(Some(state))) => {
                let mut bytes = Vec::with_capacity(1 + state.len());
                bytes.push(StateStatus::Found as u8);
                bytes.extend_from_slice(&state);
                StateReply {
                    bytes,
                    _place: Some(place),
                }
            }
            Ok(Ok(None) | Err(_)) | Err(_) => status(StateStatus::NotKept),
        }
    }
}

/// The most states a node reads and serves at once: a state may be
/// hundreds of MiB, and a validator far behind needs one.
pub const MAX_STATE_SERVES: usize = 2;

/// A reply to a request for a state, holding its place among
/// [`MAX_STATE_SERVES`] until it is written.
#[derive(Debug)]
pub struct StateReply {
    /// The reply's bytes.
    pub bytes: Vec<u8>,
    _place: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// How a validator answered a request for a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StateStatus {
    /// The state follows.
    Found = 0,
    /// The node validates no hub of that ID.
    UnknownHub = 1,
    /// It keeps no state after the block at that height.
    NotKept = 2,
    /// The asker is no validator of the hub, or the request is broken.
    Refused = 3,
    /// It serves as many states as it does at once already.
    Busy = 4,
}

/// A request for the state of `hub` after its block at `height`.
#[must_use]
pub fn encode_state_request(hub: &HubId, height: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(STATE_REQUEST_BYTES);
    bytes.extend_from_slice(hub.0.as_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes
}

/// The hub and height of a request for a state, if it is one.
#[must_use]
pub fn decode_state_request(bytes: &[u8]) -> Option<(HubId, u64)> {
    let (hub, height) = bytes.split_first_chunk::<HUB_ID_BYTES>()?;
    let height: [u8; 8] = height.try_into().ok()?;
    Some((HubId(Digest(*hub)), u64::from_be_bytes(height)))
}

/// Why an exchange with a validator failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubExchangeError {
    /// It could not be reached, or the node has no key of its own yet:
    /// nothing was sent.
    Unreachable,
    /// The request was sent, and no reply came: it may have been taken.
    Failed(String),
    /// It answered, but took nothing: it validates no such hub, its queue
    /// is full, or the request was not one it takes (over HTTPS, only a
    /// member's request).
    Refused(HubAnswer),
    /// Too many of the node's requests are in flight to its host: nothing
    /// was sent.
    Busy,
    /// It answered with something that is no reply.
    BadReply,
    /// It keeps no state after the block asked for.
    NotKept,
}

impl fmt::Display for HubExchangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => f.write_str("the validator cannot be reached"),
            Self::Failed(error) => write!(f, "the exchange failed: {error}"),
            Self::Refused(answer) => write!(f, "the validator took nothing: {answer:?}"),
            Self::Busy => f.write_str("too many requests in flight"),
            Self::BadReply => f.write_str("the validator's reply is broken"),
            Self::NotKept => f.write_str("the validator keeps no such state"),
        }
    }
}

impl std::error::Error for HubExchangeError {}

/// How the node reaches hubs' validators: over its QUIC endpoint, pinned to
/// each validator's node key, and as a member over HTTPS when QUIC cannot
/// reach one.
#[derive(Clone)]
pub struct HubClient {
    quic: Option<Arc<Quic>>,
    client: PeerClient,
}

impl fmt::Debug for HubClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubClient")
            .field("quic", &self.quic.is_some())
            .finish_non_exhaustive()
    }
}

impl HubClient {
    /// The node's client: its QUIC endpoint, if it has one, and its peer
    /// client, which resolves names as for every request of the node's.
    #[must_use]
    pub fn of(node: &Node) -> Option<Self> {
        Some(Self::new(node.quic().cloned(), node.peer_client()?.clone()))
    }

    /// A client of `quic`, resolving names with `client`.
    #[must_use]
    pub const fn new(quic: Option<Arc<Quic>>, client: PeerClient) -> Self {
        Self { quic, client }
    }

    /// Sends `message` (a [`Message`] encoded) for `hub` to the validator
    /// whose node is `node`, at its `endpoint` (`host:port`, as its
    /// acceptance names it), and returns what it sent back, each a
    /// [`Message`] encoded, in order: for a member's `Request`, its
    /// `Statements`. Over QUIC, pinned to the validator's node key, in at
    /// most half of `timeout` for the connection; when QUIC cannot reach
    /// it, over HTTPS (`POST /hub` at the endpoint) in the time left, which
    /// a validator answers only for a member's request. A request that may
    /// have reached the validator is never sent again over HTTPS.
    ///
    /// # Errors
    ///
    /// See [`HubExchangeError`].
    pub async fn exchange(
        &self,
        node: NodeId,
        endpoint: &str,
        hub: &HubId,
        message: &[u8],
        timeout: Duration,
    ) -> Result<Vec<Vec<u8>>, HubExchangeError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let request = encode_request(hub, message);
        if let Some(quic) = &self.quic {
            let within = timeout / 2;
            let sent = quic.hub_exchange(node, endpoint, &request, &self.client, within);
            match tokio::time::timeout_at(deadline, sent).await {
                Ok(Err(HubExchangeError::Unreachable)) => {}
                Ok(result) => return result,
                Err(_) => return Err(HubExchangeError::Failed("timed out".into())),
            }
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        self.exchange_https(endpoint, request, left).await
    }

    /// As [`exchange`](Self::exchange), over QUIC only: how validators
    /// reach each other.
    ///
    /// # Errors
    ///
    /// See [`HubExchangeError`].
    pub async fn exchange_quic(
        &self,
        node: NodeId,
        endpoint: &str,
        hub: &HubId,
        message: &[u8],
        timeout: Duration,
    ) -> Result<Vec<Vec<u8>>, HubExchangeError> {
        let Some(quic) = &self.quic else {
            return Err(HubExchangeError::Unreachable);
        };
        let request = encode_request(hub, message);
        let sent = quic.hub_exchange(node, endpoint, &request, &self.client, timeout / 2);
        tokio::time::timeout(timeout, sent)
            .await
            .unwrap_or_else(|_| Err(HubExchangeError::Failed("timed out".into())))
    }

    /// Asks the validator whose node is `node`, at `endpoint`, for the
    /// state of `hub` after its block at `height`, over QUIC only: how a
    /// validator far behind catches up. The state is the hub crate's
    /// snapshot, which the asker checks against the block's root.
    ///
    /// # Errors
    ///
    /// See [`HubExchangeError`].
    pub async fn fetch_state(
        &self,
        node: NodeId,
        endpoint: &str,
        hub: &HubId,
        height: u64,
        timeout: Duration,
    ) -> Result<Vec<u8>, HubExchangeError> {
        let Some(quic) = &self.quic else {
            return Err(HubExchangeError::Unreachable);
        };
        let request = encode_state_request(hub, height);
        let sent = quic.hub_state_exchange(node, endpoint, &request, &self.client, timeout / 2);
        let reply = tokio::time::timeout(timeout, sent)
            .await
            .unwrap_or_else(|_| Err(HubExchangeError::Failed("timed out".into())))?;
        match reply.split_first() {
            Some((0, state)) => Ok(state.to_vec()),
            Some((1, [])) => Err(HubExchangeError::Refused(HubAnswer::UnknownHub)),
            Some((2, [])) => Err(HubExchangeError::NotKept),
            Some((3, [])) => Err(HubExchangeError::Refused(HubAnswer::Malformed)),
            Some((4, [])) => Err(HubExchangeError::Refused(HubAnswer::Busy)),
            _ => Err(HubExchangeError::BadReply),
        }
    }

    async fn exchange_https(
        &self,
        endpoint: &str,
        request: Vec<u8>,
        left: Duration,
    ) -> Result<Vec<Vec<u8>>, HubExchangeError> {
        let timeout_ms = u64::try_from(left.as_millis()).unwrap_or(u64::MAX);
        if timeout_ms == 0 {
            return Err(HubExchangeError::Unreachable);
        }
        let request = WebRequest {
            method: http::Method::POST,
            url: format!("https://{endpoint}/hub"),
            headers: vec![
                (http::header::CONTENT_TYPE, MEDIA_TYPE.into()),
                (http::header::ACCEPT, MEDIA_TYPE.into()),
            ],
            body: Some(request),
            timeout_ms,
            max_response_bytes: MAX_REPLY_BYTES,
        };
        let response = match self.client.web(request).await {
            Ok(response) => response,
            Err(WebError::Refused) => return Err(HubExchangeError::Unreachable),
            Err(WebError::Busy) => return Err(HubExchangeError::Busy),
            Err(error) => return Err(HubExchangeError::Failed(error.to_string())),
        };
        match response.status {
            200 => decode_frames(&response.body).ok_or(HubExchangeError::BadReply),
            404 => Err(HubExchangeError::Refused(HubAnswer::UnknownHub)),
            400 | 413 | 415 => Err(HubExchangeError::Refused(HubAnswer::Malformed)),
            429 | 503 => Err(HubExchangeError::Refused(HubAnswer::Busy)),
            status => Err(HubExchangeError::Failed(format!("status {status}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_read_back_and_refuse_what_is_not_one() {
        let messages = vec![vec![1, 2, 3], vec![4]];
        assert_eq!(decode_frames(&encode_frames(&messages)), Some(messages));
        assert_eq!(decode_frames(&[]), Some(Vec::new()));
        assert_eq!(decode_frames(&[0, 0, 0, 2, 1]), None, "cut short");
        assert_eq!(decode_frames(&[0, 0, 0, 0]), None, "an empty message");
        let too_many = vec![vec![1]; MAX_REPLY_MESSAGES + 1];
        assert_eq!(decode_frames(&encode_frames(&too_many)), None);
        assert_eq!(bounded(too_many).len(), MAX_REPLY_MESSAGES);
    }

    #[test]
    fn answers_read_back() {
        for answer in [
            HubAnswer::Taken(vec![vec![9; 5]]),
            HubAnswer::Taken(Vec::new()),
            HubAnswer::UnknownHub,
            HubAnswer::Busy,
            HubAnswer::Malformed,
        ] {
            assert_eq!(HubAnswer::decode(&answer.encode()), Some(answer));
        }
        assert_eq!(HubAnswer::decode(&[]), None);
        assert_eq!(HubAnswer::decode(&[1, 0]), None);
        assert_eq!(HubAnswer::decode(&[4]), None);
    }

    #[test]
    fn a_request_names_its_hub_and_carries_a_message() {
        let hub = HubId(Digest([7; 32]));
        let request = encode_request(&hub, &[1, 2]);
        assert_eq!(decode_request(&request), Some((hub, &[1_u8, 2][..])));
        assert_eq!(decode_request(&request[..32]), None, "no message");
        assert_eq!(decode_request(&request[..31]), None);
    }

    #[test]
    fn a_sender_keeps_its_handle() {
        let a = Sender::Address("192.0.2.1".parse().unwrap());
        let b = Sender::Address("::ffff:192.0.2.1".parse().unwrap());
        assert_eq!(a.peer(), b.peer(), "an IPv4 client is one however it came");
        let c = Sender::Address("192.0.2.2".parse().unwrap());
        assert_ne!(a.peer(), c.peer());
    }

    #[tokio::test]
    async fn requests_go_to_their_hubs_validator() {
        let hubs = Hubs::default();
        let hub = HubId(Digest([7; 32]));
        let key = dassie_identity::NodeKey::from_bytes(&[1; 32]).public_key();
        let request = dassie_hub::client::Request::Account(key);
        let message = dassie_oer::Encode::to_vec(&Message::Request(request));
        let from = Sender::Address("192.0.2.1".parse().unwrap());
        let unknown = hubs
            .answer(from, &encode_request(&hub, &message), false)
            .await;
        assert_eq!(unknown, HubAnswer::UnknownHub);
        let (members, mut queue) = mpsc::channel(1);
        let (validators, mut theirs) = mpsc::channel(1);
        let co_key = dassie_identity::NodeKey::from_bytes(&[2; 32]).public_key();
        let co_validator = co_key.node_id();
        let route = Route {
            validators,
            members,
            nodes: BTreeSet::from([co_validator]),
        };
        hubs.register(hub, route);
        let validator = tokio::spawn(async move {
            let inbound = queue.recv().await.unwrap();
            inbound.reply.unwrap().send(vec![inbound.message]).unwrap();
            let inbound = theirs.recv().await.unwrap();
            inbound.reply.unwrap().send(Vec::new()).unwrap();
        });
        let answer = hubs
            .answer(from, &encode_request(&hub, &message), true)
            .await;
        assert_eq!(answer, HubAnswer::Taken(vec![message.clone()]));
        // A co-validator's message goes to a queue members cannot fill.
        let theirs = Sender::Node(co_key);
        let answer = hubs
            .answer(theirs, &encode_request(&hub, &message), false)
            .await;
        assert_eq!(answer, HubAnswer::Taken(Vec::new()));
        validator.await.unwrap();
        let vote = dassie_oer::Encode::to_vec(&Message::Consensus(vec![1]));
        let refused = hubs.answer(from, &encode_request(&hub, &vote), true).await;
        assert_eq!(refused, HubAnswer::Malformed, "HTTPS takes only requests");
        assert_eq!(
            hubs.answer(from, &encode_request(&hub, &[0xff]), false)
                .await,
            HubAnswer::Malformed
        );
    }

    // A state goes only to a validator of its hub, only if it is kept, and
    // to no more askers at once than MAX_STATE_SERVES.
    #[tokio::test]
    async fn states_go_to_the_hubs_validators_only() {
        let hubs = Hubs::default();
        let hub = HubId(Digest([7; 32]));
        let store = Arc::new(Store::open_in_memory().unwrap());
        let step = crate::store::HubStep {
            consensus: Some(&[1]),
            ledger: Some((5, &[9, 9])),
            boundaries: &[],
        };
        store.save_hub_step(hub.0.as_bytes(), 1, step).unwrap();
        let co = dassie_identity::NodeKey::from_bytes(&[2; 32]).public_key();
        let stranger = dassie_identity::NodeKey::from_bytes(&[3; 32]).public_key();
        let request = encode_state_request(&hub, 5);
        let ask = |from, request: Vec<u8>| {
            let (hubs, store) = (&hubs, Arc::clone(&store));
            async move { hubs.answer_state(store, from, &request).await }
        };
        let unknown = ask(&co, request.clone()).await;
        assert_eq!(unknown.bytes, vec![StateStatus::UnknownHub as u8]);
        let (validators, _theirs) = mpsc::channel(1);
        let (members, _queue) = mpsc::channel(1);
        let route = Route {
            validators,
            members,
            nodes: BTreeSet::from([co.node_id()]),
        };
        hubs.register(hub, route);
        let found = ask(&co, request.clone()).await;
        assert_eq!(found.bytes, vec![0, 9, 9]);
        let refused = ask(&stranger, request.clone()).await;
        assert_eq!(refused.bytes, vec![StateStatus::Refused as u8]);
        let other = ask(&co, encode_state_request(&hub, 4)).await;
        assert_eq!(other.bytes, vec![StateStatus::NotKept as u8]);
        // While as many replies as it serves at once are being written, it
        // serves no more.
        let second = ask(&co, request.clone()).await;
        let busy = ask(&co, request.clone()).await;
        assert_eq!(busy.bytes, vec![StateStatus::Busy as u8]);
        drop((found, second));
        assert_eq!(ask(&co, request.clone()).await.bytes, vec![0, 9, 9]);
        assert_eq!(decode_state_request(&request), Some((hub, 5)));
        assert_eq!(decode_state_request(&request[1..]), None);
    }

    // An account's exchanges with a slow validator are bounded, and leave
    // the room of its other validators and of other accounts alone.
    #[test]
    fn exchanges_are_bounded_per_account_and_validator() {
        let hubs = Hubs::default();
        let busy: Vec<ExchangeSlot> = (0..MAX_EXCHANGES_PER_VALIDATOR)
            .map(|_| hubs.exchange_slot("hub-a", 0).unwrap())
            .collect();
        assert!(
            hubs.exchange_slot("hub-a", 0).is_none(),
            "a poll is skipped"
        );
        assert!(hubs.exchange_slot("hub-a", 1).is_some());
        assert!(hubs.exchange_slot("hub-b", 0).is_some());
        drop(busy);
        assert!(hubs.exchange_slot("hub-a", 0).is_some());
        assert!(hubs.exchanges.lock().unwrap().is_empty(), "nothing lingers");
    }
}
