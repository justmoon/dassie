//! The whole state of a ledger as bytes: what a validator persists after
//! each final block, and what one that fell behind takes from another and
//! checks against a certified block's state root
//! (spec/designs/settlement-hubs.md#9-shutdown, "The state").
//!
//! Decoding never panics and allocates only as it reads, whatever the
//! input. A state taken from someone else is trusted only once its root is
//! the one a certified block carries; only then are its invariants
//! asserted.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_oer::{Encode, Reader};

use super::Ledger;
use super::state::{
    Account, Escrow, FeePool, Hold, PendingDeposit, PendingRules, Phase, Reduction, ValidatorState,
    Withdrawal,
};
use crate::codec::{HubDecodeError, read_digest, read_key};
use crate::operation::{AccountNumber, Deposit, Destination};
use crate::promise::HoldNumber;
use crate::rules::{HubId, Rules};
use crate::validators::Signers;

/// The most validator states a snapshot carries.
const MAX_VALIDATOR_STATES: u32 = 64;

/// Bytes written as OER reads them: fixed-width big-endian integers,
/// booleans as `00`/`ff`.
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    fn bool(&mut self, value: bool) {
        self.0.push(if value { 0xff } else { 0 });
    }
    fn count(&mut self, count: usize) {
        self.u32(u32::try_from(count).unwrap_or(u32::MAX));
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
    fn item<T: Encode>(&mut self, value: &T) {
        self.0.extend_from_slice(&value.to_vec());
    }
    fn votes(&mut self, votes: &[(u32, Signers)]) {
        self.count(votes.len());
        for (value, signers) in votes {
            self.u32(*value);
            self.u16(signers.bits());
        }
    }
}

