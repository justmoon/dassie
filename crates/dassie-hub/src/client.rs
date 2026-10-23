//! What members and validators say to each other
//! (spec/designs/settlement-hubs.md#3-the-ledger, "Receipts").
//!
//! A member submits its operations to every validator, and asks them what
//! the log holds. Validators answer with **statements** each signs alone
//! with its consensus key: an operation executed in a final block and its
//! outcome, the newest final block, an account or a hold as of a final
//! block. A member trusts a statement once f + 1 distinct validators signed
//! the same one: one of them is honest. So every message proves itself,
//! and a member need not be known to the validators
//! (spec/decisions/settlement-hubs.md).
//!
//! A payer learns of claims on its holds from the statements of the
//! operations that made them, which carry the fulfillments it needs to
//! fulfill upstream (spec/designs/settlement-hubs.md#4-peering-over-a-hub).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey, Signature};
use dassie_oer::{Encode, Reader, Writer, size};

use crate::codec::{HubDecodeError, read_key, read_signature, write_key, write_signature};
use crate::digest::{Digest, Domain};
use crate::ledger::{Account, Hold, Ledger, Outcome};
use crate::operation::{AccountNumber, Operation};
use crate::promise::HoldNumber;
use crate::rules::{Amount, HubId, Millis};
use crate::validators::{Signers, ValidatorIndex, ValidatorSet};

/// The most statements one answer or push carries.
pub const MAX_STATEMENTS: usize = 64;

/// The most holds one [`Statement::Holds`] lists: one page of them.
pub const MAX_HOLDS_LISTED: usize = 1_024;

/// The most pages the holds of one account take: an account takes part in
/// at most `u16::MAX` holds (`ledger/member.rs`).
pub const MAX_HOLD_PAGES: usize = (u16::MAX as usize).div_ceil(MAX_HOLDS_LISTED);

/// What an account holds, as a statement tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountView {
    /// Its key.
    pub key: PublicKey,
    /// What it can spend.
    pub balance: Amount,
    /// Its account reserve.
    pub reserve: Amount,
    /// The sequence its next operation must carry.
    pub sequence: u64,
    /// Whether it has a disbursal address.
    pub disbursal: bool,
    /// Whether it is closing.
    pub closing: bool,
}

/// A hold, as a statement tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldView {
    /// Who set the funds aside.
    pub payer: AccountNumber,
    /// Who may take from them.
    pub payee: AccountNumber,
    /// What is left in it.
    pub amount: Amount,
    /// A reduction waiting for its notice: how much, and from when.
    pub reduction: Option<(Amount, Millis)>,
    /// T of the last redeemed claim.
    pub total: Amount,
    /// W of the last redeemed claim.
    pub through: u64,
}

/// What a validator says the log holds, as of a final block.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Statement {
    /// `operation` was in the final block at `height`, stamped
    /// `timestamp`, at place `index` of its operations, with `outcome`.
    /// The place names it: a faulty leader may put one operation in a
    /// block twice, applied the first time and refused the second, and a
    /// member reads a block's statements in the log's order.
    Executed {
        /// The block's height.
        height: u64,
        /// Its timestamp.
        timestamp: Millis,
        /// Its place among the block's operations, from 0.
        index: u32,
        /// The operation, whole.
        operation: Operation,
        /// What became of it.
        outcome: Outcome,
    },
    /// The newest final block: what a payer waits for before it rejects a
    /// packet whose commitment expired.
    Final {
        /// Its height.
        height: u64,
        /// Its timestamp.
        timestamp: Millis,
    },
    /// The account of `key`, if it has one, after the block at `height`.
    Account {
        /// The block's height.
        height: u64,
        /// The key asked about.
        key: PublicKey,
        /// Its account, if any.
        account: Option<(AccountNumber, AccountView)>,
    },
    /// What happened below `below` is no longer kept: an account that
    /// asked from further back reads its state anew, as of `height`.
    Trimmed {
        /// The newest final block.
        height: u64,
        /// The lowest height still kept in full.
        below: u64,
    },
    /// Hold `number` after the block at `height`, if it exists.
    Hold {
        /// The block's height.
        height: u64,
        /// The hold asked about.
        number: HoldNumber,
        /// The hold, if it exists.
        hold: Option<HoldView>,
    },
    /// Page `page` of `pages` of every hold `account` pays into or takes
    /// from after the block at `height`, stamped `timestamp`, in order of
    /// their numbers, [`MAX_HOLDS_LISTED`] to a page: the pages together
    /// are the whole list. What a member that can no longer read its
    /// history in order reads its holds anew from.
    Holds {
        /// The block's height.
        height: u64,
        /// Its timestamp.
        timestamp: Millis,
        /// The account.
        account: AccountNumber,
        /// This page, from 0.
        page: u16,
        /// How many pages the list takes; at least one.
        pages: u16,
        /// Its holds on this page.
        holds: Vec<(HoldNumber, HoldView)>,
    },
}

