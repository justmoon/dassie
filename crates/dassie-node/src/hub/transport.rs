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
use dassie_hub::service::wire::{ENVELOPE, MAX_MESSAGE, Message};
use dassie_identity::{NodeId, PublicKey};
use sha2::{Digest as _, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::node::Node;
use crate::peer_client::{PeerClient, WebError, WebRequest};
use crate::quic::Quic;

/// The bytes of a hub's ID, ahead of each request.
pub const HUB_ID_BYTES: usize = 32;

/// The largest message on the hub's stream: its envelope and the most
/// content a message carries.
pub const MAX_MESSAGE_BYTES: usize = MAX_MESSAGE;

/// The largest request: the hub's ID and one message.
pub const MAX_REQUEST_BYTES: usize = HUB_ID_BYTES + MAX_MESSAGE_BYTES;

/// The most messages one reply carries: an answer to `Since` takes a few,
/// one to `Holds` a page each, and a validator's answer to another's fetch
/// one.
pub const MAX_REPLY_MESSAGES: usize = 256;

/// The largest reply read: the largest message, or the longest answer a
/// validator gives a member, whichever is larger, with every frame's
/// length and envelope.
pub const MAX_REPLY_BYTES: usize = 2 * MAX_MESSAGE_BYTES;

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

/// The hubs the node validates: where what arrives for each goes, and how
/// each stands.
#[derive(Debug, Default)]
pub struct Hubs {
    routes: Mutex<BTreeMap<HubId, Route>>,
    status: Mutex<BTreeMap<HubId, HubStatus>>,
}

impl Hubs {
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
        match Message::decode_exact(message) {
            Ok(Message::Request(_)) => {}
            Ok(_) if !requests_only => {}
            _ => return HubAnswer::Malformed,
        }
        let inbox = self
            .routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&hub)
            .map(|route| match from {
                Sender::Node(node) if route.nodes.contains(&node.node_id()) => {
                    route.validators.clone()
                }
                Sender::Node(_) | Sender::Address(_) => route.members.clone(),
            });
        let Some(inbox) = inbox else {
            return HubAnswer::UnknownHub;
        };
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
}

impl fmt::Display for HubExchangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => f.write_str("the validator cannot be reached"),
            Self::Failed(error) => write!(f, "the exchange failed: {error}"),
            Self::Refused(answer) => write!(f, "the validator took nothing: {answer:?}"),
            Self::Busy => f.write_str("too many requests in flight"),
            Self::BadReply => f.write_str("the validator's reply is broken"),
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
}
