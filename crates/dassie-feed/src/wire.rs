//! The datagrams (spec/areas/rate-feed.md#wire-format).
//!
//! ```text
//! Datagram ::= SEQUENCE {
//!   version  UInt8 (2),
//!   message  CHOICE {
//!     subscribe [0] SEQUENCE {
//!       cookie   CHOICE { absent [0] NULL, present [1] OCTET STRING (SIZE (16)) },
//!       padding  zero octets, up to SUBSCRIBE_BYTES in all
//!     },
//!     challenge [1] SEQUENCE { cookie OCTET STRING (SIZE (16)) },
//!     feed      [2] SEQUENCE {
//!       feed       OCTET STRING CONTAINING Feed,  -- what the signature covers
//!       signature  OCTET STRING (SIZE (64))
//!     }
//!   }
//! }
//!
//! Feed ::= SEQUENCE {
//!   type             OBJECT IDENTIFIER (FEED_OID),
//!   publicKey        OCTET STRING (SIZE (32)),     -- the node's ID derives from it
//!   sequence         UInt64,                       -- ms, only grows
//!   validUntil       UInt64,                       -- ms, after sequence
//!   feePpm           UInt32 (0..1000000),
//!   methods          SEQUENCE (SIZE (0..8)) OF SEQUENCE {  -- ascending by id
//!     id               IA5String (SIZE (2..32)),    -- a scheme's, [a-z0-9-]
//!     maxPacketAmount  UInt64
//!   },
//!   rates            SEQUENCE (SIZE (0..56)) OF SEQUENCE {  -- ascending
//!     from UInt8, to UInt8,                         -- indexes into methods
//!     rate UInt64                                   -- out per in, × 10^15
//!   }
//! }
//! ```
//!
//! Every field has one encoding and every list one order, so a feed has
//! exactly one encoding, and the signature is checked over the bytes as
//! received (spec/areas/encoding.md#canonical-only). The feed names the
//! node by its key alone: a subscriber checks that the key's node ID is
//! the node it subscribed to. Decoding borrows from the datagram and never
//! panics; signing checks the feed first, so it never writes what the
//! decoder would refuse.

use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey, Signature};
use dassie_oer::{Alphabet, DecodeError, DecodeErrorKind, Reader, Size, Writer, oid, size};

use crate::cookie::Cookie;

/// The version byte every datagram starts with. A datagram of any other
/// version is refused, so a new version is a new byte, and the feed it
/// carries a new OID. Version 1 named a node's ledgers; version 2 names
/// the settlement methods its peers pay it on.
pub const VERSION: u8 = 2;

/// The OID a feed's signed bytes start with,
/// `1.3.6.1.4.1.54392.5.1399.3.2` (spec/areas/identity.md#object-identifiers),
/// so that a feed's signature never passes for a link state's, nor for a
/// feed of version 1, signed under `….3.1`, or the other way round.
pub const FEED_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 3, 2]);

/// Most bytes in a datagram, sent or taken: what fits in one packet on any
/// path IPv6 runs on.
pub const MAX_DATAGRAM_BYTES: usize = 1200;

/// Bytes in a cookie.
pub const COOKIE_BYTES: usize = 16;

/// Bytes in a Subscribe, padding included. At least a Challenge's, so a
/// node never answers an address it has not proven with more bytes than it
/// got from it.
pub const SUBSCRIBE_BYTES: usize = 64;

/// Bytes in a Challenge.
pub const CHALLENGE_BYTES: usize = 2 + COOKIE_BYTES;

/// Most settlement methods in a feed: the most that fit in a datagram with
/// a rate for every pair and every ID at its longest, which a constant
/// assertion below checks, and more than the methods of any realm.
pub const MAX_METHODS: usize = 8;

/// Most bytes in a method's ID, its settlement scheme's.
pub const MAX_METHOD_ID_BYTES: usize = 32;

/// Most rates in a feed: one per ordered pair of distinct methods.
pub const MAX_RATES: usize = MAX_METHODS * (MAX_METHODS - 1);

/// The most parts per million there are.
pub const MAX_PPM: u32 = 1_000_000;

/// Rates are fixed-point with this many decimal places, as the connector's.
pub const RATE_PRECISION: u32 = 15;

const SIGNATURE_BYTES: usize = 64;
const PUBLIC_KEY_BYTES: usize = 32;
/// A rate: two indexes and a `u64`.
const RATE_BYTES: usize = 1 + 1 + 8;