/// A statement with the validator's signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedStatement {
    /// Who signs.
    pub validator: ValidatorIndex,
    /// What it says.
    pub statement: Statement,
    /// Its consensus key's signature over [`Statement::digest`].
    pub signature: Signature,
}

/// What a member asks a validator. Requests are decoded once each and
/// taken apart at once: boxing would only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Put this operation in a block.
    Submit(Operation),
    /// The account of this key.
    Account(PublicKey),
    /// This hold.
    Hold(HoldNumber),
    /// The account of this number, and every hold it pays into or takes
    /// from, as of one block.
    Holds(AccountNumber),
    /// What was executed that touches `account`, from the block at `from`
    /// on, and the newest final block. Asked again and again by a member
    /// a validator cannot reach, and kept open by one it can.
    Since {
        /// The account.
        account: AccountNumber,
        /// The first height of interest.
        from: u64,
    },
}

impl AccountView {
    /// The view of `account`.
    #[must_use]
    pub const fn of(account: &Account) -> Self {
        Self {
            key: account.key,
            balance: account.balance,
            reserve: account.reserve,
            sequence: account.sequence,
            disbursal: account.disbursal.is_some(),
            closing: account.closing,
        }
    }
}

impl HoldView {
    /// The view of `hold`.
    #[must_use]
    pub fn of(hold: &Hold) -> Self {
        Self {
            payer: hold.payer,
            payee: hold.payee,
            amount: hold.amount,
            reduction: hold.reduction.map(|r| (r.amount, r.at)),
            total: hold.total,
            through: hold.through,
        }
    }
}

impl Statement {
    /// The digest a validator signs: the hub ID and the statement.
    #[must_use]
    pub fn digest(&self, hub: &HubId) -> Digest {
        Digest::of(Domain::Statement, &[hub.0.as_bytes(), &self.to_vec()])
    }

    /// What `ledger` says of the account of `key`.
    #[must_use]
    pub fn account(ledger: &Ledger, key: PublicKey) -> Self {
        Self::Account {
            height: ledger.height(),
            key,
            account: ledger.account_of(&key).and_then(|number| {
                ledger
                    .account(number)
                    .map(|account| (number, AccountView::of(account)))
            }),
        }
    }

    /// What `ledger` says of hold `number`.
    #[must_use]
    pub fn hold(ledger: &Ledger, number: HoldNumber) -> Self {
        Self::Hold {
            height: ledger.height(),
            number,
            hold: ledger.hold(number).map(HoldView::of),
        }
    }

    /// What `ledger` says of the holds of `account`: every one, over as
    /// many pages as they take.
    #[must_use]
    pub fn holds(ledger: &Ledger, account: AccountNumber) -> Vec<Self> {
        let listed: Vec<(HoldNumber, HoldView)> = ledger
            .holds_of(account)
            .map(|(number, hold)| (number, HoldView::of(hold)))
            .collect();
        let chunks: Vec<&[(HoldNumber, HoldView)]> = if listed.is_empty() {
            alloc::vec![&[]]
        } else {
            listed.chunks(MAX_HOLDS_LISTED).collect()
        };
        assert!(
            chunks.len() <= MAX_HOLD_PAGES,
            "more holds than an account takes part in"
        );
        let pages = u16::try_from(chunks.len()).unwrap_or(u16::MAX);
        (0u16..)
            .zip(chunks)
            .map(|(page, holds)| Self::Holds {
                height: ledger.height(),
                timestamp: ledger.timestamp(),
                account,
                page,
                pages,
                holds: holds.to_vec(),
            })
            .collect()
    }

    /// The height it is as of.
    #[must_use]
    pub const fn height(&self) -> u64 {
        match self {
            Self::Executed { height, .. }
            | Self::Final { height, .. }
            | Self::Account { height, .. }
            | Self::Trimmed { height, .. }
            | Self::Hold { height, .. }
            | Self::Holds { height, .. } => *height,
        }
    }
}

impl SignedStatement {
    /// Signs `statement` as validator `validator`.
    #[must_use]
    pub fn sign(
        hub: &HubId,
        validator: ValidatorIndex,
        statement: Statement,
        key: &NodeKey,
    ) -> Self {
        let signature = key.sign(statement.digest(hub).as_bytes());
        Self {
            validator,
            statement,
            signature,
        }
    }

