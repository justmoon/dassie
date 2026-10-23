//! The consensus messages on the wire, in canonical OER
//! (spec/areas/encoding.md): each message is a CHOICE, read with bounds on
//! every count and length, and re-encodes to the bytes it came from, so a
//! digest or signature over a decoded value is one over the bytes received.
//!
//! ```text
//! Message ::= CHOICE {
//!   proposal    [0] SEQUENCE { block Block, signature OCTET STRING (SIZE(64)) },
//!   vote        [1] Vote,
//!   certificate [2] Certificate,
//!   fetch       [3] SEQUENCE { height UInt64, epoch UInt64, view UInt64 },
//!   delivery    [4] SEQUENCE {
//!     finalized    SEQUENCE (SIZE(0..32)) OF Block,
//!     finalization CHOICE { none [0] NULL, some [1] Certificate },
//!     certified    SEQUENCE (SIZE(0..64)) OF SEQUENCE {
//!       certificate Certificate,
//!       block       CHOICE { none [0] NULL, some [1] Block } } } }
//! Block ::= SEQUENCE { epoch UInt64, view UInt64, height UInt64,
//!   parent OCTET STRING (SIZE(32)), parentView UInt64, timestamp UInt64,
//!   payload OCTET STRING (SIZE(0..524288)) }
//! Vote ::= SEQUENCE { kind UInt8, epoch UInt64, view UInt64,
//!   block OCTET STRING (SIZE(32)), signer UInt8 (0..15),
//!   signature OCTET STRING (SIZE(64)) }
//! Certificate ::= SEQUENCE { kind UInt8, epoch UInt64, view UInt64,
//!   block OCTET STRING (SIZE(32)), signers UInt16,
//!   signatures SEQUENCE (SIZE(0..16)) OF OCTET STRING (SIZE(64)) }
//! ```
//!
//! Fixed-size fields carry no length; `payload` is length-prefixed, and
//! `signatures` must hold one signature per bit of `signers`.

use alloc::vec::Vec;

use dassie_identity::Signature;
use dassie_oer::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, size};

use super::types::{Block, Certificate, MAX_PAYLOAD_BYTES, Vote, VoteKind};
use crate::digest::Digest;
use crate::validators::{MAX_VALIDATORS, Signers, ValidatorIndex};

/// The largest message a validator reads: a host drops anything longer
/// before decoding it.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// The most finalized blocks one delivery carries.
pub const MAX_DELIVERED_BLOCKS: usize = 32;

/// The most certificates one delivery carries.
pub const MAX_DELIVERED_CERTIFICATES: usize = 64;

const SIGNATURE_BYTES: usize = 64;
const DIGEST_BYTES: usize = 32;

/// A leader's proposal: the block and the leader's notarize vote for it,
/// which is its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// The block.
    pub block: Block,
    /// The leader's notarize signature over the block.
    pub signature: Signature,
}

/// A request for what the asker lacks: finalized blocks above `height`, and
/// certificates (with their blocks) of epoch `epoch` from view `view` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fetch {
    /// The asker's finalized height.
    pub height: u64,
    /// The asker's epoch.
    pub epoch: u64,
    /// The first view it wants certificates of.
    pub view: u64,
}

/// A certificate, with its block when the sender holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certified {
    /// The certificate.
    pub certificate: Certificate,
    /// The block it certifies, if any is held.
    pub block: Option<Block>,
}

/// The answer to a [`Fetch`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delivery {
    /// Consecutive finalized blocks above the asker's height.
    pub finalized: Vec<Block>,
    /// The finalization of the last of them, which proves them all.
    pub finalization: Option<Certificate>,
    /// Certificates of views not yet finalized at the sender, with their
    /// blocks.
    pub certified: Vec<Certified>,
}

/// A consensus message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// A leader's proposal.
    Proposal(Proposal),
    /// A vote.
    Vote(Vote),
    /// A notarization, finalization or nullification.
    Certificate(Certificate),
    /// A request for catch-up.
    Fetch(Fetch),
    /// The answer to one.
    Delivery(Delivery),
}

fn unexpected(reader: &Reader<'_>) -> DecodeError {
    reader.error(DecodeErrorKind::UnexpectedValue)
}

fn read_digest(reader: &mut Reader<'_>) -> Result<Digest, DecodeError> {
    Ok(Digest(*reader.read_array::<DIGEST_BYTES>()?))
}

fn read_signature(reader: &mut Reader<'_>) -> Result<Signature, DecodeError> {
    Ok(Signature::from_bytes(
        *reader.read_array::<SIGNATURE_BYTES>()?,
    ))
}

