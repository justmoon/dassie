//! Snapshots and the requests for them (spec/areas/rate-feed.md#snapshots,
//! spec/decisions/peer-swaps.md).
//!
//! ```text
//! Datagram ::= SEQUENCE {
//!   version  UInt8 (2),
//!   message  CHOICE {
//!     request  [0] zero octets, to exactly REQUEST_BYTES in all,
//!     snapshot [1] SEQUENCE {
//!       sequence     UInt64,                       -- ms, only grows
//!       validUntil   UInt64,                       -- ms, after sequence
//!       grid         SEQUENCE (SIZE (8)) OF UInt16, -- drops in basis
//!                                                  -- points, ascending,
//!                                                  -- each below 10000
//!       methods      SEQUENCE (SIZE (0..MAX_SNAPSHOT_METHODS)) OF SEQUENCE {
//!         tag        OCTET STRING (SIZE (4)),      -- ascending, each once
//!         bid        UInt32,                       -- a Quote
//!         ask        UInt32,                       -- a Quote
//!         bidLadder  OCTET STRING (SIZE (9)),      -- a Ladder
//!         askLadder  OCTET STRING (SIZE (9))       -- a Ladder
//!       },
//!       signature    OCTET STRING (SIZE (64))
//!     }
//!   }
//! }
//! ```
//!
//! A snapshot is signed by the node key over [`FEED_OID`] followed by every
//! byte of the datagram before the signature, the version byte first. The
//! OID is not sent: a reader knows it, as it knows the node's key, which is
//! not sent either. A snapshot's bytes are the same whether a node answers
//! a request with them or sends them to a peer, so it signs each once.
//!
//! These datagrams replace the feed's ([`crate::wire`]) as version 2, which
//! no release carries (spec/decisions/peer-swaps.md#decision). Until the
//! node serves them, both decoders read version 2, and each refuses the
//! other's datagrams: a test checks it.
//!
//! Every value has exactly one encoding, so a snapshot's bytes are what was
//! signed, and a decoded datagram re-encodes to its bytes. Decoding borrows
//! from the datagram and never panics; signing checks the snapshot first,
//! so it never writes what the decoder would refuse.

use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey, Signature};
use dassie_oer::{DecodeError, DecodeErrorKind, Reader, Writer, size};
use sha2::{Digest, Sha256};

use crate::number::{LADDER_BYTES, LEVELS, Ladder, Quote};
use crate::wire::{FEED_OID, MAX_DATAGRAM_BYTES};

/// The version byte a snapshot and a request start with.
pub const SNAPSHOT_VERSION: u8 = 2;

/// Bytes in a method's tag.
pub const TAG_BYTES: usize = 4;

/// A method's tag: the first [`TAG_BYTES`] of the SHA-256 of
/// [`TAG_DOMAIN`] followed by its scheme's ID ([`method_tag`]).
pub type MethodTag = [u8; TAG_BYTES];

/// What a method's tag hashes before its scheme's ID, so that a tag is
/// never another hash of the same ID.
pub const TAG_DOMAIN: &[u8] = b"dassie-feed/method-tag";

/// One drop more than the largest: a drop is below 100%.
pub const DROP_BP_LIMIT: u16 = 10_000;

/// Most methods in a snapshot: the most that fit in a datagram, which a
/// constant assertion below checks.
pub const MAX_SNAPSHOT_METHODS: usize = 36;

/// Bytes in a request: exactly the largest snapshot's, so a node never
/// answers anyone with more bytes than it got from them.
pub const REQUEST_BYTES: usize = MAX_SNAPSHOT_BYTES;

/// Most bytes of a snapshot.
pub const MAX_SNAPSHOT_BYTES: usize = snapshot_bytes_with(MAX_SNAPSHOT_METHODS);

const TAG_REQUEST: u64 = 0;
const TAG_SNAPSHOT: u64 = 1;
const SIGNATURE_BYTES: usize = 64;
/// A method: its tag, two quotes and two ladders.
const METHOD_BYTES: usize = TAG_BYTES + 4 + 4 + 2 * LADDER_BYTES;
/// Everything before the methods: the version, the tag, the sequence, the
/// validity and the grid.
const HEADER_BYTES: usize = 1 + size::tag(TAG_SNAPSHOT) + 8 + 8 + 2 * LEVELS;

