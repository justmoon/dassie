//! Operations waiting for a block. Members submit to every validator, so a
//! faulty leader holds one back for a view at most
//! (spec/designs/settlement-hubs.md#10-consensus).
//!
//! Whatever the pool keeps either pays when a block takes it or proves
//! nothing, and is gone after the next final block:
//!
//! - **A member's operation** is taken only if its signature verifies, its
//!   sequence is in the account's window, and the account can pay the fee
//!   of it and of everything it already has waiting. Per account, only the
//!   run of consecutive sequences from the account's current one is ever
//!   selected; an operation past a gap waits in a small set of its own
//!   ([`MAX_GAPPED`]) and is the first to go when the pool is full.
//! - **A validator's operation** is taken only if its signature verifies
//!   under the set in force and its sequence is above the validator's last;
//!   each validator has a bounded share ([`VALIDATOR_WINDOW`]), and after a
//!   change of validators what no longer verifies is dropped.
//! - **Deadlines have room of their own.** `claim` and `redeem` wait in a
//!   part of the pool ordinary operations never take
//!   (spec/designs/settlement-hubs.md#7-fees, "Deadlines first").
//! - **After each final block** the pool drops what the block refused, and
//!   what the new state makes impossible: a sequence behind, a fee the
//!   account can no longer pay, an account gone or closing.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::digest::Digest;
use crate::ledger::{Account, Ledger};
use crate::operation::{AccountNumber, Operation, SignedMemberOperation, SignedValidatorOperation};
use crate::rules::Amount;
use crate::validators::ValidatorIndex;

/// How far ahead of an account's sequence the pool takes its operations,
/// and so the most it keeps of one account.
pub const SEQUENCE_WINDOW: u64 = 16;

/// The most operations of one account the pool keeps past a gap in its
/// sequences. They are never selected until the gap fills.
pub const MAX_GAPPED: usize = 2;

/// The most operations of one validator the pool keeps. A validator's
/// sequences are times, not consecutive, so its window is a count.
pub const VALIDATOR_WINDOW: usize = 32;

/// How many blocks' worth of operations the pool holds.
const BLOCKS_KEPT: usize = 4;

#[derive(Clone, Debug)]
struct Entry {
    hash: Digest,
    operation: Operation,
}

/// One signer's operations, by sequence.
type Queue = BTreeMap<u64, Entry>;

/// Per signer, operations to take in order.
type Lists<'a> = Vec<Vec<&'a Operation>>;

/// How many members' operations wait, by part.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    ordinary: usize,
    deadlines: usize,
}

/// The pool.
#[derive(Clone, Debug)]
pub struct Mempool {
    ordinary_capacity: usize,
    deadline_capacity: usize,
    members: BTreeMap<AccountNumber, Queue>,
    validators: BTreeMap<ValidatorIndex, Queue>,
    counts: Counts,
    /// The height from which the set the validators' entries verify under
    /// is in force.
    set_since: u64,
}

/// What `fee` times `count` comes to, if `balance` covers it.
fn affords(balance: Amount, fee: Amount, count: usize) -> bool {
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    fee.checked_mul(count).is_some_and(|total| total <= balance)
}

/// How many of `queue`'s entries run on from `current` without a gap.
fn run_len(queue: &Queue, current: u64) -> usize {
    queue
        .keys()
        .zip(current..)
        .take_while(|(sequence, expected)| *sequence == expected)
        .count()
}

const fn max_fee(operation: &Operation) -> Amount {
    match operation {
        Operation::Member(signed) => signed.max_fee,
        Operation::Validator(_) => 0,
    }
}

impl Mempool {
    /// The pool of a hub whose final state is `ledger`: room for a few
    /// blocks of operations, the places kept for deadlines in their own
    /// part.
    #[must_use]
    pub fn new(ledger: &Ledger) -> Self {
        let limits = ledger.terms().limits;
        let deadlines = usize::from(limits.deadline_operations);
        let ordinary = usize::from(
            limits
                .block_operations
                .saturating_sub(limits.deadline_operations),
        );
        Self {
            ordinary_capacity: ordinary.saturating_mul(BLOCKS_KEPT).max(1),
            deadline_capacity: deadlines.saturating_mul(BLOCKS_KEPT).max(1),
            members: BTreeMap::new(),
            validators: BTreeMap::new(),
            counts: Counts::default(),
            set_since: ledger.set_since(),
        }
    }