    /// Whether validator `validator` of `set` signed it.
    #[must_use]
    pub fn verifies(&self, hub: &HubId, set: &ValidatorSet) -> bool {
        set.key(self.validator).is_some_and(|key| {
            key.verify(self.statement.digest(hub).as_bytes(), &self.signature)
                .is_ok()
        })
    }
}

/// How far above the height f + 1 validators have reached a statement may
/// be: no honest validator signs one further ahead than the hub is, and at
/// least one of any f + 1 is honest.
pub const MAX_HEIGHTS_AHEAD: u64 = 1 << 16;

/// Signatures [`Agreement`] gave up on before their statement agreed: the
/// lowest height of them, and whose they were. A member that counted on one
/// of them, as part of an answer up to a `Final`, reads from below that
/// height again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropped {
    /// The lowest height of a signature dropped.
    pub lowest: u64,
    /// Whose signatures were dropped.
    pub signers: Signers,
}

/// A statement some validators signed.
#[derive(Clone, Debug)]
struct Pending {
    statement: Statement,
    signers: Signers,
    agreed: bool,
}

/// Gathers signed statements until f + 1 validators agree on one.
///
/// Bounded per signer, so no validator can push out another's signatures:
/// each signer has at most `capacity / n` signatures on statements still
/// waiting for agreement, and one more of its signatures is refused.
/// Statements that agreed are kept, up to `capacity`, so that each agrees
/// once; the lowest go first. A statement further than
/// [`MAX_HEIGHTS_AHEAD`] above the height f + 1 validators have reached is
/// refused. What it drops or refuses of a statement still waiting it
/// reports ([`Agreement::take_dropped`]).
#[derive(Clone, Debug)]
pub struct Agreement {
    hub: HubId,
    set: ValidatorSet,
    capacity: usize,
    quota: usize,
    pending: BTreeMap<(u64, Digest), Pending>,
    /// Per signer, its signatures on statements still waiting.
    waiting: Vec<usize>,
    /// Per signer, the highest height it signed a statement at.
    reached: Vec<u64>,
    dropped: Option<Dropped>,
}

impl Agreement {
    /// A collector for statements of hub `hub` by `set`, keeping at most
    /// `capacity` statements that agreed and `capacity / n` signatures per
    /// validator on statements that wait.
    #[must_use]
    pub fn new(hub: HubId, set: ValidatorSet, capacity: usize) -> Self {
        let n = set.len();
        let capacity = capacity.max(1);
        Self {
            hub,
            quota: (capacity / n.max(1)).max(1),
            capacity,
            pending: BTreeMap::new(),
            waiting: alloc::vec![0; n],
            reached: alloc::vec![0; n],
            dropped: None,
            set,
        }
    }

    /// Takes in a signed statement. Returns the statement once f + 1
    /// validators signed it, exactly once while it is kept; a statement
    /// that does not verify is dropped.
    pub fn add(&mut self, signed: SignedStatement) -> Option<Statement> {
        if !signed.verifies(&self.hub, &self.set) {
            return None;
        }
        let signer = signed.validator;
        let height = signed.statement.height();
        if let Some(reached) = self.reached.get_mut(signer.get()) {
            *reached = (*reached).max(height);
        }
        if height > self.expected().saturating_add(MAX_HEIGHTS_AHEAD) {
            self.drop_signature(height, signer);
            return None;
        }
        let key = (height, signed.statement.digest(&self.hub));
        match self.pending.get(&key) {
            Some(entry) if entry.signers.contains(signer) => return None,
            Some(entry) if entry.agreed => {
                let signers = entry.signers.with(signer);
                if let Some(entry) = self.pending.get_mut(&key) {
                    entry.signers = signers;
                }
                return None;
            }
            _ => {}
        }
        // A signer at its quota has its new signature refused, not its old
        // ones dropped: the lowest are the ones to agree first, and once
        // they did the signer has room again.
        if self.full(signer) {
            self.drop_signature(height, signer);
            return None;
        }
        let entry = self.pending.entry(key).or_insert_with(|| Pending {
            statement: signed.statement,
            signers: Signers::NONE,
            agreed: false,
        });
        entry.signers = entry.signers.with(signer);
        if let Some(count) = self.waiting.get_mut(signer.get()) {
            *count = count.saturating_add(1);
        }
        self.agree(key)
    }

