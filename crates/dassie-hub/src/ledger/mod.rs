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
mod issuer;
mod member;
mod merkle;
mod outcome;
mod snapshot;
mod state;
mod validator;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

pub use issuer::{CLAWBACK_SHARE, MAX_CLAWBACKS};
pub use merkle::{merkle_proof, merkle_root, result_leaf, results_root, verify_proof};
pub use outcome::{Detail, Failure, Outcome, Refusal};
pub use state::{
    Account, Escrow, FeePool, Hold, Key, PendingDeposit, PendingRules, Phase, Reduction,
    ValidatorState, Withdrawal,
};

use crate::digest::Digest;
use crate::operation::{AccountNumber, Clawback, Issuer, Operation};
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
    /// Each hold by its payer and by its payee: derived from `holds`, and
    /// not in the state root.
    pub(crate) hold_index: alloc::collections::BTreeSet<(AccountNumber, HoldNumber)>,
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
    /// The validators that attested a reset since the last write-off.
    pub(crate) reset_votes: Signers,
    /// The timestamp of the block that took the first of `reset_votes`.
    pub(crate) reset_votes_since: Option<Millis>,
    /// The timestamp of the block that last wrote off a reset.
    pub(crate) written_off_at: Option<Millis>,
    /// The height from which the validators in force sign blocks: no block
    /// records votes for blocks before it, signed by others.
    pub(crate) set_since: u64,
    /// The timestamp of the block from which the hub is frozen, while it
    /// is (spec/decisions/token-hubs.md).
    pub(crate) frozen: Option<Millis>,
    /// What a quorum last attested of the issuer's hold on the escrow.
    pub(crate) issuer: Issuer,
    /// Votes for newer states of the issuer's hold, one per validator.
    pub(crate) issuer_votes: Vec<(Issuer, Signers)>,
    /// Clawbacks fewer than a quorum attested, by what each attested.
    pub(crate) clawback_votes: Vec<(Clawback, Signers)>,
    /// Clawbacks applied and still remembered, by transaction, with their
    /// ledger.
    pub(crate) recent_clawbacks: BTreeMap<Digest, u32>,
}

/// The XRP Ledger testnet's network ID, the one network on which the hub
/// writes off a reset (spec/decisions/ledger-resets.md).
pub const TESTNET_NETWORK: u32 = 1;

