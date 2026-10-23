//! What became of an operation in a block, as its receipt reports it.

use dassie_oer::{Encode, Writer};

use crate::operation::AccountNumber;
use crate::promise::HoldNumber;
use crate::rules::Amount;

/// The outcome of one operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It took effect, and paid `fee`.
    Applied {
        /// The fee it paid.
        fee: Amount,
        /// What it made or moved, where that is not in the operation.
        detail: Detail,
    },
    /// It was valid but could not take effect. It paid its fee and used
    /// its sequence, as a `tec` result does on the XRP Ledger.
    Failed {
        /// The fee it paid.
        fee: Amount,
        /// Why.
        reason: Failure,
    },
    /// It was not the signer's to make, or could not pay its fee. It paid
    /// nothing and used no sequence.
    Refused(Refusal),
}

/// What an applied operation made or moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Nothing beyond what it said.
    None,
    /// It opened this account.
    Account(AccountNumber),
    /// It opened this hold.
    Hold(HoldNumber),
    /// It paid the payee this much from a hold.
    Paid(Amount),
    /// It gave its withdrawal this escrow sequence.
    Withdrawal(u32),
}

/// Why a valid operation could not take effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The balance does not cover it.
    Funds,
    /// The account, hold or payee does not exist, or is closing.
    Missing,
    /// It is not the signer's account or hold to act on.
    NotYours,
    /// A table is full.
    Full,
    /// The amount is below a minimum of the rules.
    TooSmall,
    /// A claim's total or sequence went backwards, or a commitment is
    /// claimed already, expired, unfulfilled or not the payer's.
    Claim,
    /// A new account must set its disbursal address first.
    NoDisbursal,
    /// The account still has holds or withdrawals.
    Busy,
    /// The hub is shutting down, and takes only redeems and claims.
    ShuttingDown,
    /// The key already has an account.
    Exists,
    /// A deposit in a ledger the hub no longer remembers.
    Stale,
}

/// Why an operation was refused without effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No such account or validator.
    Unknown,
    /// The signature does not verify.
    Signature,
    /// The sequence is not the account's next, or not above the
    /// validator's last.
    Sequence,
    /// The fee exceeds the most the signer pays, or its balance.
    Fee,
    /// The operation is a validator's to make on another network, or a
    /// vote the validator already cast.
    NotAllowed,
}

impl Encode for Outcome {
    fn encoded_len(&self) -> usize {
        1 + match self {
            Self::Applied { .. } => 8 + 1 + 8,
            Self::Failed { .. } => 8 + 1,
            Self::Refused(_) => 1,
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Applied { fee, detail } => {
                writer.write_u8(0);
                writer.write_u64(*fee);
                let (kind, value) = match detail {
                    Detail::None => (0, 0),
                    Detail::Account(account) => (1, u64::from(account.0)),
                    Detail::Hold(hold) => (2, hold.0),
                    Detail::Paid(amount) => (3, *amount),
                    Detail::Withdrawal(sequence) => (4, u64::from(*sequence)),
                };
                writer.write_u8(kind);
                writer.write_u64(value);
            }
            Self::Failed { fee, reason } => {
                writer.write_u8(1);
                writer.write_u64(*fee);
                writer.write_u8(*reason as u8);
            }
            Self::Refused(refusal) => {
                writer.write_u8(2);
                writer.write_u8(*refusal as u8);
            }
        }
    }
}

impl Outcome {
    /// Whether it used the signer's sequence.
    #[must_use]
    pub const fn used_sequence(&self) -> bool {
        !matches!(self, Self::Refused(_))
    }
}
