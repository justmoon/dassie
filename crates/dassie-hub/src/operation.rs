//! The operations a hub's log orders
//! (spec/designs/settlement-hubs.md#3-the-ledger): those a member signs with
//! its account key, and those a validator signs with its consensus key.
//!
//! A member's signature covers the hub ID, its account number, the
//! account's next sequence and the most fee it pays, so it cannot be
//! replayed in another hub or twice in one: account numbers are never
//! reused and sequences only grow. A validator's operations carry a
//! sequence of their own that only grows.

use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey, Signature};
use dassie_oer::{Encode, Reader, Writer};

use crate::codec::{
    DIGEST_LEN, HubDecodeError, KEY_LEN, SIGNATURE_LEN, read_digest, read_key, read_signature,
    write_digest, write_key, write_signature,
};
use crate::digest::{Digest, Domain};
use crate::promise::{Claim, Fulfilled, HoldNumber, Signed};
use crate::rules::{Amount, HubId, LEDGER_ACCOUNT_LEN, Rules};
use crate::validators::{MAX_VALIDATORS, ValidatorIndex};

/// An account's number, assigned in order from 1 and never reused. A
/// deposit names it as its destination tag, so it fits in 32 bits; tag 0
/// opens a new account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountNumber(pub u32);

/// The most commitments one `claim` may carry, whatever the rules say: a
/// bound for decoding before the rules are known.
pub const MAX_COMMITMENTS: usize = 1024;

/// Where funds go on the underlying ledger: an account and, if it needs
/// one, a destination tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Destination {
    /// The account ID.
    pub account: [u8; LEDGER_ACCOUNT_LEN],
    /// The destination tag.
    pub tag: Option<u32>,
}

/// Where `close` sends what an account holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseTo {
    /// Another account in the hub.
    Account(AccountNumber),
    /// An account on the underlying ledger, as `withdraw` pays.
    Ledger(Destination),
}

/// What a member asks of the hub.
// Operations live in a block's list once; boxing would cost an allocation
// per decode for nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MemberOperation {
    /// Moves funds to another account.
    Transfer {
        /// The payee.
        to: AccountNumber,
        /// How much.
        amount: Amount,
    },
    /// Creates the payee's account with `key` and pays it at least the
    /// hub's minimum for a new account (spec/designs/settlement-hubs.md#5-joining).
    Pay {
        /// The new account's key.
        key: PublicKey,
        /// How much, reserve included.
        amount: Amount,
    },
    /// Sets where the account's funds go at shutdown.
    SetDisbursal(Destination),
    /// Sets funds aside for a payee, locking a hold reserve.
    HoldOpen {
        /// The payee.
        payee: AccountNumber,
        /// How much.
        amount: Amount,
    },
    /// Adds to a hold.
    HoldFund {
        /// The hold.
        hold: HoldNumber,
        /// How much.
        amount: Amount,
    },
    /// Takes `amount` back from a hold, once the hold's notice passed.
    HoldReduce {
        /// The hold.
        hold: HoldNumber,
        /// How much.
        amount: Amount,
    },
    /// The payee gives up a hold toward it, which goes back to its payer.
    HoldRelease {
        /// The hold.
        hold: HoldNumber,
    },
    /// Takes from a hold with the payer's latest claim.
    Redeem(Signed<Claim>),
    /// Takes from a hold with fulfilled commitments.
    Claim {
        /// The hold.
        hold: HoldNumber,
        /// The commitments and their fulfillments.
        fulfilled: Vec<Fulfilled>,
    },
    /// Pays funds out on the underlying ledger.
    Withdraw {
        /// How much arrives.
        amount: Amount,
        /// Where.
        to: Destination,
    },
    /// Pays out everything and ends the account.
    Close(CloseTo),
}

/// A member's operation with its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedMemberOperation {
    /// The account that signs.
    pub account: AccountNumber,
    /// The account's next sequence.
    pub sequence: u64,
    /// The most fee the member pays.
    pub max_fee: Amount,
    /// What it asks.
    pub operation: MemberOperation,
    /// The account key's signature over [`SignedMemberOperation::digest`].
    pub signature: Signature,
}

