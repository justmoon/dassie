//! A subscriber's side (spec/areas/rate-feed.md#subscribing).
//!
//! A subscriber sends a Subscribe to the node's feed port every
//! [`RENEW_INTERVAL_MS`], with the last cookie it was handed, and again at
//! once when a Challenge hands it a new one. It takes a feed only from the
//! node it subscribed to, verified, newer than the last it took, and not
//! yet expired, so a replayed or delayed datagram never sets rates back.
//! The caller takes datagrams only from the node's address.

use dassie_identity::NodeIdentity;
use dassie_kit::MINUTE_MS;

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
    /// It is another node's feed: its key is not the key of the node
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
    node: NodeIdentity,
    cookie: Option<Cookie>,
    last_sequence_ms: Option<u64>,
}

impl FeedReceiver {
    /// A subscription to the feed of `node`, which has no cookie yet. The
    /// caller has `node`'s identity, both its keys, from where it learned
    /// of the feed, so it knows its ID is the node's
    /// (spec/areas/rate-feed.md#signing); a feed carries only the key that
    /// signs it.
    #[must_use]
    pub const fn new(node: NodeIdentity) -> Self {
        Self {
            node,
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
        // The key first, so another node's feed costs no signature check.
        if signed.feed.public_key != *self.node.key().as_bytes() {
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
