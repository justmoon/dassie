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

/// The most bytes of one message's content: the largest consensus message,
/// such as a catch-up delivery.
pub const MAX_CONTENT: usize = crate::consensus::wire::MAX_MESSAGE_BYTES;

/// What a message adds around its content, at most: its tag and a length
/// prefix.
pub const ENVELOPE: usize = 1 + 9;

/// The most bytes of one message on a hub's stream. A host drops anything
/// longer before decoding it.
pub const MAX_MESSAGE: usize = MAX_CONTENT + ENVELOPE;

const _: () = assert!(MAX_CONTENT >= crate::consensus::wire::MAX_MESSAGE_BYTES);

/// Splits signed statements into lists that each fit one message: at most
/// [`MAX_STATEMENTS`] and [`MAX_CONTENT`] bytes each. A statement too large
/// for a message alone is dropped; none is, since an operation fits a block
/// and a block fits a consensus message.
#[must_use]
pub fn split_statements(statements: Vec<SignedStatement>) -> Vec<Vec<SignedStatement>> {
    let mut lists: Vec<Vec<SignedStatement>> = Vec::new();
    let mut current: Vec<SignedStatement> = Vec::new();
    let mut bytes = 0usize;
    for statement in statements {
        let len = statement.encoded_len();
        if len.saturating_add(ENVELOPE) > MAX_CONTENT {
            continue;
        }
        let full = current.len() >= MAX_STATEMENTS
            || bytes.saturating_add(len).saturating_add(ENVELOPE) > MAX_CONTENT;
        if full {
            lists.push(core::mem::take(&mut current));
            bytes = 0;
        }
        bytes = bytes.saturating_add(len);
        current.push(statement);
    }
    if !current.is_empty() {
        lists.push(current);
    }
    lists
}

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
        if input.len() > MAX_MESSAGE {
            return Err(HubDecodeError::Invalid("message too long"));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Statement;
    use crate::consensus::wire::MAX_MESSAGE_BYTES;
    use crate::validators::ValidatorIndex;

    #[test]
    fn the_largest_consensus_message_travels() {
        let message = Message::Consensus(alloc::vec![7; MAX_MESSAGE_BYTES]);
        let bytes = message.to_vec();
        assert!(bytes.len() <= MAX_MESSAGE);
        assert_eq!(Message::decode_exact(&bytes).unwrap(), message);
    }

    #[test]
    fn statements_split_by_count_and_bytes() {
        let key = dassie_identity::NodeKey::from_bytes(&[1; 32]);
        let hub = crate::rules::HubId(crate::digest::Digest([1; 32]));
        let statement = SignedStatement::sign(
            &hub,
            ValidatorIndex::new(0).unwrap(),
            Statement::Final {
                height: 1,
                timestamp: 2,
            },
            &key,
        );
        let lists = split_statements(alloc::vec![statement; 3 * MAX_STATEMENTS + 1]);
        assert_eq!(lists.len(), 4);
        assert!(lists.iter().all(|list| list.len() <= MAX_STATEMENTS));
        for list in lists {
            let bytes = Message::Statements(list).to_vec();
            assert!(bytes.len() <= MAX_MESSAGE);
        }
    }
}