    /// How many operations wait.
    #[must_use]
    pub fn len(&self) -> usize {
        self.counts
            .ordinary
            .saturating_add(self.counts.deadlines)
            .saturating_add(self.validators.values().map(BTreeMap::len).sum())
    }

    /// Whether none waits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Takes `operation` if it could apply on `ledger`, the final state,
    /// and there is room. Whether it was taken. An operation for a sequence
    /// already taken does not replace it.
    pub fn admit(&mut self, ledger: &Ledger, operation: Operation) -> bool {
        match &operation {
            Operation::Member(signed) => {
                let Some(account) = ledger.account(signed.account) else {
                    return false;
                };
                if !Self::member_fits(ledger, account, signed) {
                    return false;
                }
                let at = (signed.account, signed.sequence);
                self.admit_member(ledger, account, at, operation)
            }
            Operation::Validator(signed) => {
                if !Self::validator_fits(ledger, signed) {
                    return false;
                }
                let at = (signed.validator, signed.sequence);
                self.admit_validator(at, operation)
            }
        }
    }

    fn member_fits(ledger: &Ledger, account: &Account, signed: &SignedMemberOperation) -> bool {
        let current = account.sequence;
        signed.sequence >= current
            && signed.sequence < current.saturating_add(SEQUENCE_WINDOW)
            && signed.max_fee >= ledger.terms().fees.operation
            && !account.closing
            && signed.verifies(ledger.hub(), &account.key)
    }

    fn validator_fits(ledger: &Ledger, signed: &SignedValidatorOperation) -> bool {
        let (Some(key), Some(state)) = (
            ledger.validators().key(signed.validator),
            ledger.validator(signed.validator),
        ) else {
            return false;
        };
        signed.sequence > state.sequence && signed.verifies(ledger.hub(), key)
    }

    fn admit_member(
        &mut self,
        ledger: &Ledger,
        account: &Account,
        (number, sequence): (AccountNumber, u64),
        operation: Operation,
    ) -> bool {
        let queue = self.members.get(&number);
        let waiting = queue.map_or(0, BTreeMap::len);
        if queue.is_some_and(|q| q.contains_key(&sequence))
            || !affords(
                account.balance,
                ledger.terms().fees.operation,
                waiting.saturating_add(1),
            )
        {
            return false;
        }
        let run = queue.map_or(0, |q| run_len(q, account.sequence));
        let extends = u64::try_from(run)
            .ok()
            .and_then(|run| account.sequence.checked_add(run))
            == Some(sequence);
        if !extends && waiting.saturating_sub(run) >= MAX_GAPPED {
            return false;
        }
        let deadline = operation.has_deadline();
        if self.full(deadline) && (!extends || !self.evict_gapped(ledger, deadline)) {
            return false;
        }
        let entry = Entry {
            hash: operation.hash(),
            operation,
        };
        self.members
            .entry(number)
            .or_default()
            .insert(sequence, entry);
        self.count(deadline, true);
        true
    }

    fn admit_validator(
        &mut self,
        (index, sequence): (ValidatorIndex, u64),
        operation: Operation,
    ) -> bool {
        let queue = self.validators.entry(index).or_default();
        if queue.contains_key(&sequence) || queue.len() >= VALIDATOR_WINDOW {
            return false;
        }
        let hash = operation.hash();
        queue.insert(sequence, Entry { hash, operation });
        true
    }

    fn full(&self, deadline: bool) -> bool {
        if deadline {
            self.counts.deadlines >= self.deadline_capacity
        } else {
            self.counts.ordinary >= self.ordinary_capacity
        }
    }

    fn count(&mut self, deadline: bool, added: bool) {
        let count = if deadline {
            &mut self.counts.deadlines
        } else {
            &mut self.counts.ordinary
        };
        *count = if added {
            count.saturating_add(1)
        } else {
            count.saturating_sub(1)
        };
    }