const TAG_SUBSCRIBE: u64 = 0;
const TAG_CHALLENGE: u64 = 1;
const TAG_FEED: u64 = 2;
const COOKIE_ABSENT: u64 = 0;
const COOKIE_PRESENT: u64 = 1;

/// The size of a method's ID: a scheme's has at least two bytes.
const METHOD_ID_SIZE: Size = Size::range(2, MAX_METHOD_ID_BYTES);
/// The characters of a method's ID, a scheme's such as
/// `hub-xrpl-testnet-xrp`; a ledger's ID, `hub-xrpl-testnet-xrp+xrp`, is
/// not one.
const METHOD_ID_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'0', b'9')
    .with_chars(b"-");

/// Most bytes of the signed part of a feed with `methods` methods, a rate
/// for every pair and every field at its bound.
const fn signed_bytes_with(methods: usize) -> usize {
    let rates = methods * methods.saturating_sub(1);
    FEED_OID.len()
        + PUBLIC_KEY_BYTES
        + 8
        + 8
        + 4
        + size::quantity(methods)
        + methods * (size::prefixed(MAX_METHOD_ID_BYTES) + 8)
        + size::quantity(rates)
        + rates * RATE_BYTES
}

/// Most bytes of a feed datagram with `methods` methods, every field at
/// its bound.
const fn feed_bytes_with(methods: usize) -> usize {
    2 + size::prefixed(signed_bytes_with(methods)) + SIGNATURE_BYTES
}

/// Most bytes of a feed's signed part, every field at its bound.
const MAX_SIGNED_BYTES: usize = signed_bytes_with(MAX_METHODS);

/// Most bytes of a feed datagram, every field at its bound.
pub const MAX_FEED_BYTES: usize = feed_bytes_with(MAX_METHODS);

// Every feed the bounds allow fits in a datagram, so signing never meets
// one that does not, and one with a method more would not; every reply to
// an unproven address is no larger than what it answers; and method
// indexes fit their octet.
const _: () = assert!(MAX_FEED_BYTES <= MAX_DATAGRAM_BYTES);
const _: () = assert!(feed_bytes_with(MAX_METHODS + 1) > MAX_DATAGRAM_BYTES);
const _: () = assert!(CHALLENGE_BYTES <= SUBSCRIBE_BYTES);
const _: () = assert!(2 + size::tag(COOKIE_PRESENT) + COOKIE_BYTES <= SUBSCRIBE_BYTES);
const _: () = assert!(MAX_METHODS <= u8::MAX as usize);

/// A datagram.
// Decoded once per datagram and taken apart at once, so its size costs
// nothing a box would save.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message<'a> {
    /// Asks for the feed, with the cookie the node handed out, if any.
    Subscribe(Option<Cookie>),
    /// The node's answer to a Subscribe without a valid cookie: the cookie
    /// to send back.
    Challenge(Cookie),
    /// A signed feed.
    Feed(SignedFeed<'a>),
}

/// Why a datagram was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    /// It is longer than [`MAX_DATAGRAM_BYTES`].
    TooLong,
    /// It is of another version.
    Version(u8),
    /// It is not canonical OER of a datagram, or exceeds a bound.
    Decode(DecodeError),
    /// It is a feed whose content breaks a rule.
    Invalid(InvalidFeed),
}

impl From<DecodeError> for WireError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

/// Why a feed is not one: its content breaks a rule, or it does not verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidFeed {
    /// The fee is more than a million parts per million.
    FeePpm,
    /// It is valid only until its own sequence or before.
    ValidUntil,
    /// More than [`MAX_METHODS`] methods.
    TooManyMethods,
    /// A method's ID is shorter than two bytes, longer than
    /// [`MAX_METHOD_ID_BYTES`], or has a character outside `[a-z0-9-]`.
    MethodId,
    /// The methods are not in ascending order, or one is there twice.
    MethodOrder,
    /// More than [`MAX_RATES`] rates.
    TooManyRates,
    /// A rate names a method the feed does not list, or the same method on
    /// both sides.
    RatePair,
    /// The rates are not in ascending order of their pair, or a pair is
    /// there twice.
    RateOrder,
    /// A rate is zero; a pair the node does not convert is left out.
    ZeroRate,
    /// The public key is not a point, or one of small order, or not the
    /// signer's.
    PublicKey,
    /// The signature does not verify.
    Signature,
}

/// A settlement method: a scheme the node's peers pay it on, which it
/// converts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Method<'a> {
    /// Its scheme's ID, such as `hub-xrpl-testnet-xrp`.
    pub id: &'a str,
    /// The most one packet may carry when it comes in on this method, in
    /// the units of its scheme's ledger.
    pub max_packet_amount: u64,
}