fn read_kind(reader: &mut Reader<'_>) -> Result<VoteKind, DecodeError> {
    let code = reader.read_u8()?;
    VoteKind::from_code(code).ok_or_else(|| unexpected(reader))
}

/// A nullify names no block: anything else is not canonical.
fn check_nullify(reader: &Reader<'_>, kind: VoteKind, block: &Digest) -> Result<(), DecodeError> {
    if kind == VoteKind::Nullify && *block != Digest::default() {
        return Err(unexpected(reader));
    }
    Ok(())
}

impl Encode for Block {
    fn encoded_len(&self) -> usize {
        8 + 8 + 8 + DIGEST_BYTES + 8 + 8 + size::prefixed(self.payload.len())
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u64(self.epoch);
        writer.write_u64(self.view);
        writer.write_u64(self.height);
        writer.write_bytes(self.parent.as_bytes());
        writer.write_u64(self.parent_view);
        writer.write_u64(self.timestamp_ms);
        writer.write_octets(&self.payload, Size::range(0, MAX_PAYLOAD_BYTES));
    }
}

impl<'a> Decode<'a> for Block {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let epoch = reader.read_u64()?;
        let view = reader.read_u64()?;
        let height = reader.read_u64()?;
        let parent = read_digest(reader)?;
        let parent_view = reader.read_u64()?;
        let timestamp_ms = reader.read_u64()?;
        let payload = reader
            .read_octets(Size::range(0, MAX_PAYLOAD_BYTES))?
            .to_vec();
        Ok(Self {
            epoch,
            view,
            height,
            parent,
            parent_view,
            timestamp_ms,
            payload,
        })
    }
}

impl Encode for Vote {
    fn encoded_len(&self) -> usize {
        1 + 8 + 8 + DIGEST_BYTES + 1 + SIGNATURE_BYTES
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u8(self.kind.code());
        writer.write_u64(self.epoch);
        writer.write_u64(self.view);
        writer.write_bytes(self.block.as_bytes());
        // An index is below MAX_VALIDATORS, which fits a byte.
        writer.write_u8(u8::try_from(self.signer.get()).unwrap_or(u8::MAX));
        writer.write_bytes(self.signature.as_bytes());
    }
}

impl<'a> Decode<'a> for Vote {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let kind = read_kind(reader)?;
        let epoch = reader.read_u64()?;
        let view = reader.read_u64()?;
        let block = read_digest(reader)?;
        check_nullify(reader, kind, &block)?;
        let signer = reader.read_u8()?;
        let signer = ValidatorIndex::new(usize::from(signer)).ok_or_else(|| unexpected(reader))?;
        let signature = read_signature(reader)?;
        Ok(Self {
            kind,
            epoch,
            view,
            block,
            signer,
            signature,
        })
    }
}

impl Encode for Certificate {
    fn encoded_len(&self) -> usize {
        1 + 8
            + 8
            + DIGEST_BYTES
            + 2
            + size::quantity(self.signatures.len())
            + SIGNATURE_BYTES * self.signatures.len()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        assert!(
            self.signatures.len() == self.signers.count(),
            "a certificate has one signature per signer"
        );
        writer.write_u8(self.kind.code());
        writer.write_u64(self.epoch);
        writer.write_u64(self.view);
        writer.write_bytes(self.block.as_bytes());
        writer.write_u16(self.signers.bits());
        writer.write_quantity(self.signatures.len());
        for signature in &self.signatures {
            writer.write_bytes(signature.as_bytes());
        }
    }
}

impl<'a> Decode<'a> for Certificate {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let kind = read_kind(reader)?;
        let epoch = reader.read_u64()?;
        let view = reader.read_u64()?;
        let block = read_digest(reader)?;
        check_nullify(reader, kind, &block)?;
        let signers = Signers::from_bits(reader.read_u16()?);
        let count = reader.read_quantity(MAX_VALIDATORS)?;
        if count != signers.count() {
            return Err(unexpected(reader));
        }
        let mut signatures = Vec::with_capacity(count);
        for _ in 0..count {
            signatures.push(read_signature(reader)?);
        }
        Ok(Self {
            kind,
            epoch,
            view,
            block,
            signers,
            signatures,
        })
    }
}

/// The length of an optional value: a tag, and the value if present.
fn optional_len<T: Encode>(value: Option<&T>) -> usize {
    size::tag(0) + value.map_or(0, Encode::encoded_len)
}

fn write_optional<T: Encode>(writer: &mut Writer<'_>, value: Option<&T>) {
    match value {
        None => writer.write_context_tag(0),
        Some(value) => {
            writer.write_context_tag(1);
            value.encode_to(writer);
        }
    }
}