/// What a validator saw on the underlying ledger, or asks of the hub.
// As a member's: operations live in a block's list once, and new rules,
// with a token hub's asset in them, are rare.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidatorOperation {
    /// A payment to the escrow in a validated ledger.
    AttestDeposit(Deposit),
    /// The outcome of a withdrawal the escrow sent.
    AttestWithdrawal {
        /// The escrow sequence the hub gave it.
        escrow_sequence: u32,
        /// Whether it delivered (`tesSUCCESS`) or failed (`tec`).
        delivered: bool,
    },
    /// The escrow is set up as the rules say, and this is its next
    /// sequence on the underlying ledger.
    AttestEscrow {
        /// The escrow's next sequence.
        sequence: u32,
    },
    /// A reset of the test network the validator saw.
    AttestReset {
        /// The first ledger of the new chain the validator saw. It informs:
        /// votes count validators, since honest ones see the chain at
        /// different ledgers.
        ledger: u32,
    },
    /// Changes the validators or the terms, after the rules' notice.
    Reconfigure(Rules),
    /// The validator's own endpoint, at once.
    SetEndpoint(alloc::string::String),
    /// A vote to end the hub.
    Shutdown,
    /// The old validators' change of the escrow's signer list, at the
    /// sequence the hub kept for it, applied on the underlying ledger.
    AttestSignerList {
        /// Its escrow sequence.
        sequence: u32,
    },
}

/// A payment to the escrow, as each validator attests it
/// (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deposit {
    /// The transaction's hash.
    pub transaction: Digest,
    /// The validated ledger it is in.
    pub ledger: u32,
    /// Its `delivered_amount`, never its `Amount`.
    pub amount: Amount,
    /// Who sent it, for paying it back.
    pub sender: [u8; LEDGER_ACCOUNT_LEN],
    /// Its destination tag: an account number, or 0 for a new account.
    pub tag: u32,
    /// For tag 0, the new account's key from the memo.
    pub key: Option<PublicKey>,
}

/// A validator's operation with its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedValidatorOperation {
    /// The validator.
    pub validator: ValidatorIndex,
    /// Grows with each of its operations.
    pub sequence: u64,
    /// What it reports or asks.
    pub operation: ValidatorOperation,
    /// Its consensus key's signature over the digest.
    pub signature: Signature,
}

/// An operation in a block.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    /// A member's.
    Member(SignedMemberOperation),
    /// A validator's.
    Validator(SignedValidatorOperation),
}

impl MemberOperation {
    /// Whether it has a deadline, and so a place kept for it in every
    /// block (spec/designs/settlement-hubs.md#7-fees).
    #[must_use]
    pub const fn has_deadline(&self) -> bool {
        matches!(self, Self::Redeem(_) | Self::Claim { .. })
    }
}

impl Operation {
    /// Whether it goes first in a block.
    #[must_use]
    pub const fn has_deadline(&self) -> bool {
        match self {
            Self::Member(signed) => signed.operation.has_deadline(),
            Self::Validator(_) => false,
        }
    }

    /// The operation's hash, which its receipt names.
    #[must_use]
    pub fn hash(&self) -> Digest {
        Digest::of(Domain::OperationId, &[&self.to_vec()])
    }
}

impl SignedMemberOperation {
    /// The digest the account key signs.
    #[must_use]
    pub fn digest(&self, hub: &HubId) -> Digest {
        let mut body = alloc::vec![0; self.operation.encoded_len()];
        self.operation.encode(&mut body);
        Digest::of(
            Domain::Operation,
            &[
                hub.0.as_bytes(),
                b"member",
                &self.account.0.to_be_bytes(),
                &self.sequence.to_be_bytes(),
                &self.max_fee.to_be_bytes(),
                &body,
            ],
        )
    }

    /// Signs `operation` with the account key.
    #[must_use]
    pub fn sign(
        hub: &HubId,
        account: AccountNumber,
        sequence: u64,
        max_fee: Amount,
        operation: MemberOperation,
        key: &NodeKey,
    ) -> Self {
        let mut signed = Self {
            account,
            sequence,
            max_fee,
            operation,
            signature: Signature::from_bytes([0; 64]),
        };
        signed.signature = key.sign(signed.digest(hub).as_bytes());
        signed
    }

    /// Whether `key` signed it.
    #[must_use]
    pub fn verifies(&self, hub: &HubId, key: &PublicKey) -> bool {
        key.verify(self.digest(hub).as_bytes(), &self.signature)
            .is_ok()
    }
}