    /// The highest height f + 1 validators have each reached: one of them
    /// is honest, so the hub has reached it.
    fn expected(&self) -> u64 {
        let mut reached = self.reached.clone();
        reached.sort_unstable_by(|a, b| b.cmp(a));
        reached
            .get(self.set.one_honest().saturating_sub(1))
            .copied()
            .unwrap_or(0)
    }

    /// Marks the statement at `key` agreed if f + 1 signed it, and takes
    /// its signatures off their signers' counts.
    fn agree(&mut self, key: (u64, Digest)) -> Option<Statement> {
        let needed = self.set.one_honest();
        let entry = self.pending.get_mut(&key)?;
        if entry.agreed || entry.signers.count() < needed {
            return None;
        }
        entry.agreed = true;
        let (statement, signers) = (entry.statement.clone(), entry.signers);
        for index in signers.iter() {
            if let Some(count) = self.waiting.get_mut(index.get()) {
                *count = count.saturating_sub(1);
            }
        }
        // One more agreed: at most one over the bound.
        if self.pending.values().filter(|p| p.agreed).count() > self.capacity {
            let lowest = self.pending.iter().find(|(_, p)| p.agreed).map(|(k, _)| *k);
            if let Some(lowest) = lowest {
                self.pending.remove(&lowest);
            }
        }
        Some(statement)
    }

    /// Whether `signer` has its quota of signatures on statements still
    /// waiting.
    fn full(&self, signer: ValidatorIndex) -> bool {
        self.waiting
            .get(signer.get())
            .is_some_and(|count| *count >= self.quota)
    }

    fn drop_signature(&mut self, height: u64, signer: ValidatorIndex) {
        self.dropped = Some(match self.dropped {
            Some(dropped) => Dropped {
                lowest: dropped.lowest.min(height),
                signers: dropped.signers.with(signer),
            },
            None => Dropped {
                lowest: height,
                signers: Signers::NONE.with(signer),
            },
        });
    }

    /// What was dropped or refused of statements still waiting since the
    /// last call, if anything.
    pub fn take_dropped(&mut self) -> Option<Dropped> {
        self.dropped.take()
    }

    /// Forgets statements below `height`.
    pub fn forget_below(&mut self, height: u64) {
        self.pending = self.pending.split_off(&(height, Digest::default()));
        self.waiting = self.count_waiting();
    }

    fn count_waiting(&self) -> Vec<usize> {
        let mut waiting = alloc::vec![0usize; self.set.len()];
        for entry in self.pending.values().filter(|p| !p.agreed) {
            for index in entry.signers.iter() {
                if let Some(count) = waiting.get_mut(index.get()) {
                    *count = count.saturating_add(1);
                }
            }
        }
        waiting
    }

    /// Asserts its bounds.
    pub fn check_invariants(&self) {
        assert_eq!(
            self.waiting,
            self.count_waiting(),
            "agreement counts out of step"
        );
        assert!(
            self.waiting.iter().all(|count| *count <= self.quota),
            "a signer over its quota"
        );
        assert!(
            self.pending.values().filter(|p| p.agreed).count() <= self.capacity,
            "agreed statements over their bound"
        );
    }
}

// Encoding: a CHOICE is a one-byte tag; options a presence byte.

impl Encode for AccountView {
    fn encoded_len(&self) -> usize {
        32 + 8 * 3 + 2
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        write_key(writer, &self.key);
        writer.write_u64(self.balance);
        writer.write_u64(self.reserve);
        writer.write_u64(self.sequence);
        writer.write_bool(self.disbursal);
        writer.write_bool(self.closing);
    }
}

impl AccountView {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(Self {
            key: read_key(reader)?,
            balance: reader.read_u64()?,
            reserve: reader.read_u64()?,
            sequence: reader.read_u64()?,
            disbursal: reader.read_bool()?,
            closing: reader.read_bool()?,
        })
    }
}

impl Encode for HoldView {
    fn encoded_len(&self) -> usize {
        4 + 4 + 8 + 1 + 16 + 8 + 8
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u32(self.payer.0);
        writer.write_u32(self.payee.0);
        writer.write_u64(self.amount);
        writer.write_bool(self.reduction.is_some());
        let (amount, at) = self.reduction.unwrap_or((0, 0));
        writer.write_u64(amount);
        writer.write_u64(at);
        writer.write_u64(self.total);
        writer.write_u64(self.through);
    }
}

impl HoldView {
    fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let payer = AccountNumber(reader.read_u32()?);
        let payee = AccountNumber(reader.read_u32()?);
        let amount = reader.read_u64()?;
        let has_reduction = reader.read_bool()?;
        let reduction = (reader.read_u64()?, reader.read_u64()?);
        if !has_reduction && reduction != (0, 0) {
            return Err(HubDecodeError::Invalid("reduction without presence"));
        }
        Ok(Self {
            payer,
            payee,
            amount,
            reduction: has_reduction.then_some(reduction),
            total: reader.read_u64()?,
            through: reader.read_u64()?,
        })
    }
}

