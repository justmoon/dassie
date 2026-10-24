//! Operations validators sign with their consensus keys: what they saw on
//! the underlying ledger, and what they decide together. Each counts once
//! per validator, and takes effect when a quorum agrees on the same thing,
//! so at most f faulty validators can neither make one happen nor stop it.

use alloc::vec::Vec;

use super::Ledger;
use super::outcome::{Detail, Failure, Outcome, Refusal};
use super::state::{Account, PendingRules, Phase, ValidatorState};
use super::{RESET_QUIET_MS, TESTNET_NETWORK};
use crate::operation::{AccountNumber, SignedValidatorOperation, ValidatorOperation};
use crate::rules::Rules;
use crate::validators::{Signers, ValidatorIndex};

type Executed = Result<Detail, Result<Failure, Refusal>>;

/// Adds `index`'s vote for `value`, in place of any other value it voted
/// for: a validator may change its mind, as when it saw a reset or new
/// rules differently from the others first. Whether a quorum now agrees on
/// `value`.
fn vote<T: PartialEq + Copy>(
    votes: &mut Vec<(T, Signers)>,
    index: ValidatorIndex,
    value: T,
    quorum: usize,
) -> Result<bool, Refusal> {
    if votes
        .iter()
        .any(|(v, signers)| *v == value && signers.contains(index))
    {
        return Err(Refusal::NotAllowed);
    }
    let mine = Signers::NONE.with(index).bits();
    for (_, signers) in votes.iter_mut() {
        *signers = Signers::from_bits(signers.bits() & !mine);
    }
    votes.retain(|(_, signers)| signers.count() > 0);
    match votes.iter_mut().find(|(v, _)| *v == value) {
        Some((_, signers)) => *signers = signers.with(index),
        None => votes.push((value, Signers::NONE.with(index))),
    }
    Ok(votes
        .iter()
        .any(|(v, signers)| *v == value && signers.count() >= quorum))
}

impl Ledger {
    pub(super) fn apply_validator(&mut self, signed: &SignedValidatorOperation) -> Outcome {
        let index = signed.validator;
        let (Some(key), Some(state)) = (
            self.validators.key(index),
            self.validator_states.get(index.get()),
        ) else {
            return Outcome::Refused(Refusal::Unknown);
        };
        if !signed.verifies(&self.hub, key) {
            return Outcome::Refused(Refusal::Signature);
        }
        if signed.sequence <= state.sequence {
            return Outcome::Refused(Refusal::Sequence);
        }
        let outcome = match self.execute_validator(index, &signed.operation) {
            Ok(detail) => Outcome::Applied { fee: 0, detail },
            Err(Ok(reason)) => Outcome::Failed { fee: 0, reason },
            Err(Err(refusal)) => Outcome::Refused(refusal),
        };
        // A refused operation uses no sequence, as a member's does not.
        if outcome.used_sequence()
            && let Some(state) = self.validator_states.get_mut(index.get())
        {
            state.sequence = signed.sequence;
        }
        outcome
    }

    fn execute_validator(
        &mut self,
        index: ValidatorIndex,
        operation: &ValidatorOperation,
    ) -> Executed {
        let quorum = self.validators.quorum();
        match operation {
            ValidatorOperation::AttestDeposit(deposit) => self.attest_deposit(index, deposit),
            ValidatorOperation::AttestWithdrawal {
                escrow_sequence,
                delivered,
            } => self.attest_withdrawal(index, *escrow_sequence, *delivered),
            ValidatorOperation::AttestEscrow { sequence } => self.attest_escrow(index, *sequence),
            ValidatorOperation::AttestReset { ledger } => self.attest_reset(index, *ledger),
            ValidatorOperation::AttestSignerList { sequence } => {
                self.attest_signer_list(index, *sequence)
            }
            ValidatorOperation::Reconfigure(rules) => self.vote_rules(index, rules),
            ValidatorOperation::SetEndpoint(endpoint) => {
                if let Some(state) = self.validator_states.get_mut(index.get()) {
                    state.endpoint.clone_from(endpoint);
                }
                Ok(Detail::None)
            }
            ValidatorOperation::Shutdown => {
                if self.shutdown_votes.contains(index) {
                    return Err(Err(Refusal::NotAllowed));
                }
                self.shutdown_votes = self.shutdown_votes.with(index);
                if self.shutdown_votes.count() >= quorum && self.phase == Phase::Running {
                    self.begin_shutdown();
                }
                Ok(Detail::None)
            }
        }
    }

