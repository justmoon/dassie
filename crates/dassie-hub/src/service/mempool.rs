//! Operations waiting for a block. Members submit to every validator, so a
//! faulty leader holds one back for a view at most
//! (spec/designs/settlement-hubs.md#10-consensus). An operation is taken
//! only if it could apply now: its signature verifies, and its sequence is
//! not behind; so whatever floods the pool pays, or proves nothing.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::digest::Digest;
use crate::ledger::Ledger;
use crate::operation::{AccountNumber, Operation};
use crate::validators::ValidatorIndex;

/// How far ahead of an account's sequence the pool takes its operations.
pub const SEQUENCE_WINDOW: u64 = 16;

/// Whose operation it is, and its place in their sequence: the order a
/// block takes them in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    Validator(ValidatorIndex, u64),
    Member(AccountNumber, u64),
}

/// The pool.
#[derive(Clone, Debug)]
pub struct Mempool {
    capacity: usize,
    by_slot: BTreeMap<Slot, (Digest, Operation)>,
}

impl Mempool {
    /// A pool of at most `capacity` operations.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            by_slot: BTreeMap::new(),
        }
    }

    /// How many operations wait.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_slot.len()
    }

    /// Whether none waits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_slot.is_empty()
    }

    /// Takes `operation` if it could apply on `ledger`, the final state,
    /// and there is room. Whether it was taken. An operation for a slot
    /// already taken replaces it only if it is the same.
    pub fn admit(&mut self, ledger: &Ledger, operation: Operation) -> bool {
        let Some(slot) = Self::check(ledger, &operation) else {
            return false;
        };
        if self.by_slot.contains_key(&slot) || self.by_slot.len() >= self.capacity {
            return false;
        }
        self.by_slot.insert(slot, (operation.hash(), operation));
        true
    }

    fn check(ledger: &Ledger, operation: &Operation) -> Option<Slot> {
        match operation {
            Operation::Member(signed) => {
                let account = ledger.account(signed.account)?;
                let current = account.sequence;
                let fits = signed.sequence >= current
                    && signed.sequence < current.saturating_add(SEQUENCE_WINDOW)
                    && signed.max_fee >= ledger.terms().fees.operation
                    && !account.closing;
                (fits && signed.verifies(ledger.hub(), &account.key))
                    .then_some(Slot::Member(signed.account, signed.sequence))
            }
            Operation::Validator(signed) => {
                let key = ledger.validators().key(signed.validator)?;
                let last = ledger.validator(signed.validator)?.sequence;
                (signed.sequence > last && signed.verifies(ledger.hub(), key))
                    .then_some(Slot::Validator(signed.validator, signed.sequence))
            }
        }
    }

    /// The operations for a block on `base`, at most `limit` in all: those
    /// with a deadline first, then the others in the places not `kept` for
    /// deadlines; each account's in sequence order
    /// (spec/designs/settlement-hubs.md#7-fees, "Deadlines first").
    #[must_use]
    pub fn select(&self, base: &Ledger, limit: usize, kept: usize) -> Vec<Operation> {
        let usable = |slot: &Slot| match slot {
            Slot::Member(account, sequence) => base
                .account(*account)
                .is_some_and(|a| *sequence >= a.sequence),
            Slot::Validator(index, sequence) => base
                .validator(*index)
                .is_some_and(|v| *sequence > v.sequence),
        };
        let candidates: Vec<&Operation> = self
            .by_slot
            .iter()
            .filter(|(slot, _)| usable(slot))
            .map(|(_, (_, operation))| operation)
            .collect();
        let others_room = limit.saturating_sub(kept);
        let deadlines: Vec<Operation> = candidates
            .iter()
            .filter(|operation| operation.has_deadline())
            .take(limit)
            .map(|operation| (*operation).clone())
            .collect();
        let room = others_room.min(limit.saturating_sub(deadlines.len()));
        let others = candidates
            .iter()
            .filter(|operation| !operation.has_deadline())
            .take(room)
            .map(|operation| (*operation).clone());
        deadlines.into_iter().chain(others).collect()
    }

    /// Forgets what `ledger`, a new final state, has passed: operations in
    /// it, and those whose sequence is now behind or out of the window.
    pub fn prune(&mut self, ledger: &Ledger, included: &[Digest]) {
        self.by_slot.retain(|slot, (hash, _)| {
            let current = match slot {
                Slot::Member(account, sequence) => ledger
                    .account(*account)
                    .is_some_and(|a| *sequence >= a.sequence && !a.closing),
                Slot::Validator(index, sequence) => ledger
                    .validator(*index)
                    .is_some_and(|v| *sequence > v.sequence),
            };
            current && !included.contains(hash)
        });
    }

    /// Asserts its bound.
    pub fn check_invariants(&self) {
        assert!(
            self.by_slot.len() <= self.capacity,
            "mempool over its bound"
        );
    }
}
