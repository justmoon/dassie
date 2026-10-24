//! What the hub's ledger keeps (spec/designs/settlement-hubs.md#3-the-ledger).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::PublicKey;
use dassie_oer::{Encode, Writer};

use crate::codec::{KEY_LEN, write_key};
use crate::operation::{AccountNumber, Deposit, Destination};
use crate::promise::HoldNumber;
use crate::rules::{Amount, Millis, Rules};
use crate::validators::Signers;

/// A member's or a validator's account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// The key that signs its operations.
    pub key: PublicKey,
    /// What it can spend.
    pub balance: Amount,
    /// The account reserve it locks; never spendable.
    pub reserve: Amount,
    /// The sequence its next operation must carry.
    pub sequence: u64,
    /// Where its funds go at shutdown.
    pub disbursal: Option<Destination>,
    /// Whether `close` paid it out on the underlying ledger and the payout
    /// has not settled yet.
    pub closing: bool,
    /// Holds it pays into or takes from.
    pub holds: u16,
    /// Of those, the holds it pays into: what the rules' limit per account
    /// counts. Holds toward it count apart, since anyone can open one, and
    /// it can release them (spec/designs/settlement-hubs.md#3-the-ledger).
    pub paying: u16,
    /// Its withdrawals not yet settled.
    pub withdrawals: u16,
    /// Whether it is a validator's: those lock no reserve and need no
    /// disbursal address before they act.
    pub validator: bool,
    /// Whether it was opened as a member's, and so takes a place under the
    /// rules' bound on accounts. One opened for a validator never does,
    /// even once that validator leaves, so a change of validators never
    /// finds the table full or overfills it.
    pub member_slot: bool,
    /// Everything deposits credited it, opening it included: what its
    /// member books as its owner's deposits, since validators'
    /// attestations never reach it as an operation
    /// (spec/areas/hubs.md#the-members-side). Saturating.
    pub deposited: Amount,
    /// Everything its withdrawals and its payout took from it, their ledger
    /// fees included: what its member books as its owner's withdrawals.
    /// Saturating.
    pub withdrawn: Amount,
    /// What withdrawals the escrow could not pay brought back: booked as
    /// the owner's deposits too, and counted apart, so that a deposit on
    /// its way is told only by `deposited`. Saturating.
    pub returned: Amount,
}

/// A reduction of a hold the payer asked for, and when it takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reduction {
    /// How much it takes back.
    pub amount: Amount,
    /// The first block timestamp at which it applies.
    pub at: Millis,
}

/// Funds a payer set aside for one payee
/// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    /// Who set the funds aside.
    pub payer: AccountNumber,
    /// Who may take from them.
    pub payee: AccountNumber,
    /// What is left in it.
    pub amount: Amount,
    /// The hold reserve its payer locks.
    pub reserve: Amount,
    /// A reduction waiting for its notice to pass.
    pub reduction: Option<Reduction>,
    /// T of the last redeemed claim.
    pub total: Amount,
    /// W of the last redeemed claim.
    pub through: u64,
    /// Sequences above `through` taken with `claim`, and what each paid.
    pub claimed: BTreeMap<u64, Amount>,
}

/// A payment out of the escrow the hub decided on and the underlying
/// ledger has not settled yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Withdrawal {
    /// The account it comes from; none for a deposit paid back.
    pub account: Option<AccountNumber>,
    /// What arrives.
    pub amount: Amount,
    /// The underlying ledger's fee, which the escrow pays on top.
    pub ledger_fee: Amount,
    /// Where it goes.
    pub to: Destination,
    /// Whether it pays out a closing account.
    pub closes: bool,
    /// Validators that saw it deliver.
    pub delivered: Signers,
    /// Validators that saw it fail.
    pub failed: Signers,
}

/// A deposit fewer than a quorum have attested, by what each attested.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingDeposit {
    /// Each distinct attestation and who made it. Faulty validators may
    /// attest something else than honest ones; only one reaches a quorum.
    pub attestations: Vec<(Deposit, Signers)>,
}

/// What the hub knows of its escrow on the underlying ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Escrow {
    /// Every drop the hub accounts for: credited deposits less what the
    /// escrow paid out, fees included.
    pub balance: Amount,
    /// The escrow's next sequence on the underlying ledger.
    pub next_sequence: u32,
    /// Whether the validators checked the escrow, so deposits may come.
    pub checked: bool,
    /// The lowest ledger whose deposits the hub still remembers: a deposit
    /// below it that is not pending already is not taken.
    pub deposit_floor: u32,
    /// The escrow sequence kept for the old validators' change of the
    /// escrow's signer list, once a new set took over
    /// (spec/designs/settlement-hubs.md#8-changing-the-validators).
    pub signer_list: Option<u32>,
}

/// Where the hub is in its life (spec/designs/settlement-hubs.md#9-shutdown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Taking every operation.
    Running,
    /// After `shutdown`: payees may still redeem and claim until `ends`.
    ShuttingDown {
        /// The block timestamp from which the hub is final.
        ends: Millis,
    },
    /// The last block was committed; its state is what the payout set pays.
    Ended,
}

/// The fees of one block, waiting for the votes for it to be recorded
/// (spec/designs/settlement-hubs.md#7-fees).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeePool {
    /// The block's height.
    pub height: u64,
    /// The fees it collected for the validators.
    pub amount: Amount,
    /// Validators whose votes for it a block recorded.
    pub voters: Signers,
}