/// Bytes of a snapshot with `methods` methods.
const fn snapshot_bytes_with(methods: usize) -> usize {
    HEADER_BYTES + size::quantity(methods) + methods * METHOD_BYTES + SIGNATURE_BYTES
}

// The largest snapshot fits in a datagram, and one with a method more would
// not; a request is as large as the largest snapshot and fits too; and the
// OID and a snapshot fit the buffer a signature is checked in.
const _: () = assert!(MAX_SNAPSHOT_BYTES <= MAX_DATAGRAM_BYTES);
const _: () = assert!(snapshot_bytes_with(MAX_SNAPSHOT_METHODS + 1) > MAX_DATAGRAM_BYTES);
const _: () = assert!(REQUEST_BYTES >= MAX_SNAPSHOT_BYTES && REQUEST_BYTES <= MAX_DATAGRAM_BYTES);
const _: () = assert!(METHOD_BYTES == 30);

/// The tag of the method whose scheme's ID is `scheme_id`.
#[must_use]
pub fn method_tag(scheme_id: &str) -> MethodTag {
    let digest = Sha256::new()
        .chain_update(TAG_DOMAIN)
        .chain_update(scheme_id.as_bytes())
        .finalize();
    let mut tag = [0u8; TAG_BYTES];
    for (slot, byte) in tag.iter_mut().zip(digest) {
        *slot = byte;
    }
    tag
}

/// What a node quotes for one settlement method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodQuotes {
    /// Its scheme's tag ([`method_tag`]).
    pub tag: MethodTag,
    /// The least the node gives, in its virtual unit, for a unit of the
    /// method's ledger that comes in, rounded down; zero if it takes none.
    pub bid: Quote,
    /// The most the node asks, in its virtual unit, for a unit of the
    /// method's ledger that goes out, rounded up; zero if it gives none.
    pub ask: Quote,
    /// For each drop of the grid, the most that can come in on the method
    /// with its last unit still given at least `bid` less the drop. All
    /// zero when `bid` is.
    pub bid_ladder: Ladder,
    /// For each drop of the grid, the most that can go out on the method
    /// with its last unit still asked at most `ask` over one less the drop:
    /// what a unit of the node's virtual unit buys, `1 / ask`, less the
    /// drop. All zero when `ask` is.
    pub ask_ladder: Ladder,
}

/// A snapshot: a node's quotes at one moment, and until when they count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// When it was made, in ms; each snapshot of a node has a larger one.
    pub sequence_ms: u64,
    /// When it stops counting, in ms; after `sequence_ms`.
    pub valid_until_ms: u64,
    /// The drops each ladder's amounts are for, in basis points: strictly
    /// ascending, each below [`DROP_BP_LIMIT`].
    pub grid_bp: [u16; LEVELS],
    /// The methods, in ascending order of their tags, each once.
    pub methods: Vec<MethodQuotes>,
}

/// Why a snapshot is not one: its content breaks a rule, or it does not
/// verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidSnapshot {
    /// It is valid only until its own sequence or before.
    ValidUntil,
    /// Its drops are not strictly ascending, or one is 100% or more.
    Grid,
    /// More than [`MAX_SNAPSHOT_METHODS`] methods.
    TooManyMethods,
    /// The methods are not in ascending order of their tags, or a tag is
    /// there twice.
    TagOrder,
    /// A quote is not a canonical one.
    Quote,
    /// A ladder is not a canonical one.
    Ladder,
    /// A side is closed, its quote zero, but its ladder is not all zero.
    ClosedLadder,
    /// The key it is checked against is not a point, or one of small order.
    PublicKey,
    /// The signature does not verify.
    Signature,
}

/// Why a datagram was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotWireError {
    /// It is longer than [`MAX_DATAGRAM_BYTES`].
    TooLong,
    /// It is of another version.
    Version(u8),
    /// It is not canonical OER of a datagram, or exceeds a bound.
    Decode(DecodeError),
    /// It is a snapshot whose content breaks a rule.
    Invalid(InvalidSnapshot),
}