impl Ledger {
    /// The whole state, as bytes.
    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = Out(Vec::new());
        out.item(&self.rules);
        out.bytes(self.hub.0.as_bytes());
        out.u64(self.height);
        out.u64(self.timestamp);
        match self.phase {
            Phase::Running => out.u8(0),
            Phase::ShuttingDown { ends } => {
                out.u8(1);
                out.u64(ends);
            }
            Phase::Ended => out.u8(2),
        }
        out.u64(self.escrow.balance);
        out.u32(self.escrow.next_sequence);
        out.bool(self.escrow.checked);
        out.u32(self.escrow.deposit_floor);
        out.bool(self.escrow.signer_list.is_some());
        out.u32(self.escrow.signer_list.unwrap_or(0));
        out.u32(self.next_account);
        out.u64(self.next_hold);
        out.u64(self.reserve_account);
        out.u64(self.set_since);
        self.snapshot_tables(&mut out);
        self.snapshot_votes(&mut out);
        out.0
    }

    fn snapshot_tables(&self, out: &mut Out) {
        out.count(self.validator_states.len());
        for state in &self.validator_states {
            out.u16(u16::try_from(state.endpoint.len()).unwrap_or(u16::MAX));
            out.bytes(state.endpoint.as_bytes());
            out.u64(state.sequence);
            out.u64(state.last_recorded);
            out.u32(state.account.0);
        }
        out.count(self.accounts.len());
        for (number, account) in &self.accounts {
            out.u32(number.0);
            out.item(account);
        }
        out.count(self.holds.len());
        for (number, hold) in &self.holds {
            out.u64(number.0);
            out.item(hold);
        }
        out.count(self.withdrawals.len());
        for (sequence, withdrawal) in &self.withdrawals {
            out.u32(*sequence);
            out.item(withdrawal);
        }
        out.count(self.pending_deposits.len());
        for (hash, pending) in &self.pending_deposits {
            out.bytes(hash.as_bytes());
            out.item(pending);
        }
        out.count(self.recent_deposits.len());
        for (hash, ledger) in &self.recent_deposits {
            out.bytes(hash.as_bytes());
            out.u32(*ledger);
        }
        out.count(self.pools.len());
        for pool in &self.pools {
            out.u64(pool.height);
            out.u64(pool.amount);
            out.u16(pool.voters.bits());
        }
    }

    fn snapshot_votes(&self, out: &mut Out) {
        out.bool(self.pending_rules.is_some());
        if let Some(pending) = &self.pending_rules {
            out.item(&pending.rules);
            out.u64(pending.at);
            out.bool(pending.from_height.is_some());
            out.u64(pending.from_height.unwrap_or(0));
        }
        out.count(self.rules_votes.len());
        for (digest, signers) in &self.rules_votes {
            out.bytes(digest.as_bytes());
            out.u16(signers.bits());
        }
        out.votes(&self.escrow_votes);
        out.votes(&self.signer_list_votes);
        out.votes(&self.reset_votes);
        out.u16(self.shutdown_votes.bits());
    }

    /// The ledger a snapshot holds. Its own snapshots a validator restores
    /// as they are; one taken from anyone else must first have the state
    /// root a certified block carries, before anything relies on it.
    pub fn restore(bytes: &[u8]) -> Result<Self, HubDecodeError> {
        let mut reader = Reader::new(bytes);
        let rules = Rules::decode(&mut reader)?;
        let validators = rules
            .validator_set()
            .ok_or(HubDecodeError::Invalid("validators"))?;
        let hub = HubId(read_digest(&mut reader)?);
        let height = reader.read_u64()?;
        let timestamp = reader.read_u64()?;
        let phase = match reader.read_u8()? {
            0 => Phase::Running,
            1 => Phase::ShuttingDown {
                ends: reader.read_u64()?,
            },
            2 => Phase::Ended,
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        };
        let escrow = read_escrow(&mut reader)?;
        let next_account = reader.read_u32()?;
        let next_hold = reader.read_u64()?;
        let reserve_account = reader.read_u64()?;
        let set_since = reader.read_u64()?;
        let mut ledger = Self {
            hub,
            validators,
            validator_states: Vec::new(),
            height,
            timestamp,
            phase,
            escrow,
            accounts: BTreeMap::new(),
            keys: BTreeMap::new(),
            next_account,
            holds: BTreeMap::new(),
            next_hold,
            withdrawals: BTreeMap::new(),
            pending_deposits: BTreeMap::new(),
            recent_deposits: BTreeMap::new(),
            reserve_account,
            pools: Vec::new(),
            pending_rules: None,
            rules_votes: Vec::new(),
            escrow_votes: Vec::new(),
            signer_list_votes: Vec::new(),
            shutdown_votes: Signers::NONE,
            reset_votes: Vec::new(),
            set_since,
            rules,
        };
        ledger.restore_tables(&mut reader)?;
        ledger.restore_votes(&mut reader)?;
        reader.finish()?;
        Ok(ledger)
    }

    fn restore_tables(&mut self, reader: &mut Reader<'_>) -> Result<(), HubDecodeError> {
        let count = reader.read_u32()?;
        if count > MAX_VALIDATOR_STATES {
            return Err(HubDecodeError::Invalid("validator states"));
        }
        for _ in 0..count {
            self.validator_states.push(read_validator_state(reader)?);
        }
        for _ in 0..reader.read_u32()? {
            let number = AccountNumber(reader.read_u32()?);
            let account = read_account(reader)?;
            if self.keys.insert(*account.key.as_bytes(), number).is_some() {
                return Err(HubDecodeError::Invalid("a key with two accounts"));
            }
            self.accounts.insert(number, account);
        }
        for _ in 0..reader.read_u32()? {
            let number = HoldNumber(reader.read_u64()?);
            self.holds.insert(number, read_hold(reader)?);
        }
        for _ in 0..reader.read_u32()? {
            let sequence = reader.read_u32()?;
            self.withdrawals.insert(sequence, read_withdrawal(reader)?);
        }
        for _ in 0..reader.read_u32()? {
            let hash = read_digest(reader)?;
            self.pending_deposits.insert(hash, read_pending(reader)?);
        }
        for _ in 0..reader.read_u32()? {
            let hash = read_digest(reader)?;
            self.recent_deposits.insert(hash, reader.read_u32()?);
        }
        for _ in 0..reader.read_u32()? {
            self.pools.push(FeePool {
                height: reader.read_u64()?,
                amount: reader.read_u64()?,
                voters: Signers::from_bits(reader.read_u16()?),
            });
        }
        Ok(())
    }

    fn restore_votes(&mut self, reader: &mut Reader<'_>) -> Result<(), HubDecodeError> {
        if reader.read_bool()? {
            let rules = Rules::decode(reader)?;
            let at = reader.read_u64()?;
            let has = reader.read_bool()?;
            let from = reader.read_u64()?;
            self.pending_rules = Some(PendingRules {
                rules,
                at,
                from_height: has.then_some(from),
            });
        }
        for _ in 0..reader.read_u32()? {
            let digest = read_digest(reader)?;
            self.rules_votes
                .push((digest, Signers::from_bits(reader.read_u16()?)));
        }
        self.escrow_votes = read_votes(reader)?;
        self.signer_list_votes = read_votes(reader)?;
        self.reset_votes = read_votes(reader)?;
        self.shutdown_votes = Signers::from_bits(reader.read_u16()?);
        Ok(())
    }
}

fn read_votes(reader: &mut Reader<'_>) -> Result<Vec<(u32, Signers)>, HubDecodeError> {
    let mut votes = Vec::new();
    for _ in 0..reader.read_u32()? {
        votes.push((reader.read_u32()?, Signers::from_bits(reader.read_u16()?)));
    }
    Ok(votes)
}