impl SignedValidatorOperation {
    /// The digest the consensus key signs.
    #[must_use]
    pub fn digest(&self, hub: &HubId) -> Digest {
        let mut body = alloc::vec![0; self.operation.encoded_len()];
        self.operation.encode(&mut body);
        let index = u8::try_from(self.validator.get()).unwrap_or(u8::MAX);
        Digest::of(
            Domain::Operation,
            &[
                hub.0.as_bytes(),
                b"validator",
                &[index],
                &self.sequence.to_be_bytes(),
                &body,
            ],
        )
    }

    /// Signs `operation` with validator `validator`'s consensus key.
    #[must_use]
    pub fn sign(
        hub: &HubId,
        validator: ValidatorIndex,
        sequence: u64,
        operation: ValidatorOperation,
        key: &NodeKey,
    ) -> Self {
        let mut signed = Self {
            validator,
            sequence,
            operation,
            signature: Signature::from_bytes([0; 64]),
        };
        signed.signature = key.sign(signed.digest(hub).as_bytes());
        signed
    }

    /// Whether `key` signed it.
    #[must_use]
    pub fn verifies(&self, hub: &HubId, key: &PublicKey) -> bool {
        key.verify(self.digest(hub).as_bytes(), &self.signature)
            .is_ok()
    }
}

// Encoding. Each CHOICE is a one-byte tag and its alternative; options are
// a presence byte and the value.

const DESTINATION_LEN: usize = LEDGER_ACCOUNT_LEN + 1 + 4;

fn write_destination(writer: &mut Writer<'_>, destination: &Destination) {
    writer.write_bytes(&destination.account);
    writer.write_bool(destination.tag.is_some());
    writer.write_u32(destination.tag.unwrap_or(0));
}

fn read_destination(reader: &mut Reader<'_>) -> Result<Destination, HubDecodeError> {
    let account = *reader.read_array::<LEDGER_ACCOUNT_LEN>()?;
    let has_tag = reader.read_bool()?;
    let tag = reader.read_u32()?;
    if !has_tag && tag != 0 {
        return Err(HubDecodeError::Invalid("tag without presence"));
    }
    Ok(Destination {
        account,
        tag: has_tag.then_some(tag),
    })
}

impl Encode for MemberOperation {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::Transfer { .. } | Self::HoldOpen { .. } => 4 + 8,
            Self::Pay { .. } => KEY_LEN + 8,
            Self::SetDisbursal(_) => DESTINATION_LEN,
            Self::HoldFund { .. } | Self::HoldReduce { .. } => 8 + 8,
            Self::HoldRelease { .. } => 8,
            Self::Redeem(claim) => claim.encoded_len(),
            Self::Claim { fulfilled, .. } => {
                8 + dassie_oer::size::quantity(fulfilled.len())
                    + fulfilled.iter().map(Encode::encoded_len).sum::<usize>()
            }
            Self::Withdraw { .. } => 8 + DESTINATION_LEN,
            Self::Close(CloseTo::Account(_)) => 1 + 4,
            Self::Close(CloseTo::Ledger(_)) => 1 + DESTINATION_LEN,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Transfer { to, amount } => {
                writer.write_u8(0);
                writer.write_u32(to.0);
                writer.write_u64(*amount);
            }
            Self::Pay { key, amount } => {
                writer.write_u8(1);
                write_key(writer, key);
                writer.write_u64(*amount);
            }
            Self::SetDisbursal(destination) => {
                writer.write_u8(2);
                write_destination(writer, destination);
            }
            Self::HoldOpen { payee, amount } => {
                writer.write_u8(3);
                writer.write_u32(payee.0);
                writer.write_u64(*amount);
            }
            Self::HoldFund { hold, amount } | Self::HoldReduce { hold, amount } => {
                writer.write_u8(if matches!(self, Self::HoldFund { .. }) {
                    4
                } else {
                    5
                });
                writer.write_u64(hold.0);
                writer.write_u64(*amount);
            }
            _ => self.encode_rest(writer),
        }
    }
}

