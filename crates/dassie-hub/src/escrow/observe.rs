//! What a validator attests from the escrow's transactions on the XRP
//! Ledger (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
//! Each validator follows the escrow on its own servers, and reads only
//! what it proved: transactions in ledgers its UNL validated, each read as
//! a record from its proven bytes, and the escrow's setup and a token's
//! issuer's hold on it from a state proof at one such ledger
//! (spec/designs/xrpl-validation.md#12-who-else-uses-it).

use alloc::vec::Vec;

use dassie_xrpl::AccountId;
use dassie_xrpl::anchor::ProvenState;
use dassie_xrpl::api::{TRANSFER_RATE_PAR, TxRecord, account_flags};
use dassie_xrpl::binary::{FieldId, Fields, Type};
use dassie_xrpl::codec::{Amount, TokenValue};
use dassie_xrpl::records::{
    Thread, account_key, account_root, line_key, ripple_state, signer_list, signer_list_key,
};

use super::transactions::{EscrowFault, is_escrow_line, signer_entries, signer_quorum};
use crate::digest::Digest;
use crate::operation::{Clawback, Deposit, Issuer};
use crate::rules::{Asset, Rules, TOKEN_DECIMALS, Terms};

/// The memo type of a deposit that opens an account: its data is the new
/// account's key, 32 bytes. Wallets outside Dassie write it too, so it is
/// pinned by `testdata/hub/vectors.json` (spec/areas/hubs.md#the-escrow).
pub const OPEN_MEMO_TYPE: &[u8] = b"https://dassie.land/hub/open/v1";

/// What `record` delivered, of whatever asset.
fn delivered(record: &TxRecord) -> Option<Amount> {
    record
        .delivered
        .map(Amount::Xrp)
        .or(record.delivered_token.map(Amount::Token))
}

/// A deposit to the escrow of `terms`, if `record` is one: a successful
/// Payment of the hub's asset to it, in a validated ledger, with a
/// destination tag. Its amount is what it delivered, never what it named:
/// a partial payment can name much and deliver little. In a token hub,
/// that is the hub's token, of its issuer, in whole millionths, rounded
/// down; XRP sent to a token hub's escrow is gas for its transactions,
/// never a deposit, and nothing is attested of it. For tag 0, the new
/// account's key is the first memo's data, as 32 bytes, under
/// [`OPEN_MEMO_TYPE`]; anything else attests no key, and the hub pays the
/// deposit back. A memo of any other type is never read as a key, however
/// its data reads: about half of all 32-byte strings decode as a point,
/// and an account would open under a key nobody holds.
#[must_use]
pub fn deposit_of(record: &TxRecord, terms: &Terms) -> Option<Deposit> {
    let escrow = AccountId(terms.escrow);
    let to_escrow = record.validated
        && record.succeeded()
        && record.is_payment
        && record.destination == Some(escrow)
        && record.account != escrow;
    if !to_escrow {
        return None;
    }
    let tag = record.destination_tag?;
    let amount = terms
        .hub_amount(&delivered(record)?)
        .filter(|amount| *amount > 0)?;
    let key = record
        .memo
        .as_ref()
        .filter(|(memo_type, _)| tag == 0 && memo_type.as_slice() == OPEN_MEMO_TYPE)
        .and_then(|(_, data)| <[u8; 32]>::try_from(data.as_slice()).ok())
        .and_then(|bytes| dassie_identity::PublicKey::from_bytes(&bytes).ok())
        .filter(|key| !key.is_weak());
    Some(Deposit {
        transaction: Digest(record.hash),
        ledger: record.ledger_index,
        amount,
        sender: record.account.0,
        tag,
        key,
    })
}

/// Whether withdrawal `sequence` delivered, if `record` is its outcome:
/// the escrow's Payment at that sequence, to the withdrawal's destination
/// and tag, delivering its amount of the hub's asset, as its Payment names
/// it, when it succeeded. Anything else at that sequence is not the
/// withdrawal's, and burns nothing.
#[must_use]
pub fn withdrawal_outcome(
    record: &TxRecord,
    terms: &Terms,
    sequence: u32,
    withdrawal: &crate::ledger::Withdrawal,
) -> Option<bool> {
    let (seen, succeeded) = outcome_of(record, AccountId(terms.escrow))?;
    let same = seen == sequence
        && record.is_payment
        && record.destination == Some(AccountId(withdrawal.to.account))
        && record.destination_tag == withdrawal.to.tag;
    let expected = terms.ledger_amount(withdrawal.amount);
    if !same || (succeeded && delivered(record) != Some(expected)) {
        return None;
    }
    Some(succeeded)
}

