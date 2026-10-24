//! What a hub's block carries for its ledger: the operations in order, the
//! votes for earlier blocks it records (spec/designs/settlement-hubs.md#7-fees),
//! and the roots of the outcomes and of the state after it, which every
//! validator checks by executing the block itself.

use alloc::vec::Vec;

use dassie_oer::{Encode, Reader, Writer, size};

use crate::codec::{DIGEST_LEN, HubDecodeError, SIGNATURE_LEN, read_digest, write_digest};
use crate::consensus::MAX_PAYLOAD_BYTES;
use crate::digest::Digest;
use crate::operation::{MAX_COMMITMENTS, Operation};
use crate::promise::COMMITMENT_LEN;
use crate::validators::Signers;

/// The most operations a payload may carry, whatever the rules say: a bound
/// for decoding before the rules are applied.
pub const MAX_OPERATIONS: usize = 4096;

/// The most heights a payload may record votes for.
pub const MAX_VOTE_HEIGHTS: usize = 256;

/// What a payload carries besides its operations, at most: two counts,
/// the votes for every height it may record, and two roots.
pub const FRAME_BYTES: usize = 2 * 9 + MAX_VOTE_HEIGHTS * (8 + 2) + 2 * DIGEST_LEN;

/// The most bytes of operations a leader puts in one block. A payload over
/// [`MAX_PAYLOAD_BYTES`] would have the leader nullify its own view, and
/// every leader would build the same one again, so the leader fills a
/// block by bytes as well as by count, below the bound by a margin.
pub const OPERATION_BYTES: usize = MAX_PAYLOAD_BYTES - 16 * 1024;

/// The encoding of a member's `claim` with [`MAX_COMMITMENTS`] commitments,
/// the largest operation a member signs:
/// the operation's tag, account, sequence and fee, the claim's tag, hold and
/// count, the commitments with their fulfillments, and the signature.
pub const MAX_CLAIM_BYTES: usize = 1
    + 4
    + 8
    + 8
    + 1
    + 8
    + 9
    + MAX_COMMITMENTS * (COMMITMENT_LEN + SIGNATURE_LEN + DIGEST_LEN)
    + SIGNATURE_LEN;

const _: () = assert!(OPERATION_BYTES + FRAME_BYTES <= MAX_PAYLOAD_BYTES);
// Any claim the rules can allow fits in a block.
const _: () = assert!(MAX_CLAIM_BYTES <= OPERATION_BYTES);

/// A block's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    /// The operations, those with deadlines first.
    pub operations: Vec<Operation>,
    /// For earlier blocks, by height, the validators whose votes for them
    /// this block records.
    pub votes: Vec<(u64, Signers)>,
    /// The root of the operations' outcomes.
    pub results: Digest,
    /// The root of the state after the block.
    pub state: Digest,
}

impl Encode for Payload {
    fn encoded_len(&self) -> usize {
        size::quantity(self.operations.len())
            + self
                .operations
                .iter()
                .map(Encode::encoded_len)
                .sum::<usize>()
            + size::quantity(self.votes.len())
            + self.votes.len() * (8 + 2)
            + 32
            + 32
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_quantity(self.operations.len());
        for operation in &self.operations {
            operation.encode_to(writer);
        }
        writer.write_quantity(self.votes.len());
        for (height, signers) in &self.votes {
            writer.write_u64(*height);
            writer.write_u16(signers.bits());
        }
        write_digest(writer, &self.results);
        write_digest(writer, &self.state);
    }
}

impl Payload {
    /// Decodes a payload that fills `input`.
    pub fn decode_exact(input: &[u8]) -> Result<Self, HubDecodeError> {
        let mut reader = Reader::new(input);
        let count = reader.read_quantity(MAX_OPERATIONS)?;
        let mut operations = Vec::with_capacity(count);
        for _ in 0..count {
            operations.push(Operation::decode(&mut reader)?);
        }
        let count = reader.read_quantity(MAX_VOTE_HEIGHTS)?;
        let mut votes = Vec::with_capacity(count);
        for _ in 0..count {
            votes.push((reader.read_u64()?, Signers::from_bits(reader.read_u16()?)));
        }
        let results = read_digest(&mut reader)?;
        let state = read_digest(&mut reader)?;
        reader.finish()?;
        Ok(Self {
            operations,
            votes,
            results,
            state,
        })
    }
}