impl MemberOperation {
    fn encode_rest(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Redeem(claim) => {
                writer.write_u8(6);
                claim.encode_to(writer);
            }
            Self::Claim { hold, fulfilled } => {
                writer.write_u8(7);
                writer.write_u64(hold.0);
                writer.write_quantity(fulfilled.len());
                for item in fulfilled {
                    item.encode_to(writer);
                }
            }
            Self::Withdraw { amount, to } => {
                writer.write_u8(8);
                writer.write_u64(*amount);
                write_destination(writer, to);
            }
            Self::HoldRelease { hold } => {
                writer.write_u8(10);
                writer.write_u64(hold.0);
            }
            Self::Close(to) => {
                writer.write_u8(9);
                match to {
                    CloseTo::Account(account) => {
                        writer.write_u8(0);
                        writer.write_u32(account.0);
                    }
                    CloseTo::Ledger(destination) => {
                        writer.write_u8(1);
                        write_destination(writer, destination);
                    }
                }
            }
            _ => {}
        }
    }

    /// Decodes a member's operation.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let tag = reader.read_u8()?;
        Ok(match tag {
            0 => Self::Transfer {
                to: AccountNumber(reader.read_u32()?),
                amount: reader.read_u64()?,
            },
            1 => Self::Pay {
                key: read_key(reader)?,
                amount: reader.read_u64()?,
            },
            2 => Self::SetDisbursal(read_destination(reader)?),
            3 => Self::HoldOpen {
                payee: AccountNumber(reader.read_u32()?),
                amount: reader.read_u64()?,
            },
            4 => Self::HoldFund {
                hold: HoldNumber(reader.read_u64()?),
                amount: reader.read_u64()?,
            },
            5 => Self::HoldReduce {
                hold: HoldNumber(reader.read_u64()?),
                amount: reader.read_u64()?,
            },
            _ => Self::decode_rest(tag, reader)?,
        })
    }

    fn decode_rest(tag: u8, reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(match tag {
            6 => Self::Redeem(Signed::<Claim>::decode(reader)?),
            7 => {
                let hold = HoldNumber(reader.read_u64()?);
                let count = reader.read_quantity(MAX_COMMITMENTS)?;
                let mut fulfilled = Vec::with_capacity(count);
                for _ in 0..count {
                    fulfilled.push(Fulfilled::decode(reader)?);
                }
                Self::Claim { hold, fulfilled }
            }
            8 => Self::Withdraw {
                amount: reader.read_u64()?,
                to: read_destination(reader)?,
            },
            10 => Self::HoldRelease {
                hold: HoldNumber(reader.read_u64()?),
            },
            9 => Self::Close(match reader.read_u8()? {
                0 => CloseTo::Account(AccountNumber(reader.read_u32()?)),
                1 => CloseTo::Ledger(read_destination(reader)?),
                other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
            }),
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        })
    }
}

const DEPOSIT_LEN: usize = DIGEST_LEN + 4 + 8 + LEDGER_ACCOUNT_LEN + 4 + 1 + KEY_LEN;

impl Encode for Deposit {
    fn encoded_len(&self) -> usize {
        DEPOSIT_LEN
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        write_digest(writer, &self.transaction);
        writer.write_u32(self.ledger);
        writer.write_u64(self.amount);
        writer.write_bytes(&self.sender);
        writer.write_u32(self.tag);
        writer.write_bool(self.key.is_some());
        writer.write_bytes(self.key.as_ref().map_or(&[0; 32], |key| key.as_bytes()));
    }
}

impl Deposit {
    /// Decodes a deposit. A key that is absent must be all zeros. A
    /// validator attests a memo that holds no valid key as no key, so the
    /// hub pays the deposit back.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        let transaction = read_digest(reader)?;
        let ledger = reader.read_u32()?;
        let amount = reader.read_u64()?;
        let sender = *reader.read_array::<LEDGER_ACCOUNT_LEN>()?;
        let tag = reader.read_u32()?;
        let key = if reader.read_bool()? {
            Some(read_key(reader)?)
        } else {
            if reader.read_array::<32>()? != &[0; 32] {
                return Err(HubDecodeError::Invalid("key without presence"));
            }
            None
        };
        Ok(Self {
            transaction,
            ledger,
            amount,
            sender,
            tag,
            key,
        })
    }
}

