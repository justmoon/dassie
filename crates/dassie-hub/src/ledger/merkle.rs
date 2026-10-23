//! Merkle roots: of the state after a block, which every block carries so a
//! validator that lacks the state can take it from anyone and check it
//! (spec/designs/settlement-hubs.md#9-shutdown), and of a block's outcomes,
//! which receipts prove a result against
//! (spec/designs/settlement-hubs.md#3-the-ledger).
//!
//! The tree is a plain binary one over the leaves in key order; an odd node
//! is carried up as it is. Inner nodes and leaves hash under domains of
//! their own, so no inner node can pass for a leaf.

use alloc::vec::Vec;

use dassie_oer::Encode;

use super::Ledger;
use super::outcome::Outcome;
use super::state::{Key, Phase};
use crate::digest::{Digest, Domain};
use crate::operation::Operation;

/// The root over `leaves`.
#[must_use]
pub fn merkle_root(leaves: &[Digest]) -> Digest {
    let mut level: Vec<Digest> = leaves.to_vec();
    if level.is_empty() {
        return Digest::of(Domain::StateNode, &[]);
    }
    while level.len() > 1 {
        level = merkle_root_level(&level);
    }
    level.first().copied().unwrap_or_default()
}

/// The siblings from leaf `index` up to the root, each with whether it is
/// on the left.
#[must_use]
pub fn merkle_proof(leaves: &[Digest], index: usize) -> Vec<(Digest, bool)> {
    let mut proof = Vec::new();
    let mut level: Vec<Digest> = leaves.to_vec();
    let mut position = index;
    while level.len() > 1 {
        let sibling = position ^ 1;
        if let Some(digest) = level.get(sibling) {
            proof.push((*digest, sibling < position));
        }
        level = merkle_root_level(&level);
        position /= 2;
    }
    proof
}

fn merkle_root_level(level: &[Digest]) -> Vec<Digest> {
    level
        .chunks(2)
        .map(|pair| match pair {
            [left, right] => Digest::of(Domain::StateNode, &[left.as_bytes(), right.as_bytes()]),
            [single] => *single,
            _ => Digest::default(),
        })
        .collect()
}

/// Whether `proof` leads from `leaf` to `root`.
#[must_use]
pub fn verify_proof(leaf: Digest, proof: &[(Digest, bool)], root: &Digest) -> bool {
    let computed = proof.iter().fold(leaf, |node, (sibling, on_left)| {
        if *on_left {
            Digest::of(Domain::StateNode, &[sibling.as_bytes(), node.as_bytes()])
        } else {
            Digest::of(Domain::StateNode, &[node.as_bytes(), sibling.as_bytes()])
        }
    });
    computed == *root
}

/// The leaf of one outcome: the operation's hash and its result.
#[must_use]
pub fn result_leaf(operation: &Digest, outcome: &Outcome) -> Digest {
    Digest::of(Domain::Result, &[operation.as_bytes(), &outcome.to_vec()])
}

/// The root over a block's outcomes, in the order of its operations.
#[must_use]
pub fn results_root(operations: &[Operation], outcomes: &[Outcome]) -> Digest {
    let leaves: Vec<Digest> = operations
        .iter()
        .zip(outcomes)
        .map(|(operation, outcome)| result_leaf(&operation.hash(), outcome))
        .collect();
    merkle_root(&leaves)
}

fn leaf(key: Key, value: &[u8]) -> Digest {
    Digest::of(Domain::StateLeaf, &[&key.bytes(), value])
}

impl Ledger {
    /// The root of the state after the last block.
    #[must_use]
    pub fn state_root(&self) -> Digest {
        let mut leaves = Vec::with_capacity(
            1 + self.accounts.len()
                + self.holds.len()
                + self.withdrawals.len()
                + self.pending_deposits.len()
                + self.recent_deposits.len(),
        );
        leaves.push(leaf(Key::Header, &self.header()));
        for (number, account) in &self.accounts {
            leaves.push(leaf(Key::Account(*number), &account.to_vec()));
        }
        for (number, hold) in &self.holds {
            leaves.push(leaf(Key::Hold(*number), &hold.to_vec()));
        }
        for (sequence, withdrawal) in &self.withdrawals {
            leaves.push(leaf(Key::Withdrawal(*sequence), &withdrawal.to_vec()));
        }
        for (hash, pending) in &self.pending_deposits {
            leaves.push(leaf(Key::PendingDeposit(hash.0), &pending.to_vec()));
        }
        for (hash, ledger) in &self.recent_deposits {
            leaves.push(leaf(Key::RecentDeposit(hash.0), &ledger.to_be_bytes()));
        }
        merkle_root(&leaves)
    }