impl Encode for Statement {
    fn encoded_len(&self) -> usize {
        1 + 8
            + match self {
                Self::Executed {
                    operation, outcome, ..
                } => 8 + 4 + operation.encoded_len() + outcome.encoded_len(),
                Self::Final { .. } | Self::Trimmed { .. } => 8,
                Self::Account { account, .. } => {
                    32 + 1 + account.map_or(0, |(_, view)| 4 + view.encoded_len())
                }
                Self::Hold { hold, .. } => 8 + 1 + hold.map_or(0, |view| view.encoded_len()),
                Self::Holds { holds, .. } => {
                    8 + 4
                        + 2
                        + 2
                        + size::quantity(holds.len())
                        + holds
                            .iter()
                            .map(|(_, view)| 8 + view.encoded_len())
                            .sum::<usize>()
                }
            }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Executed {
                height,
                timestamp,
                index,
                operation,
                outcome,
            } => {
                writer.write_u8(0);
                writer.write_u64(*height);
                writer.write_u64(*timestamp);
                writer.write_u32(*index);
                operation.encode_to(writer);
                outcome.encode_to(writer);
            }
            Self::Final { height, timestamp } => {
                writer.write_u8(1);
                writer.write_u64(*height);
                writer.write_u64(*timestamp);
            }
            Self::Account {
                height,
                key,
                account,
            } => {
                writer.write_u8(2);
                writer.write_u64(*height);
                write_key(writer, key);
                writer.write_bool(account.is_some());
                if let Some((number, view)) = account {
                    writer.write_u32(number.0);
                    view.encode_to(writer);
                }
            }
            Self::Trimmed { height, below } => {
                writer.write_u8(4);
                writer.write_u64(*height);
                writer.write_u64(*below);
            }
            Self::Hold {
                height,
                number,
                hold,
            } => {
                writer.write_u8(3);
                writer.write_u64(*height);
                writer.write_u64(number.0);
                writer.write_bool(hold.is_some());
                if let Some(view) = hold {
                    view.encode_to(writer);
                }
            }
            Self::Holds { .. } => self.encode_holds(writer),
        }
    }
}

impl Statement {
    fn encode_holds(&self, writer: &mut Writer<'_>) {
        let Self::Holds {
            height,
            timestamp,
            account,
            page,
            pages,
            holds,
        } = self
        else {
            return;
        };
        writer.write_u8(5);
        writer.write_u64(*height);
        writer.write_u64(*timestamp);
        writer.write_u32(account.0);
        writer.write_u16(*page);
        writer.write_u16(*pages);
        writer.write_quantity(holds.len());
        for (number, view) in holds {
            writer.write_u64(number.0);
            view.encode_to(writer);
        }
    }

    fn decode_holds(reader: &mut Reader<'_>, height: u64) -> Result<Self, HubDecodeError> {
        let timestamp = reader.read_u64()?;
        let account = AccountNumber(reader.read_u32()?);
        let page = reader.read_u16()?;
        let pages = reader.read_u16()?;
        if page >= pages || usize::from(pages) > MAX_HOLD_PAGES {
            return Err(HubDecodeError::Invalid("page"));
        }
        let count = reader.read_quantity(MAX_HOLDS_LISTED)?;
        let mut holds = Vec::with_capacity(count);
        for _ in 0..count {
            holds.push((HoldNumber(reader.read_u64()?), HoldView::decode(reader)?));
        }
        Ok(Self::Holds {
            height,
            timestamp,
            account,
            page,
            pages,
            holds,
        })
    }
}

impl Statement {
    /// Decodes a statement.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let tag = reader.read_u8()?;
        let height = reader.read_u64()?;
        Ok(match tag {
            0 => Self::Executed {
                height,
                timestamp: reader.read_u64()?,
                index: reader.read_u32()?,
                operation: Operation::decode(reader)?,
                outcome: Outcome::decode(reader)?,
            },
            1 => Self::Final {
                height,
                timestamp: reader.read_u64()?,
            },
            2 => {
                let key = read_key(reader)?;
                let account = if reader.read_bool()? {
                    Some((
                        AccountNumber(reader.read_u32()?),
                        AccountView::decode(reader)?,
                    ))
                } else {
                    None
                };
                Self::Account {
                    height,
                    key,
                    account,
                }
            }
            3 => {
                let number = HoldNumber(reader.read_u64()?);
                let hold = if reader.read_bool()? {
                    Some(HoldView::decode(reader)?)
                } else {
                    None
                };
                Self::Hold {
                    height,
                    number,
                    hold,
                }
            }
            4 => Self::Trimmed {
                height,
                below: reader.read_u64()?,
            },
            5 => Self::decode_holds(reader, height)?,
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        })
    }
}

