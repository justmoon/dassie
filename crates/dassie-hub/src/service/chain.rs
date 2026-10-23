//! The hub's ledger as consensus's application: it builds a leader's
//! block, checks another leader's by executing it, and keeps the state
//! after every block it executed until one becomes final. A block may build
//! on a parent that is not final yet, so the state after each one is kept,
//! bounded as consensus bounds the blocks it holds.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use dassie_oer::Encode;

use super::mempool::Mempool;
use super::payload::Payload;
use crate::consensus::{
    Application, Block, Declined, MAX_BLOCKS, ProposeContext, VerifyContext, Vote,
};
use crate::digest::Digest;
use crate::ledger::{self, Ledger, Outcome, results_root};
use crate::validators::Signers;

/// A block executed, and the state after it.
#[derive(Clone, Debug)]
pub(super) struct Executed {
    pub ledger: Ledger,
    pub payload: Payload,
    pub outcomes: Vec<Outcome>,
}

/// A block that became final, for the statements validators make of it.
#[derive(Clone, Debug)]
pub struct FinalBlock {
    /// Its height.
    pub height: u64,
    /// Its timestamp.
    pub timestamp: u64,
    /// Its digest.
    pub digest: Digest,
    /// Its operations.
    pub operations: Vec<crate::operation::Operation>,
    /// Their outcomes.
    pub outcomes: Vec<Outcome>,
}

/// The ledger's side of consensus.
#[derive(Clone, Debug)]
pub struct Chain {
    /// The state after the last final block.
    pub(super) ledger: Ledger,
    /// The last final block's digest.
    pub(super) tip: Digest,
    executed: BTreeMap<Digest, Executed>,
    proposed: Option<(u64, Executed)>,
    /// The heights of recent blocks, to record votes for them by height.
    heights: BTreeMap<Digest, u64>,
    pub(super) mempool: Mempool,
    pub(super) finals: VecDeque<FinalBlock>,
}

impl Chain {
    /// The chain of `ledger`, the state after the final block `tip`.
    #[must_use]
    pub fn new(ledger: Ledger, tip: Digest, mempool_capacity: usize) -> Self {
        let mut heights = BTreeMap::new();
        heights.insert(tip, ledger.height());
        Self {
            ledger,
            tip,
            executed: BTreeMap::new(),
            proposed: None,
            heights,
            mempool: Mempool::new(mempool_capacity),
            finals: VecDeque::new(),
        }
    }

    /// The state a block on `parent` builds on.
    fn base(&self, parent: &Digest) -> Option<&Ledger> {
        if *parent == self.tip {
            return Some(&self.ledger);
        }
        self.executed.get(parent).map(|executed| &executed.ledger)
    }

    /// The votes a block at `height` on `base` records: those for blocks
    /// within the vote window that no block recorded yet.
    fn votes_to_record(&self, base: &Ledger, height: u64, votes: &[Vote]) -> Vec<(u64, Signers)> {
        let window = u64::from(base.terms().limits.vote_window);
        let floor = height.saturating_sub(window).max(1);
        let mut by_height: BTreeMap<u64, Signers> = BTreeMap::new();
        for vote in votes {
            let Some(voted) = self.heights.get(&vote.block).copied() else {
                continue;
            };
            if voted < floor || voted >= height {
                continue;
            }
            let entry = by_height.entry(voted).or_default();
            *entry = entry.with(vote.signer);
        }
        by_height
            .into_iter()
            .filter_map(|(voted, signers)| {
                let new = signers.bits() & !base.recorded_voters(voted).bits();
                (new != 0).then_some((voted, Signers::from_bits(new)))
            })
            .collect()
    }

    /// Executes `operations` and `votes` as the block at `height` on
    /// `base`.
    fn execute(
        base: &Ledger,
        height: u64,
        timestamp: u64,
        operations: Vec<crate::operation::Operation>,
        votes: Vec<(u64, Signers)>,
    ) -> Option<Executed> {
        let mut ledger = base.clone();
        let block = ledger::Block {
            height,
            timestamp,
            operations: &operations,
            votes: &votes,
        };
        let outcomes = ledger.apply(&block).ok()?;
        let payload = Payload {
            results: results_root(&operations, &outcomes),
            state: ledger.state_root(),
            operations,
            votes,
        };
        Some(Executed {
            ledger,
            payload,
            outcomes,
        })
    }

    /// Our own proposal got its digest: it is kept as any executed block.
    pub(super) fn adopt_proposal(&mut self, view: u64, digest: Digest) {
        if let Some((proposed_view, executed)) = self.proposed.take()
            && proposed_view == view
        {
            self.keep(digest, executed);
        }
    }

