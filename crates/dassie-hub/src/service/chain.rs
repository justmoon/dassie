//! The hub's ledger as consensus's application: it builds a leader's
//! block, checks another leader's by executing it, and keeps the state
//! after every block it executed until one becomes final. A block may build
//! on a parent that is not final yet, so the state after each one is kept.
//!
//! Each kept state is a copy of the ledger, as large as the terms' tables
//! let it grow, so at most [`MAX_EXECUTED`] are kept: while finality
//! stalls the lowest go first, never the one just verified. The chain says
//! which it holds (`holds`), and the engine verifies a block whose state
//! went again, from the nearest state still held, before it relies on it.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

use dassie_oer::Encode;

/// The most executed blocks whose state is kept while they wait to become
/// final.
pub const MAX_EXECUTED: usize = 64;

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
    /// For each operation, the accounts it touches: its signer's, and the
    /// payer's and payee's of a hold it acts on, as of before the block,
    /// so a hold the block closes still counts.
    pub touched: Vec<Vec<crate::operation::AccountNumber>>,
}

impl FinalBlock {
    /// About what it holds, in bytes: its operations as encoded, and what
    /// is kept beside each.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.operations
            .iter()
            .zip(&self.touched)
            .map(|(operation, touched)| {
                operation
                    .encoded_len()
                    .saturating_add(core::mem::size_of::<Outcome>())
                    .saturating_add(touched.len().saturating_mul(4))
            })
            .sum()
    }
}

/// The accounts `operation` touches on `before`, the state before its
/// block, and the account a `pay` opened, by its `outcome`, so that the
/// payee reads who paid it in (spec/designs/settlement-hubs.md#5-joining).
fn accounts_touched(
    before: &Ledger,
    operation: &crate::operation::Operation,
    outcome: Outcome,
) -> Vec<crate::operation::AccountNumber> {
    use crate::operation::{CloseTo, MemberOperation as M, Operation as O};
    let O::Member(signed) = operation else {
        return Vec::new();
    };
    let mut accounts = alloc::vec![signed.account];
    if let (
        M::Pay { .. },
        Outcome::Applied {
            detail: crate::ledger::Detail::Account(opened),
            ..
        },
    ) = (&signed.operation, outcome)
    {
        accounts.push(opened);
    }
    let hold = match &signed.operation {
        M::HoldFund { hold, .. }
        | M::HoldReduce { hold, .. }
        | M::HoldRelease { hold }
        | M::Claim { hold, .. } => Some(*hold),
        M::Redeem(claim) => Some(claim.value.hold),
        M::Dispute { hold, .. } => Some(*hold),
        M::HoldOpen { payee, .. }
        | M::Transfer { to: payee, .. }
        | M::Close(CloseTo::Account(payee)) => {
            accounts.push(*payee);
            None
        }
        _ => None,
    };
    if let Some(hold) = hold.and_then(|number| before.hold(number)) {
        accounts.push(hold.payer);
        accounts.push(hold.payee);
    }
    accounts.sort_unstable();
    accounts.dedup();
    accounts
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
    /// A block was final that does not decode or execute as certified.
    halted: bool,
    /// The highest final block dropped before the service took it.
    pub(super) lost: Option<u64>,
}

impl Chain {
    /// The chain of `ledger`, the state after the final block `tip`.
    #[must_use]
    pub fn new(ledger: Ledger, tip: Digest) -> Self {
        let mut heights = BTreeMap::new();
        heights.insert(tip, ledger.height());
        let mempool = Mempool::new(&ledger);
        Self {
            ledger,
            tip,
            executed: BTreeMap::new(),
            proposed: None,
            heights,
            mempool,
            finals: VecDeque::new(),
            halted: false,
            lost: None,
        }
    }

    /// The state a block on `parent` builds on.
    pub(super) fn base(&self, parent: &Digest) -> Option<&Ledger> {
        if *parent == self.tip {
            return Some(&self.ledger);
        }
        self.executed.get(parent).map(|executed| &executed.ledger)
    }

