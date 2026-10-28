//! The whole state of a ledger as bytes: what a validator persists after
//! each final block, and what one that fell behind takes from another and
//! checks against a certified block's state root
//! (spec/designs/settlement-hubs.md#9-shutdown, "The state").
//!
//! Decoding never panics and allocates only as it reads, whatever the
//! input. Restoring takes the state root the snapshot must have, the one a
//! certified block carries, so no state is relied on before its root is
//! checked; only then are its invariants asserted.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_oer::{Encode, Reader};

use super::Ledger;
use super::state::{
    Account, Escrow, FeePool, Hold, PendingDeposit, PendingRules, PendingUpgrade, Phase, Reduction,
    ValidatorState, Withdrawal,
};
use crate::codec::{HubDecodeError, read_digest, read_key};
use crate::digest::Digest;
use crate::operation::{AccountNumber, Clawback, Deposit, Destination, Issuer};
use crate::promise::HoldNumber;
use crate::rules::{HubId, Rules};
use crate::validators::Signers;

/// The most validator states a snapshot carries.
const MAX_VALIDATOR_STATES: u32 = 64;

/// The most votes for the issuer's hold a snapshot carries: one per
/// validator.
const MAX_ISSUER_VOTES: usize = crate::validators::MAX_VALIDATORS;

