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
use crate::escrow::PayoutEntry;
use crate::escrow::payout::MAX_PAYOUT_TRANSACTION_BYTES;
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

/// The most bytes of a member's message: a request, the largest of which
/// submits a claim with the most commitments any rules allow. A host reads
/// no more from anyone it does not know as a validator.
pub const MAX_MEMBER_MESSAGE: usize = 1 + 1 + crate::service::payload::MAX_CLAIM_BYTES;

const _: () = assert!(MAX_MEMBER_MESSAGE < MAX_MESSAGE);

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

/// The most transactions of a payout set in one page of it.
pub const MAX_PAYOUT_PAGE: usize = 1024;

/// The most bytes of the transactions and signatures of one page.
pub const MAX_PAYOUT_PAGE_BYTES: usize = 512 * 1024;

const _: () = assert!(
    MAX_PAYOUT_PAGE_BYTES + MAX_PAYOUT_TRANSACTION_BYTES + 256 < MAX_CONTENT,
    "a page fits a message"
);

/// A page of the payout set a validator signed: from the transaction at
/// `first` on, as many as fit a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutPage {
    /// The height of the block the set pays out.
    pub height: u64,
    /// How many transactions the whole set has.
    pub total: u32,
    /// The place of this page's first in the set.
    pub first: u32,
    /// The transactions, each with the validator's signature.
    pub entries: Vec<PayoutEntry>,
}

const SIGNATURE_SIZE: Size = Size::range(1, 72);
/// The size of one transaction of a payout set.
pub(crate) const TRANSACTION_SIZE: Size = Size::range(1, MAX_PAYOUT_TRANSACTION_BYTES);

impl PayoutPage {
    /// The page of `entries`, the whole set of `height`, from `first` on:
    /// at least one transaction, if any is left, and at most a page's
    /// count and bytes.
    #[must_use]
    pub fn of(height: u64, entries: &[PayoutEntry], first: u32) -> Self {
        let mut bytes = 0_usize;
        let page = entries
            .iter()
            .skip(usize::try_from(first).unwrap_or(usize::MAX))
            .take(MAX_PAYOUT_PAGE)
            .take_while(|entry| {
                let fits =
                    bytes == 0 || bytes.saturating_add(entry_len(entry)) <= MAX_PAYOUT_PAGE_BYTES;
                bytes = bytes.saturating_add(entry_len(entry));
                fits
            })
            .cloned()
            .collect();
        Self {
            height,
            total: u32::try_from(entries.len()).unwrap_or(u32::MAX),
            first,
            entries: page,
        }
    }
}

/// The encoded length of one entry of a payout set.
pub(crate) fn entry_len(entry: &PayoutEntry) -> usize {
    size::prefixed(entry.transaction.len())
        + 20
        + 33
        + size::prefixed(entry.signer.signature.as_bytes().len())
}

impl Encode for PayoutPage {
    fn encoded_len(&self) -> usize {
        8 + 4
            + 4
            + size::quantity(self.entries.len())
            + self.entries.iter().map(entry_len).sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u64(self.height);
        writer.write_u32(self.total);
        writer.write_u32(self.first);
        writer.write_quantity(self.entries.len());
        for entry in &self.entries {
            writer.write_octets(&entry.transaction, TRANSACTION_SIZE);
            write_signer(writer, &entry.signer);
        }
    }
}

impl PayoutPage {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let height = reader.read_u64()?;
        let total = reader.read_u32()?;
        let first = reader.read_u32()?;
        let count = reader.read_quantity(MAX_PAYOUT_PAGE)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let transaction = reader.read_octets(TRANSACTION_SIZE)?.to_vec();
            entries.push(PayoutEntry {
                transaction,
                signer: read_signer(reader)?,
            });
        }
        Ok(Self {
            height,
            total,
            first,
            entries,
        })
    }
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
    /// A page of the payout set a validator signed, to a member that
    /// asked for it.
    Payout(PayoutPage),
    /// A validator's signature of the escrow's transaction at `sequence`.
    Share {
        /// The escrow sequence.
        sequence: u32,
        /// The signature.
        signer: Signer,
    },
}

/// Writes an escrow signature.
pub(crate) fn write_signer(writer: &mut Writer<'_>, signer: &Signer) {
    writer.write_bytes(&signer.account.0);
    writer.write_bytes(signer.signing_pub_key.as_bytes());
    writer.write_octets(signer.signature.as_bytes(), SIGNATURE_SIZE);
}

/// Reads an escrow signature; whether it verifies is the caller's to check.
pub(crate) fn read_signer(reader: &mut Reader<'_>) -> Result<Signer, HubDecodeError> {
    let account = AccountId(*reader.read_array::<20>()?);
    let key =
        PublicKey::from_bytes(*reader.read_array::<33>()?).ok_or(HubDecodeError::InvalidKey)?;
    let signature = Signature::from_slice(reader.read_octets(SIGNATURE_SIZE)?)
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
            Self::Payout(page) => page.encoded_len(),
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
            Self::Payout(page) => {
                writer.write_u8(5);
                page.encode_to(writer);
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
            5 => Self::Payout(PayoutPage::decode(&mut reader)?),
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