fn read_optional<'a, T: Decode<'a, Error = DecodeError>>(
    reader: &mut Reader<'a>,
) -> Result<Option<T>, DecodeError> {
    let offset = reader.offset();
    match reader.read_context_tag()? {
        0 => Ok(None),
        1 => T::decode(reader).map(Some),
        _ => Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
    }
}

impl Encode for Certified {
    fn encoded_len(&self) -> usize {
        self.certificate.encoded_len() + optional_len(self.block.as_ref())
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.certificate.encode_to(writer);
        write_optional(writer, self.block.as_ref());
    }
}

impl<'a> Decode<'a> for Certified {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let certificate = Certificate::decode(reader)?;
        let block = read_optional(reader)?;
        Ok(Self { certificate, block })
    }
}

impl Encode for Delivery {
    fn encoded_len(&self) -> usize {
        size::quantity(self.finalized.len())
            + self
                .finalized
                .iter()
                .map(Encode::encoded_len)
                .sum::<usize>()
            + optional_len(self.finalization.as_ref())
            + size::quantity(self.certified.len())
            + self
                .certified
                .iter()
                .map(Encode::encoded_len)
                .sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        assert!(
            self.finalized.len() <= MAX_DELIVERED_BLOCKS
                && self.certified.len() <= MAX_DELIVERED_CERTIFICATES,
            "a delivery within its bounds"
        );
        writer.write_quantity(self.finalized.len());
        for block in &self.finalized {
            block.encode_to(writer);
        }
        write_optional(writer, self.finalization.as_ref());
        writer.write_quantity(self.certified.len());
        for certified in &self.certified {
            certified.encode_to(writer);
        }
    }
}

impl<'a> Decode<'a> for Delivery {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let count = reader.read_quantity(MAX_DELIVERED_BLOCKS)?;
        let mut finalized = Vec::with_capacity(count);
        for _ in 0..count {
            finalized.push(Block::decode(reader)?);
        }
        let finalization = read_optional(reader)?;
        let count = reader.read_quantity(MAX_DELIVERED_CERTIFICATES)?;
        let mut certified = Vec::with_capacity(count);
        for _ in 0..count {
            certified.push(Certified::decode(reader)?);
        }
        Ok(Self {
            finalized,
            finalization,
            certified,
        })
    }
}

impl Message {
    const fn tag(&self) -> u64 {
        match self {
            Self::Proposal(_) => 0,
            Self::Vote(_) => 1,
            Self::Certificate(_) => 2,
            Self::Fetch(_) => 3,
            Self::Delivery(_) => 4,
        }
    }

    /// Reads a message of at most [`MAX_MESSAGE_BYTES`] that fills `bytes`.
    ///
    /// # Errors
    ///
    /// The bytes are not one canonical message.
    pub fn decode_message(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err(DecodeError::new(0, DecodeErrorKind::LimitExceeded));
        }
        Self::decode_exact(bytes)
    }
}

impl Encode for Message {
    fn encoded_len(&self) -> usize {
        size::tag(self.tag())
            + match self {
                Self::Proposal(proposal) => proposal.block.encoded_len() + SIGNATURE_BYTES,
                Self::Vote(vote) => vote.encoded_len(),
                Self::Certificate(certificate) => certificate.encoded_len(),
                Self::Fetch(_) => 24,
                Self::Delivery(delivery) => delivery.encoded_len(),
            }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_context_tag(self.tag());
        match self {
            Self::Proposal(proposal) => {
                proposal.block.encode_to(writer);
                writer.write_bytes(proposal.signature.as_bytes());
            }
            Self::Vote(vote) => vote.encode_to(writer),
            Self::Certificate(certificate) => certificate.encode_to(writer),
            Self::Fetch(fetch) => {
                writer.write_u64(fetch.height);
                writer.write_u64(fetch.epoch);
                writer.write_u64(fetch.view);
            }
            Self::Delivery(delivery) => delivery.encode_to(writer),
        }
    }
}

impl<'a> Decode<'a> for Message {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        Ok(match reader.read_context_tag()? {
            0 => {
                let block = Block::decode(reader)?;
                let signature = read_signature(reader)?;
                Self::Proposal(Proposal { block, signature })
            }
            1 => Self::Vote(Vote::decode(reader)?),
            2 => Self::Certificate(Certificate::decode(reader)?),
            3 => Self::Fetch(Fetch {
                height: reader.read_u64()?,
                epoch: reader.read_u64()?,
                view: reader.read_u64()?,
            }),
            4 => Self::Delivery(Delivery::decode(reader)?),
            _ => return Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
        })
    }
}