/// The most pending clawbacks a snapshot carries: each validator's share.
const MAX_CLAWBACK_VOTES: usize = crate::validators::MAX_VALIDATORS * super::CLAWBACK_SHARE;

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
    fn issuer(&mut self, issuer: &Issuer) {
        self.u32(issuer.ledger);
        self.bool(issuer.frozen);
        self.bool(issuer.fee);
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
        self.snapshot_issuer(&mut out);
        if self.has_protocol_state() {
            self.snapshot_protocol(&mut out);
        }
        out.0
    }

    /// Whether the state holds anything version 1 has no place for: a
    /// version other than its rules', votes for a newer one, a move waiting,
    /// or claims waiting for their payout. A state without any writes the
    /// bytes it wrote before version 2, so validators of a hub of version 1
    /// that run a release from before it read one another's snapshots.
    pub(crate) fn has_protocol_state(&self) -> bool {
        self.has_protocol_header() || self.holds.values().any(|h| !h.pending.is_empty())
    }

    /// Whether the header holds the hub's version and its votes: once it
    /// runs another version than its rules', or votes for a newer one.
    pub(crate) fn has_protocol_header(&self) -> bool {
        self.version != self.rules.terms.version
            || !self.upgrade_votes.is_empty()
            || self.pending_upgrade.is_some()
    }

    fn snapshot_protocol(&self, out: &mut Out) {
        out.u16(self.version);
        out.count(self.upgrade_votes.len());
        for (version, signers) in &self.upgrade_votes {
            out.u16(*version);
            out.u16(signers.bits());
        }
        out.bool(self.pending_upgrade.is_some());
        let upgrade = self
            .pending_upgrade
            .unwrap_or(PendingUpgrade { version: 0, at: 0 });
        out.u16(upgrade.version);
        out.u64(upgrade.at);
        let waiting: Vec<_> = self
            .holds
            .iter()
            .filter(|(_, hold)| !hold.pending.is_empty())
            .collect();
        out.count(waiting.len());
        for (number, hold) in waiting {
            out.u64(number.0);
            out.bytes(&hold.pending_bytes());
        }
    }

    fn snapshot_issuer(&self, out: &mut Out) {
        out.bool(self.frozen.is_some());
        out.u64(self.frozen.unwrap_or(0));
        out.issuer(&self.issuer);
        out.count(self.issuer_votes.len());
        for (issuer, signers) in &self.issuer_votes {
            out.issuer(issuer);
            out.u16(signers.bits());
        }
        out.u64(self.escrow.clawed_back);
        out.u32(self.escrow.clawback_floor);
        out.count(self.clawback_votes.len());
        for (clawback, signers) in &self.clawback_votes {
            out.bytes(clawback.transaction.as_bytes());
            out.u32(clawback.ledger);
            out.u64(clawback.amount);
            out.u16(signers.bits());
        }
        out.count(self.recent_clawbacks.len());
        for (hash, ledger) in &self.recent_clawbacks {
            out.bytes(hash.as_bytes());
            out.u32(*ledger);
        }
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
        out.u16(self.reset_votes.bits());
        out.bool(self.reset_votes_since.is_some());
        out.u64(self.reset_votes_since.unwrap_or(0));
        out.bool(self.written_off_at.is_some());
        out.u64(self.written_off_at.unwrap_or(0));
        out.u16(self.shutdown_votes.bits());
    }

    /// The ledger a snapshot holds, if its state root is `root`: the root a
    /// certified block carries, or the one a validator recorded with its
    /// own snapshot. A snapshot has one encoding per state, every table in
    /// strictly ascending order, and the index of keys is rebuilt from the
    /// accounts, so a state whose root matches is the state that root
    /// names.
    ///
    /// # Errors
    ///
    /// The bytes are not one canonical snapshot, or its root is not `root`.
    pub fn restore(bytes: &[u8], root: &Digest) -> Result<Self, HubDecodeError> {
        let ledger = Self::decode_snapshot(bytes)?;
        if ledger.state_root() != *root {
            return Err(HubDecodeError::Invalid("state root"));
        }
        Ok(ledger)
    }

    fn decode_snapshot(bytes: &[u8]) -> Result<Self, HubDecodeError> {
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
            hold_index: alloc::collections::BTreeSet::new(),
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
            reset_votes: Signers::NONE,
            reset_votes_since: None,
            written_off_at: None,
            set_since,
            frozen: None,
            issuer: Issuer::default(),
            issuer_votes: Vec::new(),
            clawback_votes: Vec::new(),
            recent_clawbacks: BTreeMap::new(),
            version: rules.terms.version,
            upgrade_votes: Vec::new(),
            pending_upgrade: None,
            rules,
        };
        ledger.restore_tables(&mut reader)?;
        ledger.restore_votes(&mut reader)?;
        ledger.restore_issuer(&mut reader)?;
        if !reader.is_empty() {
            ledger.restore_protocol(&mut reader)?;
        }
        reader.finish()?;
        Ok(ledger)
    }

    /// What `snapshot_protocol` wrote, which is there only if the state
    /// has any of it.
    fn restore_protocol(&mut self, reader: &mut Reader<'_>) -> Result<(), HubDecodeError> {
        self.version = reader.read_u16()?;
        if !crate::rules::runs(self.version) {
            return Err(HubDecodeError::Invalid(
                "a hub version this code does not run",
            ));
        }
        let count = reader.read_u32()?;
        if usize::try_from(count).unwrap_or(usize::MAX) > crate::validators::MAX_VALIDATORS {
            return Err(HubDecodeError::Invalid("upgrade votes"));
        }
        for _ in 0..count {
            let version = reader.read_u16()?;
            let signers = Signers::from_bits(reader.read_u16()?);
            self.upgrade_votes.push((version, signers));
        }
        let has_upgrade = reader.read_bool()?;
        let upgrade = PendingUpgrade {
            version: reader.read_u16()?,
            at: reader.read_u64()?,
        };
        if !has_upgrade && upgrade != (PendingUpgrade { version: 0, at: 0 }) {
            return Err(HubDecodeError::Invalid("upgrade without presence"));
        }
        self.pending_upgrade = has_upgrade.then_some(upgrade);
        let mut last = None;
        for _ in 0..reader.read_u32()? {
            let number = HoldNumber(reader.read_u64()?);
            if last.is_some_and(|last| last >= number) {
                return Err(HubDecodeError::Invalid("pending claims out of order"));
            }
            last = Some(number);
            let hold = self
                .holds
                .get_mut(&number)
                .ok_or(HubDecodeError::Invalid("pending claims of no hold"))?;
            hold.pending_due = reader.read_u64()?;
            for _ in 0..reader.read_u32()? {
                let sequence = reader.read_u64()?;
                insert_ascending(&mut hold.pending, sequence, reader.read_u64()?)?;
            }
            if hold.pending.is_empty() {
                return Err(HubDecodeError::Invalid("a hold without pending claims"));
            }
        }
        if !self.has_protocol_state() {
            return Err(HubDecodeError::Invalid("protocol state that holds nothing"));
        }
        Ok(())
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
            insert_ascending(&mut self.accounts, number, read_account(reader)?)?;
        }
        // The index of keys is derived: it is not in the state root, so it
        // is rebuilt, never read.
        for (number, account) in &self.accounts {
            if self.keys.insert(*account.key.as_bytes(), *number).is_some() {
                return Err(HubDecodeError::Invalid("a key with two accounts"));
            }
        }
        for _ in 0..reader.read_u32()? {
            let number = HoldNumber(reader.read_u64()?);
            insert_ascending(&mut self.holds, number, read_hold(reader)?)?;
        }
        self.hold_index = self.derived_hold_index();
        for _ in 0..reader.read_u32()? {
            let sequence = reader.read_u32()?;
            insert_ascending(&mut self.withdrawals, sequence, read_withdrawal(reader)?)?;
        }
        for _ in 0..reader.read_u32()? {
            let hash = read_digest(reader)?;
            insert_ascending(&mut self.pending_deposits, hash, read_pending(reader)?)?;
        }
        for _ in 0..reader.read_u32()? {
            let hash = read_digest(reader)?;
            insert_ascending(&mut self.recent_deposits, hash, reader.read_u32()?)?;
        }
        for _ in 0..reader.read_u32()? {
            let pool = FeePool {
                height: reader.read_u64()?,
                amount: reader.read_u64()?,
                voters: Signers::from_bits(reader.read_u16()?),
            };
            if self
                .pools
                .last()
                .is_some_and(|last| last.height >= pool.height)
            {
                return Err(HubDecodeError::Invalid("fee pools out of order"));
            }
            self.pools.push(pool);
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
        self.reset_votes = Signers::from_bits(reader.read_u16()?);
        let since = reader.read_bool()?;
        let since_at = reader.read_u64()?;
        self.reset_votes_since = since.then_some(since_at);
        let written_off = reader.read_bool()?;
        let at = reader.read_u64()?;
        self.written_off_at = written_off.then_some(at);
        self.shutdown_votes = Signers::from_bits(reader.read_u16()?);
        Ok(())
    }

    fn restore_issuer(&mut self, reader: &mut Reader<'_>) -> Result<(), HubDecodeError> {
        let frozen = reader.read_bool()?;
        let since = reader.read_u64()?;
        self.frozen = frozen.then_some(since);
        self.issuer = read_issuer(reader)?;
        let count = reader.read_u32()?;
        if usize::try_from(count).unwrap_or(usize::MAX) > MAX_ISSUER_VOTES {
            return Err(HubDecodeError::Invalid("issuer votes"));
        }
        for _ in 0..count {
            let issuer = read_issuer(reader)?;
            self.issuer_votes
                .push((issuer, Signers::from_bits(reader.read_u16()?)));
        }
        self.escrow.clawed_back = reader.read_u64()?;
        self.escrow.clawback_floor = reader.read_u32()?;
        let count = reader.read_u32()?;
        if usize::try_from(count).unwrap_or(usize::MAX) > MAX_CLAWBACK_VOTES {
            return Err(HubDecodeError::Invalid("clawback votes"));
        }
        for _ in 0..count {
            let clawback = Clawback {
                transaction: read_digest(reader)?,
                ledger: reader.read_u32()?,
                amount: reader.read_u64()?,
            };
            self.clawback_votes
                .push((clawback, Signers::from_bits(reader.read_u16()?)));
        }
        let count = reader.read_u32()?;
        if usize::try_from(count).unwrap_or(usize::MAX) > super::MAX_CLAWBACKS {
            return Err(HubDecodeError::Invalid("clawbacks"));
        }
        for _ in 0..count {
            let hash = read_digest(reader)?;
            insert_ascending(&mut self.recent_clawbacks, hash, reader.read_u32()?)?;
        }
        Ok(())
    }
}