impl Encode for ValidatorOperation {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::AttestDeposit(deposit) => deposit.encoded_len(),
            Self::AttestWithdrawal { .. } => 4 + 1,
            Self::AttestEscrow { .. }
            | Self::AttestReset { .. }
            | Self::AttestSignerList { .. } => 4,
            Self::Reconfigure(rules) => rules.encoded_len(),
            Self::SetEndpoint(endpoint) => dassie_oer::size::prefixed(endpoint.len()),
            Self::Shutdown => 0,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::AttestDeposit(deposit) => {
                writer.write_u8(0);
                deposit.encode_to(writer);
            }
            Self::AttestWithdrawal {
                escrow_sequence,
                delivered,
            } => {
                writer.write_u8(1);
                writer.write_u32(*escrow_sequence);
                writer.write_bool(*delivered);
            }
            Self::AttestReset { ledger } => {
                writer.write_u8(2);
                writer.write_u32(*ledger);
            }
            Self::Reconfigure(rules) => {
                writer.write_u8(3);
                rules.encode_to(writer);
            }
            Self::SetEndpoint(endpoint) => {
                writer.write_u8(4);
                writer.write_octets(endpoint.as_bytes(), dassie_oer::Size::range(1, 255));
            }
            Self::Shutdown => writer.write_u8(5),
            Self::AttestEscrow { sequence } => {
                writer.write_u8(6);
                writer.write_u32(*sequence);
            }
            Self::AttestSignerList { sequence } => {
                writer.write_u8(7);
                writer.write_u32(*sequence);
            }
        }
    }
}

impl ValidatorOperation {
    /// Decodes a validator's operation.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(match reader.read_u8()? {
            0 => Self::AttestDeposit(Deposit::decode(reader)?),
            1 => Self::AttestWithdrawal {
                escrow_sequence: reader.read_u32()?,
                delivered: reader.read_bool()?,
            },
            2 => Self::AttestReset {
                ledger: reader.read_u32()?,
            },
            3 => Self::Reconfigure(Rules::decode(reader)?),
            4 => {
                let bytes = reader.read_octets(dassie_oer::Size::range(1, 255))?;
                let endpoint = core::str::from_utf8(bytes)
                    .ok()
                    .filter(|e| crate::rules::Acceptance::valid_endpoint(e))
                    .ok_or(HubDecodeError::Invalid("endpoint"))?;
                Self::SetEndpoint(endpoint.into())
            }
            5 => Self::Shutdown,
            6 => Self::AttestEscrow {
                sequence: reader.read_u32()?,
            },
            7 => Self::AttestSignerList {
                sequence: reader.read_u32()?,
            },
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        })
    }
}

impl Encode for Operation {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::Member(signed) => 4 + 8 + 8 + signed.operation.encoded_len() + SIGNATURE_LEN,
            Self::Validator(signed) => 1 + 8 + signed.operation.encoded_len() + SIGNATURE_LEN,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Member(signed) => {
                writer.write_u8(0);
                writer.write_u32(signed.account.0);
                writer.write_u64(signed.sequence);
                writer.write_u64(signed.max_fee);
                signed.operation.encode_to(writer);
                write_signature(writer, &signed.signature);
            }
            Self::Validator(signed) => {
                writer.write_u8(1);
                writer.write_u8(u8::try_from(signed.validator.get()).unwrap_or(u8::MAX));
                writer.write_u64(signed.sequence);
                signed.operation.encode_to(writer);
                write_signature(writer, &signed.signature);
            }
        }
    }
}