/// What a node converts at, from one method to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rate {
    /// The index of the method a packet comes in on.
    pub from: u8,
    /// The index of the method it goes out on.
    pub to: u8,
    /// What one unit in gets out, × 10^[`RATE_PRECISION`], in the units of
    /// each method's ledger, the node's spread included: a packet that
    /// comes in with `amount` goes out with at most `floor((amount − fee) ×
    /// rate_e15 / 10^15)`, the fee taken from `amount` at
    /// [`Feed::fee_ppm`], and less by its own impact on the node's
    /// inventory (spec/areas/rate-feed.md).
    pub rate_e15: u64,
}

/// A feed: what one node converts at, and until when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feed<'a> {
    /// The node's Ed25519 public key, from which its ID derives.
    pub public_key: [u8; 32],
    /// When it was made, in ms; each feed of a node has a larger one.
    pub sequence_ms: u64,
    /// When it stops counting, in ms; after `sequence_ms`.
    pub valid_until_ms: u64,
    /// The fee the node takes from what it passes on, in parts per million.
    pub fee_ppm: u32,
    /// The methods it converts between, in ascending order of their IDs.
    pub methods: Vec<Method<'a>>,
    /// The rates, in ascending order of `(from, to)`.
    pub rates: Vec<Rate>,
}

impl Feed<'_> {
    /// Checks every rule a feed's content follows, short of its key and
    /// signature.
    ///
    /// # Errors
    ///
    /// The first rule it breaks.
    pub fn check(&self) -> Result<(), InvalidFeed> {
        if self.fee_ppm > MAX_PPM {
            return Err(InvalidFeed::FeePpm);
        }
        if self.valid_until_ms <= self.sequence_ms {
            return Err(InvalidFeed::ValidUntil);
        }
        if self.methods.len() > MAX_METHODS {
            return Err(InvalidFeed::TooManyMethods);
        }
        let valid_id = |method: &Method<'_>| {
            METHOD_ID_SIZE.contains(method.id.len())
                && METHOD_ID_ALPHABET.contains_all(method.id.as_bytes())
        };
        if !self.methods.iter().all(valid_id) {
            return Err(InvalidFeed::MethodId);
        }
        let ascending = self.methods.windows(2).all(|pair| match pair {
            [a, b] => a.id < b.id,
            _ => true,
        });
        if !ascending {
            return Err(InvalidFeed::MethodOrder);
        }
        if self.rates.len() > MAX_RATES {
            return Err(InvalidFeed::TooManyRates);
        }
        let count = self.methods.len();
        let valid_pair = |rate: &Rate| {
            rate.from != rate.to && usize::from(rate.from) < count && usize::from(rate.to) < count
        };
        if !self.rates.iter().all(valid_pair) {
            return Err(InvalidFeed::RatePair);
        }
        if self.rates.iter().any(|rate| rate.rate_e15 == 0) {
            return Err(InvalidFeed::ZeroRate);
        }
        let pairs = self.rates.windows(2).all(|pair| match pair {
            [a, b] => (a.from, a.to) < (b.from, b.to),
            _ => true,
        });
        if !pairs {
            return Err(InvalidFeed::RateOrder);
        }
        Ok(())
    }

    /// The rate from the method `from` to the method `to`, each named by
    /// its scheme's ID, if the node converts between them.
    #[must_use]
    pub fn rate(&self, from: &str, to: &str) -> Option<u64> {
        let index = |id: &str| {
            let position = self.methods.iter().position(|method| method.id == id)?;
            u8::try_from(position).ok()
        };
        let (from, to) = (index(from)?, index(to)?);
        let found = self
            .rates
            .binary_search_by_key(&(from, to), |rate| (rate.from, rate.to))
            .ok()?;
        self.rates.get(found).map(|rate| rate.rate_e15)
    }

    /// Checks the feed and encodes the part its signature covers, which
    /// starts with [`FEED_OID`].
    ///
    /// # Errors
    ///
    /// The first rule it breaks.
    pub fn signed_bytes(&self) -> Result<Vec<u8>, InvalidFeed> {
        self.check()?;
        let length = self.signed_len();
        assert!(
            length <= MAX_SIGNED_BYTES,
            "a checked feed is within its bounds"
        );
        let mut signed = alloc::vec![0u8; length];
        let mut writer = Writer::new(&mut signed);
        self.write_signed(&mut writer);
        assert!(
            writer.position() == length,
            "the feed is as long as declared"
        );
        Ok(signed)
    }

    /// The length of the signed part. Only for a checked feed.
    fn signed_len(&self) -> usize {
        FEED_OID.len()
            + PUBLIC_KEY_BYTES
            + 8
            + 8
            + 4
            + size::quantity(self.methods.len())
            + self
                .methods
                .iter()
                .map(|method| size::prefixed(method.id.len()) + 8)
                .sum::<usize>()
            + size::quantity(self.rates.len())
            + self.rates.len() * RATE_BYTES
    }

    /// Writes the signed part. Only for a checked feed: the writer asserts
    /// what [`check`](Self::check) checks.
    fn write_signed(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&FEED_OID);
        writer.write_bytes(&self.public_key);
        writer.write_u64(self.sequence_ms);
        writer.write_u64(self.valid_until_ms);
        writer.write_u32(self.fee_ppm);
        writer.write_quantity(self.methods.len());
        for method in &self.methods {
            writer.write_ascii(method.id, METHOD_ID_SIZE, METHOD_ID_ALPHABET);
            writer.write_u64(method.max_packet_amount);
        }
        writer.write_quantity(self.rates.len());
        for rate in &self.rates {
            writer.write_u8(rate.from);
            writer.write_u8(rate.to);
            writer.write_u64(rate.rate_e15);
        }
    }
}