fn read_issuer(reader: &mut Reader<'_>) -> Result<Issuer, HubDecodeError> {
    Ok(Issuer {
        ledger: reader.read_u32()?,
        frozen: reader.read_bool()?,
        fee: reader.read_bool()?,
    })
}

/// Inserts an entry of a table whose keys a snapshot writes in strictly
/// ascending order: one out of order, or twice, is not a snapshot.
fn insert_ascending<K: Ord, V>(
    map: &mut BTreeMap<K, V>,
    key: K,
    value: V,
) -> Result<(), HubDecodeError> {
    if map.last_key_value().is_some_and(|(last, _)| *last >= key) {
        return Err(HubDecodeError::Invalid("table out of order"));
    }
    map.insert(key, value);
    Ok(())
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
    // What the issuer clawed back comes with the issuer's acts, last.
    Ok(Escrow {
        balance,
        next_sequence,
        checked,
        deposit_floor,
        signer_list: has_list.then_some(list),
        clawed_back: 0,
        clawback_floor: 0,
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
        deposited: reader.read_u64()?,
        withdrawn: reader.read_u64()?,
        returned: reader.read_u64()?,
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
        insert_ascending(&mut claimed, reader.read_u64()?, reader.read_u64()?)?;
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
        pending: BTreeMap::new(),
        pending_due: 0,
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
        let restored = Ledger::restore(&ledger.snapshot(), &ledger.state_root()).unwrap();
        assert_eq!(restored, ledger);
        assert!(Ledger::restore(&ledger.snapshot(), &Digest([1; 32])).is_err());
    }

    fn entry(number: AccountNumber, account: &Account) -> Vec<u8> {
        [number.0.to_be_bytes().as_slice(), &account.to_vec()].concat()
    }

    fn find(haystack: &[u8], needle: &[u8]) -> usize {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap()
    }

    #[test]
    fn a_snapshot_with_an_account_twice_is_refused() {
        let mut hub = crate::ledger::tests::Hub::new();
        hub.join(1, 10_000_000);
        let ledger = hub.ledger.clone();
        let snapshot = ledger.snapshot();
        let (first, first_account) = ledger.accounts().next().unwrap();
        let (last, account) = ledger.accounts().last().unwrap();
        // The last account once more before itself, under another key: the
        // second entry wins the table, so the root stays the same, but the
        // index of keys would name the account twice.
        let mut forged = account.clone();
        forged.key = dassie_identity::NodeKey::from_bytes(&[77; 32]).public_key();
        let mut bytes = snapshot.clone();
        let at = find(&snapshot, &entry(last, account));
        bytes.splice(at..at, entry(last, &forged));
        let count_at = find(&snapshot, &entry(first, first_account)) - 4;
        let count = u32::from_be_bytes(bytes[count_at..count_at + 4].try_into().unwrap());
        bytes[count_at..count_at + 4].copy_from_slice(&(count + 1).to_be_bytes());
        assert!(Ledger::restore(&bytes, &ledger.state_root()).is_err());
        assert!(Ledger::restore(&snapshot, &ledger.state_root()).is_ok());
    }

    proptest! {
        #[test]
        fn restoring_garbage_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
            let _ = Ledger::restore(&bytes, &Digest::default());
        }
    }
}