impl Operation {
    /// Decodes an operation; its signature is checked when it is applied.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, HubDecodeError> {
        Ok(match reader.read_u8()? {
            0 => Self::Member(SignedMemberOperation {
                account: AccountNumber(reader.read_u32()?),
                sequence: reader.read_u64()?,
                max_fee: reader.read_u64()?,
                operation: MemberOperation::decode(reader)?,
                signature: read_signature(reader)?,
            }),
            1 => {
                let index = usize::from(reader.read_u8()?);
                let validator = ValidatorIndex::new(index)
                    .filter(|_| index < MAX_VALIDATORS)
                    .ok_or(HubDecodeError::Invalid("validator index"))?;
                Self::Validator(SignedValidatorOperation {
                    validator,
                    sequence: reader.read_u64()?,
                    operation: ValidatorOperation::decode(reader)?,
                    signature: read_signature(reader)?,
                })
            }
            other => return Err(HubDecodeError::UnknownChoice(u64::from(other))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promise::Commitment;
    use proptest::prelude::*;

    fn round_trip(operation: &Operation) {
        let bytes = operation.to_vec();
        let mut reader = Reader::new(&bytes);
        let decoded = Operation::decode(&mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(&decoded, operation);
    }

    fn member(operation: MemberOperation) -> Operation {
        let key = NodeKey::from_bytes(&[1; 32]);
        let hub = HubId(Digest([2; 32]));
        Operation::Member(SignedMemberOperation::sign(
            &hub,
            AccountNumber(7),
            3,
            10,
            operation,
            &key,
        ))
    }

    /// The operations that carry a payer's promises.
    fn promise_operations() -> [MemberOperation; 2] {
        let key = NodeKey::from_bytes(&[4; 32]);
        let hub = HubId(Digest([2; 32]));
        let commitment = Signed::sign(
            Commitment {
                hold: HoldNumber(1),
                sequence: 2,
                amount: 3,
                condition: [4; 32],
                expiry: 5,
            },
            &hub,
            &key,
        );
        let claim = Signed::sign(
            Claim {
                hold: HoldNumber(1),
                total: 9,
                through: 2,
            },
            &hub,
            &key,
        );
        [
            MemberOperation::Redeem(claim),
            MemberOperation::Claim {
                hold: HoldNumber(1),
                fulfilled: alloc::vec![Fulfilled {
                    commitment,
                    fulfillment: [6; 32]
                }],
            },
        ]
    }

    #[test]
    fn every_member_operation_round_trips() {
        let key = NodeKey::from_bytes(&[4; 32]);
        let to = Destination {
            account: [5; 20],
            tag: Some(9),
        };
        for operation in [
            MemberOperation::Transfer {
                to: AccountNumber(2),
                amount: 5,
            },
            MemberOperation::Pay {
                key: key.public_key(),
                amount: 5,
            },
            MemberOperation::SetDisbursal(Destination {
                account: [1; 20],
                tag: None,
            }),
            MemberOperation::HoldOpen {
                payee: AccountNumber(3),
                amount: 6,
            },
            MemberOperation::HoldFund {
                hold: HoldNumber(4),
                amount: 7,
            },
            MemberOperation::HoldReduce {
                hold: HoldNumber(4),
                amount: 8,
            },
            MemberOperation::HoldRelease {
                hold: HoldNumber(4),
            },
            MemberOperation::Withdraw {
                amount: 1_000_000,
                to,
            },
            MemberOperation::Close(CloseTo::Account(AccountNumber(8))),
            MemberOperation::Close(CloseTo::Ledger(to)),
        ]
        .into_iter()
        .chain(promise_operations())
        {
            round_trip(&member(operation));
        }
    }

    #[test]
    fn every_validator_operation_round_trips() {
        let key = NodeKey::from_bytes(&[1; 32]);
        let hub = HubId(Digest([2; 32]));
        let (rules, _) = crate::rules::tests::rules(4);
        for operation in [
            ValidatorOperation::AttestDeposit(Deposit {
                transaction: Digest([3; 32]),
                ledger: 10,
                amount: 20,
                sender: [4; 20],
                tag: 0,
                key: Some(key.public_key()),
            }),
            ValidatorOperation::AttestWithdrawal {
                escrow_sequence: 5,
                delivered: true,
            },
            ValidatorOperation::AttestReset { ledger: 1 },
            ValidatorOperation::Reconfigure(rules),
            ValidatorOperation::SetEndpoint("v1.example:443".into()),
            ValidatorOperation::Shutdown,
            ValidatorOperation::AttestEscrow { sequence: 3 },
            ValidatorOperation::AttestSignerList { sequence: 4 },
        ] {
            let signed = SignedValidatorOperation::sign(
                &hub,
                ValidatorIndex::new(2).unwrap(),
                9,
                operation,
                &key,
            );
            assert!(signed.verifies(&hub, &key.public_key()));
            round_trip(&Operation::Validator(signed));
        }
    }

    #[test]
    fn signatures_cover_the_sequence() {
        let key = NodeKey::from_bytes(&[1; 32]);
        let hub = HubId(Digest([2; 32]));
        let mut signed = SignedMemberOperation::sign(
            &hub,
            AccountNumber(1),
            1,
            10,
            MemberOperation::Transfer {
                to: AccountNumber(2),
                amount: 1,
            },
            &key,
        );
        assert!(signed.verifies(&hub, &key.public_key()));
        signed.sequence = 2;
        assert!(!signed.verifies(&hub, &key.public_key()));
    }

    proptest! {
        #[test]
        fn decoding_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let mut reader = Reader::new(&bytes);
            let _ = Operation::decode(&mut reader);
        }
    }
}