    /// A quorum attesting the same next sequence of a correctly set up
    /// escrow lets deposits be credited (spec/designs/settlement-hubs.md#2-founding-a-hub).
    fn attest_escrow(&mut self, index: ValidatorIndex, sequence: u32) -> Executed {
        if self.escrow.checked {
            return Err(Err(Refusal::NotAllowed));
        }
        let quorum = self.validators.quorum();
        if vote(&mut self.escrow_votes, index, sequence, quorum).map_err(Err)? {
            self.escrow.checked = true;
            self.escrow.next_sequence = sequence;
            self.escrow_votes.clear();
            self.credit_deposits();
        }
        Ok(Detail::None)
    }

    /// The old validators' change of the escrow's signer list applied: once
    /// a quorum saw it, the hub stops keeping its sequence, and takes new
    /// rules again (spec/designs/settlement-hubs.md#8-changing-the-validators).
    fn attest_signer_list(&mut self, index: ValidatorIndex, sequence: u32) -> Executed {
        if self.escrow.signer_list != Some(sequence) {
            return Err(Ok(Failure::Missing));
        }
        let quorum = self.validators.quorum();
        if vote(&mut self.signer_list_votes, index, sequence, quorum).map_err(Err)? {
            self.escrow.signer_list = None;
            self.signer_list_votes.clear();
            // Its fee left the escrow, and the reserve account pays the
            // hub's own transactions (spec/designs/settlement-hubs.md#7-fees).
            let fee = self.rules.terms.fees.ledger.min(self.reserve_account);
            self.reserve_account = self.reserve_account.saturating_sub(fee);
            self.escrow.balance = self.escrow.balance.saturating_sub(fee);
        }
        Ok(Detail::None)
    }

    /// A reset of the test network, once a quorum of validators attested
    /// one, writes off everything the escrow held
    /// (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals). One
    /// faulty validator, or one lying server, cannot. Votes count
    /// validators, whatever ledger of the new chain each first saw, which
    /// differs between honest ones; so for [`RESET_QUIET_MS`] after a
    /// write-off no vote is taken, and the late votes for the reset written
    /// off never count toward another; and votes that reach no quorum within
    /// it lapse.
    fn attest_reset(&mut self, index: ValidatorIndex, _ledger: u32) -> Executed {
        if self.rules.terms.network != TESTNET_NETWORK {
            return Err(Err(Refusal::NotAllowed));
        }
        let quiet = self
            .written_off_at
            .is_some_and(|at| self.timestamp < at.saturating_add(RESET_QUIET_MS));
        let lapsed = self
            .reset_votes_since
            .is_some_and(|since| self.timestamp >= since.saturating_add(RESET_QUIET_MS));
        if lapsed {
            self.reset_votes = Signers::NONE;
            self.reset_votes_since = None;
            self.reset_votes_since = None;
        }
        if quiet || self.reset_votes.contains(index) {
            return Err(Err(Refusal::NotAllowed));
        }
        if self.reset_votes_since.is_none() {
            self.reset_votes_since = Some(self.timestamp);
        }
        self.reset_votes = self.reset_votes.with(index);
        if self.reset_votes.count() >= self.validators.quorum() {
            self.write_off();
            self.written_off_at = Some(self.timestamp);
        }
        Ok(Detail::None)
    }

    /// Votes for new rules. They must keep what keys and the escrow hang
    /// on (the nonce, the sponsor, the network and the escrow) and every
    /// table at least as large. A validator may change its vote.
    fn vote_rules(&mut self, index: ValidatorIndex, rules: &Rules) -> Executed {
        let current = &self.rules.terms;
        let same_hub = rules.terms.nonce == current.nonce
            && rules.terms.sponsor == current.sponsor
            && rules.terms.network == current.network
            && rules.terms.escrow == current.escrow;
        // A change of signers the old set has not made yet keeps its escrow
        // sequence; another waits until it is made.
        let acceptable = same_hub
            && rules.terms.keeps_tables_of(current)
            && rules.check().is_ok()
            && self.pending_rules.is_none()
            && self.escrow.signer_list.is_none();
        if !acceptable {
            return Err(Ok(Failure::NotYours));
        }
        let quorum = self.validators.quorum();
        if vote(&mut self.rules_votes, index, rules.id().0, quorum).map_err(Err)? {
            self.pending_rules = Some(PendingRules {
                rules: rules.clone(),
                from_height: None,
                at: self
                    .timestamp
                    .saturating_add(self.rules.terms.timers.reconfigure_notice),
            });
            self.rules_votes.clear();
        }
        Ok(Detail::None)
    }

