//! The hub's ledger: its state, and the deterministic function that takes
//! it from one block to the next (spec/designs/settlement-hubs.md#3-the-ledger).
//!
//! Every validator executes the same blocks in the same order and must
//! reach the same state, so nothing here reads a clock or an RNG: time is
//! the block's timestamp. Every table is bounded by the rules; an operation
//! that would exceed a bound fails, it never grows the table. After every
//! block, [`Ledger::check_invariants`] asserts that every drop the escrow
//! holds is accounted for exactly once.

mod escrow;
mod fees;
mod holds;
mod invariants;
mod member;
mod merkle;
mod outcome;
mod state;
mod validator;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub use merkle::{merkle_proof, merkle_root, result_leaf, results_root, verify_proof};
pub use outcome::{Detail, Failure, Outcome, Refusal};
pub use state::{
    Account, Escrow, FeePool, Hold, Key, PendingDeposit, PendingRules, Phase, Reduction,
    ValidatorState, Withdrawal,
};

use crate::digest::Digest;
use crate::operation::{AccountNumber, Operation};
use crate::promise::HoldNumber;
use crate::rules::{Amount, HubId, Millis, Rules, RulesError, Terms};
use crate::validators::{Signers, ValidatorIndex, ValidatorSet};

/// A block as the ledger executes it: what consensus ordered, and the votes
/// for earlier blocks its proposer recorded
/// (spec/designs/settlement-hubs.md#7-fees). Consensus has checked the
/// votes' signatures.
#[derive(Clone, Copy, Debug)]
pub struct Block<'a> {
    /// The block's height; one more than the last block's.
    pub height: u64,
    /// The proposer's timestamp, which every rule about time follows.
    pub timestamp: Millis,
    /// The operations, those with deadlines first.
    pub operations: &'a [Operation],
    /// For earlier blocks, by height, the validators whose votes for them
    /// this block records.
    pub votes: &'a [(u64, Signers)],
}

/// Why a block cannot be executed at all. A proposer that builds one is
/// faulty; validators do not vote for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockError {
    /// Its height does not follow the last block's.
    Height,
    /// Its timestamp is not after the last block's.
    Timestamp,
    /// It has more operations than the rules allow, or more without
    /// deadlines than the places not kept for deadlines.
    TooManyOperations,
    /// An operation with a deadline comes after one without.
    Order,
    /// It records a vote for a block outside the vote window, or twice.
    Votes,
    /// The hub has ended.
    Ended,
}

/// The ledger of one hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    pub(crate) hub: HubId,
    pub(crate) rules: Rules,
    pub(crate) validators: ValidatorSet,
    pub(crate) validator_states: Vec<ValidatorState>,
    pub(crate) height: u64,
    pub(crate) timestamp: Millis,
    pub(crate) phase: Phase,
    pub(crate) escrow: Escrow,
    pub(crate) accounts: BTreeMap<AccountNumber, Account>,
    pub(crate) keys: BTreeMap<[u8; 32], AccountNumber>,
    pub(crate) next_account: u32,
    pub(crate) holds: BTreeMap<HoldNumber, Hold>,
    pub(crate) next_hold: u64,
    pub(crate) withdrawals: BTreeMap<u32, Withdrawal>,
    pub(crate) pending_deposits: BTreeMap<Digest, PendingDeposit>,
    pub(crate) recent_deposits: BTreeMap<Digest, u32>,
    pub(crate) reserve_account: Amount,
    pub(crate) pools: Vec<FeePool>,
    pub(crate) pending_rules: Option<PendingRules>,
    pub(crate) rules_votes: Vec<(Digest, Signers)>,
    pub(crate) escrow_votes: Vec<(u32, Signers)>,
    pub(crate) signer_list_votes: Vec<(u32, Signers)>,
    pub(crate) shutdown_votes: Signers,
    pub(crate) reset_votes: Vec<(u32, Signers)>,
}

/// The XRP Ledger testnet's network ID, the one network on which the hub
/// writes off a reset (spec/decisions/ledger-resets.md).
pub const TESTNET_NETWORK: u32 = 1;