/// A change of rules the validators voted for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingRules {
    /// The new rules.
    pub rules: Rules,
    /// The first block timestamp at which they are due.
    pub at: Millis,
    /// Once due, the height from which they are in force: two blocks after
    /// the one they fell due in, so consensus learns of the new validators
    /// before it certifies by them, and both hand over at one height.
    pub from_height: Option<u64>,
}

/// What is known about each validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorState {
    /// Where to reach it.
    pub endpoint: String,
    /// The sequence of its last operation.
    pub sequence: u64,
    /// The timestamp of the last block that recorded a vote of its.
    pub last_recorded: Millis,
    /// Its account, which receives its fees.
    pub account: AccountNumber,
}

impl Encode for Account {
    fn encoded_len(&self) -> usize {
        KEY_LEN + 8 * 3 + 1 + DESTINATION_LEN + 1 + 2 + 2 + 2 + 1 + 1 + 8 * 3
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        write_key(writer, &self.key);
        writer.write_u64(self.balance);
        writer.write_u64(self.reserve);
        writer.write_u64(self.sequence);
        write_optional_destination(writer, self.disbursal.as_ref());
        writer.write_bool(self.closing);
        writer.write_u16(self.holds);
        writer.write_u16(self.paying);
        writer.write_u16(self.withdrawals);
        writer.write_bool(self.validator);
        writer.write_bool(self.member_slot);
        writer.write_u64(self.deposited);
        writer.write_u64(self.withdrawn);
        writer.write_u64(self.returned);
    }
}

const DESTINATION_LEN: usize = 20 + 1 + 4;

fn write_optional_destination(writer: &mut Writer<'_>, destination: Option<&Destination>) {
    writer.write_bool(destination.is_some());
    writer.write_bytes(&destination.map_or([0; 20], |d| d.account));
    writer.write_bool(destination.is_some_and(|d| d.tag.is_some()));
    writer.write_u32(destination.and_then(|d| d.tag).unwrap_or(0));
}

impl Encode for Hold {
    fn encoded_len(&self) -> usize {
        4 + 4 + 8 + 8 + 1 + 16 + 8 + 8 + 4 + self.claimed.len() * 16
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u32(self.payer.0);
        writer.write_u32(self.payee.0);
        writer.write_u64(self.amount);
        writer.write_u64(self.reserve);
        writer.write_bool(self.reduction.is_some());
        writer.write_u64(self.reduction.map_or(0, |r| r.amount));
        writer.write_u64(self.reduction.map_or(0, |r| r.at));
        writer.write_u64(self.total);
        writer.write_u64(self.through);
        writer.write_u32(u32::try_from(self.claimed.len()).unwrap_or(u32::MAX));
        for (sequence, amount) in &self.claimed {
            writer.write_u64(*sequence);
            writer.write_u64(*amount);
        }
    }
}

impl Encode for Withdrawal {
    fn encoded_len(&self) -> usize {
        1 + 4 + 8 + 8 + DESTINATION_LEN + 1 + 2 + 2
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bool(self.account.is_some());
        writer.write_u32(self.account.map_or(0, |a| a.0));
        writer.write_u64(self.amount);
        writer.write_u64(self.ledger_fee);
        writer.write_bytes(&self.to.account);
        writer.write_bool(self.to.tag.is_some());
        writer.write_u32(self.to.tag.unwrap_or(0));
        writer.write_bool(self.closes);
        writer.write_u16(self.delivered.bits());
        writer.write_u16(self.failed.bits());
    }
}

impl Encode for PendingDeposit {
    fn encoded_len(&self) -> usize {
        4 + self
            .attestations
            .iter()
            .map(|(deposit, _)| deposit.encoded_len() + 2)
            .sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u32(u32::try_from(self.attestations.len()).unwrap_or(u32::MAX));
        for (deposit, signers) in &self.attestations {
            deposit.encode_to(writer);
            writer.write_u16(signers.bits());
        }
    }
}

/// An account's or a hold's place in the state's Merkle tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    /// The hub-wide values: escrow, phase, counters, validators.
    Header,
    /// An account.
    Account(AccountNumber),
    /// A hold.
    Hold(HoldNumber),
    /// A withdrawal, by its escrow sequence.
    Withdrawal(u32),
    /// A deposit pending attestation, by its transaction.
    PendingDeposit([u8; 32]),
    /// A deposit credited and still remembered.
    RecentDeposit([u8; 32]),
}

impl Key {
    /// The key's bytes: a kind and a big-endian number, so keys sort as
    /// their bytes do.
    #[must_use]
    pub fn bytes(&self) -> [u8; 33] {
        let mut bytes = [0; 33];
        let (kind, rest) = bytes.split_at_mut(1);
        let (kind_byte, number): (u8, &[u8]) = match self {
            Self::Header => (0, &[]),
            Self::Account(number) => (1, &number.0.to_be_bytes()),
            Self::Hold(number) => (2, &number.0.to_be_bytes()),
            Self::Withdrawal(sequence) => (3, &sequence.to_be_bytes()),
            Self::PendingDeposit(hash) => (4, hash),
            Self::RecentDeposit(hash) => (5, hash),
        };
        if let Some(first) = kind.first_mut() {
            *first = kind_byte;
        }
        for (out, byte) in rest.iter_mut().zip(number) {
            *out = *byte;
        }
        bytes
    }
}