/// A feed as received, with the bytes its signature covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedFeed<'a> {
    /// The feed's own bytes, exactly as received: what the signature covers.
    pub signed: &'a [u8],
    /// The feed.
    pub feed: Feed<'a>,
    /// The Ed25519 signature.
    pub signature: [u8; 64],
}

impl SignedFeed<'_> {
    /// Checks the key and the signature over the bytes received, by the
    /// ZIP-215 rules (spec/areas/identity.md#node-key). Returns the key,
    /// whose node ID the caller checks against the node it expects.
    ///
    /// # Errors
    ///
    /// The first check that fails.
    pub fn verify(&self) -> Result<PublicKey, InvalidFeed> {
        let key =
            PublicKey::from_bytes(&self.feed.public_key).map_err(|_| InvalidFeed::PublicKey)?;
        if key.is_weak() {
            return Err(InvalidFeed::PublicKey);
        }
        key.verify(self.signed, &Signature::from_bytes(self.signature))
            .map_err(|_| InvalidFeed::Signature)?;
        Ok(key)
    }
}

/// Encodes a Subscribe, with `cookie` if the node handed one out.
#[must_use]
pub fn encode_subscribe(cookie: Option<&Cookie>) -> [u8; SUBSCRIBE_BYTES] {
    let mut datagram = [0u8; SUBSCRIBE_BYTES];
    let mut writer = Writer::new(&mut datagram);
    writer.write_u8(VERSION);
    writer.write_context_tag(TAG_SUBSCRIBE);
    match cookie {
        None => writer.write_context_tag(COOKIE_ABSENT),
        Some(cookie) => {
            writer.write_context_tag(COOKIE_PRESENT);
            writer.write_bytes(cookie.as_bytes());
        }
    }
    // The rest stays zero: the padding.
    datagram
}

/// Encodes a Challenge that hands out `cookie`.
#[must_use]
pub fn encode_challenge(cookie: &Cookie) -> [u8; CHALLENGE_BYTES] {
    let mut datagram = [0u8; CHALLENGE_BYTES];
    let mut writer = Writer::new(&mut datagram);
    writer.write_u8(VERSION);
    writer.write_context_tag(TAG_CHALLENGE);
    writer.write_bytes(cookie.as_bytes());
    assert!(writer.position() == CHALLENGE_BYTES);
    datagram
}

/// Checks `feed`, encodes it and signs it with `key`: the datagram to send.
///
/// # Errors
///
/// The feed breaks a rule, or its key is not `key`'s.
pub fn sign_feed(key: &NodeKey, feed: &Feed<'_>) -> Result<Vec<u8>, InvalidFeed> {
    if feed.public_key != *key.public_key().as_bytes() {
        return Err(InvalidFeed::PublicKey);
    }
    let signed = feed.signed_bytes()?;
    Ok(assemble_feed(&signed, &key.sign(&signed)))
}