impl Ledger {
    /// The ledger at genesis: one account per validator, for its fees, and
    /// nothing in the escrow.
    pub fn genesis(rules: Rules, timestamp: Millis) -> Result<Self, RulesError> {
        rules.check()?;
        let validators = rules.validator_set().ok_or(RulesError::Validators)?;
        let hub = rules.id();
        let mut ledger = Self {
            hub,
            validators,
            validator_states: Vec::new(),
            height: 0,
            timestamp,
            phase: Phase::Running,
            escrow: Escrow {
                balance: 0,
                next_sequence: 0,
                checked: false,
                deposit_floor: 0,
                signer_list: None,
            },
            accounts: BTreeMap::new(),
            keys: BTreeMap::new(),
            next_account: 1,
            holds: BTreeMap::new(),
            next_hold: 1,
            withdrawals: BTreeMap::new(),
            pending_deposits: BTreeMap::new(),
            recent_deposits: BTreeMap::new(),
            reserve_account: 0,
            pools: Vec::new(),
            pending_rules: None,
            rules_votes: Vec::new(),
            escrow_votes: Vec::new(),
            signer_list_votes: Vec::new(),
            shutdown_votes: Signers::NONE,
            reset_votes: Vec::new(),
            rules,
        };
        ledger.open_validator_accounts(timestamp);
        ledger.check_invariants();
        Ok(ledger)
    }

    fn open_validator_accounts(&mut self, timestamp: Millis) {
        let acceptances = self.rules.validators.clone();
        for acceptance in acceptances {
            let number = self.open_validator_account(acceptance.consensus);
            self.validator_states.push(ValidatorState {
                endpoint: acceptance.endpoint,
                sequence: 0,
                last_recorded: timestamp,
                account: number,
            });
        }
    }

    /// Checks that a block can be executed: its height, timestamp, size,
    /// order and recorded votes. A validator votes only for blocks that
    /// pass.
    pub fn check_block(&self, block: &Block<'_>) -> Result<(), BlockError> {
        if self.phase == Phase::Ended {
            return Err(BlockError::Ended);
        }
        if Some(block.height) != self.height.checked_add(1) {
            return Err(BlockError::Height);
        }
        if block.timestamp <= self.timestamp {
            return Err(BlockError::Timestamp);
        }
        let limits = &self.rules.terms.limits;
        let deadlines = block.operations.iter().filter(|o| o.has_deadline()).count();
        let others = block.operations.len().saturating_sub(deadlines);
        let free_places = usize::from(
            limits
                .block_operations
                .saturating_sub(limits.deadline_operations),
        );
        if block.operations.len() > usize::from(limits.block_operations) || others > free_places {
            return Err(BlockError::TooManyOperations);
        }
        if block
            .operations
            .iter()
            .skip(deadlines)
            .any(Operation::has_deadline)
        {
            return Err(BlockError::Order);
        }
        self.check_votes(block)
    }

    /// Executes a block and returns each operation's outcome, in order.
    /// The block must pass [`Ledger::check_block`].
    pub fn apply(&mut self, block: &Block<'_>) -> Result<Vec<Outcome>, BlockError> {
        self.check_block(block)?;
        self.height = block.height;
        self.timestamp = block.timestamp;
        self.record_votes(block);
        self.apply_due();
        let mut outcomes = Vec::with_capacity(block.operations.len());
        for operation in block.operations {
            let outcome = match operation {
                Operation::Member(signed) => self.apply_member(signed),
                Operation::Validator(signed) => self.apply_validator(signed),
            };
            outcomes.push(outcome);
        }
        self.end_block();
        self.check_invariants();
        Ok(outcomes)
    }

    /// What falls due at the block's timestamp before its operations run:
    /// reductions of holds, new rules, the end of a shutdown's notice, and
    /// a shutdown forced by an absent validator.
    fn apply_due(&mut self) {
        self.apply_reductions();
        self.apply_pending_rules();
        if let Phase::ShuttingDown { ends } = self.phase {
            if self.timestamp >= ends {
                self.phase = Phase::Ended;
            }
        } else if self.phase == Phase::Running && self.validator_absent() {
            self.begin_shutdown();
        }
    }

