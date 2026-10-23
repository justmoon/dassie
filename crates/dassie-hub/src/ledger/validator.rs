//! Operations validators sign with their consensus keys: what they saw on
//! the underlying ledger, and what they decide together. Each counts once
//! per validator, and takes effect when a quorum agrees, so at most f
//! faulty validators can neither make one happen nor stop it.

use super::Ledger;
use super::TESTNET_NETWORK;
use super::outcome::{Detail, Failure, Outcome, Refusal};
use super::state::{PendingRules, Phase, ValidatorState};
use crate::operation::{SignedValidatorOperation, ValidatorOperation};
use crate::rules::Rules;
use crate::validators::{Signers, ValidatorIndex};

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
        if let Some(state) = self.validator_states.get_mut(index.get()) {
            state.sequence = signed.sequence;
        }
        match self.execute_validator(index, &signed.operation) {
            Ok(detail) => Outcome::Applied { fee: 0, detail },
            Err(Ok(reason)) => Outcome::Failed { fee: 0, reason },
            Err(Err(refusal)) => Outcome::Refused(refusal),
        }
    }

    fn execute_validator(
        &mut self,
        index: ValidatorIndex,
        operation: &ValidatorOperation,
    ) -> Result<Detail, Result<Failure, Refusal>> {
        match operation {
            ValidatorOperation::AttestDeposit(deposit) => self.attest_deposit(index, deposit),
            ValidatorOperation::AttestWithdrawal {
                escrow_sequence,
                delivered,
            } => self.attest_withdrawal(index, *escrow_sequence, *delivered),
            ValidatorOperation::AttestEscrow { sequence } => self.attest_escrow(index, *sequence),
            ValidatorOperation::AttestReset { .. } => self.attest_reset(index),
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
                if self.shutdown_votes.count() >= self.validators.quorum()
                    && self.phase == Phase::Running
                {
                    self.begin_shutdown();
                }
                Ok(Detail::None)
            }
        }
    }

    /// A quorum attesting the same next sequence of a correctly set up
    /// escrow lets deposits be credited (spec/designs/settlement-hubs.md#2-founding-a-hub).
    fn attest_escrow(
        &mut self,
        index: ValidatorIndex,
        sequence: u32,
    ) -> Result<Detail, Result<Failure, Refusal>> {
        if self.escrow.checked {
            return Err(Err(Refusal::NotAllowed));
        }
        let quorum = self.validators.quorum();
        let votes = &mut self.escrow_votes;
        if votes.iter().any(|(_, signers)| signers.contains(index)) {
            return Err(Err(Refusal::NotAllowed));
        }
        match votes.iter_mut().find(|(s, _)| *s == sequence) {
            Some((_, signers)) => *signers = signers.with(index),
            None => votes.push((sequence, Signers::NONE.with(index))),
        }
        let agreed = votes
            .iter()
            .find(|(_, signers)| signers.count() >= quorum)
            .map(|(sequence, _)| *sequence);
        if let Some(sequence) = agreed {
            self.escrow.checked = true;
            self.escrow.next_sequence = sequence;
            self.escrow_votes.clear();
        }
        Ok(Detail::None)
    }

    /// A reset of the test network, once a quorum saw it, writes off
    /// everything the escrow held (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
    /// One faulty validator, or one lying server, cannot.
    fn attest_reset(&mut self, index: ValidatorIndex) -> Result<Detail, Result<Failure, Refusal>> {
        if self.rules.terms.network != TESTNET_NETWORK || self.reset_votes.contains(index) {
            return Err(Err(Refusal::NotAllowed));
        }
        self.reset_votes = self.reset_votes.with(index);
        if self.reset_votes.count() >= self.validators.quorum() {
            self.write_off();
        }
        Ok(Detail::None)
    }

    /// Votes for new rules. They must keep what keys and the escrow hang
    /// on: the nonce, the sponsor, the network and the escrow.
    fn vote_rules(
        &mut self,
        index: ValidatorIndex,
        rules: &Rules,
    ) -> Result<Detail, Result<Failure, Refusal>> {
        let current = &self.rules.terms;
        let same_hub = rules.terms.nonce == current.nonce
            && rules.terms.sponsor == current.sponsor
            && rules.terms.network == current.network
            && rules.terms.escrow == current.escrow;
        if !same_hub || rules.check().is_err() || self.pending_rules.is_some() {
            return Err(Ok(Failure::NotYours));
        }
        if self
            .rules_votes
            .iter()
            .any(|(_, signers)| signers.contains(index))
        {
            return Err(Err(Refusal::NotAllowed));
        }
        let digest = rules.id().0;
        match self.rules_votes.iter_mut().find(|(d, _)| *d == digest) {
            Some((_, signers)) => *signers = signers.with(index),
            None => self.rules_votes.push((digest, Signers::NONE.with(index))),
        }
        let agreed = self
            .rules_votes
            .iter()
            .any(|(d, signers)| *d == digest && signers.count() >= self.validators.quorum());
        if agreed {
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
    /// votes are paid out first, by the old set's indices; validators that
    /// stay keep their accounts, new ones get accounts of their own.
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
        let old = core::mem::take(&mut self.validator_states);
        for acceptance in &rules.validators {
            let kept = old.iter().find(|state| {
                self.accounts
                    .get(&state.account)
                    .is_some_and(|a| a.key == acceptance.consensus)
            });
            let account = match kept {
                Some(state) => state.account,
                None => self.open_validator_account(acceptance.consensus),
            };
            self.validator_states.push(ValidatorState {
                endpoint: acceptance.endpoint.clone(),
                sequence: kept.map_or(0, |s| s.sequence),
                last_recorded: self.timestamp,
                account,
            });
        }
        self.validators = set;
        self.rules = rules;
        self.shutdown_votes = Signers::NONE;
        self.reset_votes = Signers::NONE;
        self.escrow_votes.clear();
    }

    pub(super) fn open_validator_account(
        &mut self,
        key: dassie_identity::PublicKey,
    ) -> crate::operation::AccountNumber {
        // A key that already has a member account keeps it, now as a
        // validator's.
        if let Some(existing) = self.keys.get(key.as_bytes()).copied() {
            if let Some(account) = self.accounts.get_mut(&existing) {
                account.validator = true;
            }
            return existing;
        }
        let number = crate::operation::AccountNumber(self.next_account);
        self.next_account = self.next_account.saturating_add(1);
        self.keys.insert(*key.as_bytes(), number);
        self.accounts.insert(
            number,
            super::state::Account {
                key,
                balance: 0,
                reserve: 0,
                sequence: 0,
                disbursal: None,
                closing: false,
                holds: 0,
                withdrawals: 0,
                validator: true,
            },
        );
        number
    }
}