impl From<DecodeError> for SnapshotWireError {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

impl Snapshot {
    /// Checks every rule a snapshot's content follows.
    ///
    /// # Errors
    ///
    /// The first rule it breaks.
    pub fn check(&self) -> Result<(), InvalidSnapshot> {
        if self.valid_until_ms <= self.sequence_ms {
            return Err(InvalidSnapshot::ValidUntil);
        }
        let ascending = self.grid_bp.windows(2).all(|pair| match pair {
            [a, b] => a < b,
            _ => true,
        });
        if !ascending || self.grid_bp[LEVELS - 1] >= DROP_BP_LIMIT {
            return Err(InvalidSnapshot::Grid);
        }
        if self.methods.len() > MAX_SNAPSHOT_METHODS {
            return Err(InvalidSnapshot::TooManyMethods);
        }
        let ascending = self.methods.windows(2).all(|pair| match pair {
            [a, b] => a.tag < b.tag,
            _ => true,
        });
        if !ascending {
            return Err(InvalidSnapshot::TagOrder);
        }
        let closed = |quote: Quote, ladder: &Ladder| quote.is_zero() && *ladder != Ladder::ZERO;
        let open_ladders = self.methods.iter().all(|method| {
            !closed(method.bid, &method.bid_ladder) && !closed(method.ask, &method.ask_ladder)
        });
        if !open_ladders {
            return Err(InvalidSnapshot::ClosedLadder);
        }
        Ok(())
    }

    /// The quotes of the method tagged `tag`, if the snapshot has it.
    #[must_use]
    pub fn method(&self, tag: MethodTag) -> Option<&MethodQuotes> {
        let index = self
            .methods
            .binary_search_by_key(&tag, |method| method.tag)
            .ok()?;
        self.methods.get(index)
    }

    /// Checks the snapshot and encodes every byte of its datagram before
    /// the signature.
    ///
    /// # Errors
    ///
    /// The first rule it breaks.
    pub fn unsigned_bytes(&self) -> Result<Vec<u8>, InvalidSnapshot> {
        self.check()?;
        let length = snapshot_bytes_with(self.methods.len()) - SIGNATURE_BYTES;
        let mut unsigned = alloc::vec![0u8; length];
        let mut writer = Writer::new(&mut unsigned);
        writer.write_u8(SNAPSHOT_VERSION);
        writer.write_context_tag(TAG_SNAPSHOT);
        writer.write_u64(self.sequence_ms);
        writer.write_u64(self.valid_until_ms);
        for drop in self.grid_bp {
            writer.write_u16(drop);
        }
        writer.write_quantity(self.methods.len());
        for method in &self.methods {
            writer.write_bytes(&method.tag);
            writer.write_u32(method.bid.to_bits());
            writer.write_u32(method.ask.to_bits());
            writer.write_bytes(&method.bid_ladder.to_bytes());
            writer.write_bytes(&method.ask_ladder.to_bytes());
        }
        assert!(
            writer.position() == length,
            "the snapshot is as long as declared"
        );
        Ok(unsigned)
    }
}

/// A snapshot as received, with its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedSnapshot<'a> {
    /// The datagram, exactly as received.
    pub bytes: &'a [u8],
    /// The snapshot.
    pub snapshot: Snapshot,
    /// The Ed25519 signature.
    pub signature: [u8; SIGNATURE_BYTES],
}

impl SignedSnapshot<'_> {
    /// Checks the signature over the bytes received under `key`, by the
    /// ZIP-215 rules (spec/areas/identity.md#node-key), refusing a weak
    /// key. The caller knows the node's key from its identity.
    ///
    /// # Errors
    ///
    /// The key is weak, or the signature does not verify.
    pub fn verify(&self, key: &PublicKey) -> Result<(), InvalidSnapshot> {
        if key.is_weak() {
            return Err(InvalidSnapshot::PublicKey);
        }
        let unsigned_length = self.bytes.len().saturating_sub(SIGNATURE_BYTES);
        let unsigned = self
            .bytes
            .get(..unsigned_length)
            .ok_or(InvalidSnapshot::Signature)?;
        let mut buffer = [0u8; FEED_OID.len() + MAX_SNAPSHOT_BYTES];
        let message = signing_message(unsigned, &mut buffer).ok_or(InvalidSnapshot::Signature)?;
        key.verify(message, &Signature::from_bytes(self.signature))
            .map_err(|_| InvalidSnapshot::Signature)
    }
}

/// [`FEED_OID`] followed by `unsigned`, in `buffer`: what a snapshot's
/// signature covers. `None` if `unsigned` is longer than a snapshot.
fn signing_message<'b>(
    unsigned: &[u8],
    buffer: &'b mut [u8; FEED_OID.len() + MAX_SNAPSHOT_BYTES],
) -> Option<&'b [u8]> {
    let length = FEED_OID.len() + unsigned.len();
    let message = buffer.get_mut(..length)?;
    let (oid, rest) = message.split_at_mut(FEED_OID.len());
    oid.copy_from_slice(&FEED_OID);
    rest.copy_from_slice(unsigned);
    Some(message)
}