impl Encode for SignedStatement {
    fn encoded_len(&self) -> usize {
        1 + self.statement.encoded_len() + 64
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u8(u8::try_from(self.validator.get()).unwrap_or(u8::MAX));
        self.statement.encode_to(writer);
        write_signature(writer, &self.signature);
    }
}

impl SignedStatement {
    /// Decodes a signed statement; whether it verifies is the caller's.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let validator = ValidatorIndex::new(usize::from(reader.read_u8()?))
            .ok_or(HubDecodeError::Invalid("validator index"))?;
        Ok(Self {
            validator,
            statement: Statement::decode(reader)?,
            signature: read_signature(reader)?,
        })
    }

    /// Encodes a list of signed statements, as an answer or a push.
    #[must_use]
    pub fn encode_list(statements: &[Self]) -> Vec<u8> {
        let length = size::quantity(statements.len())
            + statements.iter().map(Encode::encoded_len).sum::<usize>();
        let mut bytes = alloc::vec![0; length];
        let mut writer = Writer::new(&mut bytes);
        writer.write_quantity(statements.len());
        for statement in statements {
            statement.encode_to(&mut writer);
        }
        bytes
    }

    /// Decodes a list of signed statements.
    pub fn decode_list(input: &[u8]) -> Result<Vec<Self>, HubDecodeError> {
        let mut reader = Reader::new(input);
        let count = reader.read_quantity(MAX_STATEMENTS)?;
        let mut list = Vec::with_capacity(count);
        for _ in 0..count {
            list.push(Self::decode(&mut reader)?);
        }
        reader.finish()?;
        Ok(list)
    }
}

impl Encode for Request {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::Submit(operation) => operation.encoded_len(),
            Self::Account(_) => 32,
            Self::Hold(_) => 8,
            Self::Holds(_) => 4,
            Self::Since { .. } => 4 + 8,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Submit(operation) => {
                writer.write_u8(0);
                operation.encode_to(writer);
            }
            Self::Account(key) => {
                writer.write_u8(1);
                write_key(writer, key);
            }
            Self::Hold(number) => {
                writer.write_u8(2);
                writer.write_u64(number.0);
            }
            Self::Since { account, from } => {
                writer.write_u8(3);
                writer.write_u32(account.0);
                writer.write_u64(*from);
            }
            Self::Holds(account) => {
                writer.write_u8(4);
                writer.write_u32(account.0);
            }
        }
    }
}

