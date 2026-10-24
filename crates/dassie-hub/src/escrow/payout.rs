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
use dassie_xrpl::codec::{
    Amount, Payment, Signer, SignerEntry, SignerListSet, Signing, Transact, Transaction,
};
use dassie_xrpl::keys::{PublicKey, Secp256k1Key};

use super::bip32::{ExtendedPrivate, ExtendedPublic, payout_path};
use super::keys::ValidatorKeys;
use super::transactions::{
    escrow_account, network_id, signer_list_set, signer_quorum, withdrawal_payment,
};
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

/// The most bytes of one transaction of a payout set, unsigned: a signer
/// list of the most validators is the longest.
pub const MAX_PAYOUT_TRANSACTION_BYTES: usize = 1024;

/// The most bytes of a validator's signed payout set: what a node keeps
/// of each validator's. A set larger than this, of a hub with many more
/// accounts than the defaults allow, is not kept.
pub const MAX_PAYOUT_BUNDLE_BYTES: usize = 16 * 1024 * 1024;

/// What one entry counts toward [`MAX_PAYOUT_BUNDLE_BYTES`] beyond its
/// transaction: its signature, key and account.
const SIGNER_BYTES: usize = 128;

/// One transaction of a payout set as a validator hands it out: the
/// transaction, as the escrow would submit it unsigned, and the
/// validator's signature of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutEntry {
    /// The transaction's binary form, with no signers.
    pub transaction: Vec<u8>,
    /// The validator's signature.
    pub signer: Signer,
}

/// A validator's signed payout set: the bundle it publishes, and members
/// keep, so that anyone holding a quorum's bundles of one set can submit
/// it (spec/designs/settlement-hubs.md#9-shutdown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayoutBundle {
    /// The height of the final block it pays out.
    pub height: u64,
    /// Its transactions, in sequence order, each with the signature.
    pub entries: Vec<PayoutEntry>,
}

/// Why a payout bundle is not one its validator signed of a set of the
/// hub's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayoutError {
    /// No transactions, too many, or too many bytes.
    Size,
    /// A transaction is not one a payout set holds: not the escrow's, not
    /// a Payment or signer list, already signed, of another network or
    /// fee, or out of sequence.
    Transaction(usize),
    /// No selector of this height's keys, or one in the wrong place.
    Selector,
    /// A signature is not the validator's key's for its place in the set,
    /// or does not verify.
    Signature(usize),
}

impl PayoutTransaction {
    /// Its binary form, unsigned.
    #[must_use]
    pub fn blob(&self) -> Vec<u8> {
        match self {
            Self::Withdrawal(payment) | Self::Payout(payment) => payment.blob(),
            Self::SignerList(list) | Self::Selector(list) => list.blob(),
        }
    }
}

impl PayoutBundle {
    /// The bundle of `set` as signed with `signatures`, one per
    /// transaction.
    #[must_use]
    pub fn of(set: &PayoutSet, signatures: &[Signer]) -> Self {
        Self {
            height: set.height,
            entries: set
                .transactions
                .iter()
                .zip(signatures)
                .map(|(transaction, signer)| PayoutEntry {
                    transaction: transaction.blob(),
                    signer: *signer,
                })
                .collect(),
        }
    }

    /// Its size in bytes, as the bound counts them.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.transaction.len().saturating_add(SIGNER_BYTES))
            .sum()
    }

    /// Checks that it is a payout set of `rules`' escrow signed by the
    /// validator at `index` of them: every transaction the escrow's, in
    /// sequence order, with the rules' fee and network; one selector,
    /// whose list is the set's own keys for its height; before it,
    /// signatures by the validator's escrow key, after it by its key for
    /// this set; and every signature verifies. What it cannot check is
    /// that the set is the block's: that takes the state after it.
    ///
    /// # Errors
    ///
    /// What is wrong with it.
    pub fn verify(&self, rules: &Rules, index: usize) -> Result<(), PayoutError> {
        if self.entries.is_empty() || self.bytes() > MAX_PAYOUT_BUNDLE_BYTES {
            return Err(PayoutError::Size);
        }
        let (escrow_key, payout_key) = signing_keys(rules, index, self.height)?;
        let selector = selector_entries(rules, self.height).ok_or(PayoutError::Selector)?;
        let mut selected = false;
        let mut sequence = None;
        for (i, entry) in self.entries.iter().enumerate() {
            let transaction = read_transaction(rules, &entry.transaction, &mut sequence)
                .ok_or(PayoutError::Transaction(i))?;
            let is_selector = matches!(&transaction, Transaction::SignerListSet(list)
                if list.signer_entries == selector && list.signer_quorum == signer_quorum(rules));
            // After the selector, only payouts; a second selector is none.
            if selected && !matches!(transaction, Transaction::Payment(_)) {
                return Err(PayoutError::Selector);
            }
            let key = if selected { payout_key } else { escrow_key };
            let signed_by = entry.signer.account == key.account()
                && entry.signer.signing_pub_key == key
                && transaction.verify_signer(&entry.signer);
            if !signed_by {
                return Err(PayoutError::Signature(i));
            }
            selected |= is_selector;
        }
        if selected {
            Ok(())
        } else {
            Err(PayoutError::Selector)
        }
    }
}