    /// The hub-wide values, in a fixed order. Every list has its count
    /// and every option a presence byte, so no two states write the same
    /// bytes: a validator that takes the state from someone else checks it
    /// against the certified root (spec/designs/settlement-hubs.md#9-shutdown).
    fn header(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(self.hub.0.as_bytes());
        bytes.extend_from_slice(self.rules.id().0.as_bytes());
        for value in [
            self.height,
            self.timestamp,
            self.escrow.balance,
            self.next_hold,
            self.reserve_account,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        for value in [
            self.escrow.next_sequence,
            self.escrow.deposit_floor,
            self.next_account,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.push(u8::from(self.escrow.checked));
        push_option(&mut bytes, self.escrow.signer_list.map(u64::from));
        match self.phase {
            Phase::Running => bytes.push(0),
            Phase::ShuttingDown { ends } => {
                bytes.push(1);
                bytes.extend_from_slice(&ends.to_be_bytes());
            }
            Phase::Ended => bytes.push(2),
        }
        push_count(&mut bytes, self.pools.len());
        for pool in &self.pools {
            bytes.extend_from_slice(&pool.height.to_be_bytes());
            bytes.extend_from_slice(&pool.amount.to_be_bytes());
            bytes.extend_from_slice(&pool.voters.bits().to_be_bytes());
        }
        self.header_votes(&mut bytes);
        bytes
    }

    fn header_votes(&self, bytes: &mut Vec<u8>) {
        bytes.push(u8::from(self.pending_rules.is_some()));
        if let Some(pending) = &self.pending_rules {
            bytes.extend_from_slice(pending.rules.id().0.as_bytes());
            bytes.extend_from_slice(&pending.at.to_be_bytes());
        }
        push_count(bytes, self.rules_votes.len());
        for (digest, signers) in &self.rules_votes {
            bytes.extend_from_slice(digest.as_bytes());
            bytes.extend_from_slice(&signers.bits().to_be_bytes());
        }
        for votes in [
            &self.escrow_votes,
            &self.reset_votes,
            &self.signer_list_votes,
        ] {
            push_count(bytes, votes.len());
            for (value, signers) in votes {
                bytes.extend_from_slice(&value.to_be_bytes());
                bytes.extend_from_slice(&signers.bits().to_be_bytes());
            }
        }
        bytes.extend_from_slice(&self.shutdown_votes.bits().to_be_bytes());
        push_count(bytes, self.validator_states.len());
        for state in &self.validator_states {
            push_count(bytes, state.endpoint.len());
            bytes.extend_from_slice(state.endpoint.as_bytes());
            bytes.extend_from_slice(&state.sequence.to_be_bytes());
            bytes.extend_from_slice(&state.last_recorded.to_be_bytes());
            bytes.extend_from_slice(&state.account.0.to_be_bytes());
        }
    }
}

fn push_count(bytes: &mut Vec<u8>, count: usize) {
    bytes.extend_from_slice(&u32::try_from(count).unwrap_or(u32::MAX).to_be_bytes());
}

fn push_option(bytes: &mut Vec<u8>, value: Option<u64>) {
    bytes.push(u8::from(value.is_some()));
    bytes.extend_from_slice(&value.unwrap_or(0).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proofs_verify_for_every_leaf() {
        for count in 1..=9u8 {
            let leaves: Vec<Digest> = (0..count).map(|i| Digest([i; 32])).collect();
            let root = merkle_root(&leaves);
            for (index, leaf) in leaves.iter().enumerate() {
                let proof = merkle_proof(&leaves, index);
                assert!(
                    verify_proof(*leaf, &proof, &root),
                    "{count} leaves, leaf {index}"
                );
                assert!(!verify_proof(Digest([99; 32]), &proof, &root));
            }
        }
    }
}
