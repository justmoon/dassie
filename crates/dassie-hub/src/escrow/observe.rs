//! What a validator attests from the escrow's transactions on the XRP
//! Ledger (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
//! Each validator follows the escrow on its own servers, and reads only
//! validated ledgers.

use dassie_xrpl::AccountId;
use dassie_xrpl::api::TxRecord;
use dassie_xrpl::codec::Amount;

use crate::digest::Digest;
use crate::operation::Deposit;
use crate::rules::Terms;

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
/// account's key is the memo, as 32 bytes; anything else attests no key,
/// and the hub pays the deposit back.
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
        .filter(|_| tag == 0)
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