/// What became of one of the escrow's own transactions, if `record` is
/// one in a validated ledger: its sequence, and whether it did what it was
/// for. A `tec` result failed but used the sequence; anything else that
/// is not `tesSUCCESS` is not in a validated ledger.
#[must_use]
pub fn outcome_of(record: &TxRecord, escrow: AccountId) -> Option<(u32, bool)> {
    if !record.validated || record.account != escrow {
        return None;
    }
    if record.succeeded() {
        Some((record.sequence, true))
    } else if record.result.starts_with("tec") {
        Some((record.sequence, false))
    } else {
        None
    }
}

/// The keys of what the validators prove of the escrow's setup, at one
/// validated ledger (spec/designs/settlement-hubs.md#2-founding-a-hub):
/// its account root, its signer list, and for a token hub its line to the
/// issuer.
#[must_use]
pub fn escrow_keys(terms: &Terms) -> Vec<[u8; 32]> {
    let escrow = AccountId(terms.escrow);
    let mut keys = alloc::vec![account_key(escrow), signer_list_key(escrow)];
    if let Asset::Token { currency, issuer } = terms.asset {
        keys.push(line_key(escrow, issuer, currency));
    }
    keys
}

/// The keys of what the validators prove of a token hub's issuer's hold on
/// the escrow: the escrow's line to it and its account root; `None` for an
/// XRP hub.
#[must_use]
pub fn issuer_keys(terms: &Terms) -> Option<[[u8; 32]; 2]> {
    let Asset::Token { currency, issuer } = terms.asset else {
        return None;
    };
    Some([
        line_key(AccountId(terms.escrow), issuer, currency),
        account_key(issuer),
    ])
}

/// The object `state` proves under `key`: `Err` if it proves nothing
/// there, `Ok(None)` if it proves the key holds none.
fn proven<'a>(state: &'a ProvenState, key: &[u8; 32]) -> Result<Option<&'a [u8]>, EscrowFault> {
    state.object(key).ok_or(EscrowFault::NotValidated)
}

/// The validators' check of the escrow ([`check_escrow`](super::check_escrow)) from a state
/// proof of [`escrow_keys`] at one validated ledger: the account root with
/// its master key disabled, no regular key and `RequireDest`, the signer
/// list exactly the rules', for a token hub its line to the issuer as the
/// setup makes it, at par on both sides, and nothing else it owns: its
/// owner count is what its signer list and that line count, one each. What
/// others make that names the escrow, a line to it or a check to it, is
/// charged to them and gives no one a way in. Its next sequence, once it
/// passes.
///
/// # Errors
///
/// Why the escrow is not one the validators take.
pub fn check_proven_escrow(rules: &Rules, state: &ProvenState) -> Result<u32, EscrowFault> {
    let terms = &rules.terms;
    let escrow = AccountId(terms.escrow);
    let root = proven(state, &account_key(escrow))?.ok_or(EscrowFault::NoAccount)?;
    let root = account_root(root).map_err(|_| EscrowFault::NoAccount)?;
    if root.account != escrow {
        return Err(EscrowFault::NoAccount);
    }
    if root.flags & account_flags::DISABLE_MASTER == 0 {
        return Err(EscrowFault::MasterKey);
    }
    if root.regular_key.is_some() {
        return Err(EscrowFault::RegularKey);
    }
    if root.flags & account_flags::REQUIRE_DEST_TAG == 0 {
        return Err(EscrowFault::NoRequireDest);
    }
    let list = proven(state, &signer_list_key(escrow))?.ok_or(EscrowFault::SignerList)?;
    let (list, _) = signer_list(list).map_err(|_| EscrowFault::SignerList)?;
    if list.quorum != signer_quorum(rules) || list.entries != signer_entries(rules) {
        return Err(EscrowFault::SignerList);
    }
    let mut owned = 1_u32;
    if let Asset::Token { currency, issuer } = terms.asset {
        let line =
            proven(state, &line_key(escrow, issuer, currency))?.ok_or(EscrowFault::TrustLine)?;
        check_escrow_line(terms, line)?;
        owned = 2;
    }
    if root.owner_count != owned {
        return Err(EscrowFault::Objects);
    }
    Ok(root.sequence)
}