    /// Makes room in a full part by dropping the last operation of that
    /// part that waits past a gap, if there is one.
    fn evict_gapped(&mut self, ledger: &Ledger, deadline: bool) -> bool {
        let mut victim = None;
        for (number, queue) in &self.members {
            let current = ledger.account(*number).map_or(0, |a| a.sequence);
            let run = run_len(queue, current);
            let last = queue
                .iter()
                .skip(run)
                .filter(|(_, entry)| entry.operation.has_deadline() == deadline)
                .map(|(sequence, _)| *sequence)
                .next_back();
            if let Some(sequence) = last {
                victim = Some((*number, sequence));
            }
        }
        let Some((number, sequence)) = victim else {
            return false;
        };
        let removed = self
            .members
            .get_mut(&number)
            .and_then(|queue| queue.remove(&sequence));
        if removed.is_some() {
            self.count(deadline, false);
        }
        self.members.retain(|_, queue| !queue.is_empty());
        removed.is_some()
    }

    /// The operations for a block on `base`: those with a deadline first,
    /// then the others in the places not kept for deadlines.
    ///
    /// Per account, only the run from its current sequence on `base` that
    /// it can pay for, its deadlines before its other operations, taken in
    /// turns across accounts so no account crowds out the rest. Validators'
    /// own operations take at most half the places not kept for deadlines,
    /// each validator an equal part of that half, so no validator can take
    /// every place.
    #[must_use]
    pub fn select(&self, base: &Ledger) -> Vec<Operation> {
        let limits = base.terms().limits;
        let limit = usize::from(limits.block_operations);
        let others_room = usize::from(
            limits
                .block_operations
                .saturating_sub(limits.deadline_operations),
        );
        let (deadlines, others) = self.stretches(base);
        let mut out = Vec::new();
        let complete = round_robin(&mut out, &deadlines, limit);
        let others: Lists<'_> = others
            .into_iter()
            .zip(complete)
            .filter_map(|(list, complete)| complete.then_some(list))
            .collect();
        let validator_room = (others_room / 2).max(1);
        let share = (validator_room / base.validators().len().max(1)).max(1);
        let validators = self.validator_runs(base, share);
        let free = others_room.min(limit.saturating_sub(out.len()));
        let before = out.len();
        round_robin(&mut out, &validators, validator_room.min(free));
        let free = free.saturating_sub(out.len().saturating_sub(before));
        round_robin(&mut out, &others, free);
        out
    }

    /// Per account, the deadlines and then the other operations a block on
    /// `base` can take.
    fn stretches(&self, base: &Ledger) -> (Lists<'_>, Lists<'_>) {
        let fee = base.terms().fees.operation;
        let mut deadlines = Vec::new();
        let mut others = Vec::new();
        for (number, queue) in &self.members {
            let Some(account) = base.account(*number).filter(|a| !a.closing) else {
                continue;
            };
            let mut mine = (Vec::new(), Vec::new());
            let run = queue.range(account.sequence..).zip(account.sequence..);
            for (count, ((sequence, entry), expected)) in (1..).zip(run) {
                let payable =
                    affords(account.balance, fee, count) && max_fee(&entry.operation) >= fee;
                let late_deadline = entry.operation.has_deadline() && !mine.1.is_empty();
                if *sequence != expected || !payable || late_deadline {
                    break;
                }
                if entry.operation.has_deadline() {
                    mine.0.push(&entry.operation);
                } else {
                    mine.1.push(&entry.operation);
                }
            }
            deadlines.push(mine.0);
            others.push(mine.1);
        }
        (deadlines, others)
    }

