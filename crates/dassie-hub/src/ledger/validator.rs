//! Operations validators sign with their consensus keys: what they saw on
//! the underlying ledger, and what they decide together. Each counts once
//! per validator, and takes effect when a quorum agrees on the same thing,
//! so at most f faulty validators can neither make one happen nor stop it.

use alloc::vec::Vec;

use super::Ledger;
use super::TESTNET_NETWORK;
use super::outcome::{Detail, Failure, Outcome, Refusal};
use super::state::{Account, PendingRules, Phase, ValidatorState};
use crate::operation::{AccountNumber, SignedValidatorOperation, ValidatorOperation};
use crate::rules::Rules;
use crate::validators::{Signers, ValidatorIndex};

type Executed = Result<Detail, Result<Failure, Refusal>>;

/// Adds `index`'s vote for `value`, unless it voted for any value already.
/// Whether a quorum now agrees on `value`.
fn vote<T: PartialEq + Copy>(
    votes: &mut Vec<(T, Signers)>,
    index: ValidatorIndex,
    value: T,
    quorum: usize,
) -> Result<bool, Refusal> {
    if votes.iter().any(|(_, signers)| signers.contains(index)) {
        return Err(Refusal::NotAllowed);
    }
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

    /// A reset of the test network, once a quorum saw the same new chain,
    /// writes off everything the escrow held
    /// (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals). One
    /// faulty validator, or one lying server, cannot; nor can sightings of
    /// different chains add up.
    fn attest_reset(&mut self, index: ValidatorIndex, ledger: u32) -> Executed {
        if self.rules.terms.network != TESTNET_NETWORK {
            return Err(Err(Refusal::NotAllowed));
        }
        let quorum = self.validators.quorum();
        if vote(&mut self.reset_votes, index, ledger, quorum).map_err(Err)? {
            self.write_off();
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
        let acceptable = same_hub
            && rules.terms.keeps_tables_of(current)
            && rules.check().is_ok()
            && self.pending_rules.is_none();
        if !acceptable {
            return Err(Ok(Failure::NotYours));
        }
        for (_, signers) in &mut self.rules_votes {
            *signers = Signers::from_bits(signers.bits() & !Signers::NONE.with(index).bits());
        }
        self.rules_votes.retain(|(_, signers)| signers.count() > 0);
        let quorum = self.validators.quorum();
        if vote(&mut self.rules_votes, index, rules.id().0, quorum).map_err(Err)? {
            self.pending_rules = Some(PendingRules {
                rules: rules.clone(),
                at: self
                    .timestamp
                    .saturating_add(self.rules.terms.timers.reconfigure_notice),
            });
            self.rules_votes.clear();
        }
        Ok(Detail::None)
    }

    /// Puts new rules in force once their notice passed. Fees waiting for
    /// votes are paid out first, by the old set's indices, and every
    /// attestation and vote is dropped, since indices now name other
    /// validators: validators attest again what is still pending.
    /// Validators that stay keep their accounts; new ones get accounts of
    /// their own, and removed ones become members. A new set keeps the
    /// escrow's next sequence for the old set's change of signer list
    /// (spec/designs/settlement-hubs.md#8-changing-the-validators).
    pub(super) fn apply_pending_rules(&mut self) {
        let due = self
            .pending_rules
            .as_ref()
            .is_some_and(|pending| pending.at <= self.timestamp);
        if !due {
            return;
        }
        let Some(PendingRules { rules, .. }) = self.pending_rules.take() else {
            return;
        };
        let Some(set) = rules.validator_set() else {
            return;
        };
        self.distribute_all_pools();
        let changed = set != self.validators;
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
        self.forget_attestations();
        if changed && let Some(next) = self.escrow.next_sequence.checked_add(1) {
            self.escrow.signer_list = Some(self.escrow.next_sequence);
            self.escrow.next_sequence = next;
        }
    }

    /// Drops every attestation and vote: after a change of validators,
    /// their indices name others.
    fn forget_attestations(&mut self) {
        self.pending_deposits.clear();
        for withdrawal in self.withdrawals.values_mut() {
            withdrawal.delivered = Signers::NONE;
            withdrawal.failed = Signers::NONE;
        }
        self.shutdown_votes = Signers::NONE;
        self.reset_votes.clear();
        self.escrow_votes.clear();
        self.rules_votes.clear();
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
            },
        );
        number
    }
}