/// The keys the validator at `index` of `rules` signs the set of `height`
/// with: its escrow key, and its payout key's child for the height.
fn signing_keys(
    rules: &Rules,
    index: usize,
    height: u64,
) -> Result<(PublicKey, PublicKey), PayoutError> {
    let acceptance = rules
        .validators
        .get(index)
        .ok_or(PayoutError::Signature(0))?;
    let escrow = PublicKey::from_bytes(acceptance.escrow).ok_or(PayoutError::Signature(0))?;
    let parent = ExtendedPublic {
        key: acceptance.payout,
        chain: acceptance.payout_chain,
    };
    let (high, low) = payout_path(height);
    let payout = parent
        .child(high)
        .and_then(|child| child.child(low))
        .and_then(|child| PublicKey::from_bytes(child.key))
        .ok_or(PayoutError::Selector)?;
    Ok((escrow, payout))
}

/// The transaction in `blob`, if a payout set of `rules` may hold it, at
/// the sequence after `sequence`'s, which it then holds.
fn read_transaction(rules: &Rules, blob: &[u8], sequence: &mut Option<u32>) -> Option<Transaction> {
    if blob.len() > MAX_PAYOUT_TRANSACTION_BYTES {
        return None;
    }
    let transaction = Transaction::decode(blob).ok()?;
    let (account, fee, network, last, plain) = match &transaction {
        Transaction::Payment(payment) => (
            payment.account,
            payment.fee,
            payment.network_id,
            payment.last_ledger_sequence,
            payment.send_max.is_none() && matches!(payment.amount, Amount::Xrp(_)),
        ),
        Transaction::SignerListSet(list) => (
            list.account,
            list.fee,
            list.network_id,
            list.last_ledger_sequence,
            true,
        ),
        _ => return None,
    };
    let unsigned = matches!(transaction.signing(), Signing::Multi(signers) if signers.is_empty());
    let next = sequence.map_or(Some(transaction.sequence()), |last| last.checked_add(1))?;
    let fits = plain
        && unsigned
        && account == escrow_account(rules)
        && fee == rules.terms.fees.ledger
        && network == network_id(rules.terms.network)
        && last.is_none()
        && transaction.sequence() == next;
    *sequence = Some(next);
    fits.then_some(transaction)
}

/// The payout set's transactions multi-signed by the validators whose
/// bundles are `bundles` (each with its place in `rules`), ready to
/// submit in order: once a quorum of them verify and hold the same
/// transactions. `None` until they do.
#[must_use]
pub fn assemble(rules: &Rules, bundles: &[(usize, &PayoutBundle)]) -> Option<(u64, Vec<Vec<u8>>)> {
    let quorum = usize::try_from(signer_quorum(rules)).ok()?;
    let valid: Vec<&(usize, &PayoutBundle)> = bundles
        .iter()
        .filter(|(index, bundle)| bundle.verify(rules, *index).is_ok())
        .collect();
    for (_, candidate) in &valid {
        let agreeing: Vec<&PayoutBundle> = valid
            .iter()
            .filter(|(_, bundle)| same_set(bundle, candidate))
            .map(|(_, bundle)| *bundle)
            .take(quorum)
            .collect();
        if agreeing.len() < quorum {
            continue;
        }
        let blobs = candidate
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let signers: Vec<Signer> = agreeing
                    .iter()
                    .filter_map(|bundle| bundle.entries.get(i).map(|e| e.signer))
                    .collect();
                multi_signed(&entry.transaction, signers)
            })
            .collect::<Option<Vec<_>>>()?;
        return Some((candidate.height, blobs));
    }
    None
}

/// Whether two bundles sign the same set.
fn same_set(a: &PayoutBundle, b: &PayoutBundle) -> bool {
    a.height == b.height
        && a.entries.len() == b.entries.len()
        && a.entries
            .iter()
            .zip(&b.entries)
            .all(|(x, y)| x.transaction == y.transaction)
}

/// The transaction in `blob` signed by `signers`, as submitted.
fn multi_signed(blob: &[u8], signers: Vec<Signer>) -> Option<Vec<u8>> {
    Some(match Transaction::decode(blob).ok()? {
        Transaction::Payment(payment) => payment.with_signers(signers).ok()?.blob,
        Transaction::SignerListSet(list) => list.with_signers(signers).ok()?.blob,
        _ => return None,
    })
}
