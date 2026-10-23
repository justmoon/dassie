//! What travels on a hub's stream (spec/decisions/settlement-hubs.md: "The
//! hub's messages prove themselves"): consensus messages between
//! validators, a validator's own operations passed to the others, members'
//! requests, statements to members, and validators' signatures of the
//! escrow's transactions.

use alloc::vec::Vec;

use dassie_oer::{Encode, Reader, Size, Writer, size};
use dassie_xrpl::AccountId;
use dassie_xrpl::codec::Signer;
use dassie_xrpl::keys::{PublicKey, Signature};

use crate::client::{MAX_STATEMENTS, Request, SignedStatement};
use crate::codec::HubDecodeError;
use crate::operation::Operation;

/// The most bytes of one message's content.
pub const MAX_CONTENT: usize = 1 << 20;

/// One message on a hub's stream.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// A consensus message, between validators.
    Consensus(Vec<u8>),
    /// A validator's own operation, for whichever validator leads.
    Operation(Operation),
    /// A member's request.
    Request(Request),
    /// Statements, to a member.
    Statements(Vec<SignedStatement>),
    /// A validator's signature of the escrow's transaction at `sequence`.
    Share {
        /// The escrow sequence.
        sequence: u32,
        /// The signature.
        signer: Signer,
    },
}

fn write_signer(writer: &mut Writer<'_>, signer: &Signer) {
    writer.write_bytes(&signer.account.0);
    writer.write_bytes(signer.signing_pub_key.as_bytes());
    writer.write_octets(signer.signature.as_bytes(), Size::range(1, 72));
}

fn read_signer(reader: &mut Reader<'_>) -> Result<Signer, HubDecodeError> {
    let account = AccountId(*reader.read_array::<20>()?);
    let key =
        PublicKey::from_bytes(*reader.read_array::<33>()?).ok_or(HubDecodeError::InvalidKey)?;
    let signature = Signature::from_slice(reader.read_octets(Size::range(1, 72))?)
        .ok_or(HubDecodeError::Invalid("signature"))?;
    Ok(Signer {
        account,
        signing_pub_key: key,
        signature,
    })
}

impl Encode for Message {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::Consensus(bytes) => size::prefixed(bytes.len()),
            Self::Operation(operation) => operation.encoded_len(),
            Self::Request(request) => request.encoded_len(),
            Self::Statements(list) => {
                size::quantity(list.len()) + list.iter().map(Encode::encoded_len).sum::<usize>()
            }
            Self::Share { signer, .. } => {
                4 + 20 + 33 + size::prefixed(signer.signature.as_bytes().len())
            }
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Consensus(bytes) => {
                writer.write_u8(0);
                writer.write_octets(bytes, Size::range(0, MAX_CONTENT));
            }
            Self::Operation(operation) => {
                writer.write_u8(1);
                operation.encode_to(writer);
            }
            Self::Request(request) => {
                writer.write_u8(2);
                request.encode_to(writer);
            }
            Self::Statements(list) => {
                writer.write_u8(3);
                writer.write_quantity(list.len());
                for statement in list {
                    statement.encode_to(writer);
                }
            }
            Self::Share { sequence, signer } => {
                writer.write_u8(4);
                writer.write_u32(*sequence);
                write_signer(writer, signer);
            }
        }
    }
}

impl Message {
    /// Decodes a message that fills `input`.
    pub fn decode_exact(input: &[u8]) -> Result<Self, HubDecodeError> {
        let mut reader = Reader::new(input);
        let message = match reader.read_u8()? {
            0 => Self::Consensus(reader.read_octets(Size::range(0, MAX_CONTENT))?.to_vec()),
            1 => Self::Operation(Operation::decode(&mut reader)?),
            2 => {
                let rest = reader.read_bytes(input.len().saturating_sub(1))?;
                return Ok(Self::Request(Request::decode_exact(rest)?));
            }
            3 => {
                let count = reader.read_quantity(MAX_STATEMENTS)?;
                let mut list = Vec::with_capacity(count);
                for _ in 0..count {
                    list.push(SignedStatement::decode(&mut reader)?);
                }
                Self::Statements(list)
            }
            4 => Self::Share {
                sequence: reader.read_u32()?,
                signer: read_signer(&mut reader)?,
            },
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        };
        reader.finish()?;
        Ok(message)
    }
}