    fn validator_absent(&self) -> bool {
        let absence = self.rules.terms.timers.absence;
        self.validator_states
            .iter()
            .any(|v| self.timestamp.saturating_sub(v.last_recorded) > absence)
    }

    fn begin_shutdown(&mut self) {
        let notice = self.rules.terms.timers.shutdown_notice;
        self.phase = Phase::ShuttingDown {
            ends: self.timestamp.saturating_add(notice),
        };
    }

    /// The hub ID.
    #[must_use]
    pub const fn hub(&self) -> &HubId {
        &self.hub
    }

    /// The rules in force.
    #[must_use]
    pub const fn rules(&self) -> &Rules {
        &self.rules
    }

    /// The terms in force.
    #[must_use]
    pub const fn terms(&self) -> &Terms {
        &self.rules.terms
    }

    /// The validator set in force.
    #[must_use]
    pub const fn validators(&self) -> &ValidatorSet {
        &self.validators
    }

    /// What is known of validator `index`.
    #[must_use]
    pub fn validator(&self, index: ValidatorIndex) -> Option<&ValidatorState> {
        self.validator_states.get(index.get())
    }

    /// The height of the last block executed.
    #[must_use]
    pub const fn height(&self) -> u64 {
        self.height
    }

    /// The timestamp of the last block executed.
    #[must_use]
    pub const fn timestamp(&self) -> Millis {
        self.timestamp
    }

    /// Where the hub is in its life.
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// What the hub knows of its escrow.
    #[must_use]
    pub const fn escrow(&self) -> &Escrow {
        &self.escrow
    }

    /// An account.
    #[must_use]
    pub fn account(&self, number: AccountNumber) -> Option<&Account> {
        self.accounts.get(&number)
    }

    /// The account of `key`, if it has one.
    #[must_use]
    pub fn account_of(&self, key: &dassie_identity::PublicKey) -> Option<AccountNumber> {
        self.keys.get(key.as_bytes()).copied()
    }

    /// Every account, in order.
    pub fn accounts(&self) -> impl Iterator<Item = (AccountNumber, &Account)> {
        self.accounts
            .iter()
            .map(|(number, account)| (*number, account))
    }

    /// A hold.
    #[must_use]
    pub fn hold(&self, number: HoldNumber) -> Option<&Hold> {
        self.holds.get(&number)
    }

    /// Every hold, in order.
    pub fn holds(&self) -> impl Iterator<Item = (HoldNumber, &Hold)> {
        self.holds.iter().map(|(number, hold)| (*number, hold))
    }

    /// Withdrawals not yet settled, by escrow sequence.
    pub fn withdrawals(&self) -> impl Iterator<Item = (u32, &Withdrawal)> {
        self.withdrawals.iter().map(|(sequence, w)| (*sequence, w))
    }

    /// How many accounts were opened as members': the rules' bound on
    /// accounts counts only these, so a change of validators never finds
    /// the table full or overfills it.
    #[must_use]
    pub fn members(&self) -> usize {
        self.accounts.values().filter(|a| a.member_slot).count()
    }

    /// The validators whose votes for the block at `height` a block has
    /// recorded, while its fees wait for the vote window to pass: a
    /// proposer records only those not recorded yet.
    #[must_use]
    pub fn recorded_voters(&self, height: u64) -> Signers {
        self.pools
            .iter()
            .find(|pool| pool.height == height)
            .map_or(Signers::NONE, |pool| pool.voters)
    }

    /// What the reserve account holds.
    #[must_use]
    pub const fn reserve_account(&self) -> Amount {
        self.reserve_account
    }

    /// The hub's total supply: everything it owes, which every validator
    /// publishes with its certificate so anyone can compare it with the
    /// escrow (spec/designs/settlement-hubs.md#3-the-ledger).
    #[must_use]
    pub const fn supply(&self) -> Amount {
        self.escrow.balance
    }
}

#[cfg(test)]
mod tests;