    /// Per validator of `base`'s set, its operations a block on `base` can
    /// take, at most `share` of them.
    fn validator_runs(&self, base: &Ledger, share: usize) -> Lists<'_> {
        self.validators
            .iter()
            .filter_map(|(index, queue)| {
                base.validators().key(*index)?;
                let last = base.validator(*index)?.sequence;
                let after = last.checked_add(1)?;
                Some(
                    queue
                        .range(after..)
                        .take(share)
                        .map(|(_, entry)| &entry.operation)
                        .collect(),
                )
            })
            .collect()
    }

    /// Drops the operations with these hashes, as a block on some state
    /// refused them.
    pub fn evict(&mut self, refused: &BTreeSet<Digest>) {
        if refused.is_empty() {
            return;
        }
        for queue in self
            .members
            .values_mut()
            .chain(self.validators.values_mut())
        {
            queue.retain(|_, entry| !refused.contains(&entry.hash));
        }
        self.tidy();
    }

    /// Forgets what `ledger`, a new final state, has passed or made
    /// impossible, and what its block `refused`.
    pub fn prune(&mut self, ledger: &Ledger, refused: &BTreeSet<Digest>) {
        let reverify = ledger.set_since() != self.set_since;
        self.set_since = ledger.set_since();
        for (index, queue) in &mut self.validators {
            let (Some(key), Some(state)) =
                (ledger.validators().key(*index), ledger.validator(*index))
            else {
                queue.clear();
                continue;
            };
            queue.retain(|sequence, entry| {
                let verifies = !reverify
                    || matches!(&entry.operation, Operation::Validator(signed)
                        if signed.verifies(ledger.hub(), key));
                *sequence > state.sequence && verifies && !refused.contains(&entry.hash)
            });
        }
        let fee = ledger.terms().fees.operation;
        for (number, queue) in &mut self.members {
            let account = ledger.account(*number).filter(|a| !a.closing);
            normalize(account, fee, queue, refused);
        }
        self.tidy();
    }

    /// Drops empty queues and counts anew.
    fn tidy(&mut self) {
        self.members.retain(|_, queue| !queue.is_empty());
        self.validators.retain(|_, queue| !queue.is_empty());
        self.counts = self.recount();
    }

    fn recount(&self) -> Counts {
        let mut counts = Counts::default();
        for entry in self.members.values().flat_map(BTreeMap::values) {
            if entry.operation.has_deadline() {
                counts.deadlines = counts.deadlines.saturating_add(1);
            } else {
                counts.ordinary = counts.ordinary.saturating_add(1);
            }
        }
        counts
    }

    /// Asserts its bounds.
    pub fn check_invariants(&self) {
        assert_eq!(self.counts, self.recount(), "mempool counts out of step");
        assert!(
            self.counts.ordinary <= self.ordinary_capacity
                && self.counts.deadlines <= self.deadline_capacity,
            "mempool over its bound"
        );
        let window = usize::try_from(SEQUENCE_WINDOW).unwrap_or(usize::MAX);
        assert!(
            self.members.values().all(|q| q.len() <= window),
            "an account over its window"
        );
        assert!(
            self.validators
                .values()
                .all(|q| q.len() <= VALIDATOR_WINDOW)
                && self.validators.len() <= crate::validators::MAX_VALIDATORS,
            "a validator over its window"
        );
    }
}

/// Keeps of an account's queue only what can still apply on the new final
/// state: its sequences from the account's current one within the window,
/// what it can pay the fees of, in sequence order, and at most
/// [`MAX_GAPPED`] past a gap.
fn normalize(
    account: Option<&Account>,
    fee: Amount,
    queue: &mut Queue,
    refused: &BTreeSet<Digest>,
) {
    let Some(account) = account else {
        queue.clear();
        return;
    };
    let current = account.sequence;
    let end = current.saturating_add(SEQUENCE_WINDOW);
    queue.retain(|sequence, entry| {
        (current..end).contains(sequence)
            && max_fee(&entry.operation) >= fee
            && !refused.contains(&entry.hash)
    });
    let payable =
        usize::try_from(account.balance.checked_div(fee).unwrap_or(u64::MAX)).unwrap_or(usize::MAX);
    let keep = payable.min(run_len(queue, current).saturating_add(MAX_GAPPED));
    if let Some(cut) = queue.keys().nth(keep).copied() {
        queue.split_off(&cut);
    }
}

/// A list of one signer's operations being taken in turns.
struct Turn<'a, 'b> {
    list: &'b [&'a Operation],
    next: usize,
}