/// How long after a write-off the hub takes no attestation of a reset, by
/// block timestamps, and how long votes for a reset count without reaching
/// a quorum: one hour.
///
/// Reset votes count validators, not the ledger each names, since honest
/// validators see a new chain at different ledgers. So a vote must never
/// outlive its reset: up to f honest validators may still be on their way
/// to attest a reset a quorum already wrote off, and those late votes,
/// with f faulty ones and one validator a lying server fooled, would be a
/// quorum for a second write-off. An online validator sees a new chain at
/// its server's next validated ledger, or when it connects again (backing
/// off at most a minute), and its attestation is in a block within a few
/// views: seconds to a minute or two. An hour is well above that, and far
/// below the months between resets of the test network. Votes that reached
/// no quorum within it lapse, so stray votes months apart, each a faulty
/// validator's or one fooled by a lying server, never add up.
pub const RESET_QUIET_MS: Millis = 60 * 60 * 1_000;

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
                clawed_back: 0,
                clawback_floor: 0,
            },
            accounts: BTreeMap::new(),
            keys: BTreeMap::new(),
            next_account: 1,
            holds: BTreeMap::new(),
            hold_index: alloc::collections::BTreeSet::new(),
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
            reset_votes: Signers::NONE,
            reset_votes_since: None,
            written_off_at: None,
            set_since: 1,
            frozen: None,
            issuer: Issuer::default(),
            issuer_votes: Vec::new(),
            clawback_votes: Vec::new(),
            recent_clawbacks: BTreeMap::new(),
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

    /// What falls due by the hub's clock before the block's operations
    /// run: reductions of holds, new rules, the end of a shutdown's notice,
    /// and a shutdown forced by an absent validator. While the hub is
    /// frozen its clock stands still, so nothing falls due.
    fn apply_due(&mut self) {
        self.apply_reductions();
        self.apply_pending_rules();
        let clock = self.clock();
        if let Phase::ShuttingDown { ends } = self.phase {
            if clock >= ends {
                self.phase = Phase::Ended;
            }
        } else if self.phase == Phase::Running && self.validator_absent() {
            self.begin_shutdown();
        }
    }

    fn validator_absent(&self) -> bool {
        let absence = self.rules.terms.timers.absence;
        let clock = self.clock();
        self.validator_states
            .iter()
            .any(|v| clock.saturating_sub(v.last_recorded) > absence)
    }

    /// The time every notice and the absence of validators run by: the
    /// block's timestamp, but while the hub is frozen the freeze's, so no
    /// notice runs during a freeze (spec/decisions/token-hubs.md). Every
    /// deadline is set by it, and the thaw moves those still to come back
    /// by the freeze's length.
    #[must_use]
    pub fn clock(&self) -> Millis {
        self.frozen.unwrap_or(self.timestamp)
    }

    fn begin_shutdown(&mut self) {
        let notice = self.rules.terms.timers.shutdown_notice;
        self.phase = Phase::ShuttingDown {
            ends: self.clock().saturating_add(notice),
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

    /// The timestamp of the block from which the hub is frozen, while it
    /// is: it takes no member's operation but claims within the freeze's
    /// window ([`Ledger::freeze_window_end`]), credits no deposit, settles
    /// no withdrawal and lets no notice run, and no payout set is to be
    /// signed until the thaw (spec/decisions/token-hubs.md).
    #[must_use]
    pub const fn frozen(&self) -> Option<Millis> {
        self.frozen
    }

    /// What a quorum last attested of the issuer's hold on a token hub's
    /// escrow.
    #[must_use]
    pub const fn issuer(&self) -> &Issuer {
        &self.issuer
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

    /// Every hold `account` pays into or takes from, in order, found by an
    /// index rather than by looking at every hold.
    pub fn holds_of(&self, account: AccountNumber) -> impl Iterator<Item = (HoldNumber, &Hold)> {
        self.hold_index
            .range((account, HoldNumber(0))..=(account, HoldNumber(u64::MAX)))
            .filter_map(|(_, number)| self.holds.get(number).map(|hold| (*number, hold)))
    }

    /// The index of holds by account, as the holds make it.
    pub(crate) fn derived_hold_index(
        &self,
    ) -> alloc::collections::BTreeSet<(AccountNumber, HoldNumber)> {
        self.holds
            .iter()
            .flat_map(|(number, hold)| [(hold.payer, *number), (hold.payee, *number)])
            .collect()
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

    /// A change of validators the log scheduled: the height from which the
    /// new set signs, and the set.
    #[must_use]
    pub fn scheduled_change(&self) -> Option<(u64, ValidatorSet)> {
        let pending = self.pending_rules.as_ref()?;
        Some((pending.from_height?, pending.rules.validator_set()?))
    }

    /// New rules the validators agreed on, waiting for their notice: a host
    /// reaches the validators they name, which take over later, as well as
    /// those in force.
    #[must_use]
    pub const fn pending_rules(&self) -> Option<&PendingRules> {
        self.pending_rules.as_ref()
    }

    /// The timestamp of the block that last wrote off a reset of the test
    /// network, if one did.
    #[must_use]
    pub const fn written_off_at(&self) -> Option<Millis> {
        self.written_off_at
    }

    /// The height from which the validators in force sign blocks.
    #[must_use]
    pub const fn set_since(&self) -> u64 {
        self.set_since
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
mod issuer_tests;
#[cfg(test)]
pub(crate) mod tests;
