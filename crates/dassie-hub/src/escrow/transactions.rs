//! The escrow's transactions on the XRP Ledger
//! (spec/designs/settlement-hubs.md#2-founding-a-hub,
//! spec/designs/settlement-hubs.md#6-deposits-and-withdrawals): the
//! sponsor's setup, the check the validators make of it, the payments that
//! carry withdrawals out, and the change of signer list when the validators
//! change.
//!
//! Every validator builds the same transaction from the same block, so each
//! signs alone and the signatures fit together. None has a last ledger: a
//! withdrawal stays valid until it applies, and its sequence was given to
//! it by the log.

use alloc::vec::Vec;

use dassie_xrpl::api::{AccountObjects, AccountSettings};
use dassie_xrpl::codec::{
    ASF_DISABLE_MASTER, ASF_REQUIRE_DEST, AccountSet, Amount, Payment, Signed, SignerEntry,
    SignerListSet, Signing, Transact,
};
use dassie_xrpl::keys::PublicKey;
use dassie_xrpl::{AccountId, Wallet};

use crate::ledger::Withdrawal;
use crate::rules::Rules;

/// The network ID a transaction names: the ledger requires it above 1024,
/// and refuses it below.
#[must_use]
pub const fn network_id(network: u32) -> Option<u32> {
    if network > 1024 { Some(network) } else { None }
}

/// The escrow's account.
#[must_use]
pub fn escrow_account(rules: &Rules) -> AccountId {
    AccountId(rules.terms.escrow)
}

/// The signer list of a validator set: each validator's escrow key, weight
/// 1, sorted by account as the ledger keeps them.
#[must_use]
pub fn signer_entries(rules: &Rules) -> Vec<SignerEntry> {
    let mut entries: Vec<SignerEntry> = rules
        .validators
        .iter()
        .filter_map(|acceptance| PublicKey::from_bytes(acceptance.escrow))
        .map(|key| SignerEntry {
            account: key.account(),
            weight: 1,
        })
        .collect();
    entries.sort_unstable_by_key(|entry| entry.account);
    entries
}

/// The quorum of the escrow's signer list: the validators' q.
#[must_use]
pub fn signer_quorum(rules: &Rules) -> u32 {
    let n = rules.validators.len();
    u32::try_from(n.saturating_sub(n.saturating_sub(1) / 3)).unwrap_or(u32::MAX)
}

/// The change of the escrow's signer list to `rules`' validators, at
/// `sequence`, to be multi-signed by the set before them.
#[must_use]
pub fn signer_list_set(rules: &Rules, sequence: u32) -> SignerListSet {
    SignerListSet {
        account: escrow_account(rules),
        signer_quorum: signer_quorum(rules),
        signer_entries: signer_entries(rules),
        fee: rules.terms.fees.ledger,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: network_id(rules.terms.network),
        memos: Vec::new(),
        signing: Signing::multi(),
    }
}

/// The Payment that carries withdrawal `sequence` out of the escrow,
/// multi-signed by the validators.
#[must_use]
pub fn withdrawal_payment(rules: &Rules, sequence: u32, withdrawal: &Withdrawal) -> Payment {
    Payment {
        account: escrow_account(rules),
        destination: AccountId(withdrawal.to.account),
        destination_tag: withdrawal.to.tag,
        amount: Amount::Xrp(withdrawal.amount),
        send_max: None,
        fee: withdrawal.ledger_fee,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: network_id(rules.terms.network),
        memos: Vec::new(),
        signing: Signing::multi(),
    }
}

/// The sponsor's setup of the escrow, signed by its master key, in the
/// order it must apply from `sequence` on: the signer list, then
/// `RequireDest`, then the master key disabled, last. `fee` is the ledger's
/// for a single-signed transaction. The escrow is funded first, by an
/// ordinary payment that creates it.
#[must_use]
pub fn setup(rules: &Rules, master: &Wallet, sequence: u32, fee: u64) -> [Transaction; 3] {
    let list = SignerListSet {
        fee,
        signing: Signing::by(master.public_key()),
        ..signer_list_set(rules, sequence)
    };
    let flag = |flag, sequence| AccountSet {
        account: escrow_account(rules),
        set_flag: Some(flag),
        clear_flag: None,
        fee,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: network_id(rules.terms.network),
        memos: Vec::new(),
        signing: Signing::by(master.public_key()),
    };
    [
        Transaction::List(list.sign(master)),
        Transaction::Set(flag(ASF_REQUIRE_DEST, sequence.saturating_add(1)).sign(master)),
        Transaction::Set(flag(ASF_DISABLE_MASTER, sequence.saturating_add(2)).sign(master)),
    ]
}

/// A signed setup transaction of either kind.
#[derive(Clone, Debug)]
pub enum Transaction {
    /// The signer list.
    List(Signed<SignerListSet>),
    /// A setting.
    Set(Signed<AccountSet>),
}

impl Transaction {
    /// The blob to submit.
    #[must_use]
    pub fn blob(&self) -> &[u8] {
        match self {
            Self::List(signed) => &signed.blob,
            Self::Set(signed) => &signed.blob,
        }
    }
}

/// Why an escrow is not one the validators take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscrowFault {
    /// What was read is not as of a validated ledger.
    NotValidated,
    /// Its master key still signs.
    MasterKey,
    /// It has a regular key.
    RegularKey,
    /// Payments without a destination tag reach it.
    NoRequireDest,
    /// Its signer list is not the rules'.
    SignerList,
    /// It owns other objects, or more than one page of them.
    Objects,
}

/// The validators' check of the escrow before the first block, and after a
/// reset (spec/designs/settlement-hubs.md#2-founding-a-hub): the master key
/// disabled, no regular key, the signer list exactly as the rules say,
/// `RequireDest` set, and no other objects. A sponsor that kept a way in is
/// caught here.
pub fn check_escrow(
    rules: &Rules,
    settings: &AccountSettings,
    objects: &AccountObjects,
) -> Result<(), EscrowFault> {
    if !settings.validated || !objects.validated {
        return Err(EscrowFault::NotValidated);
    }
    if !settings.master_disabled() {
        return Err(EscrowFault::MasterKey);
    }
    if settings.regular_key.is_some() {
        return Err(EscrowFault::RegularKey);
    }
    if !settings.requires_destination_tag() {
        return Err(EscrowFault::NoRequireDest);
    }
    if objects.others > 0 || objects.marker.is_some() {
        return Err(EscrowFault::Objects);
    }
    let list = objects
        .signer_list
        .as_ref()
        .ok_or(EscrowFault::SignerList)?;
    if list.quorum != signer_quorum(rules) || list.entries != signer_entries(rules) {
        return Err(EscrowFault::SignerList);
    }
    Ok(())
}