/// What a snapshot's signature covers, given every byte of its datagram
/// before the signature ([`Snapshot::unsigned_bytes`]): [`FEED_OID`]
/// followed by them.
#[must_use]
pub fn snapshot_signing_message(unsigned: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(FEED_OID.len() + unsigned.len());
    message.extend_from_slice(&FEED_OID);
    message.extend_from_slice(unsigned);
    message
}

/// Checks `snapshot`, encodes it and signs it with `key`: the bytes a node
/// caches, answers requests with and sends its peers.
///
/// # Errors
///
/// The snapshot breaks a rule.
pub fn sign_snapshot(key: &NodeKey, snapshot: &Snapshot) -> Result<Vec<u8>, InvalidSnapshot> {
    let unsigned = snapshot.unsigned_bytes()?;
    let signature = key.sign(&snapshot_signing_message(&unsigned));
    Ok(attach_signature(unsigned, &signature))
}

/// A snapshot's datagram from every byte before its signature
/// ([`Snapshot::unsigned_bytes`]) and the signature. Whether the signature
/// is right is the caller's business: tests build snapshots that do not
/// verify with it.
///
/// # Panics
///
/// If `unsigned` is too long to be a snapshot's.
#[must_use]
pub fn attach_signature(mut unsigned: Vec<u8>, signature: &Signature) -> Vec<u8> {
    assert!(
        unsigned.len() + SIGNATURE_BYTES <= MAX_SNAPSHOT_BYTES,
        "a checked snapshot is within its bounds"
    );
    unsigned.extend_from_slice(signature.as_bytes());
    unsigned
}

/// Encodes a request.
#[must_use]
pub fn encode_request() -> [u8; REQUEST_BYTES] {
    let mut datagram = [0u8; REQUEST_BYTES];
    let mut writer = Writer::new(&mut datagram);
    writer.write_u8(SNAPSHOT_VERSION);
    writer.write_context_tag(TAG_REQUEST);
    // The rest stays zero: the padding.
    datagram
}

/// A datagram of the snapshot's version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotMessage<'a> {
    /// Asks for the node's snapshot.
    Request,
    /// A snapshot, its content checked but not its signature.
    Snapshot(SignedSnapshot<'a>),
}

/// Decodes a datagram. A snapshot's content is checked, but not its
/// signature: [`SignedSnapshot::verify`] does that.
///
/// # Errors
///
/// The datagram is too long, of another version, not canonical OER of a
/// request or a snapshot, or a snapshot that breaks a rule.
pub fn decode_snapshot_datagram(datagram: &[u8]) -> Result<SnapshotMessage<'_>, SnapshotWireError> {
    if datagram.len() > MAX_DATAGRAM_BYTES {
        return Err(SnapshotWireError::TooLong);
    }
    let mut reader = Reader::new(datagram);
    let version = reader.read_u8()?;
    if version != SNAPSHOT_VERSION {
        return Err(SnapshotWireError::Version(version));
    }
    let offset = reader.offset();
    let message = match reader.read_context_tag()? {
        TAG_REQUEST => {
            read_padding(&mut reader, datagram.len())?;
            SnapshotMessage::Request
        }
        TAG_SNAPSHOT => SnapshotMessage::Snapshot(read_snapshot(&mut reader, datagram)?),
        _ => {
            let error = DecodeError::new(offset, DecodeErrorKind::UnexpectedTag);
            return Err(error.into());
        }
    };
    reader.finish()?;
    Ok(message)
}

/// Reads a request's padding: all zero, to exactly [`REQUEST_BYTES`] in
/// all.
fn read_padding(reader: &mut Reader<'_>, length: usize) -> Result<(), DecodeError> {
    if length != REQUEST_BYTES {
        return Err(DecodeError::new(0, DecodeErrorKind::SizeOutOfRange));
    }
    let offset = reader.offset();
    let padding = reader.read_bytes(reader.remaining())?;
    if padding.iter().any(|&byte| byte != 0) {
        return Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedValue));
    }
    Ok(())
}