fn read_escrow(reader: &mut Reader<'_>) -> Result<Escrow, HubDecodeError> {
    let balance = reader.read_u64()?;
    let next_sequence = reader.read_u32()?;
    let checked = reader.read_bool()?;
    let deposit_floor = reader.read_u32()?;
    let has_list = reader.read_bool()?;
    let list = reader.read_u32()?;
    Ok(Escrow {
        balance,
        next_sequence,
        checked,
        deposit_floor,
        signer_list: has_list.then_some(list),
    })
}

fn read_validator_state(reader: &mut Reader<'_>) -> Result<ValidatorState, HubDecodeError> {
    let length = usize::from(reader.read_u16()?);
    let endpoint = core::str::from_utf8(reader.read_bytes(length)?)
        .map_err(|_| HubDecodeError::Invalid("endpoint"))?
        .into();
    Ok(ValidatorState {
        endpoint,
        sequence: reader.read_u64()?,
        last_recorded: reader.read_u64()?,
        account: AccountNumber(reader.read_u32()?),
    })
}

fn read_destination(reader: &mut Reader<'_>) -> Result<Option<Destination>, HubDecodeError> {
    let present = reader.read_bool()?;
    let account = *reader.read_array::<20>()?;
    let has_tag = reader.read_bool()?;
    let tag = reader.read_u32()?;
    Ok(present.then_some(Destination {
        account,
        tag: has_tag.then_some(tag),
    }))
}

fn read_account(reader: &mut Reader<'_>) -> Result<Account, HubDecodeError> {
    Ok(Account {
        key: read_key(reader)?,
        balance: reader.read_u64()?,
        reserve: reader.read_u64()?,
        sequence: reader.read_u64()?,
        disbursal: read_destination(reader)?,
        closing: reader.read_bool()?,
        holds: reader.read_u16()?,
        paying: reader.read_u16()?,
        withdrawals: reader.read_u16()?,
        validator: reader.read_bool()?,
        member_slot: reader.read_bool()?,
    })
}

fn read_hold(reader: &mut Reader<'_>) -> Result<Hold, HubDecodeError> {
    let payer = AccountNumber(reader.read_u32()?);
    let payee = AccountNumber(reader.read_u32()?);
    let amount = reader.read_u64()?;
    let reserve = reader.read_u64()?;
    let has_reduction = reader.read_bool()?;
    let reduction = Reduction {
        amount: reader.read_u64()?,
        at: reader.read_u64()?,
    };
    let total = reader.read_u64()?;
    let through = reader.read_u64()?;
    let mut claimed = BTreeMap::new();
    for _ in 0..reader.read_u32()? {
        claimed.insert(reader.read_u64()?, reader.read_u64()?);
    }
    Ok(Hold {
        payer,
        payee,
        amount,
        reserve,
        reduction: has_reduction.then_some(reduction),
        total,
        through,
        claimed,
    })
}

fn read_withdrawal(reader: &mut Reader<'_>) -> Result<Withdrawal, HubDecodeError> {
    let has_account = reader.read_bool()?;
    let account = reader.read_u32()?;
    let amount = reader.read_u64()?;
    let ledger_fee = reader.read_u64()?;
    let to_account = *reader.read_array::<20>()?;
    let has_tag = reader.read_bool()?;
    let tag = reader.read_u32()?;
    Ok(Withdrawal {
        account: has_account.then_some(AccountNumber(account)),
        amount,
        ledger_fee,
        to: Destination {
            account: to_account,
            tag: has_tag.then_some(tag),
        },
        closes: reader.read_bool()?,
        delivered: Signers::from_bits(reader.read_u16()?),
        failed: Signers::from_bits(reader.read_u16()?),
    })
}

fn read_pending(reader: &mut Reader<'_>) -> Result<PendingDeposit, HubDecodeError> {
    let mut pending = PendingDeposit::default();
    for _ in 0..reader.read_u32()? {
        let deposit = Deposit::decode(reader)?;
        pending
            .attestations
            .push((deposit, Signers::from_bits(reader.read_u16()?)));
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_ledger_restores_from_its_snapshot() {
        let (rules, _) = crate::rules::tests::rules(4);
        let ledger = Ledger::genesis(rules, 1_000).unwrap();
        let restored = Ledger::restore(&ledger.snapshot()).unwrap();
        assert_eq!(restored, ledger);
        assert_eq!(restored.state_root(), ledger.state_root());
    }

    proptest! {
        #[test]
        fn restoring_garbage_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let _ = Ledger::restore(&bytes);
        }
    }
}
