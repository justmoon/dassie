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
    /// `timestamp`, with `outcome`.
    Executed {
        /// The block's height.
        height: u64,
        /// Its timestamp.
        timestamp: Millis,
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
    /// Hold `number` after the block at `height`, if it exists.
    Hold {
        /// The block's height.
        height: u64,
        /// The hold asked about.
        number: HoldNumber,
        /// The hold, if it exists.
        hold: Option<HoldView>,
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

    /// The height it is as of.
    #[must_use]
    pub const fn height(&self) -> u64 {
        match self {
            Self::Executed { height, .. }
            | Self::Final { height, .. }
            | Self::Account { height, .. }
            | Self::Hold { height, .. } => *height,
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

/// Gathers signed statements until f + 1 validators agree on one. Bounded:
/// it keeps at most `capacity` distinct statements, dropping the lowest
/// heights first, which are the stalest.
#[derive(Clone, Debug)]
pub struct Agreement {
    hub: HubId,
    set: ValidatorSet,
    capacity: usize,
    pending: BTreeMap<(u64, Digest), (Statement, Signers)>,
}

impl Agreement {
    /// A collector for statements of hub `hub` by `set`, keeping at most
    /// `capacity` undecided ones.
    #[must_use]
    pub fn new(hub: HubId, set: ValidatorSet, capacity: usize) -> Self {
        Self {
            hub,
            set,
            capacity: capacity.max(1),
            pending: BTreeMap::new(),
        }
    }

    /// Takes in a signed statement. Returns the statement once f + 1
    /// validators signed it, exactly once; a statement that does not verify
    /// is dropped.
    pub fn add(&mut self, signed: SignedStatement) -> Option<Statement> {
        if !signed.verifies(&self.hub, &self.set) {
            return None;
        }
        let key = (
            signed.statement.height(),
            signed.statement.digest(&self.hub),
        );
        let needed = self.set.one_honest();
        let entry = self
            .pending
            .entry(key)
            .or_insert_with(|| (signed.statement, Signers::NONE));
        let before = entry.1.count();
        entry.1 = entry.1.with(signed.validator);
        let reached = before < needed && entry.1.count() >= needed;
        let agreed = reached.then(|| entry.0.clone());
        while self.pending.len() > self.capacity {
            self.pending.pop_first();
        }
        agreed
    }

    /// Forgets statements below `height`.
    pub fn forget_below(&mut self, height: u64) {
        self.pending = self.pending.split_off(&(height, Digest::default()));
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
                } => 8 + operation.encoded_len() + outcome.encoded_len(),
                Self::Final { .. } => 8,
                Self::Account { account, .. } => {
                    32 + 1 + account.map_or(0, |(_, view)| 4 + view.encoded_len())
                }
                Self::Hold { hold, .. } => 8 + 1 + hold.map_or(0, |view| view.encoded_len()),
            }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Executed {
                height,
                timestamp,
                operation,
                outcome,
            } => {
                writer.write_u8(0);
                writer.write_u64(*height);
                writer.write_u64(*timestamp);
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
        }
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

    #[test]
    fn requests_and_outcomes_round_trip() {
        let key = NodeKey::from_bytes(&[9; 32]).public_key();
        for request in [
            Request::Account(key),
            Request::Hold(HoldNumber(3)),
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