/// Takes from `lists` in turns, one from each, at most `max` in all.
/// Whether each list was taken whole.
fn round_robin(out: &mut Vec<Operation>, lists: &Lists<'_>, max: usize) -> Vec<bool> {
    let mut turns: Vec<Turn<'_, '_>> = lists.iter().map(|list| Turn { list, next: 0 }).collect();
    let mut taken = 0usize;
    let mut progress = true;
    while progress && taken < max {
        progress = false;
        for turn in &mut turns {
            if taken >= max {
                break;
            }
            if let Some(operation) = turn.list.get(turn.next) {
                out.push((*operation).clone());
                turn.next = turn.next.saturating_add(1);
                taken = taken.saturating_add(1);
                progress = true;
            }
        }
    }
    turns
        .iter()
        .map(|turn| turn.next >= turn.list.len())
        .collect()
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use dassie_identity::{NodeKey, Signature};

    use super::*;
    use crate::ledger::tests::{Hub, OP_FEE, reconfigure, resigned};
    use crate::operation::{MemberOperation, SignedMemberOperation, ValidatorOperation};
    use crate::promise::{Claim, HoldNumber, Signed};
    use crate::rules::tests::rules;

    /// A hub whose blocks take 8 operations, 2 of them kept for deadlines:
    /// a pool of 24 ordinary operations and 8 with deadlines.
    fn small() -> Hub {
        Hub::with(|rules| {
            rules.terms.limits.block_operations = 8;
            rules.terms.limits.deadline_operations = 2;
        })
    }

    fn signed(
        hub: &Hub,
        (account, key): (AccountNumber, &NodeKey),
        sequence: u64,
        operation: MemberOperation,
    ) -> Operation {
        Operation::Member(SignedMemberOperation::sign(
            hub.ledger.hub(),
            account,
            sequence,
            OP_FEE,
            operation,
            key,
        ))
    }

    fn transfer() -> MemberOperation {
        MemberOperation::Transfer {
            to: AccountNumber(1),
            amount: 1,
        }
    }

    fn redeem() -> MemberOperation {
        MemberOperation::Redeem(Signed {
            value: Claim {
                hold: HoldNumber(1),
                total: 1,
                through: 1,
            },
            signature: Signature::from_bytes([0; 64]),
        })
    }

    /// A member whose balance pays `fees` operations.
    fn member(hub: &mut Hub, seed: u8, fees: u64) -> (AccountNumber, NodeKey, u64) {
        let minimum = hub.ledger.terms().new_account_minimum();
        let (number, key) = hub.join(seed, minimum + 2 * OP_FEE + fees * OP_FEE);
        // Spend down to what it should have.
        let spend = MemberOperation::Transfer {
            to: AccountNumber(1),
            amount: hub.balance(number) - OP_FEE - fees * OP_FEE,
        };
        hub.run(number, &key, spend);
        let sequence = hub.ledger.account(number).unwrap().sequence;
        assert_eq!(hub.balance(number), fees * OP_FEE);
        (number, key, sequence)
    }

    #[test]
    fn an_account_gets_in_only_what_it_can_pay_for() {
        let mut hub = small();
        let (number, key, sequence) = member(&mut hub, 1, 2);
        let mut pool = Mempool::new(&hub.ledger);
        for (offset, taken) in [(0, true), (1, true), (2, false)] {
            let op = signed(&hub, (number, &key), sequence + offset, transfer());
            assert_eq!(pool.admit(&hub.ledger, op), taken, "sequence +{offset}");
        }
        pool.check_invariants();
    }

    #[test]
    fn operations_past_a_gap_wait_and_are_never_selected() {
        let mut hub = small();
        let (number, key, sequence) = member(&mut hub, 1, 100);
        let mut pool = Mempool::new(&hub.ledger);
        let op = |offset: u64| signed(&hub, (number, &key), sequence + offset, transfer());
        assert!(pool.admit(&hub.ledger, op(1)));
        assert!(pool.admit(&hub.ledger, op(3)));
        assert!(!pool.admit(&hub.ledger, op(5)), "more than MAX_GAPPED");
        assert!(pool.select(&hub.ledger).is_empty());
        assert!(pool.admit(&hub.ledger, op(0)));
        assert_eq!(pool.select(&hub.ledger), [op(0), op(1)]);
        pool.check_invariants();
    }

    #[test]
    fn a_full_pool_keeps_room_for_deadlines_and_drops_what_waits_past_a_gap() {
        let mut hub = small();
        let (a, a_key, a_seq) = member(&mut hub, 1, 100);
        let (b, b_key, b_seq) = member(&mut hub, 2, 100);
        let (c, c_key, c_seq) = member(&mut hub, 3, 100);
        let mut pool = Mempool::new(&hub.ledger);
        for offset in (0..14).chain([15]) {
            let op = signed(&hub, (a, &a_key), a_seq + offset, transfer());
            assert!(pool.admit(&hub.ledger, op));
        }
        for offset in 0..9 {
            let op = signed(&hub, (b, &b_key), b_seq + offset, transfer());
            assert!(pool.admit(&hub.ledger, op));
        }
        assert_eq!(pool.len(), 24, "the ordinary part is full");
        // What waits past a gap goes first.
        let first = signed(&hub, (c, &c_key), c_seq, transfer());
        assert!(pool.admit(&hub.ledger, first));
        assert_eq!(pool.len(), 24);
        let second = signed(&hub, (c, &c_key), c_seq + 1, transfer());
        assert!(!pool.admit(&hub.ledger, second), "nothing left to drop");
        // A deadline has room of its own.
        let deadline = signed(&hub, (c, &c_key), c_seq + 1, redeem());
        assert!(pool.admit(&hub.ledger, deadline));
        pool.check_invariants();
    }

    #[test]
    fn one_validator_takes_only_its_share_of_a_block() {
        let mut hub = small();
        let (number, key, sequence) = member(&mut hub, 1, 100);
        let mut pool = Mempool::new(&hub.ledger);
        for i in 0..VALIDATOR_WINDOW {
            let op = hub.validator_op(0, ValidatorOperation::SetEndpoint(i.to_string()));
            assert!(pool.admit(&hub.ledger, op));
        }
        let op = hub.validator_op(0, ValidatorOperation::Shutdown);
        assert!(!pool.admit(&hub.ledger, op), "over its window");
        for offset in 0..10 {
            let op = signed(&hub, (number, &key), sequence + offset, transfer());
            assert!(pool.admit(&hub.ledger, op));
        }
        let block = pool.select(&hub.ledger);
        let validators = block
            .iter()
            .filter(|op| matches!(op, Operation::Validator(_)))
            .count();
        assert_eq!(validators, 1, "half of 6 places, split four ways");
        assert_eq!(block.len(), 6, "every place not kept for deadlines");
    }

    #[test]
    fn after_a_block_the_pool_drops_what_can_no_longer_apply() {
        let mut hub = small();
        let (number, key, sequence) = member(&mut hub, 1, 3);
        let (other, other_key, other_seq) = member(&mut hub, 2, 3);
        let mut pool = Mempool::new(&hub.ledger);
        for offset in 0..2 {
            let op = signed(&hub, (number, &key), sequence + offset, transfer());
            assert!(pool.admit(&hub.ledger, op));
        }
        let refused = signed(&hub, (other, &other_key), other_seq, transfer());
        assert!(pool.admit(&hub.ledger, refused.clone()));
        // Another operation at the same sequence spends all but half a
        // fee: the next one can no longer pay.
        let spend = MemberOperation::Transfer {
            to: AccountNumber(1),
            amount: OP_FEE + OP_FEE / 2,
        };
        let spent = signed(&hub, (number, &key), sequence, spend);
        hub.block(alloc::vec![spent]);
        let refused: BTreeSet<Digest> = [refused.hash()].into_iter().collect();
        pool.prune(&hub.ledger, &refused);
        assert!(pool.is_empty());
        pool.check_invariants();
    }

    #[test]
    fn a_change_of_validators_drops_what_no_longer_verifies() {
        let mut hub = Hub::new();
        let mut pool = Mempool::new(&hub.ledger);
        let op = hub.validator_op(3, ValidatorOperation::SetEndpoint("x".to_string()));
        assert!(pool.admit(&hub.ledger, op));
        let (rules, _) = rules(4);
        let rules = resigned(rules, |rules| {
            rules.validators[3].consensus = NodeKey::from_bytes(&[78; 32]).public_key();
        });
        reconfigure(&mut hub, &rules);
        pool.prune(&hub.ledger, &BTreeSet::new());
        assert!(pool.is_empty());
    }
}