    /// The votes a block at `height` on `base` records: those for blocks
    /// within the vote window that no block recorded yet.
    fn votes_to_record(&self, base: &Ledger, height: u64, votes: &[Vote]) -> Vec<(u64, Signers)> {
        let window = u64::from(base.terms().limits.vote_window);
        let floor = height.saturating_sub(window).max(base.set_since());
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
    pub(super) fn execute(
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

    /// Executes a block certified while we were behind on the final state,
    /// if it decodes and executes as certified. One that does not means
    /// more than f validators are faulty, or a bug.
    fn execute_certified(&self, block: &Block) -> Option<Executed> {
        let payload = Payload::decode_exact(&block.payload).ok()?;
        let executed = Self::execute(
            &self.ledger,
            block.height,
            block.timestamp_ms,
            payload.operations.clone(),
            payload.votes.clone(),
        )?;
        (executed.payload == payload).then_some(executed)
    }

    /// Whether a block was final that this ledger cannot follow: the
    /// validator stops (spec/areas/hubs.md#the-validators-service).
    #[must_use]
    pub const fn halted(&self) -> bool {
        self.halted
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
        while self.executed.len() >= MAX_EXECUTED {
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
            self.executed.len() <= MAX_EXECUTED,
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
    fn holds(&mut self, block: &Digest) -> bool {
        // The final state always; others while kept. One let go of is
        // verified again by the engine from the nearest state held.
        *block == self.tip || self.executed.contains_key(block)
    }

    fn scheduled_change(
        &mut self,
        after: &Digest,
    ) -> Option<(u64, crate::validators::ValidatorSet)> {
        // The state after `after` names the change once its notice passed,
        // final or not; the engine asks about every final block too
        // (spec/decisions/epoch-handover.md).
        self.base(after)?.scheduled_change()
    }

    fn propose(
        &mut self,
        context: &ProposeContext<'_>,
        payload: &mut Vec<u8>,
    ) -> Result<(), Declined> {
        if self.halted {
            return Err(Declined);
        }
        let base = self.base(&context.parent).ok_or(Declined)?;
        let votes = self.votes_to_record(base, context.height, context.votes);
        let candidates = self.mempool.select(base);
        let (height, timestamp) = (context.height, context.timestamp_ms);
        let first = Self::execute(base, height, timestamp, candidates.clone(), votes.clone())
            .ok_or(Declined)?;
        // Operations refused on this base pay nothing and do nothing: leave
        // them out, so the block carries only what applies, and drop them
        // from the pool, so they never take a place again.
        let (kept, refused): (Vec<_>, Vec<_>) = candidates
            .into_iter()
            .zip(first.outcomes.iter().map(Outcome::used_sequence))
            .partition(|(_, used)| *used);
        let executed = if refused.is_empty() {
            first
        } else {
            let kept = kept.into_iter().map(|(operation, _)| operation).collect();
            Self::execute(base, height, timestamp, kept, votes).ok_or(Declined)?
        };
        let refused = refused
            .iter()
            .map(|(operation, _)| operation.hash())
            .collect();
        self.mempool.evict(&refused);
        *payload = executed.payload.to_vec();
        self.proposed = Some((context.view, executed));
        Ok(())
    }

    fn verify(&mut self, context: &VerifyContext, payload: &[u8]) -> bool {
        if self.halted {
            return false;
        }
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
        if self.halted {
            return;
        }
        let executed = match self.executed.remove(digest) {
            Some(executed) => executed,
            // A block certified while we were behind: it is the next one on
            // the final state, and q validators executed it as we will.
            None => match self.execute_certified(block) {
                Some(executed) => executed,
                None => {
                    self.halted = true;
                    return;
                }
            },
        };
        let refused: BTreeSet<Digest> = executed
            .payload
            .operations
            .iter()
            .zip(&executed.outcomes)
            .filter(|(_, outcome)| !outcome.used_sequence())
            .map(|(operation, _)| operation.hash())
            .collect();
        let touched = executed
            .payload
            .operations
            .iter()
            .zip(&executed.outcomes)
            .map(|(operation, outcome)| accounts_touched(&self.ledger, operation, *outcome))
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
        self.mempool.prune(&self.ledger, &refused);
        // The service takes the final blocks after every step; one it never
        // took is lost to its history, and it says so.
        if self.finals.len() >= MAX_BLOCKS
            && let Some(dropped) = self.finals.pop_front()
        {
            self.lost = Some(self.lost.map_or(dropped.height, |h| h.max(dropped.height)));
        }
        self.finals.push_back(FinalBlock {
            height: block.height,
            timestamp: block.timestamp_ms,
            digest: *digest,
            operations: executed.payload.operations,
            outcomes: executed.outcomes,
            touched,
        });
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::consensus::MAX_PAYLOAD_BYTES;
    use crate::ledger::tests::Hub;
    use crate::service::mempool::tests::{claim, member, signed};
    use crate::validators::ValidatorIndex;

    /// The context of the next block on `chain`'s final state.
    fn next(chain: &Chain) -> ProposeContext<'static> {
        ProposeContext {
            view: 1,
            proposer: ValidatorIndex::new(0).unwrap(),
            parent: chain.tip,
            height: chain.ledger.height() + 1,
            timestamp_ms: chain.ledger.timestamp() + 10,
            votes: &[],
        }
    }

    #[test]
    fn a_leader_fills_a_block_by_bytes_too() {
        let mut hub = Hub::with(|rules| {
            rules.terms.limits.commitments_per_claim = 1024;
        });
        let (number, key, sequence) = member(&mut hub, 1, 10);
        let mut chain = Chain::new(hub.ledger.clone(), Digest::default());
        // Four of the largest claims are more than a payload takes.
        for offset in 0..4 {
            let op = signed(&hub, (number, &key), sequence + offset, claim(1024));
            assert!(chain.mempool.admit(&chain.ledger, op));
        }
        let mut payload = Vec::new();
        chain.propose(&next(&chain), &mut payload).unwrap();
        assert!(payload.len() <= MAX_PAYLOAD_BYTES);
        let operations = Payload::decode_exact(&payload).unwrap().operations;
        assert_eq!(operations.len(), 3, "as many as fit, in sequence order");
    }

    #[test]
    fn a_close_into_another_account_touches_it() {
        let mut hub = Hub::new();
        let (number, key, sequence) = member(&mut hub, 1, 10);
        let close = crate::operation::MemberOperation::Close(crate::operation::CloseTo::Account(
            crate::operation::AccountNumber(1),
        ));
        let op = signed(&hub, (number, &key), sequence, close);
        let touched = accounts_touched(&hub.ledger, &op, Outcome::Refused(ledger::Refusal::Fee));
        assert!(touched.contains(&crate::operation::AccountNumber(1)));
    }

    // spec/designs/settlement-hubs.md#5-joining: the payee of a `pay` reads
    // it, in the history of the account it opened.
    #[test]
    fn a_pay_touches_the_account_it_opened() {
        let mut hub = Hub::new();
        let (number, key, sequence) = member(&mut hub, 1, 10);
        let pay = crate::operation::MemberOperation::Pay {
            key: dassie_identity::NodeKey::from_bytes(&[77; 32]).public_key(),
            amount: 5_000_000,
        };
        let op = signed(&hub, (number, &key), sequence, pay);
        let opened = crate::operation::AccountNumber(9);
        let applied = Outcome::Applied {
            fee: 10,
            detail: ledger::Detail::Account(opened),
        };
        assert!(accounts_touched(&hub.ledger, &op, applied).contains(&opened));
        let failed = Outcome::Failed {
            fee: 10,
            reason: ledger::Failure::Exists,
        };
        assert_eq!(accounts_touched(&hub.ledger, &op, failed), [number]);
    }

    /// The next block on `chain`'s final state, with nothing in it, and
    /// its state.
    pub(in crate::service) fn empty_block(chain: &Chain) -> (Block, Executed) {
        let height = chain.ledger.height() + 1;
        let timestamp = chain.ledger.timestamp() + 10;
        let executed =
            Chain::execute(&chain.ledger, height, timestamp, Vec::new(), Vec::new()).unwrap();
        let block = Block {
            epoch: 0,
            view: height,
            height,
            parent: chain.tip,
            parent_view: height - 1,
            timestamp_ms: timestamp,
            payload: executed.payload.to_vec(),
        };
        (block, executed)
    }

    #[test]
    fn a_final_block_that_does_not_execute_halts_the_chain() {
        let hub = Hub::new();
        let mut chain = Chain::new(hub.ledger.clone(), Digest::default());
        let (mut block, _) = empty_block(&chain);
        block.payload = alloc::vec![1, 2, 3];
        chain.finalized(&block, &Digest([9; 32]));
        assert!(chain.halted());
        assert_eq!(chain.ledger, hub.ledger, "the final state stays");
        let mut payload = Vec::new();
        assert!(chain.propose(&next(&chain), &mut payload).is_err());
    }

    #[test]
    fn the_states_kept_for_blocks_not_yet_final_are_bounded() {
        let hub = Hub::new();
        let mut chain = Chain::new(hub.ledger.clone(), Digest::default());
        let (_, executed) = empty_block(&chain);
        for i in 0..=MAX_EXECUTED {
            let digest = Digest([u8::try_from(i % 256).unwrap(); 32]);
            chain.keep(digest, executed.clone());
        }
        assert_eq!(chain.executed.len(), MAX_EXECUTED);
        chain.check_invariants();
    }

    // The end of an epoch is a property of the chain being extended: a
    // block not yet final that passed the change's notice names it, before
    // the final state does (spec/decisions/epoch-handover.md).
    #[test]
    fn a_change_is_seen_on_the_chain_before_it_is_final() {
        use crate::ledger::tests::resigned;
        use crate::operation::ValidatorOperation;

        let mut hub = Hub::new();
        let rules = resigned(hub.ledger.rules().clone(), |rules| {
            rules.terms.fees.operation = 20;
        });
        let ops = (0..3)
            .map(|i| hub.validator_op(i, ValidatorOperation::Reconfigure(rules.clone())))
            .collect();
        hub.block(ops);
        let mut chain = Chain::new(hub.ledger.clone(), Digest([1; 32]));
        assert_eq!(chain.scheduled_change(&chain.tip.clone()), None);
        // A block past the notice, verified and not final.
        let at = hub.ledger.timestamp() + hub.ledger.terms().timers.reconfigure_notice;
        let height = hub.ledger.height() + 1;
        let executed = Chain::execute(&chain.ledger, height, at, Vec::new(), Vec::new()).unwrap();
        let context = VerifyContext {
            view: 1,
            proposer: ValidatorIndex::new(0).unwrap(),
            parent: chain.tip,
            height,
            timestamp_ms: at,
            block: Digest([2; 32]),
        };
        assert!(chain.verify(&context, &executed.payload.to_vec()));
        let change = chain.scheduled_change(&Digest([2; 32]));
        let (from, set) = change.clone().expect("the chain names the change");
        assert_eq!(from, height + 2);
        assert_eq!(Some(set), rules.validator_set());
        // Asking again about the same block gives the same answer.
        assert!(chain.verify(&context, &executed.payload.to_vec()));
        assert_eq!(chain.scheduled_change(&Digest([2; 32])), change);
        // Once it is final, the final state agrees.
        let block = Block {
            epoch: 0,
            view: 1,
            height,
            parent: chain.tip,
            parent_view: 0,
            timestamp_ms: at,
            payload: executed.payload.to_vec(),
        };
        chain.finalized(&block, &Digest([2; 32]));
        assert_eq!(chain.scheduled_change(&Digest([2; 32])), change);
    }

    // The service keeps a bounded number of states and says which it holds:
    // the final one always, the one just verified at least until the next,
    // and none it let go of (spec/areas/hubs.md#consensus).
    #[test]
    fn the_states_it_holds_are_the_ones_it_says() {
        let hub = Hub::new();
        let mut chain = Chain::new(hub.ledger.clone(), Digest([1; 32]));
        let tip = chain.tip;
        assert!(chain.holds(&tip));
        let mut parent = chain.tip;
        let mut digests = Vec::new();
        for i in 0..=MAX_EXECUTED {
            let base = chain.base(&parent).unwrap().clone();
            let height = base.height() + 1;
            let timestamp = base.timestamp() + 10;
            let executed =
                Chain::execute(&base, height, timestamp, Vec::new(), Vec::new()).unwrap();
            let digest = Digest([u8::try_from(i % 200).unwrap() + 2; 32]);
            let context = VerifyContext {
                view: height,
                proposer: ValidatorIndex::new(0).unwrap(),
                parent,
                height,
                timestamp_ms: timestamp,
                block: digest,
            };
            assert!(chain.verify(&context, &executed.payload.to_vec()));
            assert!(chain.holds(&digest), "the block just verified");
            digests.push(digest);
            parent = digest;
        }
        assert!(!chain.holds(&digests[0]), "the lowest was let go of");
        assert!(chain.holds(&tip), "the final state always");
        assert!(chain.holds(digests.last().unwrap()));
        chain.check_invariants();
    }
}