/// The feed datagram of `signed`, a feed's signed bytes, and `signature`.
/// Whether the signature is right is the caller's business: tests build
/// feeds that do not verify with it.
///
/// # Panics
///
/// If `signed` is longer than a checked feed can be.
#[must_use]
pub fn assemble_feed(signed: &[u8], signature: &Signature) -> Vec<u8> {
    assert!(
        signed.len() <= MAX_SIGNED_BYTES,
        "a checked feed is within its bounds"
    );
    let length = 2 + size::prefixed(signed.len()) + SIGNATURE_BYTES;
    let mut datagram = alloc::vec![0u8; length];
    let mut writer = Writer::new(&mut datagram);
    writer.write_u8(VERSION);
    writer.write_context_tag(TAG_FEED);
    writer.write_prefixed(signed.len(), |inner| inner.write_bytes(signed));
    writer.write_bytes(signature.as_bytes());
    assert!(writer.position() == length);
    assert!(length <= MAX_DATAGRAM_BYTES);
    datagram
}

/// Decodes a datagram. A feed's content is checked, but not its signature:
/// [`SignedFeed::verify`] does that.
///
/// # Errors
///
/// The datagram is too long, of another version, not canonical OER of one
/// of the messages, or a feed that breaks a rule.
pub fn decode(datagram: &[u8]) -> Result<Message<'_>, WireError> {
    if datagram.len() > MAX_DATAGRAM_BYTES {
        return Err(WireError::TooLong);
    }
    let mut reader = Reader::new(datagram);
    let version = reader.read_u8()?;
    if version != VERSION {
        return Err(WireError::Version(version));
    }
    let offset = reader.offset();
    let message = match reader.read_context_tag()? {
        TAG_SUBSCRIBE => Message::Subscribe(read_subscribe(&mut reader, datagram.len())?),
        TAG_CHALLENGE => Message::Challenge(Cookie::from_bytes(*reader.read_array()?)),
        TAG_FEED => Message::Feed(read_signed_feed(&mut reader)?),
        _ => {
            let error = DecodeError::new(offset, DecodeErrorKind::UnexpectedTag);
            return Err(error.into());
        }
    };
    reader.finish()?;
    Ok(message)
}

/// Reads a Subscribe's cookie and padding: all zero, to exactly
/// [`SUBSCRIBE_BYTES`] in all.
fn read_subscribe(reader: &mut Reader<'_>, length: usize) -> Result<Option<Cookie>, DecodeError> {
    if length != SUBSCRIBE_BYTES {
        return Err(DecodeError::new(0, DecodeErrorKind::SizeOutOfRange));
    }
    let offset = reader.offset();
    let cookie = match reader.read_context_tag()? {
        COOKIE_ABSENT => None,
        COOKIE_PRESENT => Some(Cookie::from_bytes(*reader.read_array()?)),
        _ => return Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
    };
    let offset = reader.offset();
    let padding = reader.read_bytes(reader.remaining())?;
    if padding.iter().any(|&byte| byte != 0) {
        return Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedValue));
    }
    Ok(cookie)
}

/// Reads a signed feed, keeping the bytes the signature covers, and checks
/// its content.
fn read_signed_feed<'a>(reader: &mut Reader<'a>) -> Result<SignedFeed<'a>, WireError> {
    let (feed, signed) = reader.read_prefixed(|inner| {
        let start = inner.position();
        let feed = read_feed(inner)?;
        Ok::<_, DecodeError>((feed, inner.consumed_since(start)))
    })?;
    let signature = *reader.read_array::<SIGNATURE_BYTES>()?;
    feed.check().map_err(WireError::Invalid)?;
    Ok(SignedFeed {
        signed,
        feed,
        signature,
    })
}

/// Reads a feed's fields, every list within its bound.
fn read_feed<'a>(reader: &mut Reader<'a>) -> Result<Feed<'a>, DecodeError> {
    reader.expect_bytes(&FEED_OID)?;
    let public_key = *reader.read_array::<PUBLIC_KEY_BYTES>()?;
    let sequence_ms = reader.read_u64()?;
    let valid_until_ms = reader.read_u64()?;
    let fee_ppm = reader.read_u32()?;
    let count = reader.read_quantity(MAX_METHODS)?;
    let mut methods = Vec::with_capacity(count);
    for _ in 0..count {
        methods.push(Method {
            id: reader.read_ascii(METHOD_ID_SIZE, METHOD_ID_ALPHABET)?,
            max_packet_amount: reader.read_u64()?,
        });
    }
    let count = reader.read_quantity(MAX_RATES)?;
    let mut rates = Vec::with_capacity(count);
    for _ in 0..count {
        rates.push(Rate {
            from: reader.read_u8()?,
            to: reader.read_u8()?,
            rate_e15: reader.read_u64()?,
        });
    }
    Ok(Feed {
        public_key,
        sequence_ms,
        valid_until_ms,
        fee_ppm,
        methods,
        rates,
    })
}