    /// Puts new rules in force once their notice passed. Fees waiting for
    /// votes are paid out first, by the old set's indices; attestations
    /// follow their validators to their new indices, those of validators
    /// that left are dropped, and votes start again.
    /// Validators that stay keep their accounts; new ones get accounts of
    /// their own, and removed ones become members. A new set keeps the
    /// escrow's next sequence for the old set's change of signer list
    /// (spec/designs/settlement-hubs.md#8-changing-the-validators).
    pub(super) fn apply_pending_rules(&mut self) {
        let height = self.height;
        let Some(pending) = self.pending_rules.as_mut() else {
            return;
        };
        match pending.from_height {
            None if pending.at <= self.timestamp => {
                pending.from_height = Some(height.saturating_add(2));
                return;
            }
            Some(from) if from <= height => {}
            _ => return,
        }
        let Some(PendingRules { rules, .. }) = self.pending_rules.take() else {
            return;
        };
        let Some(set) = rules.validator_set() else {
            return;
        };
        self.distribute_all_pools();
        let escrow_keys = |rules: &Rules| {
            rules
                .validators
                .iter()
                .map(|a| a.escrow)
                .collect::<Vec<_>>()
        };
        let changed = set != self.validators || escrow_keys(&rules) != escrow_keys(&self.rules);
        let moved = self.moved_indices(&set);
        let old = core::mem::take(&mut self.validator_states);
        for state in &old {
            if let Some(account) = self.accounts.get_mut(&state.account) {
                account.validator = false;
            }
        }
        for acceptance in &rules.validators {
            let kept = old.iter().find(|state| {
                self.accounts
                    .get(&state.account)
                    .is_some_and(|a| a.key == acceptance.consensus)
            });
            let account = self.open_validator_account(acceptance.consensus);
            self.validator_states.push(ValidatorState {
                endpoint: acceptance.endpoint.clone(),
                sequence: kept.map_or(0, |s| s.sequence),
                last_recorded: self.timestamp,
                account,
            });
        }
        self.validators = set;
        self.rules = rules;
        self.set_since = height;
        self.move_attestations(&moved);
        if changed && let Some(next) = self.escrow.next_sequence.checked_add(1) {
            self.escrow.signer_list = Some(self.escrow.next_sequence);
            self.escrow.next_sequence = next;
        }
    }

    /// Where each validator of the current set is in `set`, by old index.
    fn moved_indices(&self, set: &crate::validators::ValidatorSet) -> Vec<Option<ValidatorIndex>> {
        self.validators
            .keys()
            .iter()
            .map(|key| set.index_of(key))
            .collect()
    }

    /// Moves attestations to their validators' new indices, dropping those
    /// of validators that left, so they never count toward the new set's
    /// quorum. A pending deposit no one attests any more is dropped, but
    /// for one below the floor, which kept as pending is attested again
    /// rather than refused as one that may have been credited. Votes start
    /// again.
    fn move_attestations(&mut self, moved: &[Option<ValidatorIndex>]) {
        let remap = |signers: Signers| {
            signers
                .iter()
                .filter_map(|old| moved.get(old.get()).copied().flatten())
                .fold(Signers::NONE, Signers::with)
        };
        let floor = self.escrow.deposit_floor;
        self.pending_deposits.retain(|_, pending| {
            let below_floor = pending
                .attestations
                .iter()
                .any(|(deposit, _)| deposit.ledger < floor);
            for (_, signers) in &mut pending.attestations {
                *signers = remap(*signers);
            }
            pending
                .attestations
                .retain(|(_, signers)| signers.count() > 0);
            !pending.attestations.is_empty() || below_floor
        });
        for withdrawal in self.withdrawals.values_mut() {
            withdrawal.delivered = remap(withdrawal.delivered);
            withdrawal.failed = remap(withdrawal.failed);
        }
        self.shutdown_votes = Signers::NONE;
        self.reset_votes = Signers::NONE;
        self.escrow_votes.clear();
        self.rules_votes.clear();
        self.signer_list_votes.clear();
    }

    /// The account of validator key `key`. A key that has a member account
    /// keeps it, now as a validator's: its reserve becomes balance, since a
    /// validator's account locks none.
    pub(super) fn open_validator_account(
        &mut self,
        key: dassie_identity::PublicKey,
    ) -> AccountNumber {
        if let Some(existing) = self.keys.get(key.as_bytes()).copied() {
            if let Some(account) = self.accounts.get_mut(&existing) {
                account.validator = true;
                account.balance = account.balance.saturating_add(account.reserve);
                account.reserve = 0;
                // A validator's account stays: a payout that closed it is
                // a plain withdrawal now.
                account.closing = false;
            }
            for withdrawal in self.withdrawals.values_mut() {
                if withdrawal.account == Some(existing) {
                    withdrawal.closes = false;
                }
            }
            return existing;
        }
        let number = AccountNumber(self.next_account);
        self.next_account = self.next_account.saturating_add(1);
        self.keys.insert(*key.as_bytes(), number);
        self.accounts.insert(
            number,
            Account {
                key,
                balance: 0,
                reserve: 0,
                sequence: 0,
                disbursal: None,
                closing: false,
                holds: 0,
                paying: 0,
                withdrawals: 0,
                validator: true,
                member_slot: false,
            },
        );
        number
    }
}