/// A trust line's qualities, as rippled's binary form names its
/// `UInt32` fields: what each side values what it takes (`QualityIn`) and
/// pays (`QualityOut`) at, in billionths. A `TrustSet`'s own `QualityIn`
/// and `QualityOut` (20 and 21) are never in a line.
const HIGH_QUALITY_IN: FieldId = FieldId::new(Type::UInt32, 16);
const HIGH_QUALITY_OUT: FieldId = FieldId::new(Type::UInt32, 17);
const LOW_QUALITY_IN: FieldId = FieldId::new(Type::UInt32, 18);
const LOW_QUALITY_OUT: FieldId = FieldId::new(Type::UInt32, 19);
const QUALITIES: [FieldId; 4] = [
    HIGH_QUALITY_IN,
    HIGH_QUALITY_OUT,
    LOW_QUALITY_IN,
    LOW_QUALITY_OUT,
];

/// A quality at par: a billion billionths. Zero, or none, is par too.
const QUALITY_PAR: u32 = 1_000_000_000;

/// The validators' check of a token hub's escrow line to its issuer, from
/// the `RippleState` a state proof holds, in rippled's binary form: as the
/// setup makes it ([`check_escrow`](super::check_escrow) reads the same
/// from JSON), and at par on both sides, with no quality that would skew
/// what a withdrawal delivers or a deposit adds.
///
/// # Errors
///
/// [`EscrowFault::TrustLine`]: it does not read, is not the line the setup
/// makes, or values something off par.
pub fn check_escrow_line(terms: &Terms, object: &[u8]) -> Result<(), EscrowFault> {
    let (read, _) = ripple_state(object).map_err(|_| EscrowFault::TrustLine)?;
    if at_par(object) && is_escrow_line(terms, &read) {
        Ok(())
    } else {
        Err(EscrowFault::TrustLine)
    }
}

/// Whether a `RippleState` in rippled's binary form values what it pays and
/// takes at par on both sides: no quality, or one of zero or
/// [`QUALITY_PAR`].
fn at_par(object: &[u8]) -> bool {
    Fields::new(object).all(|field| {
        field.is_ok_and(|field| {
            !QUALITIES.contains(&field.id)
                || field.uint32().is_ok_and(|q| q == 0 || q == QUALITY_PAR)
        })
    })
}

/// The issuer's hold on a token hub's escrow, from a state proof of
/// [`issuer_keys`] at one validated ledger: frozen if the issuer froze or
/// deep-froze the escrow's line on its side, or froze every line; a fee if
/// it charges a transfer rate other than par. Its ledger is the later of
/// the two objects' last changes, as each names it (`PreviousTxnLgrSeq`).
/// `None` for an XRP hub, or while either is not there or does not read.
#[must_use]
pub fn issuer_of(terms: &Terms, state: &ProvenState) -> Option<Issuer> {
    let [line_key, root_key] = issuer_keys(terms)?;
    let (line, line_thread) = ripple_state(state.object(&line_key)??).ok()?;
    let view = line.view(AccountId(terms.escrow))?;
    let root = account_root(state.object(&root_key)??).ok()?;
    let thread_ledger = |thread: Option<Thread>| thread.map_or(0, |(_, ledger)| ledger);
    let frozen = view.theirs.freeze
        || view.theirs.deep_freeze
        || root.flags & account_flags::GLOBAL_FREEZE != 0;
    let fee = root.transfer_rate != 0 && root.transfer_rate != TRANSFER_RATE_PAR;
    Some(Issuer {
        ledger: thread_ledger(line_thread).max(thread_ledger(root.thread)),
        frozen,
        fee,
    })
}

/// A clawback from a token hub's escrow, if `record` is one: a transaction
/// of the issuer's that succeeded in a validated ledger, that is no
/// Payment, and that took from the escrow's balance on its line. Nothing
/// else the issuer sends lowers a holder's balance. What it took is in the
/// hub's unit, rounded up, so that what the hub keeps apart for it is
/// never less than it took.
#[must_use]
pub fn clawback_of(record: &TxRecord, terms: &Terms) -> Option<Clawback> {
    let Asset::Token { currency, issuer } = terms.asset else {
        return None;
    };
    let escrow = AccountId(terms.escrow);
    if !record.validated || !record.succeeded() || record.is_payment || record.account != issuer {
        return None;
    }
    let (before, after) = record.line(escrow, issuer, currency)?.balance_of(escrow)?;
    let took = before.checked_sub(after)?;
    if took.is_zero() || took.is_negative() {
        return None;
    }
    let floor = u64::try_from(took.floor_units(TOKEN_DECIMALS)?).ok()?;
    let exact = TokenValue::from_units(floor, TOKEN_DECIMALS) == Some(took);
    let amount = if exact { floor } else { floor.checked_add(1)? };
    Some(Clawback {
        transaction: Digest(record.hash),
        ledger: record.ledger_index,
        amount,
    })
}
