//! The payout set (spec/designs/settlement-hubs.md#9-shutdown): what the
//! escrow pays out at the end, after a committed shutdown or a stall, as a
//! set of transactions every validator builds alone from one final block
//! and signs alone, whenever it can.
//!
//! In escrow sequence order: every withdrawal not yet settled and a change
//! of signer list not yet made, as they were already signed; the selector,
//! which hands the escrow to keys made for this set alone; and one Payment
//! per account to its disbursal address, signed with those keys. The first
//! selector the XRP Ledger applies decides which set pays out: every other
//! set's payments are then signed by keys not on the list, and are refused
//! without using a sequence, so two sets never mix.

use alloc::vec::Vec;

use dassie_xrpl::AccountId;
use dassie_xrpl::codec::{Amount, Payment, Signer, SignerEntry, SignerListSet, Signing, Transact};
use dassie_xrpl::keys::{PublicKey, Secp256k1Key};

use super::bip32::{ExtendedPrivate, ExtendedPublic, payout_path};
use super::keys::ValidatorKeys;
use super::transactions::{escrow_account, network_id, signer_list_set, withdrawal_payment};
use crate::ledger::Ledger;
use crate::rules::{Amount as Drops, Rules};

/// One transaction of a payout set, and who signs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayoutTransaction {
    /// A withdrawal the log gave a sequence, signed by the escrow keys.
    Withdrawal(Payment),
    /// The old validators' change of signer list, signed by their escrow
    /// keys.
    SignerList(SignerListSet),
    /// The selector, signed by the escrow keys.
    Selector(SignerListSet),
    /// A payout to an account, signed by the set's own keys.
    Payout(Payment),
}

impl PayoutTransaction {
    /// Its sequence on the escrow.
    #[must_use]
    pub fn sequence(&self) -> u32 {
        match self {
            Self::Withdrawal(payment) | Self::Payout(payment) => payment.sequence,
            Self::SignerList(list) | Self::Selector(list) => list.sequence,
        }
    }
}

/// The payout set of a final block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutSet {
    /// The height of the block it pays out.
    pub height: u64,
    /// Its transactions, in sequence order with no gap.
    pub transactions: Vec<PayoutTransaction>,
}

/// The account a validator's key for the set of `height` signs as: its
/// payout key's child at the height's path.
fn payout_signer(parent: &ExtendedPublic, height: u64) -> Option<AccountId> {
    let (high, low) = payout_path(height);
    let child = parent.child(high)?.child(low)?;
    Some(PublicKey::from_bytes(child.key)?.account())
}

/// The selector's signer list: every validator of the set, gone or not,
/// by its key for this set, weight 1, sorted by account.
fn selector_entries(rules: &Rules, height: u64) -> Option<Vec<SignerEntry>> {
    let mut entries = rules
        .validators
        .iter()
        .map(|acceptance| {
            let parent = ExtendedPublic {
                key: acceptance.payout,
                chain: acceptance.payout_chain,
            };
            payout_signer(&parent, height).map(|account| SignerEntry { account, weight: 1 })
        })
        .collect::<Option<Vec<_>>>()?;
    entries.sort_unstable_by_key(|entry| entry.account);
    Some(entries)
}

/// What each account is paid out: its balance and reserves, and the holds
/// it paid into, which go back to their payers first.
fn owed(ledger: &Ledger) -> alloc::collections::BTreeMap<crate::operation::AccountNumber, Drops> {
    let mut owed: alloc::collections::BTreeMap<_, Drops> = ledger
        .accounts()
        .map(|(number, account)| (number, account.balance.saturating_add(account.reserve)))
        .collect();
    for (_, hold) in ledger.holds() {
        if let Some(total) = owed.get_mut(&hold.payer) {
            *total = total
                .saturating_add(hold.amount)
                .saturating_add(hold.reserve);
        }
    }
    owed
}

/// The payout set of `ledger`'s state, the state after the block at its
/// height. `None` if a validator's payout key gives no key for this
/// height, which BIP 32 makes vanishingly rare.
#[must_use]
pub fn payout_set(ledger: &Ledger) -> Option<PayoutSet> {
    let rules = ledger.rules();
    let height = ledger.height();
    let mut transactions = Vec::new();
    let mut signer_list = ledger.escrow().signer_list;
    for (sequence, withdrawal) in ledger.withdrawals() {
        if let Some(list) = signer_list.filter(|list| *list < sequence) {
            transactions.push(PayoutTransaction::SignerList(signer_list_set(rules, list)));
            signer_list = None;
        }
        transactions.push(PayoutTransaction::Withdrawal(withdrawal_payment(
            rules, sequence, withdrawal,
        )));
    }
    if let Some(list) = signer_list {
        transactions.push(PayoutTransaction::SignerList(signer_list_set(rules, list)));
    }
    let mut sequence = ledger.escrow().next_sequence;
    transactions.push(PayoutTransaction::Selector(SignerListSet {
        signer_entries: selector_entries(rules, height)?,
        ..signer_list_set(rules, sequence)
    }));
    let fees = &rules.terms.fees;
    for (number, total) in owed(ledger) {
        let Some(account) = ledger.account(number) else {
            continue;
        };
        let Some(destination) = account.disbursal else {
            continue;
        };
        let amount = total.saturating_sub(fees.ledger);
        if total < fees.ledger || amount < fees.least_payout {
            continue;
        }
        sequence = sequence.checked_add(1)?;
        transactions.push(PayoutTransaction::Payout(Payment {
            account: escrow_account(rules),
            destination: AccountId(destination.account),
            destination_tag: destination.tag,
            amount: Amount::Xrp(amount),
            send_max: None,
            fee: fees.ledger,
            sequence,
            last_ledger_sequence: None,
            flags: 0,
            network_id: network_id(rules.terms.network),
            memos: Vec::new(),
            signing: Signing::multi(),
        }));
    }
    Some(PayoutSet {
        height,
        transactions,
    })
}

/// A validator's signatures on a payout set, one per transaction in order:
/// its escrow key's on what the escrow keys sign, its key for this set's
/// on the payouts.
#[must_use]
pub fn sign_payout_set(set: &PayoutSet, keys: &ValidatorKeys) -> Option<Vec<Signer>> {
    let escrow_signer = keys.escrow.account();
    let (high, low) = payout_path(set.height);
    let child: ExtendedPrivate = keys.payout.child(high)?.child(low)?;
    let payout = Secp256k1Key::from_secret(child.secret())?;
    let payout_account = payout.account();
    let signatures = set
        .transactions
        .iter()
        .map(|transaction| match transaction {
            PayoutTransaction::Withdrawal(payment) => {
                payment.multi_sign(escrow_signer, &keys.escrow)
            }
            PayoutTransaction::SignerList(list) | PayoutTransaction::Selector(list) => {
                list.multi_sign(escrow_signer, &keys.escrow)
            }
            PayoutTransaction::Payout(payment) => payment.multi_sign(payout_account, &payout),
        })
        .collect();
    Some(signatures)
}