impl Request {
    /// Decodes a request that fills `input`.
    pub fn decode_exact(input: &[u8]) -> Result<Self, HubDecodeError> {
        let mut reader = Reader::new(input);
        let request = match reader.read_u8()? {
            0 => Self::Submit(Operation::decode(&mut reader)?),
            1 => Self::Account(read_key(&mut reader)?),
            2 => Self::Hold(HoldNumber(reader.read_u64()?)),
            3 => Self::Since {
                account: AccountNumber(reader.read_u32()?),
                from: reader.read_u64()?,
            },
            4 => Self::Holds(AccountNumber(reader.read_u32()?)),
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        };
        reader.finish()?;
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::Detail;

    fn set() -> (ValidatorSet, Vec<NodeKey>) {
        let keys: Vec<NodeKey> = (1..=4).map(|i| NodeKey::from_bytes(&[i; 32])).collect();
        (
            ValidatorSet::new(keys.iter().map(NodeKey::public_key).collect()).unwrap(),
            keys,
        )
    }

    #[test]
    fn f_plus_one_matching_statements_make_an_agreement() {
        let hub = HubId(Digest([1; 32]));
        let (set, keys) = set();
        let mut agreement = Agreement::new(hub, set, 16);
        let fact = Statement::Final {
            height: 5,
            timestamp: 100,
        };
        let lie = Statement::Final {
            height: 5,
            timestamp: 999,
        };
        let sign = |i: usize, s: &Statement| {
            SignedStatement::sign(&hub, ValidatorIndex::new(i).unwrap(), s.clone(), &keys[i])
        };
        assert_eq!(agreement.add(sign(0, &lie)), None);
        assert_eq!(agreement.add(sign(1, &fact)), None);
        // The same validator twice counts once.
        assert_eq!(agreement.add(sign(1, &fact)), None);
        assert_eq!(agreement.add(sign(2, &fact)), Some(fact.clone()));
        assert_eq!(agreement.add(sign(3, &fact)), None, "agreed once");
        // A forged signature counts for nothing.
        let mut forged = sign(0, &fact);
        forged.validator = ValidatorIndex::new(3).unwrap();
        assert_eq!(agreement.add(forged), None);
    }

    #[test]
    fn one_validators_flood_never_pushes_out_anothers_signatures() {
        let hub = HubId(Digest([1; 32]));
        let (set, keys) = set();
        let mut agreement = Agreement::new(hub, set, 16);
        let sign = |i: usize, s: Statement| {
            SignedStatement::sign(&hub, ValidatorIndex::new(i).unwrap(), s, &keys[i])
        };
        let fact = Statement::Final {
            height: 5,
            timestamp: 100,
        };
        assert_eq!(agreement.add(sign(1, fact.clone())), None);
        // Validator 0 signs many statements, near and far ahead.
        for k in 0..1_000u64 {
            for height in [6 + k, u64::MAX - k] {
                let flood = Statement::Final {
                    height,
                    timestamp: k,
                };
                assert_eq!(agreement.add(sign(0, flood)), None);
            }
            agreement.check_invariants();
        }
        let dropped = agreement.take_dropped().unwrap();
        assert_eq!(
            dropped.signers,
            Signers::NONE.with(ValidatorIndex::new(0).unwrap())
        );
        assert_eq!(agreement.add(sign(2, fact.clone())), Some(fact));
        agreement.check_invariants();
    }

    #[test]
    fn a_signer_over_its_quota_is_refused_and_it_says_so() {
        let hub = HubId(Digest([1; 32]));
        let (set, keys) = set();
        // Four signatures per validator wait at most.
        let mut agreement = Agreement::new(hub, set, 16);
        let final_at = |height: u64| Statement::Final {
            height,
            timestamp: height,
        };
        let sign = |i: usize, s: Statement| {
            SignedStatement::sign(&hub, ValidatorIndex::new(i).unwrap(), s, &keys[i])
        };
        for height in 10..14 {
            assert_eq!(agreement.add(sign(1, final_at(height))), None);
        }
        assert_eq!(agreement.take_dropped(), None);
        assert_eq!(agreement.add(sign(1, final_at(14))), None);
        let dropped = agreement.take_dropped().unwrap();
        assert_eq!(dropped.lowest, 14);
        // What it holds still agrees, and makes room once it did.
        assert_eq!(agreement.add(sign(2, final_at(10))), Some(final_at(10)));
        assert_eq!(agreement.add(sign(2, final_at(14))), None);
        assert_eq!(agreement.add(sign(1, final_at(14))), Some(final_at(14)));
        agreement.check_invariants();
    }

    #[test]
    fn statements_round_trip() {
        let hub = HubId(Digest([1; 32]));
        let (_, keys) = set();
        let key = NodeKey::from_bytes(&[9; 32]).public_key();
        let statements = [
            Statement::Final {
                height: 1,
                timestamp: 2,
            },
            Statement::Account {
                height: 3,
                key,
                account: Some((
                    AccountNumber(4),
                    AccountView {
                        key,
                        balance: 5,
                        reserve: 6,
                        sequence: 7,
                        disbursal: true,
                        closing: false,
                    },
                )),
            },
            Statement::Account {
                height: 3,
                key,
                account: None,
            },
            Statement::Hold {
                height: 8,
                number: HoldNumber(9),
                hold: Some(HoldView {
                    payer: AccountNumber(1),
                    payee: AccountNumber(2),
                    amount: 10,
                    reduction: Some((3, 4)),
                    total: 11,
                    through: 12,
                }),
            },
            Statement::Executed {
                height: 13,
                timestamp: 14,
                index: 15,
                operation: Operation::Validator(crate::operation::SignedValidatorOperation::sign(
                    &hub,
                    ValidatorIndex::new(0).unwrap(),
                    1,
                    crate::operation::ValidatorOperation::Shutdown,
                    &keys[0],
                )),
                outcome: Outcome::Applied {
                    fee: 0,
                    detail: Detail::Hold(HoldNumber(3)),
                },
            },
        ];
        let signed: Vec<SignedStatement> = statements
            .iter()
            .map(|s| {
                SignedStatement::sign(&hub, ValidatorIndex::new(2).unwrap(), s.clone(), &keys[2])
            })
            .collect();
        let bytes = SignedStatement::encode_list(&signed);
        assert_eq!(SignedStatement::decode_list(&bytes).unwrap(), signed);
    }

    // The holds of an account are listed whole, over as many pages as they
    // take, each a statement of its own.
    #[test]
    fn every_hold_is_listed_over_pages() {
        let (rules, _) = crate::rules::tests::rules(4);
        let mut ledger = Ledger::genesis(rules, 1_000).unwrap();
        let payee = AccountNumber(1);
        let count = MAX_HOLDS_LISTED + 500;
        for n in 1..=u64::try_from(count).unwrap() {
            let hold = crate::ledger::Hold {
                payer: AccountNumber(2),
                payee,
                amount: n,
                reserve: 0,
                reduction: None,
                total: 0,
                through: 0,
                claimed: BTreeMap::new(),
            };
            ledger.holds.insert(HoldNumber(n), hold);
            ledger.hold_index.insert((payee, HoldNumber(n)));
        }
        let pages = Statement::holds(&ledger, payee);
        assert_eq!(pages.len(), 2);
        let mut listed = Vec::new();
        for statement in &pages {
            let bytes = statement.to_vec();
            let mut reader = Reader::new(&bytes);
            assert_eq!(&Statement::decode(&mut reader).unwrap(), statement);
            let Statement::Holds { holds, pages, .. } = statement else {
                panic!("not holds");
            };
            assert_eq!(*pages, 2);
            listed.extend(holds.iter().map(|(number, _)| *number));
        }
        assert_eq!(listed.len(), count, "every hold");
        assert!(listed.windows(2).all(|w| w[0] < w[1]));
    }

    // What a member that can no longer read its history in order is told.
    #[test]
    fn what_a_resync_reads_round_trips() {
        let hub = HubId(Digest([1; 32]));
        let (_, keys) = set();
        let view = HoldView {
            payer: AccountNumber(1),
            payee: AccountNumber(2),
            amount: 10,
            reduction: None,
            total: 11,
            through: 12,
        };
        let signed: Vec<SignedStatement> = [
            Statement::Holds {
                height: 8,
                timestamp: 9,
                account: AccountNumber(2),
                page: 1,
                pages: 2,
                holds: alloc::vec![(HoldNumber(9), view), (HoldNumber(10), view)],
            },
            Statement::Trimmed {
                height: 8,
                below: 4,
            },
        ]
        .into_iter()
        .map(|s| SignedStatement::sign(&hub, ValidatorIndex::new(1).unwrap(), s, &keys[1]))
        .collect();
        let bytes = SignedStatement::encode_list(&signed);
        assert_eq!(SignedStatement::decode_list(&bytes).unwrap(), signed);
    }

    #[test]
    fn requests_and_outcomes_round_trip() {
        let key = NodeKey::from_bytes(&[9; 32]).public_key();
        for request in [
            Request::Account(key),
            Request::Hold(HoldNumber(3)),
            Request::Holds(AccountNumber(5)),
            Request::Since {
                account: AccountNumber(2),
                from: 9,
            },
        ] {
            assert_eq!(Request::decode_exact(&request.to_vec()).unwrap(), request);
        }
        use crate::ledger::{Failure, Refusal};
        let failures = [
            Failure::Funds,
            Failure::Missing,
            Failure::NotYours,
            Failure::Full,
            Failure::TooSmall,
            Failure::Claim,
            Failure::NoDisbursal,
            Failure::Busy,
            Failure::ShuttingDown,
            Failure::Exists,
            Failure::Stale,
            Failure::Destination,
        ];
        let refusals = [
            Refusal::Unknown,
            Refusal::Signature,
            Refusal::Sequence,
            Refusal::Fee,
            Refusal::NotAllowed,
        ];
        let outcomes = failures
            .into_iter()
            .map(|reason| Outcome::Failed { fee: 3, reason })
            .chain(refusals.into_iter().map(Outcome::Refused))
            .chain([
                Outcome::Applied {
                    fee: 1,
                    detail: Detail::None,
                },
                Outcome::Applied {
                    fee: 1,
                    detail: Detail::Account(AccountNumber(7)),
                },
                Outcome::Applied {
                    fee: 1,
                    detail: Detail::Paid(9),
                },
                Outcome::Applied {
                    fee: 1,
                    detail: Detail::Withdrawal(4),
                },
            ]);
        for outcome in outcomes {
            let bytes = outcome.to_vec();
            let mut reader = Reader::new(&bytes);
            assert_eq!(Outcome::decode(&mut reader).unwrap(), outcome);
            reader.finish().unwrap();
        }
    }
}