    fn keep(&mut self, digest: Digest, executed: Executed) {
        while self.executed.len() >= MAX_BLOCKS {
            let lowest = self
                .executed
                .iter()
                .min_by_key(|(_, e)| e.ledger.height())
                .map(|(d, _)| *d);
            match lowest {
                Some(digest) => self.executed.remove(&digest),
                None => break,
            };
        }
        self.heights.insert(digest, executed.ledger.height());
        self.executed.insert(digest, executed);
    }

    /// Asserts what holds between steps.
    pub fn check_invariants(&self) {
        assert!(
            self.executed.len() <= MAX_BLOCKS,
            "executed blocks over their bound"
        );
        assert!(
            self.executed
                .values()
                .all(|e| e.ledger.height() > self.ledger.height()),
            "a block kept at or below the final one"
        );
        self.mempool.check_invariants();
        self.ledger.check_invariants();
    }
}

impl Application for Chain {
    fn propose(
        &mut self,
        context: &ProposeContext<'_>,
        payload: &mut Vec<u8>,
    ) -> Result<(), Declined> {
        let base = self.base(&context.parent).ok_or(Declined)?;
        let votes = self.votes_to_record(base, context.height, context.votes);
        let limits = base.terms().limits;
        let candidates = self.mempool.select(
            base,
            usize::from(limits.block_operations),
            usize::from(limits.deadline_operations),
        );
        let first = Self::execute(
            base,
            context.height,
            context.timestamp_ms,
            candidates.clone(),
            votes.clone(),
        )
        .ok_or(Declined)?;
        // Operations refused on this base pay nothing and do nothing: leave
        // them out, so the block carries only what applies.
        let kept: Vec<_> = candidates
            .into_iter()
            .zip(&first.outcomes)
            .filter(|(_, outcome)| outcome.used_sequence())
            .map(|(operation, _)| operation)
            .collect();
        let executed = if kept.len() == first.outcomes.len() {
            first
        } else {
            Self::execute(base, context.height, context.timestamp_ms, kept, votes)
                .ok_or(Declined)?
        };
        *payload = executed.payload.to_vec();
        self.proposed = Some((context.view, executed));
        Ok(())
    }

    fn verify(&mut self, context: &VerifyContext, payload: &[u8]) -> bool {
        let Ok(proposed) = Payload::decode_exact(payload) else {
            return false;
        };
        let Some(base) = self.base(&context.parent) else {
            return false;
        };
        let Some(executed) = Self::execute(
            base,
            context.height,
            context.timestamp_ms,
            proposed.operations.clone(),
            proposed.votes.clone(),
        ) else {
            return false;
        };
        if executed.payload != proposed {
            return false;
        }
        self.keep(context.block, executed);
        true
    }

    fn finalized(&mut self, block: &Block, digest: &Digest) {
        let executed = match self.executed.remove(digest) {
            Some(executed) => executed,
            // A block certified while we were behind: it is the next one on
            // the final state, and q validators executed it as we will.
            None => {
                let payload = Payload::decode_exact(&block.payload);
                assert!(payload.is_ok(), "a final block's payload does not decode");
                let Ok(payload) = payload else { return };
                let executed = Self::execute(
                    &self.ledger,
                    block.height,
                    block.timestamp_ms,
                    payload.operations.clone(),
                    payload.votes.clone(),
                );
                assert!(
                    executed.as_ref().is_some_and(|e| e.payload == payload),
                    "a final block does not execute as certified"
                );
                let Some(executed) = executed else { return };
                executed
            }
        };
        let hashes: Vec<Digest> = executed
            .payload
            .operations
            .iter()
            .map(|o| o.hash())
            .collect();
        self.ledger = executed.ledger;
        self.tip = *digest;
        self.heights.insert(*digest, block.height);
        let floor = block
            .height
            .saturating_sub(u64::from(self.ledger.terms().limits.vote_window));
        self.heights.retain(|_, height| *height >= floor);
        let final_height = block.height;
        self.executed
            .retain(|_, e| e.ledger.height() > final_height);
        self.mempool.prune(&self.ledger, &hashes);
        if self.finals.len() >= MAX_BLOCKS {
            self.finals.pop_front();
        }
        self.finals.push_back(FinalBlock {
            height: block.height,
            timestamp: block.timestamp_ms,
            digest: *digest,
            operations: executed.payload.operations,
            outcomes: executed.outcomes,
        });
    }
}
