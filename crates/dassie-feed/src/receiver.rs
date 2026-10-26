//! A subscriber's side (spec/areas/rate-feed.md#subscribing).
//!
//! A subscriber sends a Subscribe to the node's feed port every
//! [`RENEW_INTERVAL_MS`], with the last cookie it was handed, and again at
//! once when a Challenge hands it a new one. It takes a feed only from the
//! node it subscribed to, verified, newer than the last it took, and not
//! yet expired, so a replayed or delayed datagram never sets rates back.
//! The caller takes datagrams only from the node's address.

use dassie_identity::NodeId;
use dassie_node_core::MINUTE_MS;

use crate::cookie::Cookie;
use crate::wire::{self, Feed, InvalidFeed, Message, SUBSCRIBE_BYTES, WireError};

/// How often a subscriber renews.
pub const RENEW_INTERVAL_MS: u64 = MINUTE_MS;

/// What a datagram from the node brought.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Received<'a> {
    /// A feed to act on.
    Feed(Feed<'a>),
    /// A new cookie: send [`FeedReceiver::subscribe`] now.
    Challenge,
}

/// Why a datagram was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// It does not decode.
    Wire(WireError),
    /// It is a Subscribe, which only subscribers send.
    NotForUs,
    /// Its key or signature does not verify.
    Invalid(InvalidFeed),
    /// It is another node's feed: its key's node ID is not the one
    /// subscribed to.
    OtherNode,
    /// Its sequence is not after the last feed taken.
    Old,
    /// It no longer counts.
    Expired,
}

/// A subscription to one node's feed.
#[derive(Clone, Debug)]
pub struct FeedReceiver {
    node_id: NodeId,
    cookie: Option<Cookie>,
    last_sequence_ms: Option<u64>,
}

impl FeedReceiver {
    /// A subscription to the feed of `node_id`, which has no cookie yet.
    #[must_use]
    pub const fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            cookie: None,
            last_sequence_ms: None,
        }
    }

    /// The Subscribe to send, with the last cookie handed out.
    #[must_use]
    pub fn subscribe(&self) -> [u8; SUBSCRIBE_BYTES] {
        wire::encode_subscribe(self.cookie.as_ref())
    }

    /// The sequence of the last feed taken.
    #[must_use]
    pub const fn last_sequence_ms(&self) -> Option<u64> {
        self.last_sequence_ms
    }

    /// Takes a datagram from the node at `now_ms`.
    ///
    /// # Errors
    ///
    /// Why it was not taken; nothing changes then.
    pub fn receive<'a>(
        &mut self,
        datagram: &'a [u8],
        now_ms: u64,
    ) -> Result<Received<'a>, Rejected> {
        let signed = match wire::decode(datagram).map_err(Rejected::Wire)? {
            Message::Challenge(cookie) => {
                self.cookie = Some(cookie);
                return Ok(Received::Challenge);
            }
            Message::Subscribe(_) => return Err(Rejected::NotForUs),
            Message::Feed(signed) => signed,
        };
        // The key's node ID first, which costs a hash, so another node's
        // feed costs no signature check.
        if NodeId::from_public_key(&signed.feed.public_key) != self.node_id {
            return Err(Rejected::OtherNode);
        }
        signed.verify().map_err(Rejected::Invalid)?;
        let feed = signed.feed;
        if self
            .last_sequence_ms
            .is_some_and(|last| feed.sequence_ms <= last)
        {
            return Err(Rejected::Old);
        }
        if now_ms >= feed.valid_until_ms {
            return Err(Rejected::Expired);
        }
        self.last_sequence_ms = Some(feed.sequence_ms);
        Ok(Received::Feed(feed))
    }
}