/// Reads a snapshot after its version and tag, every list within its
/// bound, and checks its content.
fn read_snapshot<'a>(
    reader: &mut Reader<'a>,
    datagram: &'a [u8],
) -> Result<SignedSnapshot<'a>, SnapshotWireError> {
    let sequence_ms = reader.read_u64()?;
    let valid_until_ms = reader.read_u64()?;
    let mut grid_bp = [0u16; LEVELS];
    for drop in &mut grid_bp {
        *drop = reader.read_u16()?;
    }
    let count = reader.read_quantity(MAX_SNAPSHOT_METHODS)?;
    let mut methods = Vec::with_capacity(count);
    for _ in 0..count {
        methods.push(read_method(reader)?);
    }
    let signature = *reader.read_array::<SIGNATURE_BYTES>()?;
    let snapshot = Snapshot {
        sequence_ms,
        valid_until_ms,
        grid_bp,
        methods,
    };
    snapshot.check().map_err(SnapshotWireError::Invalid)?;
    Ok(SignedSnapshot {
        bytes: datagram,
        snapshot,
        signature,
    })
}

/// Reads a method's quotes and ladders, refusing any that is not canonical.
fn read_method(reader: &mut Reader<'_>) -> Result<MethodQuotes, SnapshotWireError> {
    let invalid = SnapshotWireError::Invalid;
    let tag = *reader.read_array::<TAG_BYTES>()?;
    let bid = Quote::from_bits(reader.read_u32()?).ok_or(invalid(InvalidSnapshot::Quote))?;
    let ask = Quote::from_bits(reader.read_u32()?).ok_or(invalid(InvalidSnapshot::Quote))?;
    let bid_ladder = Ladder::from_bytes(reader.read_array::<LADDER_BYTES>()?)
        .ok_or(invalid(InvalidSnapshot::Ladder))?;
    let ask_ladder = Ladder::from_bytes(reader.read_array::<LADDER_BYTES>()?)
        .ok_or(invalid(InvalidSnapshot::Ladder))?;
    Ok(MethodQuotes {
        tag,
        bid,
        ask,
        bid_ladder,
        ask_ladder,
    })
}

/// Why a reader did not take a datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotRejected {
    /// It does not decode.
    Wire(SnapshotWireError),
    /// It is a request, which only readers send.
    NotSnapshot,
    /// The key is weak, or the signature does not verify.
    Invalid(InvalidSnapshot),
    /// Its sequence is not after the last snapshot taken.
    Old,
    /// It no longer counts.
    Expired,
}

/// A reader of one node's snapshots, whether they come over the peer
/// protocol or as answers to requests: it takes a snapshot only verified
/// under the node's key, newer than the last it took, and not expired, so a
/// replayed or delayed one never sets quotes back.
#[derive(Clone, Debug)]
pub struct SnapshotReader {
    key: PublicKey,
    last_sequence_ms: Option<u64>,
}

impl SnapshotReader {
    /// A reader of the snapshots of the node whose Ed25519 key is `key`,
    /// which the caller knows from the node's identity.
    #[must_use]
    pub const fn new(key: PublicKey) -> Self {
        Self {
            key,
            last_sequence_ms: None,
        }
    }

    /// The sequence of the last snapshot taken.
    #[must_use]
    pub const fn last_sequence_ms(&self) -> Option<u64> {
        self.last_sequence_ms
    }

    /// Takes a snapshot's bytes at `now_ms`.
    ///
    /// # Errors
    ///
    /// Why it was not taken; nothing changes then.
    pub fn receive(&mut self, bytes: &[u8], now_ms: u64) -> Result<Snapshot, SnapshotRejected> {
        let signed = match decode_snapshot_datagram(bytes).map_err(SnapshotRejected::Wire)? {
            SnapshotMessage::Request => return Err(SnapshotRejected::NotSnapshot),
            SnapshotMessage::Snapshot(signed) => signed,
        };
        // The cheap checks first, so a stale snapshot costs no signature
        // check.
        let snapshot = &signed.snapshot;
        if self
            .last_sequence_ms
            .is_some_and(|last| snapshot.sequence_ms <= last)
        {
            return Err(SnapshotRejected::Old);
        }
        if now_ms >= snapshot.valid_until_ms {
            return Err(SnapshotRejected::Expired);
        }
        signed
            .verify(&self.key)
            .map_err(SnapshotRejected::Invalid)?;
        self.last_sequence_ms = Some(signed.snapshot.sequence_ms);
        Ok(signed.snapshot)
    }
}
